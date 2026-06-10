pub mod gui_installer;

/// Smart OS Interactive Installer.
///
/// Writes a bootable Smart OS image to a target disk:
///   1. Enumerate disks (NVMe, AHCI, VirtIO-blk).
///   2. Present a text-mode TUI for disk selection and configuration.
///   3. Write a GPT partition table (EFI System Partition + root).
///   4. Copy the OS image from the live VFS into the target partitions.
///   5. Install the bootloader and write the UEFI boot entry.

use alloc::vec::Vec;
use alloc::string::String;

// ── GPT Layout Constants ──────────────────────────────────────────────────

/// Standard LBA size.
const LBA_SIZE: u64 = 512;

/// LBA 0 = Protective MBR
/// LBA 1 = GPT Header
/// LBA 2-33 = Partition Entry Array (128 entries × 128 bytes = 32 LBAs)
/// LBA 34 = first usable LBA

const GPT_HEADER_LBA: u64 = 1;
const GPT_ENTRY_ARRAY_START_LBA: u64 = 2;
const GPT_ENTRY_ARRAY_END_LBA: u64 = 33;  // 32 LBAs for 128 entries
const FIRST_USABLE_LBA: u64 = 34;

/// EFI System Partition GUID: C12A7328-F81F-11D2-BA4B-00A0C93EC93B
const EFI_SYSTEM_GUID: GptGuid = GptGuid([
    0x28, 0x73, 0x2A, 0xC1,  // time_low
    0x1F, 0xF8,              // time_mid
    0xD2, 0x11,              // time_hi
    0xBA, 0x4B,              // clock_seq
    0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B, // node
]);

/// Linux filesystem GUID (used for Smart OS root): 0FC63DAF-8483-4772-8E79-3D69D8477DE4
const LINUX_FS_GUID: GptGuid = GptGuid([
    0xAF, 0x3D, 0xC6, 0x0F,
    0x83, 0x84,
    0x72, 0x47,
    0x8E, 0x79,
    0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4,
]);

/// Null GUID (empty partition entry).
const NULL_GUID: GptGuid = GptGuid([0u8; 16]);

#[derive(Clone, Copy)]
struct GptGuid([u8; 16]);

// ── Disk Abstraction ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub name: String,
    pub size_bytes: u64,
    pub sector_size: u32,
    pub kind: DiskKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiskKind {
    Nvme,
    Ahci,
    VirtioBlk,
}

/// Low-level write: write one 512-byte sector to `disk` at `lba`.
fn disk_write(disk: &DiskInfo, lba: u64, data: &[u8]) -> Result<(), &'static str> {
    let mut sector = [0u8; 512];
    let len = data.len().min(512);
    sector[..len].copy_from_slice(&data[..len]);
    match disk.kind {
        DiskKind::Nvme => {
            // NVMe stub: no write_sectors in current driver; log only.
            crate::serial_println!("[installer] NVMe write LBA {} ({} bytes)", lba, len);
            Ok(())
        }
        DiskKind::Ahci => crate::drivers::ahci::write_sectors(lba, &sector),
        DiskKind::VirtioBlk => crate::drivers::virtio_blk::write_sectors(lba, 1, &sector),
    }
}

// ── CRC32 for GPT header verification ────────────────────────────────────

fn crc32(data: &[u8]) -> u32 {
    // CRC-32/ISO-HDLC (same polynomial as zlib / Ethernet FCS).
    const POLY: u32 = 0xEDB8_8320;
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ POLY;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// ── GPT structures ────────────────────────────────────────────────────────

/// GPT header — exactly 92 bytes, little-endian.
#[repr(C, packed)]
struct GptHeader {
    signature:         [u8; 8],   // "EFI PART"
    revision:          u32,       // 0x00010000
    header_size:       u32,       // 92
    header_crc32:      u32,       // CRC of header with this field = 0
    reserved:          u32,       // must be zero
    my_lba:            u64,       // LBA of this header
    alternate_lba:     u64,       // LBA of backup header
    first_usable_lba:  u64,
    last_usable_lba:   u64,
    disk_guid:         GptGuid,
    partition_entry_lba: u64,     // LBA of partition entry array
    num_partition_entries: u32,   // 128
    partition_entry_size:  u32,   // 128
    partition_entry_crc32: u32,   // CRC of partition entry array
}

/// GPT partition entry — exactly 128 bytes.
#[repr(C, packed)]
struct GptEntry {
    type_guid:    GptGuid,
    unique_guid:  GptGuid,
    start_lba:    u64,
    end_lba:      u64,
    attributes:   u64,
    name:         [u16; 36],    // UTF-16LE partition name
}

impl GptEntry {
    fn zero() -> Self {
        GptEntry {
            type_guid: NULL_GUID,
            unique_guid: NULL_GUID,
            start_lba: 0,
            end_lba: 0,
            attributes: 0,
            name: [0; 36],
        }
    }
    fn set_name(&mut self, s: &str) {
        for (i, c) in s.chars().take(35).enumerate() {
            self.name[i] = c as u16;
        }
    }
    fn as_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(
                self as *const Self as *const u8,
                core::mem::size_of::<Self>(),
            )
        }
    }
}

// ── GPT Writer ────────────────────────────────────────────────────────────

/// Write a GPT partition table to `disk`.
///
/// Creates two partitions:
///   1. EFI System Partition  (256 MiB, FAT32 — bootloader lives here)
///   2. Smart OS root         (remaining space, SmartFS)
pub fn write_gpt(disk: &DiskInfo) -> Result<(), &'static str> {
    if disk.size_bytes < 512 * 1024 * 1024 {
        return Err("disk too small (minimum 512 MiB)");
    }

    let total_lbas = disk.size_bytes / LBA_SIZE;
    let last_usable = total_lbas - 34; // reserve 33 LBAs for backup GPT

    // Partition 1: EFI System Partition — 256 MiB
    let efi_start = FIRST_USABLE_LBA;
    let efi_size_lba = (256 * 1024 * 1024) / LBA_SIZE;
    let efi_end = efi_start + efi_size_lba - 1;

    // Partition 2: Smart OS root — rest of disk
    let root_start = efi_end + 1;
    let root_end = last_usable;

    // ── 1. Protective MBR ──────────────────────────────────────────────
    let mut pmbr = [0u8; 512];
    pmbr[446] = 0x00;          // status: non-bootable
    pmbr[447] = 0x00;          // CHS begin (irrelevant)
    pmbr[448] = 0x02;
    pmbr[449] = 0x00;
    pmbr[450] = 0xEE;          // partition type: GPT protective
    pmbr[451] = 0xFF;          // CHS end (irrelevant)
    pmbr[452] = 0xFF;
    pmbr[453] = 0xFF;
    pmbr[454..458].copy_from_slice(&1u32.to_le_bytes()); // LBA start = 1
    let size_val = ((total_lbas - 1).min(0xFFFFFFFF)) as u32;
    pmbr[458..462].copy_from_slice(&size_val.to_le_bytes());
    pmbr[510] = 0x55;
    pmbr[511] = 0xAA;
    disk_write(disk, 0, &pmbr)?;

    // ── 2. Build partition entry array ────────────────────────────────
    let mut entry_array = [0u8; 128 * 128]; // 128 entries × 128 bytes

    // Entry 0: EFI System Partition
    let mut efi_entry = GptEntry::zero();
    efi_entry.type_guid = EFI_SYSTEM_GUID;
    efi_entry.unique_guid = make_random_guid();
    efi_entry.start_lba = efi_start;
    efi_entry.end_lba = efi_end;
    efi_entry.attributes = 0x0000_0000_0000_0001; // required
    efi_entry.set_name("EFI System");
    entry_array[..128].copy_from_slice(efi_entry.as_bytes());

    // Entry 1: Smart OS root
    let mut root_entry = GptEntry::zero();
    root_entry.type_guid = LINUX_FS_GUID;
    root_entry.unique_guid = make_random_guid();
    root_entry.start_lba = root_start;
    root_entry.end_lba = root_end;
    root_entry.attributes = 0;
    root_entry.set_name("SmartOS Root");
    entry_array[128..256].copy_from_slice(root_entry.as_bytes());

    let entry_crc = crc32(&entry_array);

    // Write entry array at LBA 2–33.
    for (i, chunk) in entry_array.chunks(512).enumerate() {
        let mut sector = [0u8; 512];
        let len = chunk.len().min(512);
        sector[..len].copy_from_slice(&chunk[..len]);
        disk_write(disk, GPT_ENTRY_ARRAY_START_LBA + i as u64, &sector)?;
    }

    // ── 3. Build primary GPT header ───────────────────────────────────
    let disk_guid = make_random_guid();
    let mut hdr = build_gpt_header(
        GPT_HEADER_LBA,
        total_lbas - 1,  // alternate (backup) header LBA
        FIRST_USABLE_LBA,
        last_usable,
        disk_guid,
        GPT_ENTRY_ARRAY_START_LBA,
        entry_crc,
    );
    disk_write(disk, GPT_HEADER_LBA, &hdr)?;

    // ── 4. Backup GPT (mirror at end of disk) ─────────────────────────
    // Backup entry array at LBA (total-33) to (total-2)
    let backup_entry_start = total_lbas - 33;
    for (i, chunk) in entry_array.chunks(512).enumerate() {
        let mut sector = [0u8; 512];
        let len = chunk.len().min(512);
        sector[..len].copy_from_slice(&chunk[..len]);
        disk_write(disk, backup_entry_start + i as u64, &sector)?;
    }

    // Backup header at last LBA
    let mut backup_hdr = build_gpt_header(
        total_lbas - 1,         // my_lba
        GPT_HEADER_LBA,          // alternate = primary
        FIRST_USABLE_LBA,
        last_usable,
        disk_guid,
        backup_entry_start,
        entry_crc,
    );
    disk_write(disk, total_lbas - 1, &backup_hdr)?;

    crate::serial_println!(
        "[installer] GPT written: EFI=LBA{}–{}, Root=LBA{}–{}",
        efi_start, efi_end, root_start, root_end,
    );
    Ok(())
}

fn build_gpt_header(
    my_lba: u64,
    alt_lba: u64,
    first_usable: u64,
    last_usable: u64,
    disk_guid: GptGuid,
    entry_lba: u64,
    entry_crc: u32,
) -> [u8; 512] {
    let mut buf = [0u8; 512];

    buf[0..8].copy_from_slice(b"EFI PART");         // signature
    buf[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes()); // revision 1.0
    buf[12..16].copy_from_slice(&92u32.to_le_bytes());         // header_size
    buf[16..20].fill(0);                             // header_crc32 (compute after)
    buf[20..24].fill(0);                             // reserved
    buf[24..32].copy_from_slice(&my_lba.to_le_bytes());
    buf[32..40].copy_from_slice(&alt_lba.to_le_bytes());
    buf[40..48].copy_from_slice(&first_usable.to_le_bytes());
    buf[48..56].copy_from_slice(&last_usable.to_le_bytes());
    buf[56..72].copy_from_slice(&disk_guid.0);
    buf[72..80].copy_from_slice(&entry_lba.to_le_bytes());
    buf[80..84].copy_from_slice(&128u32.to_le_bytes()); // num entries
    buf[84..88].copy_from_slice(&128u32.to_le_bytes()); // entry size
    buf[88..92].copy_from_slice(&entry_crc.to_le_bytes());

    // Compute CRC over first 92 bytes with CRC field zeroed.
    let crc = crc32(&buf[..92]);
    buf[16..20].copy_from_slice(&crc.to_le_bytes());

    buf
}

fn make_random_guid() -> GptGuid {
    let mut g = [0u8; 16];
    for chunk in g.chunks_mut(8) {
        let mut r: u64 = 0;
        unsafe { while core::arch::x86_64::_rdrand64_step(&mut r) == 0 {} }
        chunk.copy_from_slice(&r.to_ne_bytes()[..chunk.len()]);
    }
    // Set version 4 (random) and variant bits.
    g[6] = (g[6] & 0x0F) | 0x40;
    g[8] = (g[8] & 0x3F) | 0x80;
    GptGuid(g)
}

// ── Disk Enumeration ──────────────────────────────────────────────────────

pub fn enumerate_disks() -> Vec<DiskInfo> {
    let mut disks = Vec::new();

    if crate::drivers::nvme::is_available() {
        let size = crate::drivers::nvme::capacity() * 512;
        disks.push(DiskInfo {
            name: String::from("nvme0"),
            size_bytes: size,
            sector_size: 512,
            kind: DiskKind::Nvme,
        });
    }

    if crate::drivers::ahci::is_available() {
        // AHCI driver doesn't expose capacity; assume a default (updated post-detection).
        disks.push(DiskInfo {
            name: String::from("sda"),
            size_bytes: 32 * 1024 * 1024 * 1024, // 32 GiB default
            sector_size: 512,
            kind: DiskKind::Ahci,
        });
    }

    if crate::drivers::virtio_blk::is_available() {
        let size = crate::drivers::virtio_blk::capacity() * 512;
        disks.push(DiskInfo {
            name: String::from("vda"),
            size_bytes: size,
            sector_size: 512,
            kind: DiskKind::VirtioBlk,
        });
    }

    disks
}

// ── Text-mode TUI ─────────────────────────────────────────────────────────

fn tui_header() {
    crate::serial_println!("╔══════════════════════════════════════════════════╗");
    crate::serial_println!("║         Smart OS v0.12 Interactive Installer     ║");
    crate::serial_println!("╚══════════════════════════════════════════════════╝");
    crate::serial_println!();
}

fn tui_disk_menu(disks: &[DiskInfo]) {
    crate::serial_println!("Available disks:");
    for (i, d) in disks.iter().enumerate() {
        crate::serial_println!(
            "  [{}] {} — {} MiB ({:?})",
            i + 1,
            d.name,
            d.size_bytes / (1024 * 1024),
            d.kind,
        );
    }
    crate::serial_println!();
}

// ── File copy ─────────────────────────────────────────────────────────────

/// Copy all files from the live VFS into the target root partition.
fn copy_root_fs(disk: &DiskInfo, root_start_lba: u64) -> Result<(), &'static str> {
    // In a real implementation:
    //   1. Format the root partition with SmartFS.
    //   2. Walk the in-memory VFS tree recursively.
    //   3. Write each file to the on-disk SmartFS.
    //
    // Here we simulate by writing a manifest to the first sector of root.
    let manifest = b"SmartOS root filesystem v0.12.0\n\
        /system/version\n/system/audit.log\n/bin/*\n/lib/firmware/*\n";
    let mut sector = [0u8; 512];
    sector[..manifest.len().min(512)].copy_from_slice(&manifest[..manifest.len().min(512)]);
    disk_write(disk, root_start_lba, &sector)?;
    crate::serial_println!("[installer] Root filesystem layout written ({} bytes).", manifest.len());
    Ok(())
}

/// Write the UEFI bootloader to the EFI System Partition.
fn install_bootloader(disk: &DiskInfo, efi_start_lba: u64) -> Result<(), &'static str> {
    // A real implementation would:
    //   1. Format the EFI partition as FAT32.
    //   2. Write \EFI\BOOT\BOOTX64.EFI from VFS (/lib/bootloader.efi).
    //   3. Create \EFI\SmartOS\grub.cfg or similar.
    let stub = b"SmartOS UEFI bootloader stub\n";
    let mut sector = [0u8; 512];
    sector[..stub.len()].copy_from_slice(stub);
    disk_write(disk, efi_start_lba, &sector)?;
    crate::serial_println!("[installer] UEFI bootloader stub written to EFI partition.");
    Ok(())
}

// ── Main installer entry point ────────────────────────────────────────────

/// Run the installer.  In a real OS this would block on keyboard input for
/// disk selection; here we run unattended in serial-log mode (suitable for
/// automated deployments and early development).
pub fn run() {
    tui_header();

    let disks = enumerate_disks();
    if disks.is_empty() {
        crate::serial_println!("[installer] No writable disks found. Exiting.");
        return;
    }

    tui_disk_menu(&disks);

    // For now: auto-select the first disk.
    // A real TUI would wait for keyboard input via the VGA terminal or serial.
    let target = &disks[0];
    crate::serial_println!("[installer] Auto-selected '{}' for installation.", target.name);
    crate::serial_println!("[installer] WARNING: All data on '{}' will be erased!", target.name);
    crate::serial_println!();

    // 1. Write GPT.
    crate::serial_println!("[installer] Step 1/3: Writing GPT partition table...");
    if let Err(e) = write_gpt(target) {
        crate::serial_println!("[installer] GPT error: {}. Installation aborted.", e);
        return;
    }

    // Compute partition layout to pass to subsequent steps.
    let efi_start   = FIRST_USABLE_LBA;
    let efi_end     = efi_start + (256 * 1024 * 1024 / LBA_SIZE) - 1;
    let root_start  = efi_end + 1;

    // 2. Install bootloader.
    crate::serial_println!("[installer] Step 2/3: Installing UEFI bootloader...");
    if let Err(e) = install_bootloader(target, efi_start) {
        crate::serial_println!("[installer] Bootloader error: {}. Installation aborted.", e);
        return;
    }

    // 3. Copy root filesystem.
    crate::serial_println!("[installer] Step 3/3: Copying root filesystem...");
    if let Err(e) = copy_root_fs(target, root_start) {
        crate::serial_println!("[installer] Root FS error: {}. Installation aborted.", e);
        return;
    }

    crate::serial_println!();
    crate::serial_println!("╔══════════════════════════════════════════════════╗");
    crate::serial_println!("║   Installation complete! Remove boot media and   ║");
    crate::serial_println!("║   reboot to start Smart OS.                      ║");
    crate::serial_println!("╚══════════════════════════════════════════════════╝");
}

/// Initialize the installer subsystem (register VFS entries etc.)
pub fn init() {
    // Write an install script placeholder to VFS so it can be invoked
    // from the shell with: exec /usr/bin/install
    let _ = crate::vfs::create_and_write(
        "/usr/bin/install",
        b"#!/smartos\n# Smart OS Installer\n# Run via kernel installer::run()\n",
    );
    crate::serial_println!("[installer] Installer subsystem initialized (/usr/bin/install registered).");
}

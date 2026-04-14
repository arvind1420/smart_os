/// On-Disk Persistent Filesystem for Smart OS.
///
/// A flat filesystem stored on a block device (NVMe or VirtIO).
/// files are accessed via `/disk/<filename>`. Supports up to 64 files.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;
use super::virtio_blk;
use super::nvme;

/// Filesystem magic number: "SMFT"
const DISKFS_MAGIC: u32 = 0x534D4654;
const DISKFS_VERSION: u32 = 1;
const MAX_FILES: usize = 64;
const BITMAP_START: u64 = 1;
const BITMAP_SECTORS: u64 = 4;
const ENTRY_START: u64 = 5;
const DATA_START: u64 = 69;
const MAX_NAME_LEN: usize = 120;
const SECTOR_SIZE: usize = 512;

pub struct EncryptionEngine {
    pub key: [u8; 32],
    pub enabled: bool,
}

impl EncryptionEngine {
    pub fn encrypt(&self, _lba: u64, data: &mut [u8]) {
        if !self.enabled { return; }
        for i in 0..data.len() { data[i] ^= self.key[i % 32]; }
    }
    pub fn decrypt(&self, _lba: u64, data: &mut [u8]) {
        if !self.enabled { return; }
        for i in 0..data.len() { data[i] ^= self.key[i % 32]; }
    }
}

pub static ENCRYPTION: spin::Mutex<EncryptionEngine> = spin::Mutex::new(EncryptionEngine {
    key: [0x55; 32],
    enabled: false,
});

fn read_sector_internal(lba: u64, buf: &mut [u8; 512]) -> Result<(), &'static str> {
    let mut success = false;
    
    // Try NVMe first
    {
        let mut nvme_list = nvme::NVME_DEVICES.lock();
        if let Some(ctrl) = nvme_list.get_mut(0) {
            use super::BlockDevice;
            if ctrl.read_blocks(lba, buf).is_ok() { success = true; }
        }
    }

    // Try AHCI second
    if !success {
        let mut ahci_list = super::ahci::AHCI_CONTROLLERS.lock();
        if let Some(ctrl) = ahci_list.get_mut(0) {
            if let Some(port) = ctrl.ports.get_mut(0) {
                if port.read_sectors(lba, buf).is_ok() { success = true; }
            }
        }
    }

    // Try Legacy IDE third
    if !success {
        let mut ide_ctrl = super::ide::IDE.lock();
        if !ide_ctrl.drives.is_empty() {
            if ide_ctrl.read_sectors(0, lba, buf).is_ok() { success = true; }
        }
    }

    // Fallback to VirtIO
    if !success {
        virtio_blk::read_sector(lba, buf)?;
    }

    ENCRYPTION.lock().decrypt(lba, buf);
    Ok(())
}

fn write_sector_internal(lba: u64, buf: &[u8; 512]) -> Result<(), &'static str> {
    let mut encrypted_buf = *buf;
    ENCRYPTION.lock().encrypt(lba, &mut encrypted_buf);

    // Try NVMe first
    {
        let mut nvme_list = nvme::NVME_DEVICES.lock();
        if let Some(ctrl) = nvme_list.get_mut(0) {
            use super::BlockDevice;
            return ctrl.write_blocks(lba, &encrypted_buf);
        }
    }

    // Try AHCI second
    let mut ahci_list = super::ahci::AHCI_CONTROLLERS.lock();
    if let Some(ctrl) = ahci_list.get_mut(0) {
        if let Some(port) = ctrl.ports.get_mut(0) {
            use super::BlockDevice;
            return port.write_blocks(lba, &encrypted_buf);
        }
    }

    // Try Legacy IDE third
    let mut ide_ctrl = super::ide::IDE.lock();
    if !ide_ctrl.drives.is_empty() {
        return ide_ctrl.write_sectors(0, lba, &encrypted_buf);
    }

    virtio_blk::write_sector(lba, &encrypted_buf)
}

/// On-disk superblock.
#[repr(C)]
#[derive(Clone, Copy)]
struct Superblock {
    magic: u32,
    version: u32,
    file_count: u32,
    total_sectors: u32,
    reserved: [u8; 496],
}

/// On-disk file entry.
#[repr(C)]
#[derive(Clone, Copy)]
struct FileEntry {
    name: [u8; MAX_NAME_LEN],
    flags: u32,
    size: u64,
    start_sector: u64,
    sector_count: u64,
    reserved: [u8; 360],
}

impl FileEntry {
    fn is_active(&self) -> bool {
        self.flags == 1
    }

    fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(MAX_NAME_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }

    fn set_name(&mut self, name: &str) {
        self.name = [0u8; MAX_NAME_LEN];
        let bytes = name.as_bytes();
        let len = bytes.len().min(MAX_NAME_LEN - 1);
        self.name[..len].copy_from_slice(&bytes[..len]);
    }
}

pub struct DiskFs {
    entries: Vec<FileEntry>,
    bitmap: Vec<u8>,
    total_sectors: u64,
}

pub static DISK_FS: Mutex<Option<DiskFs>> = Mutex::new(None);

impl DiskFs {
    fn read_superblock() -> Result<Superblock, &'static str> {
        let mut buf = [0u8; SECTOR_SIZE];
        read_sector_internal(0, &mut buf)?;
        let sb = unsafe { *(buf.as_ptr() as *const Superblock) };
        Ok(sb)
    }

    pub fn format(total_sectors: u64) -> Result<Self, &'static str> {
        let sb = Superblock {
            magic: DISKFS_MAGIC,
            version: DISKFS_VERSION,
            file_count: 0,
            total_sectors: total_sectors as u32,
            reserved: [0u8; 496],
        };
        let mut buf = [0u8; SECTOR_SIZE];
        unsafe { core::ptr::copy_nonoverlapping(&sb as *const Superblock as *const u8, buf.as_mut_ptr(), 512); }
        write_sector_internal(0, &buf)?;

        let empty = [0u8; SECTOR_SIZE];
        for i in 0..BITMAP_SECTORS { write_sector_internal(BITMAP_START + i, &empty)?; }
        for i in 0..MAX_FILES as u64 { write_sector_internal(ENTRY_START + i, &empty)?; }

        let mut bitmap = vec![0u8; (BITMAP_SECTORS as usize) * SECTOR_SIZE];
        for sector in 0..DATA_START {
            bitmap[sector as usize / 8] |= 1 << (sector % 8);
        }

        for i in 0..BITMAP_SECTORS as usize {
            let offset = i * SECTOR_SIZE;
            let sector_buf: &[u8; SECTOR_SIZE] = (&bitmap[offset..offset + SECTOR_SIZE]).try_into().unwrap();
            write_sector_internal(BITMAP_START + i as u64, sector_buf)?;
        }

        Ok(DiskFs { entries: vec![unsafe { core::mem::zeroed() }; MAX_FILES], bitmap, total_sectors })
    }

    pub fn mount() -> Result<Self, &'static str> {
        let sb = Self::read_superblock()?;
        if sb.magic != DISKFS_MAGIC { return Err("No filesystem found"); }
        let total_sectors = sb.total_sectors as u64;

        let mut bitmap = vec![0u8; (BITMAP_SECTORS as usize) * SECTOR_SIZE];
        for i in 0..BITMAP_SECTORS as usize {
            let offset = i * SECTOR_SIZE;
            let sector_buf: &mut [u8; SECTOR_SIZE] = (&mut bitmap[offset..offset + SECTOR_SIZE]).try_into().unwrap();
            read_sector_internal(BITMAP_START + i as u64, sector_buf)?;
        }

        let mut entries = Vec::with_capacity(MAX_FILES);
        for i in 0..MAX_FILES {
            let mut buf = [0u8; SECTOR_SIZE];
            read_sector_internal(ENTRY_START + i as u64, &mut buf)?;
            entries.push(unsafe { *(buf.as_ptr() as *const FileEntry) });
        }

        Ok(DiskFs { entries, bitmap, total_sectors })
    }

    pub fn list_files(&self) -> Vec<String> {
        self.entries.iter().filter(|e| e.is_active()).map(|e| String::from(e.name_str())).collect()
    }

    pub fn read_file(&self, name: &str) -> Result<Vec<u8>, &'static str> {
        let entry = self.entries.iter().find(|e| e.is_active() && e.name_str() == name).ok_or("File not found")?;
        let mut data = vec![0u8; entry.size as usize];
        let mut buf = [0u8; SECTOR_SIZE];
        for i in 0..entry.sector_count as usize {
            read_sector_internal(entry.start_sector + i as u64, &mut buf)?;
            let offset = i * SECTOR_SIZE;
            let copy_len = (entry.size as usize - offset).min(SECTOR_SIZE);
            data[offset..offset + copy_len].copy_from_slice(&buf[..copy_len]);
        }
        Ok(data)
    }

    pub fn write_file(&mut self, name: &str, data: &[u8]) -> Result<(), &'static str> {
        if self.entries.iter().any(|e| e.is_active() && e.name_str() == name) { self.delete_file(name)?; }
        let slot = self.entries.iter().position(|e| !e.is_active()).ok_or("No free file entries")?;
        let sectors_needed = (data.len() + SECTOR_SIZE - 1) / SECTOR_SIZE;
        let count = sectors_needed.max(1);
        let start = self.alloc_sectors(count as u64).ok_or("No free sectors")?;

        for i in 0..sectors_needed {
            let mut buf = [0u8; SECTOR_SIZE];
            let offset = i * SECTOR_SIZE;
            let copy_len = (data.len() - offset).min(SECTOR_SIZE);
            buf[..copy_len].copy_from_slice(&data[offset..offset + copy_len]);
            write_sector_internal(start + i as u64, &buf)?;
        }

        self.entries[slot].set_name(name);
        self.entries[slot].flags = 1;
        self.entries[slot].size = data.len() as u64;
        self.entries[slot].start_sector = start;
        self.entries[slot].sector_count = count as u64;

        self.sync_entry(slot)?;
        self.sync_bitmap()?;
        self.sync_superblock()?;
        Ok(())
    }

    pub fn delete_file(&mut self, name: &str) -> Result<(), &'static str> {
        let slot = self.entries.iter().position(|e| e.is_active() && e.name_str() == name).ok_or("File not found")?;
        self.free_sectors(self.entries[slot].start_sector, self.entries[slot].sector_count);
        self.entries[slot] = unsafe { core::mem::zeroed() };
        self.sync_entry(slot)?;
        self.sync_bitmap()?;
        self.sync_superblock()?;
        Ok(())
    }

    fn alloc_sectors(&mut self, count: u64) -> Option<u64> {
        let mut run_start = DATA_START;
        let mut run_len = 0u64;
        for sector in DATA_START..self.total_sectors {
            if self.bitmap[sector as usize / 8] & (1 << (sector % 8)) == 0 {
                if run_len == 0 { run_start = sector; }
                run_len += 1;
                if run_len >= count {
                    for s in run_start..run_start + count { self.bitmap[s as usize / 8] |= 1 << (s % 8); }
                    return Some(run_start);
                }
            } else { run_len = 0; }
        }
        None
    }

    fn free_sectors(&mut self, start: u64, count: u64) {
        for s in start..start + count { self.bitmap[s as usize / 8] &= !(1 << (s % 8)); }
    }

    fn sync_entry(&self, slot: usize) -> Result<(), &'static str> {
        let mut buf = [0u8; SECTOR_SIZE];
        unsafe { core::ptr::copy_nonoverlapping(&self.entries[slot] as *const FileEntry as *const u8, buf.as_mut_ptr(), 512); }
        write_sector_internal(ENTRY_START + slot as u64, &buf)
    }

    fn sync_bitmap(&self) -> Result<(), &'static str> {
        for i in 0..BITMAP_SECTORS as usize {
            let offset = i * SECTOR_SIZE;
            let sector_buf: &[u8; SECTOR_SIZE] = (&self.bitmap[offset..offset + SECTOR_SIZE]).try_into().unwrap();
            write_sector_internal(BITMAP_START + i as u64, sector_buf)?;
        }
        Ok(())
    }

    fn sync_superblock(&self) -> Result<(), &'static str> {
        let sb = Superblock {
            magic: DISKFS_MAGIC,
            version: DISKFS_VERSION,
            file_count: self.entries.iter().filter(|e| e.is_active()).count() as u32,
            total_sectors: self.total_sectors as u32,
            reserved: [0u8; 496],
        };
        let mut buf = [0u8; SECTOR_SIZE];
        unsafe { core::ptr::copy_nonoverlapping(&sb as *const Superblock as *const u8, buf.as_mut_ptr(), 512); }
        write_sector_internal(0, &buf)
    }
}

pub fn init() {
    let mut available = false;
    let mut total = 0;
    {
        let nvme_list = nvme::NVME_DEVICES.lock();
        if let Some(ctrl) = nvme_list.get(0) {
            use super::BlockDevice;
            available = true;
            total = ctrl.capacity();
            crate::serial_println!("[diskfs] Using NVMe primary.");
        }
    }
    if !available {
        let ahci_list = super::ahci::AHCI_CONTROLLERS.lock();
        if let Some(ctrl) = ahci_list.get(0) {
            if let Some(port) = ctrl.ports.get(0) {
                use super::BlockDevice;
                available = true;
                // Treat AHCI port as block device to get capacity
                total = 1024 * 1024 * 200; // 100GB dummy for now
                crate::serial_println!("[diskfs] Using AHCI primary.");
            }
        }
    }
    if !available {
        if super::xhci::is_available() {
            let devices = super::xhci::device_list();
            if let Some(usb_dev) = devices.iter().find(|d| d.is_mass_storage) {
                available = true;
                total = 1024 * 1024 * 32; // 16GB dummy for USB
                crate::serial_println!("[diskfs] Using USB Mass Storage (slot {}) primary.", usb_dev.slot_id);
            }
        }
    }
    if !available {
        if super::ide::is_available() {
            available = true;
            total = super::ide::IDE.lock().drives[0].sectors;
            crate::serial_println!("[diskfs] Using Legacy IDE primary.");
        }
    }
    if !available && virtio_blk::is_available() {
        available = true;
        total = virtio_blk::capacity();
        crate::serial_println!("[diskfs] Using VirtIO primary.");
    }
    if !available { return; }

    match DiskFs::mount() {
        Ok(fs) => { *DISK_FS.lock() = Some(fs); }
        Err(_) => {
            if total == 0 { return; }
            if let Ok(fs) = DiskFs::format(total) {
                *DISK_FS.lock() = Some(fs);
                crate::serial_println!("[diskfs] Formatted and mounted.");
            }
        }
    }
}

pub fn is_available() -> bool { DISK_FS.lock().is_some() }

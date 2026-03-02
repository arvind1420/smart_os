/// On-Disk Persistent Filesystem for Smart OS.
///
/// A flat filesystem stored on a VirtIO block device. No subdirectories —
/// files are accessed via `/disk/<filename>`. Supports up to 64 files.
///
/// Layout:
///   Sector 0:     Superblock (magic, version, file_count, total_sectors)
///   Sectors 1-4:  Allocation bitmap (covers up to 16384 sectors = 8 MiB)
///   Sectors 5-68: File entries (64 entries, 1 sector each)
///   Sectors 69+:  Data area

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;
use super::virtio_blk;

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

/// On-disk superblock (fits in first 512 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct Superblock {
    magic: u32,
    version: u32,
    file_count: u32,
    total_sectors: u32,
    reserved: [u8; 496],
}

/// On-disk file entry (exactly 512 bytes = 1 sector).
#[repr(C)]
#[derive(Clone, Copy)]
struct FileEntry {
    name: [u8; MAX_NAME_LEN],   // null-terminated UTF-8
    flags: u32,                  // 0 = free, 1 = active
    size: u64,                   // file size in bytes
    start_sector: u64,           // first data sector
    sector_count: u64,           // allocated sector count
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

/// In-memory disk filesystem state.
pub struct DiskFs {
    entries: Vec<FileEntry>,
    bitmap: Vec<u8>,
    total_sectors: u64,
}

/// Global disk filesystem instance.
pub static DISK_FS: Mutex<Option<DiskFs>> = Mutex::new(None);

impl DiskFs {
    /// Read the superblock and check for a valid filesystem.
    fn read_superblock() -> Result<Superblock, &'static str> {
        let mut buf = [0u8; SECTOR_SIZE];
        virtio_blk::read_sector(0, &mut buf)?;
        let sb = unsafe { *(buf.as_ptr() as *const Superblock) };
        Ok(sb)
    }

    /// Format the disk with an empty filesystem.
    pub fn format(total_sectors: u64) -> Result<Self, &'static str> {
        crate::serial_println!("[diskfs] Formatting disk ({} sectors)...", total_sectors);

        // Write superblock
        let sb = Superblock {
            magic: DISKFS_MAGIC,
            version: DISKFS_VERSION,
            file_count: 0,
            total_sectors: total_sectors as u32,
            reserved: [0u8; 496],
        };
        let mut buf = [0u8; SECTOR_SIZE];
        unsafe {
            core::ptr::copy_nonoverlapping(
                &sb as *const Superblock as *const u8,
                buf.as_mut_ptr(),
                core::mem::size_of::<Superblock>(),
            );
        }
        virtio_blk::write_sector(0, &buf)?;

        // Write empty bitmap (4 sectors)
        let empty = [0u8; SECTOR_SIZE];
        for i in 0..BITMAP_SECTORS {
            virtio_blk::write_sector(BITMAP_START + i, &empty)?;
        }

        // Write empty file entries (64 sectors)
        for i in 0..MAX_FILES as u64 {
            virtio_blk::write_sector(ENTRY_START + i, &empty)?;
        }

        // Mark metadata sectors as used in bitmap
        let mut bitmap = vec![0u8; (BITMAP_SECTORS as usize) * SECTOR_SIZE];
        for sector in 0..DATA_START {
            let byte_idx = sector as usize / 8;
            let bit_idx = sector as u8 % 8;
            if byte_idx < bitmap.len() {
                bitmap[byte_idx] |= 1 << bit_idx;
            }
        }

        // Write bitmap back
        for i in 0..BITMAP_SECTORS as usize {
            let offset = i * SECTOR_SIZE;
            let sector_buf: &[u8; SECTOR_SIZE] = (&bitmap[offset..offset + SECTOR_SIZE])
                .try_into().map_err(|_| "bitmap slice error")?;
            virtio_blk::write_sector(BITMAP_START + i as u64, sector_buf)?;
        }

        let entries = vec![unsafe { core::mem::zeroed::<FileEntry>() }; MAX_FILES];

        Ok(DiskFs {
            entries,
            bitmap,
            total_sectors,
        })
    }

    /// Mount: read superblock, entries, and bitmap into memory.
    pub fn mount() -> Result<Self, &'static str> {
        let sb = Self::read_superblock()?;
        if sb.magic != DISKFS_MAGIC {
            return Err("No filesystem found");
        }
        let total_sectors = sb.total_sectors as u64;

        // Read bitmap
        let mut bitmap = vec![0u8; (BITMAP_SECTORS as usize) * SECTOR_SIZE];
        for i in 0..BITMAP_SECTORS as usize {
            let offset = i * SECTOR_SIZE;
            let sector_buf: &mut [u8; SECTOR_SIZE] = (&mut bitmap[offset..offset + SECTOR_SIZE])
                .try_into().map_err(|_| "bitmap slice error")?;
            virtio_blk::read_sector(BITMAP_START + i as u64, sector_buf)?;
        }

        // Read file entries
        let mut entries = Vec::with_capacity(MAX_FILES);
        for i in 0..MAX_FILES {
            let mut buf = [0u8; SECTOR_SIZE];
            virtio_blk::read_sector(ENTRY_START + i as u64, &mut buf)?;
            let entry = unsafe { *(buf.as_ptr() as *const FileEntry) };
            entries.push(entry);
        }

        let active_count = entries.iter().filter(|e| e.is_active()).count();
        crate::serial_println!(
            "[diskfs] Mounted: {} files, {} sectors total",
            active_count, total_sectors
        );

        Ok(DiskFs { entries, bitmap, total_sectors })
    }

    /// List all active file names.
    pub fn list_files(&self) -> Vec<String> {
        self.entries.iter()
            .filter(|e| e.is_active())
            .map(|e| String::from(e.name_str()))
            .collect()
    }

    /// Read a file by name.
    pub fn read_file(&self, name: &str) -> Result<Vec<u8>, &'static str> {
        let entry = self.entries.iter()
            .find(|e| e.is_active() && e.name_str() == name)
            .ok_or("File not found")?;

        let mut data = vec![0u8; entry.size as usize];
        let sector_count = entry.sector_count as usize;
        let mut buf = [0u8; SECTOR_SIZE];

        for i in 0..sector_count {
            virtio_blk::read_sector(entry.start_sector + i as u64, &mut buf)?;
            let offset = i * SECTOR_SIZE;
            let remaining = entry.size as usize - offset;
            let copy_len = remaining.min(SECTOR_SIZE);
            if offset + copy_len <= data.len() {
                data[offset..offset + copy_len].copy_from_slice(&buf[..copy_len]);
            }
        }

        Ok(data)
    }

    /// Write (create or overwrite) a file.
    pub fn write_file(&mut self, name: &str, data: &[u8]) -> Result<(), &'static str> {
        // If file exists, delete it first
        if self.entries.iter().any(|e| e.is_active() && e.name_str() == name) {
            self.delete_file(name)?;
        }

        // Find a free entry slot
        let slot = self.entries.iter().position(|e| !e.is_active())
            .ok_or("No free file entries")?;

        // Calculate sectors needed
        let sectors_needed = (data.len() + SECTOR_SIZE - 1) / SECTOR_SIZE;
        if sectors_needed == 0 {
            // Empty file — still allocate 1 sector
            let start = self.alloc_sectors(1).ok_or("No free sectors")?;
            let empty = [0u8; SECTOR_SIZE];
            virtio_blk::write_sector(start, &empty)?;

            self.entries[slot].set_name(name);
            self.entries[slot].flags = 1;
            self.entries[slot].size = 0;
            self.entries[slot].start_sector = start;
            self.entries[slot].sector_count = 1;
        } else {
            let start = self.alloc_sectors(sectors_needed as u64)
                .ok_or("Not enough free sectors")?;

            // Write data sectors
            for i in 0..sectors_needed {
                let mut buf = [0u8; SECTOR_SIZE];
                let offset = i * SECTOR_SIZE;
                let remaining = data.len() - offset;
                let copy_len = remaining.min(SECTOR_SIZE);
                buf[..copy_len].copy_from_slice(&data[offset..offset + copy_len]);
                virtio_blk::write_sector(start + i as u64, &buf)?;
            }

            self.entries[slot].set_name(name);
            self.entries[slot].flags = 1;
            self.entries[slot].size = data.len() as u64;
            self.entries[slot].start_sector = start;
            self.entries[slot].sector_count = sectors_needed as u64;
        }

        self.sync_entry(slot)?;
        self.sync_bitmap()?;
        self.sync_superblock()?;

        Ok(())
    }

    /// Delete a file by name.
    pub fn delete_file(&mut self, name: &str) -> Result<(), &'static str> {
        let slot = self.entries.iter().position(|e| e.is_active() && e.name_str() == name)
            .ok_or("File not found")?;

        let start = self.entries[slot].start_sector;
        let count = self.entries[slot].sector_count;

        // Free sectors in bitmap
        self.free_sectors(start, count);

        // Clear entry
        self.entries[slot] = unsafe { core::mem::zeroed() };

        self.sync_entry(slot)?;
        self.sync_bitmap()?;
        self.sync_superblock()?;

        Ok(())
    }

    /// Allocate `count` contiguous sectors from bitmap.
    fn alloc_sectors(&mut self, count: u64) -> Option<u64> {
        let total = self.total_sectors.min(self.bitmap.len() as u64 * 8);
        let mut run_start = DATA_START;
        let mut run_len = 0u64;

        for sector in DATA_START..total {
            let byte_idx = sector as usize / 8;
            let bit_idx = sector as u8 % 8;
            if byte_idx >= self.bitmap.len() {
                break;
            }
            if self.bitmap[byte_idx] & (1 << bit_idx) == 0 {
                if run_len == 0 {
                    run_start = sector;
                }
                run_len += 1;
                if run_len >= count {
                    // Mark sectors as used
                    for s in run_start..run_start + count {
                        let bi = s as usize / 8;
                        let bb = s as u8 % 8;
                        self.bitmap[bi] |= 1 << bb;
                    }
                    return Some(run_start);
                }
            } else {
                run_len = 0;
            }
        }
        None
    }

    /// Free sectors in bitmap.
    fn free_sectors(&mut self, start: u64, count: u64) {
        for s in start..start + count {
            let bi = s as usize / 8;
            let bb = s as u8 % 8;
            if bi < self.bitmap.len() {
                self.bitmap[bi] &= !(1 << bb);
            }
        }
    }

    /// Write a file entry back to disk.
    fn sync_entry(&self, slot: usize) -> Result<(), &'static str> {
        let mut buf = [0u8; SECTOR_SIZE];
        unsafe {
            core::ptr::copy_nonoverlapping(
                &self.entries[slot] as *const FileEntry as *const u8,
                buf.as_mut_ptr(),
                core::mem::size_of::<FileEntry>().min(SECTOR_SIZE),
            );
        }
        virtio_blk::write_sector(ENTRY_START + slot as u64, &buf)
    }

    /// Write bitmap back to disk.
    fn sync_bitmap(&self) -> Result<(), &'static str> {
        for i in 0..BITMAP_SECTORS as usize {
            let offset = i * SECTOR_SIZE;
            let sector_buf: &[u8; SECTOR_SIZE] = (&self.bitmap[offset..offset + SECTOR_SIZE])
                .try_into().map_err(|_| "bitmap sync error")?;
            virtio_blk::write_sector(BITMAP_START + i as u64, sector_buf)?;
        }
        Ok(())
    }

    /// Write superblock (update file count).
    fn sync_superblock(&self) -> Result<(), &'static str> {
        let file_count = self.entries.iter().filter(|e| e.is_active()).count() as u32;
        let sb = Superblock {
            magic: DISKFS_MAGIC,
            version: DISKFS_VERSION,
            file_count,
            total_sectors: self.total_sectors as u32,
            reserved: [0u8; 496],
        };
        let mut buf = [0u8; SECTOR_SIZE];
        unsafe {
            core::ptr::copy_nonoverlapping(
                &sb as *const Superblock as *const u8,
                buf.as_mut_ptr(),
                core::mem::size_of::<Superblock>(),
            );
        }
        virtio_blk::write_sector(0, &buf)
    }
}

/// Initialize the disk filesystem: mount if formatted, format if not.
pub fn init() {
    if !virtio_blk::is_available() {
        crate::serial_println!("[diskfs] No block device — skipping disk filesystem.");
        return;
    }

    match DiskFs::mount() {
        Ok(fs) => {
            *DISK_FS.lock() = Some(fs);
        }
        Err(_) => {
            let total = virtio_blk::capacity();
            if total == 0 {
                crate::serial_println!("[diskfs] Block device has zero capacity.");
                return;
            }
            match DiskFs::format(total) {
                Ok(fs) => {
                    *DISK_FS.lock() = Some(fs);
                    crate::serial_println!("[diskfs] Formatted and mounted.");
                }
                Err(e) => {
                    crate::serial_println!("[diskfs] Format failed: {}", e);
                }
            }
        }
    }
}

/// Check if disk filesystem is available.
pub fn is_available() -> bool {
    DISK_FS.lock().is_some()
}

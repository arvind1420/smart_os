/// FAT32 Filesystem Driver for Smart OS.
///
/// Hand-written FAT32 parser supporting read/write on VirtIO-blk.
/// Supports: BPB parsing, FAT chain traversal, 8.3 directory entries,
/// file read/write/create, directory listing.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use spin::Mutex;

const SECTOR_SIZE: usize = 512;

/// FAT32 end-of-chain markers.
const FAT32_EOC: u32 = 0x0FFF_FFF8;
const FAT32_FREE: u32 = 0x0000_0000;

/// Directory entry attribute flags.
const ATTR_READ_ONLY: u8 = 0x01;
const ATTR_HIDDEN: u8 = 0x02;
const ATTR_SYSTEM: u8 = 0x04;
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_LONG_NAME: u8 = 0x0F;

// ═══════════════════════════════════════════════════════════════
//  BIOS Parameter Block
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct Fat32Bpb {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    pub total_sectors: u32,
    pub fat_size: u32,
    pub root_cluster: u32,
}

// ═══════════════════════════════════════════════════════════════
//  Directory Entry (32 bytes)
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct DirEntry {
    pub name: [u8; 11],
    pub attr: u8,
    pub cluster_hi: u16,
    pub cluster_lo: u16,
    pub file_size: u32,
}

impl DirEntry {
    fn start_cluster(&self) -> u32 {
        ((self.cluster_hi as u32) << 16) | (self.cluster_lo as u32)
    }

    fn is_free(&self) -> bool {
        self.name[0] == 0x00 || self.name[0] == 0xE5
    }

    fn is_long_name(&self) -> bool {
        self.attr == ATTR_LONG_NAME
    }

    fn is_volume_id(&self) -> bool {
        self.attr & ATTR_VOLUME_ID != 0 && self.attr & ATTR_DIRECTORY == 0
    }

    fn is_directory(&self) -> bool {
        self.attr & ATTR_DIRECTORY != 0
    }
}

/// Parse a 32-byte directory entry from raw bytes.
fn parse_dir_entry(data: &[u8]) -> DirEntry {
    let mut name = [0u8; 11];
    name.copy_from_slice(&data[0..11]);
    DirEntry {
        name,
        attr: data[11],
        cluster_hi: u16::from_le_bytes([data[20], data[21]]),
        cluster_lo: u16::from_le_bytes([data[26], data[27]]),
        file_size: u32::from_le_bytes([data[28], data[29], data[30], data[31]]),
    }
}

// ═══════════════════════════════════════════════════════════════
//  FAT32 Filesystem
// ═══════════════════════════════════════════════════════════════

pub struct Fat32Fs {
    bpb: Fat32Bpb,
    fat_start_sector: u64,
    data_start_sector: u64,
    fat_cache: Vec<u32>,
}

pub static FAT32_FS: Mutex<Option<Fat32Fs>> = Mutex::new(None);

impl Fat32Fs {
    /// Mount a FAT32 filesystem by reading the boot sector.
    pub fn mount() -> Result<Self, &'static str> {
        let mut sector = [0u8; SECTOR_SIZE];
        super::virtio_blk::read_sector(0, &mut sector)?;

        // Validate boot signature
        if sector[510] != 0x55 || sector[511] != 0xAA {
            return Err("Invalid boot signature");
        }

        let bpb = Self::parse_bpb(&sector)?;

        // Validate FAT32 specifics
        if bpb.bytes_per_sector != 512 {
            return Err("Unsupported sector size (need 512)");
        }
        if bpb.sectors_per_cluster == 0 {
            return Err("Invalid sectors per cluster");
        }

        let fat_start = bpb.reserved_sectors as u64;
        let data_start = fat_start + (bpb.num_fats as u64 * bpb.fat_size as u64);

        let mut fs = Fat32Fs {
            bpb,
            fat_start_sector: fat_start,
            data_start_sector: data_start,
            fat_cache: Vec::new(),
        };

        // Load FAT into memory (for small disks)
        fs.load_fat()?;

        Ok(fs)
    }

    fn parse_bpb(sector: &[u8; SECTOR_SIZE]) -> Result<Fat32Bpb, &'static str> {
        let bytes_per_sector = u16::from_le_bytes([sector[11], sector[12]]);
        let sectors_per_cluster = sector[13];
        let reserved_sectors = u16::from_le_bytes([sector[14], sector[15]]);
        let num_fats = sector[16];

        // FAT32: total_sectors_16 is 0, use total_sectors_32
        let total_sectors = u32::from_le_bytes([sector[32], sector[33], sector[34], sector[35]]);
        let fat_size = u32::from_le_bytes([sector[36], sector[37], sector[38], sector[39]]);
        let root_cluster = u32::from_le_bytes([sector[44], sector[45], sector[46], sector[47]]);

        if fat_size == 0 {
            return Err("Not a FAT32 filesystem");
        }

        Ok(Fat32Bpb {
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sectors,
            num_fats,
            total_sectors,
            fat_size,
            root_cluster,
        })
    }

    /// Load the entire FAT table into memory.
    fn load_fat(&mut self) -> Result<(), &'static str> {
        let fat_sectors = self.bpb.fat_size as usize;
        // Limit to first 256 sectors of FAT (128K entries = 512MB of data)
        let sectors_to_read = fat_sectors.min(256);
        let entries = sectors_to_read * (SECTOR_SIZE / 4);
        self.fat_cache = vec![0u32; entries];

        let mut sector = [0u8; SECTOR_SIZE];
        for s in 0..sectors_to_read {
            super::virtio_blk::read_sector(self.fat_start_sector + s as u64, &mut sector)?;
            for i in 0..(SECTOR_SIZE / 4) {
                let idx = s * (SECTOR_SIZE / 4) + i;
                if idx < entries {
                    self.fat_cache[idx] = u32::from_le_bytes([
                        sector[i * 4],
                        sector[i * 4 + 1],
                        sector[i * 4 + 2],
                        sector[i * 4 + 3],
                    ]) & 0x0FFF_FFFF; // mask top 4 bits
                }
            }
        }
        Ok(())
    }

    /// Get the next cluster in the chain, or None if end-of-chain.
    fn next_cluster(&self, cluster: u32) -> Option<u32> {
        if cluster < 2 || cluster as usize >= self.fat_cache.len() {
            return None;
        }
        let next = self.fat_cache[cluster as usize];
        if next >= FAT32_EOC || next < 2 {
            None
        } else {
            Some(next)
        }
    }

    /// Convert a cluster number to its first sector.
    fn cluster_to_sector(&self, cluster: u32) -> u64 {
        self.data_start_sector + ((cluster - 2) as u64 * self.bpb.sectors_per_cluster as u64)
    }

    /// Read all data from a cluster chain.
    fn read_chain(&self, start_cluster: u32) -> Result<Vec<u8>, &'static str> {
        let cluster_bytes = self.bpb.sectors_per_cluster as usize * SECTOR_SIZE;
        let mut result = Vec::new();
        let mut cluster = start_cluster;
        let mut safety = 0u32;

        loop {
            let sector = self.cluster_to_sector(cluster);
            let mut buf = vec![0u8; cluster_bytes];
            for s in 0..self.bpb.sectors_per_cluster as usize {
                let mut sec = [0u8; SECTOR_SIZE];
                super::virtio_blk::read_sector(sector + s as u64, &mut sec)?;
                buf[s * SECTOR_SIZE..(s + 1) * SECTOR_SIZE].copy_from_slice(&sec);
            }
            result.extend_from_slice(&buf);

            match self.next_cluster(cluster) {
                Some(next) => cluster = next,
                None => break,
            }

            safety += 1;
            if safety > 65536 {
                return Err("FAT chain too long");
            }
        }

        Ok(result)
    }

    /// Parse directory entries from a cluster chain.
    fn read_dir_entries(&self, dir_cluster: u32) -> Result<Vec<(String, DirEntry)>, &'static str> {
        let raw = self.read_chain(dir_cluster)?;
        let mut entries = Vec::new();

        let mut i = 0;
        while i + 32 <= raw.len() {
            let entry = parse_dir_entry(&raw[i..i + 32]);
            if entry.name[0] == 0x00 {
                break; // End of directory
            }
            i += 32;

            if entry.is_free() || entry.is_long_name() || entry.is_volume_id() {
                continue;
            }

            // Skip . and .. entries
            if entry.name[0] == b'.' {
                continue;
            }

            let name = from_short_name(&entry.name);
            entries.push((name, entry));
        }

        Ok(entries)
    }

    /// List root directory.
    pub fn list_root(&self) -> Result<Vec<String>, &'static str> {
        let entries = self.read_dir_entries(self.bpb.root_cluster)?;
        Ok(entries.into_iter().map(|(name, entry)| {
            if entry.is_directory() {
                format!("{}/", name)
            } else {
                name
            }
        }).collect())
    }

    /// List a subdirectory by path.
    pub fn list_dir(&self, path: &str) -> Result<Vec<String>, &'static str> {
        if path.is_empty() || path == "/" {
            return self.list_root();
        }

        let cluster = self.find_dir_cluster(path)?;
        let entries = self.read_dir_entries(cluster)?;
        Ok(entries.into_iter().map(|(name, entry)| {
            if entry.is_directory() {
                format!("{}/", name)
            } else {
                name
            }
        }).collect())
    }

    /// Find the starting cluster of a directory by path.
    fn find_dir_cluster(&self, path: &str) -> Result<u32, &'static str> {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut cluster = self.bpb.root_cluster;

        for part in parts {
            let entries = self.read_dir_entries(cluster)?;
            let target = part.to_uppercase_simple();
            let mut found = false;
            for (name, entry) in &entries {
                if name.to_uppercase_simple() == target && entry.is_directory() {
                    cluster = entry.start_cluster();
                    found = true;
                    break;
                }
            }
            if !found {
                return Err("Directory not found");
            }
        }

        Ok(cluster)
    }

    /// Read a file by path.
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, &'static str> {
        let (dir_path, filename) = split_path(path);
        let dir_cluster = if dir_path.is_empty() {
            self.bpb.root_cluster
        } else {
            self.find_dir_cluster(dir_path)?
        };

        let entries = self.read_dir_entries(dir_cluster)?;
        let target = filename.to_uppercase_simple();
        for (name, entry) in &entries {
            if name.to_uppercase_simple() == target && !entry.is_directory() {
                let mut data = self.read_chain(entry.start_cluster())?;
                data.truncate(entry.file_size as usize);
                return Ok(data);
            }
        }

        Err("File not found")
    }

    /// Write a file (create or overwrite).
    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), &'static str> {
        let cluster_bytes = self.bpb.sectors_per_cluster as usize * SECTOR_SIZE;
        let clusters_needed = if data.is_empty() { 1 } else { (data.len() + cluster_bytes - 1) / cluster_bytes };

        // Allocate clusters
        let start_cluster = self.alloc_clusters(clusters_needed)?;

        // Write data to clusters
        let mut cluster = start_cluster;
        let mut offset = 0usize;
        for _ in 0..clusters_needed {
            let sector = self.cluster_to_sector(cluster);
            for s in 0..self.bpb.sectors_per_cluster as usize {
                let mut sec = [0u8; SECTOR_SIZE];
                let end = (offset + SECTOR_SIZE).min(data.len());
                if offset < data.len() {
                    let len = end - offset;
                    sec[..len].copy_from_slice(&data[offset..end]);
                }
                super::virtio_blk::write_sector(sector + s as u64, &sec)?;
                offset += SECTOR_SIZE;
            }
            cluster = match self.next_cluster(cluster) {
                Some(c) => c,
                None => break,
            };
        }

        // Update FAT on disk
        self.update_fat_on_disk()?;

        // Update or create directory entry
        let (dir_path, filename) = split_path(path);
        let dir_cluster = if dir_path.is_empty() {
            self.bpb.root_cluster
        } else {
            self.find_dir_cluster(dir_path)?
        };
        self.update_or_create_entry(dir_cluster, filename, start_cluster, data.len() as u32)?;

        Ok(())
    }

    /// Allocate a chain of clusters from the FAT.
    fn alloc_clusters(&mut self, count: usize) -> Result<u32, &'static str> {
        let mut allocated = Vec::new();
        for i in 2..self.fat_cache.len() {
            if self.fat_cache[i] == FAT32_FREE {
                allocated.push(i as u32);
                if allocated.len() >= count {
                    break;
                }
            }
        }
        if allocated.len() < count {
            return Err("No free clusters");
        }

        // Chain the allocated clusters
        for w in 0..allocated.len() - 1 {
            self.fat_cache[allocated[w] as usize] = allocated[w + 1];
        }
        // Mark last cluster as end-of-chain
        self.fat_cache[*allocated.last().unwrap() as usize] = FAT32_EOC;

        Ok(allocated[0])
    }

    /// Write the FAT cache back to disk.
    fn update_fat_on_disk(&self) -> Result<(), &'static str> {
        let entries_per_sector = SECTOR_SIZE / 4;
        let sectors = (self.fat_cache.len() + entries_per_sector - 1) / entries_per_sector;

        for s in 0..sectors {
            let mut sec = [0u8; SECTOR_SIZE];
            for i in 0..entries_per_sector {
                let idx = s * entries_per_sector + i;
                if idx < self.fat_cache.len() {
                    let bytes = self.fat_cache[idx].to_le_bytes();
                    sec[i * 4..i * 4 + 4].copy_from_slice(&bytes);
                }
            }
            super::virtio_blk::write_sector(self.fat_start_sector + s as u64, &sec)?;
        }
        Ok(())
    }

    /// Update or create a directory entry.
    fn update_or_create_entry(
        &self,
        dir_cluster: u32,
        filename: &str,
        start_cluster: u32,
        file_size: u32,
    ) -> Result<(), &'static str> {
        let short_name = to_short_name(filename);
        let cluster_bytes = self.bpb.sectors_per_cluster as usize * SECTOR_SIZE;
        let raw = self.read_chain(dir_cluster)?;

        // Search for existing entry or free slot
        let mut target_offset = None;
        let mut i = 0;
        while i + 32 <= raw.len() {
            let entry = parse_dir_entry(&raw[i..i + 32]);
            if entry.name[0] == 0x00 || entry.name[0] == 0xE5 {
                if target_offset.is_none() {
                    target_offset = Some(i);
                }
                if entry.name[0] == 0x00 {
                    break;
                }
            } else if entry.name == short_name {
                target_offset = Some(i);
                break;
            }
            i += 32;
        }

        let offset = target_offset.ok_or("Directory full")?;

        // Build the 32-byte entry
        let mut entry_bytes = [0u8; 32];
        entry_bytes[0..11].copy_from_slice(&short_name);
        entry_bytes[11] = 0x20; // ATTR_ARCHIVE
        entry_bytes[20] = (start_cluster >> 16) as u8;
        entry_bytes[21] = (start_cluster >> 24) as u8;
        entry_bytes[26] = start_cluster as u8;
        entry_bytes[27] = (start_cluster >> 8) as u8;
        entry_bytes[28..32].copy_from_slice(&file_size.to_le_bytes());

        // Write back to disk
        let cluster_idx = offset / cluster_bytes;
        let offset_in_cluster = offset % cluster_bytes;
        let sector_idx = offset_in_cluster / SECTOR_SIZE;
        let offset_in_sector = offset_in_cluster % SECTOR_SIZE;

        // Walk to the right cluster
        let mut cluster = dir_cluster;
        for _ in 0..cluster_idx {
            cluster = self.next_cluster(cluster).ok_or("Dir chain broken")?;
        }

        let sector = self.cluster_to_sector(cluster) + sector_idx as u64;
        let mut sec = [0u8; SECTOR_SIZE];
        super::virtio_blk::read_sector(sector, &mut sec)?;
        sec[offset_in_sector..offset_in_sector + 32].copy_from_slice(&entry_bytes);
        super::virtio_blk::write_sector(sector, &sec)?;

        Ok(())
    }

    /// Get filesystem info.
    pub fn info(&self) -> (u64, u8, u32) {
        let total = self.bpb.total_sectors as u64 * self.bpb.bytes_per_sector as u64;
        (total, self.bpb.sectors_per_cluster, self.bpb.root_cluster)
    }
}

// ═══════════════════════════════════════════════════════════════
//  Short Name Helpers
// ═══════════════════════════════════════════════════════════════

/// Convert a filename to 8.3 short name format.
fn to_short_name(name: &str) -> [u8; 11] {
    let mut result = [b' '; 11];
    let upper = name.to_uppercase_simple();
    let bytes = upper.as_bytes();

    if let Some(dot_pos) = bytes.iter().position(|&b| b == b'.') {
        // Name part (up to 8 chars)
        let name_len = dot_pos.min(8);
        result[..name_len].copy_from_slice(&bytes[..name_len]);
        // Extension (up to 3 chars)
        let ext_start = dot_pos + 1;
        let ext_len = (bytes.len() - ext_start).min(3);
        result[8..8 + ext_len].copy_from_slice(&bytes[ext_start..ext_start + ext_len]);
    } else {
        // No extension
        let name_len = bytes.len().min(8);
        result[..name_len].copy_from_slice(&bytes[..name_len]);
    }

    result
}

/// Convert an 8.3 short name to a readable string.
fn from_short_name(raw: &[u8; 11]) -> String {
    let name_part: String = raw[..8].iter()
        .take_while(|&&b| b != b' ')
        .map(|&b| (b as char).to_ascii_lowercase() as char)
        .collect();

    let ext_part: String = raw[8..11].iter()
        .take_while(|&&b| b != b' ')
        .map(|&b| (b as char).to_ascii_lowercase() as char)
        .collect();

    if ext_part.is_empty() {
        name_part
    } else {
        format!("{}.{}", name_part, ext_part)
    }
}

/// Split a path into (directory, filename).
fn split_path(path: &str) -> (&str, &str) {
    if let Some(pos) = path.rfind('/') {
        (&path[..pos], &path[pos + 1..])
    } else {
        ("", path)
    }
}

/// Simple uppercase helper for no_std (ASCII only).
trait ToUpperSimple {
    fn to_uppercase_simple(&self) -> String;
}

impl ToUpperSimple for str {
    fn to_uppercase_simple(&self) -> String {
        self.chars().map(|c| {
            if c >= 'a' && c <= 'z' {
                (c as u8 - 32) as char
            } else {
                c
            }
        }).collect()
    }
}

impl ToUpperSimple for String {
    fn to_uppercase_simple(&self) -> String {
        self.as_str().to_uppercase_simple()
    }
}

// ═══════════════════════════════════════════════════════════════
//  Public API
// ═══════════════════════════════════════════════════════════════

/// Initialize: probe the VirtIO-blk device for a FAT32 filesystem.
pub fn init() {
    if !super::virtio_blk::is_available() {
        crate::serial_println!("[fat32] No block device available.");
        return;
    }

    // Check if DiskFs already claimed the device (SMFT magic at sector 0)
    let mut sector = [0u8; SECTOR_SIZE];
    if super::virtio_blk::read_sector(0, &mut sector).is_ok() {
        // Check for our custom DiskFs magic
        if sector[0] == b'S' && sector[1] == b'M' && sector[2] == b'F' && sector[3] == b'T' {
            crate::serial_println!("[fat32] Device has DiskFs format, skipping FAT32.");
            return;
        }
    }

    match Fat32Fs::mount() {
        Ok(fs) => {
            let (total_bytes, spc, root) = fs.info();
            crate::serial_println!(
                "[fat32] Mounted: {} MiB, {} sectors/cluster, root=cluster {}",
                total_bytes / (1024 * 1024), spc, root
            );
            *FAT32_FS.lock() = Some(fs);
        }
        Err(e) => {
            crate::serial_println!("[fat32] Mount failed: {}", e);
        }
    }
}

/// Check if FAT32 is available.
pub fn is_available() -> bool {
    FAT32_FS.lock().is_some()
}

/// List files in a FAT32 directory.
pub fn list_dir(path: &str) -> Result<Vec<String>, &'static str> {
    let fs = FAT32_FS.lock();
    let fs = fs.as_ref().ok_or("FAT32 not mounted")?;
    fs.list_dir(path)
}

/// Read a file from FAT32.
pub fn read_file(path: &str) -> Result<Vec<u8>, &'static str> {
    let fs = FAT32_FS.lock();
    let fs = fs.as_ref().ok_or("FAT32 not mounted")?;
    fs.read_file(path)
}

/// Write a file to FAT32.
pub fn write_file(path: &str, data: &[u8]) -> Result<(), &'static str> {
    let mut fs = FAT32_FS.lock();
    let fs = fs.as_mut().ok_or("FAT32 not mounted")?;
    fs.write_file(path, data)
}

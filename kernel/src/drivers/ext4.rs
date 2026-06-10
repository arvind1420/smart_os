/// ext4 read-only filesystem driver for Smart OS.
///
/// Supports: superblock parsing, group descriptors (32- and 64-bit), inode lookup,
/// extent trees, indirect block maps, linear directory parsing (with filetype),
/// fast and slow symlink resolution, LRU block cache, and VFS mount points.
///
/// Usage: `ext4::mount(lba_start, "/mnt/ext4/disk")` then read via VFS paths.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

// ── On-disk constants ─────────────────────────────────────────────────────────

const EXT4_MAGIC:        u16 = 0xEF53;
const EXT4_ROOT_INODE:   u32 = 2;
const EXT4_EXTENTS_FL:   u32 = 0x0008_0000;
const EXT4_SYMLINK_MODE: u16 = 0xA000;
const EXT4_DIR_MODE:     u16 = 0x4000;

const INCOMPAT_64BIT:    u32 = 0x0080;
const INCOMPAT_FLEX_BG:  u32 = 0x0200;

const EXTENT_MAGIC:      u16 = 0xF30A;

// ── LRU block cache ───────────────────────────────────────────────────────────

struct CacheEntry { block: u64, data: Vec<u8> }

struct BlockCache {
    entries:  Vec<CacheEntry>, // evict from front (oldest)
    capacity: usize,
}

impl BlockCache {
    fn new(cap: usize) -> Self { Self { entries: Vec::new(), capacity: cap } }

    fn get(&mut self, block: u64) -> Option<&[u8]> {
        if let Some(pos) = self.entries.iter().position(|e| e.block == block) {
            // Move to back (most-recently-used)
            let entry = self.entries.remove(pos);
            self.entries.push(entry);
            return self.entries.last().map(|e| e.data.as_slice());
        }
        None
    }

    fn insert(&mut self, block: u64, data: Vec<u8>) {
        if self.entries.len() >= self.capacity { self.entries.remove(0); }
        self.entries.push(CacheEntry { block, data });
    }
}

// ── Superblock ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Superblock {
    inodes_count:      u32,
    blocks_per_group:  u32,
    inodes_per_group:  u32,
    block_size:        usize,   // bytes
    inode_size:        usize,   // bytes
    desc_size:         usize,   // bytes (32 or 64)
    incompat_flags:    u32,
    volume_name:       String,
}

fn parse_superblock(raw: &[u8]) -> Option<Superblock> {
    if raw.len() < 1024 { return None; }
    // Superblock starts at offset 0 within the supplied 1024-byte slice
    let magic = u16::from_le_bytes([raw[56], raw[57]]);
    if magic != EXT4_MAGIC { return None; }

    let log_block = u32::from_le_bytes([raw[24], raw[25], raw[26], raw[27]]);
    let block_size = 1024usize << log_block;

    let inode_size = u16::from_le_bytes([raw[88], raw[89]]) as usize;
    let inode_size = if inode_size == 0 { 128 } else { inode_size };

    let incompat = u32::from_le_bytes([raw[96], raw[97], raw[98], raw[99]]);
    let desc_size = if incompat & INCOMPAT_64BIT != 0 {
        u16::from_le_bytes([raw[232], raw[233]]) as usize
    } else { 32 };
    let desc_size = if desc_size < 32 { 32 } else { desc_size };

    let vol_bytes = &raw[120..136];
    let end = vol_bytes.iter().position(|&b| b == 0).unwrap_or(16);
    let volume_name = String::from_utf8_lossy(&vol_bytes[..end]).into_owned();

    Some(Superblock {
        inodes_count:     u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]),
        blocks_per_group: u32::from_le_bytes([raw[32], raw[33], raw[34], raw[35]]),
        inodes_per_group: u32::from_le_bytes([raw[40], raw[41], raw[42], raw[43]]),
        block_size,
        inode_size,
        desc_size,
        incompat_flags: incompat,
        volume_name,
    })
}

// ── Volume (mounted ext4 instance) ────────────────────────────────────────────

struct Ext4Volume {
    lba_start: u64,
    sb:        Superblock,
    cache:     BlockCache,
}

impl Ext4Volume {
    // ── Raw block I/O ─────────────────────────────────────────────────────────

    fn read_block_raw(&mut self, block_num: u64) -> Result<Vec<u8>, &'static str> {
        // Check cache first
        if let Some(data) = self.cache.get(block_num) {
            return Ok(data.to_vec());
        }
        let sectors_per_block = (self.sb.block_size / 512).max(1);
        let lba = self.lba_start + block_num * sectors_per_block as u64;
        let mut buf = vec![0u8; self.sb.block_size];
        crate::drivers::virtio_blk::read_sectors(lba, sectors_per_block, &mut buf)
            .map_err(|_| "ext4: disk read error")?;
        self.cache.insert(block_num, buf.clone());
        Ok(buf)
    }

    // ── Group descriptors ─────────────────────────────────────────────────────

    fn group_desc_block(&self) -> u64 {
        if self.sb.block_size == 1024 { 2 } else { 1 }
    }

    /// Read the raw group descriptor bytes for group `g`.
    fn read_group_desc(&mut self, g: u32) -> Result<Vec<u8>, &'static str> {
        let ds = self.sb.desc_size;
        let descs_per_block = self.sb.block_size / ds;
        let block_offset = g as usize / descs_per_block;
        let idx_in_block  = g as usize % descs_per_block;
        let blk = self.read_block_raw(self.group_desc_block() + block_offset as u64)?;
        Ok(blk[idx_in_block * ds .. idx_in_block * ds + ds].to_vec())
    }

    fn gd_inode_table(&mut self, g: u32) -> Result<u64, &'static str> {
        let gd = self.read_group_desc(g)?;
        let lo = u32::from_le_bytes([gd[8], gd[9], gd[10], gd[11]]) as u64;
        let hi = if self.sb.desc_size >= 64 {
            u32::from_le_bytes([gd[40], gd[41], gd[42], gd[43]]) as u64
        } else { 0 };
        Ok((hi << 32) | lo)
    }

    // ── Inode lookup ──────────────────────────────────────────────────────────

    fn read_inode_raw(&mut self, ino: u32) -> Result<Vec<u8>, &'static str> {
        if ino == 0 { return Err("ext4: inode 0 invalid"); }
        let idx = (ino - 1) as u64;
        let group = (idx / self.sb.inodes_per_group as u64) as u32;
        let local = (idx % self.sb.inodes_per_group as u64) as u64;

        let it_block = self.gd_inode_table(group)?;
        let inodes_per_block = self.sb.block_size / self.sb.inode_size;
        let blk_offset = local / inodes_per_block as u64;
        let idx_in_blk = local % inodes_per_block as u64;

        let blk = self.read_block_raw(it_block + blk_offset)?;
        let off = idx_in_blk as usize * self.sb.inode_size;
        Ok(blk[off .. off + self.sb.inode_size].to_vec())
    }

    fn inode_mode(raw: &[u8]) -> u16 {
        u16::from_le_bytes([raw[0], raw[1]])
    }
    fn inode_size(raw: &[u8]) -> u64 {
        let lo = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]) as u64;
        let hi = u32::from_le_bytes([raw[108], raw[109], raw[110], raw[111]]) as u64;
        lo | (hi << 32)
    }
    fn inode_flags(raw: &[u8]) -> u32 {
        u32::from_le_bytes([raw[32], raw[33], raw[34], raw[35]])
    }
    fn inode_blocks(raw: &[u8]) -> &[u8] { &raw[40..100] } // 60 bytes = i_block

    // ── Extent tree ───────────────────────────────────────────────────────────

    /// Resolve logical block `lblk` to physical block using the extent tree.
    fn extent_lookup(&mut self, node: Vec<u8>, lblk: u32) -> Result<u64, &'static str> {
        let magic  = u16::from_le_bytes([node[0], node[1]]);
        if magic != EXTENT_MAGIC { return Err("ext4: bad extent magic"); }
        let nent   = u16::from_le_bytes([node[2], node[3]]) as usize;
        let depth  = u16::from_le_bytes([node[6], node[7]]);

        if depth == 0 {
            // Leaf node: scan extent entries (12 bytes each, after 12-byte header)
            for i in 0..nent {
                let off = 12 + i * 12;
                let ee_block  = u32::from_le_bytes([node[off],   node[off+1], node[off+2], node[off+3]]);
                let ee_len    = u16::from_le_bytes([node[off+4], node[off+5]]) as u32;
                let ee_hi     = u16::from_le_bytes([node[off+6], node[off+7]]) as u64;
                let ee_lo     = u32::from_le_bytes([node[off+8], node[off+9], node[off+10], node[off+11]]) as u64;
                let phys_base = (ee_hi << 32) | ee_lo;
                if lblk >= ee_block && lblk < ee_block + ee_len {
                    return Ok(phys_base + (lblk - ee_block) as u64);
                }
            }
            Err("ext4: logical block not in extents")
        } else {
            // Index node: find the right child and recurse
            let mut child_ptr = None;
            for i in 0..nent {
                let off = 12 + i * 12;
                let ei_block = u32::from_le_bytes([node[off], node[off+1], node[off+2], node[off+3]]);
                if lblk >= ei_block {
                    let lo = u32::from_le_bytes([node[off+4], node[off+5], node[off+6], node[off+7]]) as u64;
                    let hi = u16::from_le_bytes([node[off+8], node[off+9]]) as u64;
                    child_ptr = Some((hi << 32) | lo);
                }
            }
            let child_blk = child_ptr.ok_or("ext4: extent index not found")?;
            let child_data = self.read_block_raw(child_blk)?;
            self.extent_lookup(child_data, lblk)
        }
    }

    // ── Indirect block maps ───────────────────────────────────────────────────

    fn indirect_lookup(&mut self, blocks: &[u8; 60], lblk: u32) -> Result<u64, &'static str> {
        let read_u32 = |b: &[u8], i: usize| -> u32 {
            u32::from_le_bytes([b[i*4], b[i*4+1], b[i*4+2], b[i*4+3]])
        };
        let ptrs_per_block = self.sb.block_size / 4;
        let ptrs = ptrs_per_block as u32;

        if (lblk as usize) < 12 {
            // Direct blocks (i_block[0..11])
            let phys = read_u32(blocks, lblk as usize) as u64;
            return if phys == 0 { Err("ext4: sparse block") } else { Ok(phys) };
        }
        let lblk = lblk - 12;

        if (lblk as usize) < ptrs_per_block {
            // Single indirect (i_block[12])
            let ind1 = read_u32(blocks, 12) as u64;
            let b1 = self.read_block_raw(ind1)?;
            let phys = read_u32(&b1, lblk as usize) as u64;
            return Ok(phys);
        }
        let lblk = lblk - ptrs;

        if (lblk as usize) < ptrs_per_block * ptrs_per_block {
            // Double indirect (i_block[13])
            let ind1 = read_u32(blocks, 13) as u64;
            let b1   = self.read_block_raw(ind1)?;
            let l1   = read_u32(&b1, (lblk / ptrs) as usize) as u64;
            let b2   = self.read_block_raw(l1)?;
            let phys = read_u32(&b2, (lblk % ptrs) as usize) as u64;
            return Ok(phys);
        }

        Err("ext4: block beyond double-indirect not supported")
    }

    // ── File reading ──────────────────────────────────────────────────────────

    fn read_file_inode(&mut self, ino: u32) -> Result<Vec<u8>, &'static str> {
        let raw = self.read_inode_raw(ino)?;
        let size  = Self::inode_size(&raw);
        let flags = Self::inode_flags(&raw);
        let iblock: [u8; 60] = raw[40..100].try_into().unwrap_or([0u8; 60]);

        let bs = self.sb.block_size;
        let blocks_needed = (size as usize + bs - 1) / bs;
        let mut data = Vec::with_capacity(size as usize);

        for lblk in 0..blocks_needed as u32 {
            let phys = if flags & EXT4_EXTENTS_FL != 0 {
                let ext_node = iblock.to_vec();
                self.extent_lookup(ext_node, lblk)?
            } else {
                self.indirect_lookup(&iblock, lblk)?
            };
            let blk_data = self.read_block_raw(phys)?;
            let to_take = (size as usize - data.len()).min(bs);
            data.extend_from_slice(&blk_data[..to_take]);
        }
        Ok(data)
    }

    // ── Symlink resolution ────────────────────────────────────────────────────

    fn read_symlink(&mut self, ino: u32) -> Result<String, &'static str> {
        let raw = self.read_inode_raw(ino)?;
        let size = Self::inode_size(&raw) as usize;
        // Fast symlink: target stored in i_block if size <= 60
        if size <= 60 {
            let target = core::str::from_utf8(&raw[40..40 + size])
                .unwrap_or("")
                .to_string();
            return Ok(target);
        }
        // Slow symlink: read from data blocks
        let content = self.read_file_inode(ino)?;
        let target = String::from_utf8_lossy(&content[..size]).into_owned();
        Ok(target)
    }

    // ── Directory parsing ─────────────────────────────────────────────────────

    fn readdir_inode(&mut self, ino: u32) -> Result<Vec<(String, u32)>, &'static str> {
        let data = self.read_file_inode(ino)?;
        let mut entries = Vec::new();
        let mut pos = 0usize;
        while pos + 8 <= data.len() {
            let inode_num = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]);
            let rec_len   = u16::from_le_bytes([data[pos+4], data[pos+5]]) as usize;
            let name_len  = data[pos+6] as usize;
            if rec_len < 8 || pos + rec_len > data.len() { break; }
            if inode_num != 0 && name_len > 0 && pos + 8 + name_len <= data.len() {
                let name_bytes = &data[pos + 8 .. pos + 8 + name_len];
                if let Ok(name) = core::str::from_utf8(name_bytes) {
                    if name != "." && name != ".." {
                        entries.push((name.to_string(), inode_num));
                    }
                }
            }
            pos += if rec_len == 0 { break; } else { rec_len };
        }
        Ok(entries)
    }

    // ── Path resolution ───────────────────────────────────────────────────────

    fn resolve_path(&mut self, path: &str) -> Result<u32, &'static str> {
        let mut ino = EXT4_ROOT_INODE;
        for component in path.split('/').filter(|s| !s.is_empty()) {
            let entries = self.readdir_inode(ino)?;
            let next = entries.into_iter()
                .find(|(n, _)| n == component)
                .map(|(_, i)| i)
                .ok_or("ext4: path component not found")?;
            // Follow symlinks (up to 8 hops)
            let raw = self.read_inode_raw(next)?;
            if Self::inode_mode(&raw) & 0xF000 == EXT4_SYMLINK_MODE as u16 & 0xF000 {
                let target = self.read_symlink(next)?;
                // Relative symlinks only for now
                if !target.starts_with('/') {
                    ino = self.resolve_relative(ino, &target)?;
                    continue;
                }
            }
            ino = next;
        }
        Ok(ino)
    }

    fn resolve_relative(&mut self, parent: u32, rel: &str) -> Result<u32, &'static str> {
        let mut ino = parent;
        for comp in rel.split('/').filter(|s| !s.is_empty()) {
            if comp == ".." {
                // Walk up — find ".." entry in parent
                let entries_raw = self.read_file_inode(ino)?;
                let mut pos = 0;
                while pos + 8 <= entries_raw.len() {
                    let inode_num = u32::from_le_bytes([entries_raw[pos], entries_raw[pos+1], entries_raw[pos+2], entries_raw[pos+3]]);
                    let rec_len   = u16::from_le_bytes([entries_raw[pos+4], entries_raw[pos+5]]) as usize;
                    let name_len  = entries_raw[pos+6] as usize;
                    if rec_len == 0 { break; }
                    if name_len == 2 && &entries_raw[pos+8..pos+10] == b".." {
                        ino = inode_num; break;
                    }
                    pos += rec_len;
                }
            } else {
                let entries = self.readdir_inode(ino)?;
                ino = entries.into_iter().find(|(n, _)| n == comp).map(|(_, i)| i)
                    .ok_or("ext4: symlink target not found")?;
            }
        }
        Ok(ino)
    }

    // ── Public high-level API ─────────────────────────────────────────────────

    pub fn list_dir(&mut self, path: &str) -> Result<Vec<String>, &'static str> {
        let ino = self.resolve_path(path)?;
        let entries = self.readdir_inode(ino)?;
        Ok(entries.into_iter().map(|(n, _)| n).collect())
    }

    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>, &'static str> {
        let ino = self.resolve_path(path)?;
        let raw = self.read_inode_raw(ino)?;
        let mode = Self::inode_mode(&raw);
        if mode & 0xF000 == EXT4_SYMLINK_MODE as u16 & 0xF000 {
            // Read the symlink target as content (callers can follow manually)
            let target = self.read_symlink(ino)?;
            return Ok(target.into_bytes());
        }
        self.read_file_inode(ino)
    }

    pub fn stat_inode(&mut self, path: &str) -> Result<InodeStat, &'static str> {
        let ino = self.resolve_path(path)?;
        let raw = self.read_inode_raw(ino)?;
        Ok(InodeStat {
            ino,
            mode:  Self::inode_mode(&raw),
            size:  Self::inode_size(&raw),
            mtime: u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]),
        })
    }
}

pub struct InodeStat { pub ino: u32, pub mode: u16, pub size: u64, pub mtime: u32 }

// ── Mount table ───────────────────────────────────────────────────────────────

struct MountEntry {
    mount_point: String, // e.g. "/mnt/ext4/disk"
    volume:      Ext4Volume,
}

static MOUNTS: Mutex<Vec<MountEntry>> = Mutex::new(Vec::new());

/// Mount an ext4 partition starting at `lba_start` at `mount_point`.
pub fn mount(lba_start: u64, mount_point: &str) -> Result<String, &'static str> {
    // Read superblock (at byte offset 1024, which is sectors 2–3 relative to partition start)
    let sector_offset = lba_start + 2; // byte 1024 = sector 2
    let mut sb_raw = vec![0u8; 1024];
    crate::drivers::virtio_blk::read_sectors(sector_offset, 2, &mut sb_raw)
        .map_err(|_| "ext4: cannot read superblock")?;

    let sb = parse_superblock(&sb_raw).ok_or("ext4: invalid superblock (bad magic)")?;
    let label = if sb.volume_name.is_empty() {
        format!("vol_{}", lba_start)
    } else {
        sb.volume_name.clone()
    };

    let vol = Ext4Volume { lba_start, sb, cache: BlockCache::new(256) };

    let mp = if mount_point.is_empty() {
        format!("/mnt/ext4/{}", label)
    } else {
        mount_point.to_string()
    };

    let mut mounts = MOUNTS.lock();
    mounts.retain(|m| m.mount_point != mp); // replace existing
    mounts.push(MountEntry { mount_point: mp.clone(), volume: vol });

    crate::serial_println!("[ext4] Mounted partition lba={} at '{}' (label='{}')", lba_start, mp, label);
    Ok(mp)
}

/// Unmount a mount point.
pub fn umount(mount_point: &str) {
    MOUNTS.lock().retain(|m| m.mount_point != mount_point);
}

/// List all current mounts.
pub fn list_mounts() -> Vec<(String, String)> {
    MOUNTS.lock().iter().map(|m| (m.mount_point.clone(), m.volume.sb.volume_name.clone())).collect()
}

// ── VFS-compatible helpers ────────────────────────────────────────────────────

fn find_volume_and_subpath<'a>(mounts: &'a mut Vec<MountEntry>, path: &str)
    -> Option<(&'a mut Ext4Volume, String)>
{
    for entry in mounts.iter_mut() {
        if let Some(rest) = path.strip_prefix(&entry.mount_point) {
            let subpath = if rest.is_empty() { "/" } else { rest };
            return Some((&mut entry.volume, subpath.to_string()));
        }
    }
    None
}

pub fn vfs_readdir(path: &str) -> Option<Vec<String>> {
    let mut mounts = MOUNTS.lock();
    let (vol, sub) = find_volume_and_subpath(&mut mounts, path)?;
    vol.list_dir(&sub).ok()
}

pub fn vfs_read_file(path: &str) -> Option<Vec<u8>> {
    let mut mounts = MOUNTS.lock();
    let (vol, sub) = find_volume_and_subpath(&mut mounts, path)?;
    vol.read_file(&sub).ok()
}

pub fn vfs_stat(path: &str) -> Option<InodeStat> {
    let mut mounts = MOUNTS.lock();
    let (vol, sub) = find_volume_and_subpath(&mut mounts, path)?;
    vol.stat_inode(&sub).ok()
}

pub fn handles_path(path: &str) -> bool {
    MOUNTS.lock().iter().any(|m| path.starts_with(&m.mount_point))
}

// ── MBR partition scanner ─────────────────────────────────────────────────────

#[derive(Clone)]
pub struct PartInfo {
    pub index:      usize,
    pub part_type:  u8,
    pub lba_start:  u64,
    pub lba_size:   u64,
    pub is_ext4:    bool,
}

/// Read the MBR at LBA 0 and return a list of primary partitions.
pub fn scan_partitions() -> Vec<PartInfo> {
    let mut mbr = [0u8; 512];
    if crate::drivers::virtio_blk::read_sectors(0, 1, &mut mbr).is_err() { return Vec::new(); }

    // Check MBR signature
    if mbr[510] != 0x55 || mbr[511] != 0xAA { return Vec::new(); }

    let mut parts = Vec::new();
    for i in 0..4 {
        let off = 446 + i * 16;
        let ptype     = mbr[off + 4];
        let lba_start = u32::from_le_bytes([mbr[off+8], mbr[off+9], mbr[off+10], mbr[off+11]]) as u64;
        let lba_size  = u32::from_le_bytes([mbr[off+12], mbr[off+13], mbr[off+14], mbr[off+15]]) as u64;
        if ptype == 0 || lba_size == 0 { continue; }
        // Check if ext4 by probing superblock magic
        let is_ext4 = probe_ext4(lba_start);
        parts.push(PartInfo { index: i + 1, part_type: ptype, lba_start, lba_size, is_ext4 });
    }
    parts
}

fn probe_ext4(lba_start: u64) -> bool {
    let mut buf = [0u8; 512];
    // Superblock starts at byte 1024 = sector 2 into the partition
    if crate::drivers::virtio_blk::read_sectors(lba_start + 2, 1, &mut buf).is_err() {
        return false;
    }
    // Magic is at offset 56 within the superblock (buf[0] = superblock byte 512)
    // We need 2 sectors to get to byte 1024+56.  Sector 2 covers bytes 1024-1535.
    // Byte 1024+56 = byte 1080 → sector 2, offset 56.
    let magic = u16::from_le_bytes([buf[56], buf[57]]);
    magic == EXT4_MAGIC
}

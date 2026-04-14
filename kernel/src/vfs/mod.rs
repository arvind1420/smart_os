/// Virtual File System (VFS) for Smart OS.
///
/// Provides a unified filesystem interface with an in-memory ramfs backend
/// and optional persistent disk backend (paths under `/disk/`).
/// All inode metadata is stored as SmartPack Values for maximum extensibility.

pub mod inode;
pub mod ramfs;
pub mod fd;

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

/// The global VFS instance.
static VFS: Mutex<Option<ramfs::RamFs>> = Mutex::new(None);
/// Global file descriptor table.
static FD_TABLE: Mutex<Option<fd::FdTable>> = Mutex::new(None);

/// Check if a path should be routed to the disk filesystem.
fn is_disk_path(path: &str) -> bool {
    path == "/disk" || path.starts_with("/disk/")
}

/// Extract the filename from a disk path (strip `/disk/` prefix).
fn disk_filename(path: &str) -> &str {
    if path.starts_with("/disk/") {
        &path[6..]
    } else {
        ""
    }
}

/// Check if a path should be routed to the FAT32 filesystem.
fn is_fat_path(path: &str) -> bool {
    path == "/fat" || path.starts_with("/fat/")
}

/// Extract the subpath from a FAT32 path (strip `/fat/` prefix).
fn fat_subpath(path: &str) -> &str {
    if path.starts_with("/fat/") {
        &path[5..]
    } else {
        ""
    }
}

/// Check if a path should be routed to the NTFS filesystem.
fn is_ntfs_path(path: &str) -> bool {
    path == "/ntfs" || path.starts_with("/ntfs/")
}

/// Extract the subpath from an NTFS path (strip `/ntfs/` prefix).
fn ntfs_subpath(path: &str) -> &str {
    if path.starts_with("/ntfs/") {
        &path[6..]
    } else {
        ""
    }
}

/// Check if a path should be routed to a character device.
fn is_dev_path(path: &str) -> bool {
    path.starts_with("/dev/")
}

/// Extract the device name from a path (strip `/dev/` prefix).
fn dev_name(path: &str) -> &str {
    if path.starts_with("/dev/") {
        &path[5..]
    } else {
        ""
    }
}

/// Initialize the VFS with a ramfs root.
pub fn init() {
    let mut fs = ramfs::RamFs::new();
    // Create default directory structure
    fs.mkdir("/").ok();
    fs.mkdir("/system").ok();
    fs.mkdir("/system/config").ok();
    fs.mkdir("/system/fonts").ok();
    fs.mkdir("/home").ok();
    fs.mkdir("/tmp").ok();
    fs.mkdir("/dev").ok();
    fs.mkdir("/ntfs").ok();

    // Create device nodes in VFS for enumeration
    fs.create_file("/dev/ttyS0", b"").ok();
    fs.create_file("/dev/ttyS1", b"").ok();
    fs.create_file("/dev/ttyS2", b"").ok();
    fs.create_file("/dev/ttyS3", b"").ok();
    fs.create_file("/dev/lp0", b"").ok();

    // Create system files
    fs.create_file("/system/version", b"Smart OS v0.8.0\n").ok();
    fs.create_file("/system/config/theme.sp", b"neon-dark").ok();
    fs.create_file("/system/config/motd", b"Smart OS v0.8.0 - Phase 8: Usability & Real Hardware").ok();
    fs.create_file("/system/config/editor.sp", b"tabsize=4\ntheme=neon").ok();

    // Create user home directory with demo content
    fs.mkdir("/home").ok();
    fs.mkdir("/home/user").ok();
    fs.mkdir("/home/user/documents").ok();
    fs.mkdir("/home/user/projects").ok();

    fs.create_file("/home/user/readme.txt",
        b"Welcome to Smart OS!\n\nThis is your home directory.\nUse the Terminal to explore.\nType 'help' for available commands.\n").ok();
    fs.create_file("/home/user/documents/notes.txt",
        b"# Notes\n\n- Explore the terminal: type 'help'\n- Browse files with File Manager\n- Check System Monitor for live stats\n- Try 'classify' to test AI engine\n").ok();
    fs.create_file("/home/user/projects/hello.rs",
        b"// Smart OS demo program\nfn main() {\n    println!(\"Hello, Smart OS!\");\n}\n").ok();
    fs.create_file("/home/user/projects/config.json",
        b"{\n  \"name\": \"smartos-app\",\n  \"version\": \"0.6.0\",\n  \"debug\": true\n}\n").ok();

    *VFS.lock() = Some(fs);
    *FD_TABLE.lock() = Some(fd::FdTable::new());

    crate::serial_println!("[vfs] Virtual file system initialized (ramfs root mounted).");
}

/// Open a file, returning a file descriptor.
pub fn open(path: &str) -> Result<usize, &'static str> {
    if is_dev_path(path) {
        let name = dev_name(path);
        let inode_base = 0xA000_0000;
        let id = match name {
            "ttyS0" => 0,
            "ttyS1" => 1,
            "ttyS2" => 2,
            "ttyS3" => 3,
            "lp0" => 4,
            _ => return Err("Device not found"),
        };
        let mut fdt = FD_TABLE.lock();
        let table = fdt.as_mut().ok_or("FD table not initialized")?;
        return Ok(table.open(inode_base + id, path));
    }
    if is_ntfs_path(path) {
        let sub = ntfs_subpath(path);
        if !crate::drivers::ntfs::is_available() {
            return Err("NTFS not mounted");
        }
        let mut fdt = FD_TABLE.lock();
        let table = fdt.as_mut().ok_or("FD table not initialized")?;
        // Use 0x8000_0000 base for NTFS inodes
        return Ok(table.open(0x8000_0000, path));
    }
    if is_fat_path(path) {
        let sub = fat_subpath(path);
        if !crate::drivers::fat32::is_available() {
            return Err("FAT32 not mounted");
        }
        // Verify file exists by trying to read it
        crate::drivers::fat32::read_file(sub).map_err(|_| "File not found on FAT32")?;
        let fake_inode = 0x9000_0000u64;
        let mut fdt = FD_TABLE.lock();
        let table = fdt.as_mut().ok_or("FD table not initialized")?;
        return Ok(table.open(fake_inode, path));
    }
    if is_disk_path(path) {
        // Disk files get a special inode ID (0x8000_0000 + index)
        // and we store the data in a temporary buffer
        let name = disk_filename(path);
        let diskfs = crate::drivers::diskfs::DISK_FS.lock();
        let fs = diskfs.as_ref().ok_or("Disk not mounted")?;
        // Check file exists
        if !fs.list_files().iter().any(|f| f == name) {
            return Err("File not found on disk");
        }
        drop(diskfs);
        // Use a high inode ID to mark as disk file
        let fake_inode = 0x8000_0000;
        let mut fdt = FD_TABLE.lock();
        let table = fdt.as_mut().ok_or("FD table not initialized")?;
        Ok(table.open(fake_inode, path))
    } else {
        let vfs = VFS.lock();
        let fs = vfs.as_ref().ok_or("VFS not initialized")?;
        let inode_id = fs.lookup(path).ok_or("File not found")?;
        let mut fdt = FD_TABLE.lock();
        let table = fdt.as_mut().ok_or("FD table not initialized")?;
        Ok(table.open(inode_id, path))
    }
}

/// Close a file descriptor.
pub fn close(fd_num: usize) -> Result<(), &'static str> {
    let mut fdt = FD_TABLE.lock();
    let table = fdt.as_mut().ok_or("FD table not initialized")?;
    table.close(fd_num).map(|_| ()).ok_or("Invalid fd")
}

/// Read from a file descriptor.
pub fn read(fd_num: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
    let fdt = FD_TABLE.lock();
    let table = fdt.as_ref().ok_or("FD table not initialized")?;
    let fd = table.get(fd_num).ok_or("Invalid fd")?;
    let inode_id = fd.inode_id;
    let offset = fd.offset;
    let path = fd.path.clone();
    drop(fdt);

    if inode_id >= 0xA000_0000 {
        // ... (device handling)
    }

    if inode_id >= 0x8000_0000 && inode_id < 0x9000_0000 {
        // NTFS file
        let sub = ntfs_subpath(&path);
        let ntfs = crate::drivers::ntfs::NTFS.lock();
        let data = ntfs.as_ref().ok_or("NTFS not mounted")?.read_file(sub)?;
        let remaining = if offset < data.len() { data.len() - offset } else { 0 };
        let count = remaining.min(buf.len());
        buf[..count].copy_from_slice(&data[offset..offset + count]);
        return Ok(count);
    }

    if inode_id >= 0x9000_0000 {
        // FAT32 file
        let sub = fat_subpath(&path);
        let data = crate::drivers::fat32::read_file(sub)?;
        let remaining = if offset < data.len() { data.len() - offset } else { 0 };
        let copy_len = remaining.min(buf.len());
        if copy_len > 0 {
            buf[..copy_len].copy_from_slice(&data[offset..offset + copy_len]);
        }
        let mut fdt = FD_TABLE.lock();
        let table = fdt.as_mut().ok_or("FD table not initialized")?;
        if let Some(fd) = table.get_mut(fd_num) {
            fd.offset += copy_len;
        }
        return Ok(copy_len);
    }

    if inode_id >= 0x8000_0000 {
        // Disk file
        let name = disk_filename(&path);
        let diskfs = crate::drivers::diskfs::DISK_FS.lock();
        let fs = diskfs.as_ref().ok_or("Disk not mounted")?;
        let data = fs.read_file(name)?;
        drop(diskfs);
        let remaining = if offset < data.len() { data.len() - offset } else { 0 };
        let copy_len = remaining.min(buf.len());
        if copy_len > 0 {
            buf[..copy_len].copy_from_slice(&data[offset..offset + copy_len]);
        }
        // Update offset
        let mut fdt = FD_TABLE.lock();
        let table = fdt.as_mut().ok_or("FD table not initialized")?;
        if let Some(fd) = table.get_mut(fd_num) {
            fd.offset += copy_len;
        }
        Ok(copy_len)
    } else {
        let vfs = VFS.lock();
        let fs = vfs.as_ref().ok_or("VFS not initialized")?;
        let bytes_read = fs.read(inode_id, offset, buf)?;
        drop(vfs);

        // Update offset
        let mut fdt = FD_TABLE.lock();
        let table = fdt.as_mut().ok_or("FD table not initialized")?;
        if let Some(fd) = table.get_mut(fd_num) {
            fd.offset += bytes_read;
        }
        Ok(bytes_read)
    }
}

/// Write to a file descriptor.
pub fn write(fd_num: usize, data: &[u8]) -> Result<usize, &'static str> {
    // Check immutable system protection
    {
        let fdt = FD_TABLE.lock();
        if let Some(table) = fdt.as_ref() {
            if let Some(fd) = table.get(fd_num) {
                crate::immutable::protection::check_write(&fd.path)?;
            }
        }
    }

    let fdt = FD_TABLE.lock();
    let table = fdt.as_ref().ok_or("FD table not initialized")?;
    let fd = table.get(fd_num).ok_or("Invalid fd")?;
    let inode_id = fd.inode_id;
    let path = fd.path.clone();
    drop(fdt);

    if inode_id >= 0xA000_0000 {
        // Character device
        let id = inode_id - 0xA000_0000;
        return match id {
            0..=3 => {
                crate::drivers::uart::write_com(id as usize, data)?;
                Ok(data.len())
            }
            4 => {
                // LPT write
                let mut lpt = crate::drivers::lpt::LPT1.lock();
                for &byte in data {
                    lpt.write_data(byte);
                    lpt.strobe();
                }
                Ok(data.len())
            }
            _ => Err("Invalid device ID"),
        };
    }

    if inode_id >= 0x9000_0000 {
        // FAT32 file write
        let sub = fat_subpath(&path);
        crate::drivers::fat32::write_file(sub, data)?;
        return Ok(data.len());
    }

    if inode_id >= 0x8000_0000 {
        // Disk file write
        let name = disk_filename(&path);
        let mut diskfs = crate::drivers::diskfs::DISK_FS.lock();
        let fs = diskfs.as_mut().ok_or("Disk not mounted")?;
        fs.write_file(name, data)?;
        Ok(data.len())
    } else {
        let mut vfs = VFS.lock();
        let fs = vfs.as_mut().ok_or("VFS not initialized")?;
        fs.write(inode_id, data)?;
        Ok(data.len())
    }
}

/// Create a directory.
pub fn mkdir(path: &str) -> Result<(), &'static str> {
    if is_disk_path(path) {
        return Ok(()); // disk FS is flat, no subdirectories
    }
    let mut vfs = VFS.lock();
    let fs = vfs.as_mut().ok_or("VFS not initialized")?;
    fs.mkdir(path)
}

/// List directory contents.
pub fn readdir(path: &str) -> Result<Vec<String>, &'static str> {
    if is_fat_path(path) || path == "/fat" {
        let sub = fat_subpath(path);
        return crate::drivers::fat32::list_dir(sub);
    }
    if is_disk_path(path) || path == "/disk" {
        let diskfs = crate::drivers::diskfs::DISK_FS.lock();
        let fs = diskfs.as_ref().ok_or("Disk not mounted")?;
        Ok(fs.list_files())
    } else {
        let vfs = VFS.lock();
        let fs = vfs.as_ref().ok_or("VFS not initialized")?;
        fs.readdir(path)
    }
}

/// Get file metadata as SmartPack Value.
pub fn stat(path: &str) -> Result<smartpack::Value, &'static str> {
    if is_fat_path(path) {
        let sub = fat_subpath(path);
        let data = crate::drivers::fat32::read_file(sub)?;
        use smartpack::Value;
        use alloc::string::String;
        let map = alloc::vec![
            (Value::String(String::from("size")), Value::UInt64(data.len() as u64)),
            (Value::String(String::from("type")), Value::String(String::from("file"))),
            (Value::String(String::from("fs")), Value::String(String::from("fat32"))),
        ];
        return Ok(Value::Map(map));
    }
    if is_disk_path(path) {
        let name = disk_filename(path);
        let diskfs = crate::drivers::diskfs::DISK_FS.lock();
        let fs = diskfs.as_ref().ok_or("Disk not mounted")?;
        let data = fs.read_file(name)?;
        // Return a simple metadata value
        use smartpack::Value;
        use alloc::string::String;
        let map = alloc::vec![
            (Value::String(String::from("size")), Value::UInt64(data.len() as u64)),
            (Value::String(String::from("type")), Value::String(String::from("file"))),
        ];
        Ok(Value::Map(map))
    } else {
        let vfs = VFS.lock();
        let fs = vfs.as_ref().ok_or("VFS not initialized")?;
        fs.stat(path)
    }
}

/// Create a file and write data to it (used by SmartFS).
pub fn create_and_write(path: &str, data: &[u8]) -> Result<(), &'static str> {
    // Check immutable system protection
    crate::immutable::protection::check_write(path)?;

    if is_fat_path(path) {
        let sub = fat_subpath(path);
        return crate::drivers::fat32::write_file(sub, data);
    }
    if is_disk_path(path) {
        let name = disk_filename(path);
        let mut diskfs = crate::drivers::diskfs::DISK_FS.lock();
        let fs = diskfs.as_mut().ok_or("Disk not mounted")?;
        fs.write_file(name, data)
    } else {
        let mut vfs = VFS.lock();
        let fs = vfs.as_mut().ok_or("VFS not initialized")?;
        // Try to create the file; if it already exists, overwrite
        if fs.lookup(path).is_some() {
            let inode_id = fs.lookup(path).unwrap();
            fs.write(inode_id, data)?;
        } else {
            fs.create_file(path, data)?;
        }
        Ok(())
    }
}

/// Delete a file from the filesystem.
pub fn delete_file(path: &str) -> Result<(), &'static str> {
    // Only supported on ramfs for now
    if is_disk_path(path) || is_fat_path(path) {
        return Err("delete not supported on this filesystem");
    }
    let mut vfs = VFS.lock();
    let fs = vfs.as_mut().ok_or("VFS not initialized")?;
    fs.delete(path)
}

/// Read an entire file into a Vec.
pub fn read_file_full(path: &str) -> Result<Vec<u8>, &'static str> {
    if is_fat_path(path) {
        let sub = fat_subpath(path);
        return crate::drivers::fat32::read_file(sub);
    }
    if is_disk_path(path) {
        let name = disk_filename(path);
        let diskfs = crate::drivers::diskfs::DISK_FS.lock();
        let fs = diskfs.as_ref().ok_or("Disk not mounted")?;
        return fs.read_file(name);
    }
    
    let vfs = VFS.lock();
    let fs = vfs.as_ref().ok_or("VFS not initialized")?;
    let inode_id = fs.lookup(path).ok_or("File not found")?;
    let stat = fs.stat(path)?;
    
    // Extract size from stat
    let size = if let smartpack::Value::Map(map) = stat {
        map.iter().find(|(k, _)| {
            if let smartpack::Value::String(s) = k { s == "size" } else { false }
        }).and_then(|(_, v)| {
            if let smartpack::Value::UInt64(n) = v { Some(*n as usize) } else { None }
        }).unwrap_or(0)
    } else {
        0
    };

    let mut data = alloc::vec![0u8; size];
    fs.read(inode_id, 0, &mut data)?;
    Ok(data)
}

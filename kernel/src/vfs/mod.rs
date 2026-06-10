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

/// Paths of files modified since last cloud sync (Phase 48).
pub static DIRTY_FILES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Mark a file as dirty for cloud synchronization.
pub fn mark_dirty(path: &str) {
    let mut dirty = DIRTY_FILES.lock();
    if !dirty.iter().any(|p| p == path) {
        dirty.push(String::from(path));
    }
}

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
    fs.create_file("/dev/audio", b"").ok();
    fs.create_file("/dev/dsp", b"").ok();

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

    // Create sample image files for testing
    let cyber_bmp = generate_cyber_bmp();
    fs.create_file("/home/user/documents/cyber.bmp", &cyber_bmp).ok();
    
    // Sample WEBP file (with magic signature RIFF....WEBP)
    let mut webp_bytes = [0u8; 32];
    webp_bytes[0..4].copy_from_slice(b"RIFF");
    webp_bytes[4..8].copy_from_slice(&24u32.to_le_bytes());
    webp_bytes[8..12].copy_from_slice(b"WEBP");
    webp_bytes[12..16].copy_from_slice(b"VP8 ");
    fs.create_file("/home/user/documents/sample.webp", &webp_bytes).ok();

    // Sample HEIC file (with magic ftypheic)
    let mut heic_bytes = [0u8; 32];
    heic_bytes[4..8].copy_from_slice(b"ftyp");
    heic_bytes[8..12].copy_from_slice(b"heic");
    fs.create_file("/home/user/documents/photo.heic", &heic_bytes).ok();

    // Sample RAW file (TIFF header II*\0)
    let mut raw_bytes = [0u8; 32];
    raw_bytes[0..4].copy_from_slice(b"II*\x00");
    fs.create_file("/home/user/documents/raw_image.raw", &raw_bytes).ok();

    // Sample PDF document (with valid %PDF signature and a basic text stream object)
    let mut pdf_bytes = alloc::vec::Vec::new();
    pdf_bytes.extend_from_slice(b"%PDF-1.4\n");
    pdf_bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    pdf_bytes.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>\nendobj\n");
    pdf_bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R >>\nendobj\n");
    pdf_bytes.extend_from_slice(b"4 0 obj\n<< /Length 60 >>\nstream\nBT /F1 12 Tf 72 712 Td (Welcome to Smart OS PDF Reader!) Tj ET\nendstream\nendobj\n");
    pdf_bytes.extend_from_slice(b"5 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 6 0 R >>\nendobj\n");
    pdf_bytes.extend_from_slice(b"6 0 obj\n<< /Length 50 >>\nstream\nBT /F1 12 Tf 72 712 Td (This is page 2 of the sample document.) Tj ET\nendstream\nendobj\n");
    pdf_bytes.extend_from_slice(b"xref\n0 7\n0000000000 65535 f\n0000000009 00000 n\n0000000056 00000 n\n0000000111 00000 n\n0000000204 00000 n\n0000000314 00000 n\n0000000407 00000 n\n");
    pdf_bytes.extend_from_slice(b"trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n507\n%%EOF\n");
    fs.create_file("/home/user/documents/sample.pdf", &pdf_bytes).ok();

    // Sample MP4 file (with standard ftypmp42 / ftypisom container signature bytes)
    let mut mp4_bytes = [0u8; 64];
    mp4_bytes[4..8].copy_from_slice(b"ftyp");
    mp4_bytes[8..12].copy_from_slice(b"isom");
    fs.create_file("/home/user/documents/cyber_grid.mp4", &mp4_bytes).ok();

    // Sample EML file
    let mut eml_bytes = alloc::vec::Vec::new();
    eml_bytes.extend_from_slice(b"From: system@smartos.org\n");
    eml_bytes.extend_from_slice(b"To: user@smartos.org\n");
    eml_bytes.extend_from_slice(b"Subject: Welcome to Smart OS Email Client!\n");
    eml_bytes.extend_from_slice(b"Date: Fri, 29 May 2026 16:15:00 +0000\n");
    eml_bytes.extend_from_slice(b"\n");
    eml_bytes.extend_from_slice(b"Hello user,\n\nWelcome to your new user-space Email Client!\nThis client allows you to view mock messages, compose emails, and simulate sending them over a virtual network.\n\nEnjoy the cyber vibes of Smart OS!\n");
    fs.create_file("/home/user/documents/welcome.eml", &eml_bytes).ok();

    // Sample DOCX document (uncompressed zip format)
    let welcome_docx = generate_welcome_docx();
    fs.create_file("/home/user/documents/welcome.docx", &welcome_docx).ok();

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
            "audio" => 5,
            "dsp" => 6,
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
        let name = disk_filename(path);
        let diskfs = crate::drivers::diskfs::DISK_FS.lock();
        let fs = diskfs.as_ref().ok_or("Disk not mounted")?;
        if !fs.list_files().iter().any(|f| f == name) {
            return Err("File not found on disk");
        }
        drop(diskfs);
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
        let sub = ntfs_subpath(&path);
        let ntfs = crate::drivers::ntfs::NTFS.lock();
        let data = ntfs.as_ref().ok_or("NTFS not mounted")?.read_file(sub)?;
        let remaining = if offset < data.len() { data.len() - offset } else { 0 };
        let count = remaining.min(buf.len());
        buf[..count].copy_from_slice(&data[offset..offset + count]);
        return Ok(count);
    }

    if inode_id >= 0x9000_0000 {
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
        let id = inode_id - 0xA000_0000;
        return match id {
            0..=3 => {
                crate::drivers::uart::write_com(id as usize, data)?;
                Ok(data.len())
            }
            4 => {
                let mut lpt = crate::drivers::lpt::LPT1.lock();
                for &byte in data {
                    lpt.write_data(byte);
                    lpt.strobe();
                }
                Ok(data.len())
            }
            5 | 6 => {
                let mut samples = alloc::collections::VecDeque::new();
                for chunk in data.chunks_exact(2) {
                    let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                    samples.push_back(sample);
                }
                
                let pid = crate::process::scheduler::current_pid().unwrap_or(0);
                let stream_id = pid as u32;
                
                let mut mixer = crate::drivers::hda::MIXER.lock();
                if let Some(stream) = mixer.streams.iter_mut().find(|s| s.id == stream_id) {
                    stream.buffer.extend(samples);
                } else {
                    let stream = crate::drivers::hda::AudioStream {
                        id: stream_id,
                        buffer: samples,
                        volume: 100,
                    };
                    mixer.add_stream(stream);
                }
                Ok(data.len())
            }
            _ => Err("Invalid device ID"),
        };
    }

    if inode_id >= 0x9000_0000 {
        let sub = fat_subpath(&path);
        crate::drivers::fat32::write_file(sub, data)?;
        return Ok(data.len());
    }

    if inode_id >= 0x8000_0000 {
        let name = disk_filename(&path);
        let mut diskfs = crate::drivers::diskfs::DISK_FS.lock();
        let fs = diskfs.as_mut().ok_or("Disk not mounted")?;
        fs.write_file(name, data)?;
        Ok(data.len())
    } else {
        let mut vfs = VFS.lock();
        let fs = vfs.as_mut().ok_or("VFS not initialized")?;
        if fs.lookup(&path).is_some() {
            let inode_id = fs.lookup(&path).unwrap();
            fs.write(inode_id, data)?;
        } else {
            fs.create_file(&path, data)?;
        }
        drop(vfs);
        
        mark_dirty(&path);
        Ok(data.len())
    }
}

/// Create a directory.
pub fn mkdir(path: &str) -> Result<(), &'static str> {
    if is_disk_path(path) {
        return Ok(());
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
    if crate::drivers::ext4::handles_path(path) {
        return crate::drivers::ext4::vfs_readdir(path).ok_or("ext4: readdir failed");
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
    if crate::drivers::ext4::handles_path(path) {
        let s = crate::drivers::ext4::vfs_stat(path).ok_or("ext4: stat failed")?;
        use smartpack::Value;
        use alloc::string::String;
        let kind = if s.mode & 0xF000 == 0x4000 { "dir" } else { "file" };
        let map = alloc::vec![
            (Value::String(String::from("size")), Value::UInt64(s.size)),
            (Value::String(String::from("type")), Value::String(String::from(kind))),
            (Value::String(String::from("fs")),   Value::String(String::from("ext4"))),
            (Value::String(String::from("ino")),  Value::UInt64(s.ino as u64)),
        ];
        return Ok(Value::Map(map));
    }
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
        if fs.lookup(path).is_some() {
            let inode_id = fs.lookup(path).unwrap();
            fs.write(inode_id, data)?;
        } else {
            fs.create_file(path, data)?;
        }
        
        if let Some(ref mut graph) = *crate::knowledge::graph::GRAPH.lock() {
            let mut props = alloc::vec![
                (smartpack::Value::from("path"), smartpack::Value::from(String::from(path))),
                (smartpack::Value::from("size"), smartpack::Value::UInt64(data.len() as u64)),
            ];
            graph.insert_node("file", smartpack::Value::Map(props));
        }

        mark_dirty(path);
        Ok(())
    }
}

/// Delete a file from the filesystem.
pub fn delete_file(path: &str) -> Result<(), &'static str> {
    if is_disk_path(path) || is_fat_path(path) {
        return Err("delete not supported on this filesystem");
    }
    let mut vfs = VFS.lock();
    let fs = vfs.as_mut().ok_or("VFS not initialized")?;
    fs.delete(path)
}

/// Return the VFS path associated with an open file descriptor.
pub fn fd_path(fd_num: usize) -> Option<String> {
    let fdt = FD_TABLE.lock();
    fdt.as_ref()?.get(fd_num).map(|fd| fd.path.clone())
}

/// Return the byte size of a file, or None if it doesn't exist.
pub fn file_size(path: &str) -> Option<usize> {
    match read_file_full(path) {
        Ok(data) => Some(data.len()),
        Err(_)   => None,
    }
}

/// Delete a file (alias for delete_file, POSIX name).
pub fn unlink(path: &str) -> Result<(), &'static str> {
    delete_file(path)
}

/// Read an entire file into a Vec.
pub fn read_file_full(path: &str) -> Result<Vec<u8>, &'static str> {
    if is_fat_path(path) {
        let sub = fat_subpath(path);
        return crate::drivers::fat32::read_file(sub);
    }
    if crate::drivers::ext4::handles_path(path) {
        return crate::drivers::ext4::vfs_read_file(path).ok_or("ext4: read failed");
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

fn generate_cyber_bmp() -> Vec<u8> {
    let width = 64;
    let height = 64;
    let row_size = (width * 3 + 3) & !3;
    let pixel_data_size = row_size * height;
    let total_size = 54 + pixel_data_size;

    let mut bmp = alloc::vec![0u8; total_size];
    bmp[0] = b'B';
    bmp[1] = b'M';
    bmp[2..6].copy_from_slice(&(total_size as u32).to_le_bytes());
    bmp[10..14].copy_from_slice(&54u32.to_le_bytes());

    bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&(width as i32).to_le_bytes());
    bmp[22..26].copy_from_slice(&(height as i32).to_le_bytes());
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
    bmp[34..38].copy_from_slice(&(pixel_data_size as u32).to_le_bytes());

    for y in 0..height {
        let row_start = 54 + y * row_size;
        for x in 0..width {
            let px_off = row_start + x * 3;
            let b = (x * 255 / width) as u8;
            let g = 0u8;
            let r = (y * 255 / height) as u8;
            bmp[px_off] = b;
            bmp[px_off+1] = g;
            bmp[px_off+2] = r;
        }
    }
    bmp
}

fn generate_welcome_docx() -> Vec<u8> {
    let mut out = Vec::new();
    
    struct FileEntry {
        name: &'static str,
        data: &'static [u8],
    }
    
    let files = [
        FileEntry {
            name: "[Content_Types].xml",
            data: br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#,
        },
        FileEntry {
            name: "_rels/.rels",
            data: br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#,
        },
        FileEntry {
            name: "word/document.xml",
            data: br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r>
        <w:rPr>
          <w:b/>
          <w:sz w:val="48"/>
        </w:rPr>
        <w:t>Welcome to Smart Office!</w:t>
      </w:r>
    </w:p>
    <w:p>
      <w:r>
        <w:t>This is a native .docx document generated at boot.</w:t>
      </w:r>
    </w:p>
    <w:p>
      <w:r>
        <w:rPr>
          <w:i/>
        </w:rPr>
        <w:t>Enjoy the cyber aesthetics of Smart OS.</w:t>
      </w:r>
    </w:p>
  </w:body>
</w:document>"#,
        },
    ];
    
    let mut files_mut = Vec::new();
    let mut current_offset = 0u32;
    
    for f in &files {
        let name_bytes = f.name.as_bytes();
        let name_len = name_bytes.len() as u16;
        let data_len = f.data.len() as u32;
        
        let local_offset = current_offset;
        
        out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]); // Signature
        out.extend_from_slice(&[10, 0]); // Version needed
        out.extend_from_slice(&[0, 0]); // Flags
        out.extend_from_slice(&[0, 0]); // Compression (stored)
        out.extend_from_slice(&[0, 0, 0, 0]); // Date/Time
        out.extend_from_slice(&[0, 0, 0, 0]); // CRC-32 (dummy 0)
        out.extend_from_slice(&data_len.to_le_bytes()); // Compressed size
        out.extend_from_slice(&data_len.to_le_bytes()); // Uncompressed size
        out.extend_from_slice(&name_len.to_le_bytes()); // Filename len
        out.extend_from_slice(&[0, 0]); // Extra field len
        
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(f.data);
        
        current_offset += 30 + name_len as u32 + data_len;
        
        files_mut.push((f.name, f.data, local_offset));
    }
    
    let cd_start = out.len() as u32;
    let mut cd_size = 0u32;
    
    for (name, data, local_offset) in &files_mut {
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len() as u16;
        let data_len = data.len() as u32;
        
        out.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]); // Signature
        out.extend_from_slice(&[20, 0]); // Version made by
        out.extend_from_slice(&[10, 0]); // Version needed
        out.extend_from_slice(&[0, 0]); // Flags
        out.extend_from_slice(&[0, 0]); // Compression
        out.extend_from_slice(&[0, 0, 0, 0]); // Mod time
        out.extend_from_slice(&[0, 0, 0, 0]); // CRC-32
        out.extend_from_slice(&data_len.to_le_bytes()); // Compressed size
        out.extend_from_slice(&data_len.to_le_bytes()); // Uncompressed size
        out.extend_from_slice(&name_len.to_le_bytes()); // Filename len
        
        out.extend_from_slice(&[0; 16]); // Extra field, comment, disk start, etc.
        out.extend_from_slice(&local_offset.to_le_bytes()); // Local header offset
        out.extend_from_slice(name_bytes);
        
        cd_size += 46 + name_len as u32;
    }
    
    out.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]); // Signature
    out.extend_from_slice(&[0, 0, 0, 0]); // Disk number/start
    let file_count = files_mut.len() as u16;
    out.extend_from_slice(&file_count.to_le_bytes()); // CD records on this disk
    out.extend_from_slice(&file_count.to_le_bytes()); // Total CD records
    out.extend_from_slice(&cd_size.to_le_bytes()); // Size of CD
    out.extend_from_slice(&cd_start.to_le_bytes()); // Offset of CD
    out.extend_from_slice(&[0, 0]); // Comment len
    
    out
}

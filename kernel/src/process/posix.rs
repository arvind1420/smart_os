/// POSIX compatibility layer for Smart OS — Phase 12.
///
/// Provides: per-process CWD, lseek, open-with-flags, select.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::format;
use alloc::vec::Vec;
use spin::Mutex;
use crate::serial_println;

// ═══════════════════════════════════════════════════════════════
//  O_ flags (Linux-compatible values)
// ═══════════════════════════════════════════════════════════════

pub const O_RDONLY: usize = 0;
pub const O_WRONLY: usize = 1;
pub const O_RDWR: usize = 2;
pub const O_CREAT: usize = 0x40;
pub const O_TRUNC: usize = 0x200;
pub const O_APPEND: usize = 0x400;

/// lseek whence values.
pub const SEEK_SET: usize = 0;
pub const SEEK_CUR: usize = 1;
pub const SEEK_END: usize = 2;

// ═══════════════════════════════════════════════════════════════
//  Per-process current working directory
// ═══════════════════════════════════════════════════════════════

static CWD_TABLE: Mutex<BTreeMap<u64, String>> = Mutex::new(BTreeMap::new());

/// Initialize CWD for a new process.
pub fn init_cwd(pid: u64) {
    CWD_TABLE.lock().insert(pid, String::from("/"));
}

/// Get the CWD for a process.
pub fn get_cwd(pid: u64) -> String {
    CWD_TABLE.lock().get(&pid).cloned().unwrap_or_else(|| String::from("/"))
}

/// Set the CWD for a process (validates path exists in VFS).
pub fn set_cwd(pid: u64, path: &str) -> Result<(), &'static str> {
    // Resolve to absolute path
    let abs = resolve_path(pid, path);
    // Validate the directory exists
    let _ = crate::vfs::readdir(&abs).map_err(|_| "No such directory")?;
    CWD_TABLE.lock().insert(pid, abs);
    Ok(())
}

/// Destroy CWD entry on process exit.
pub fn destroy_cwd(pid: u64) {
    CWD_TABLE.lock().remove(&pid);
}

/// Resolve a path relative to the process CWD.
/// Absolute paths (starting with /) are returned as-is.
/// Relative paths are prepended with the CWD.
pub fn resolve_path(pid: u64, path: &str) -> String {
    if path.starts_with('/') {
        String::from(path)
    } else {
        let cwd = get_cwd(pid);
        if cwd.ends_with('/') {
            format!("{}{}", cwd, path)
        } else {
            format!("{}/{}", cwd, path)
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  lseek — reposition file offset
// ═══════════════════════════════════════════════════════════════

/// Seek within a file descriptor. Returns the new offset.
pub fn lseek_fd(pid: u64, fd: usize, offset: i64, whence: usize) -> Result<usize, &'static str> {
    use super::fd::{PROC_FDS, FdKind};

    let mut table = PROC_FDS.lock();
    let fdt = table.get_mut(&pid).ok_or("No FD table for process")?;
    let entry = fdt.fds.get_mut(&fd).ok_or("Invalid file descriptor")?;

    match &mut entry.kind {
        FdKind::File { path, offset: file_offset } => {
            let file_size = crate::vfs::stat(path.as_str())
                .ok()
                .and_then(|v| {
                    if let smartpack::Value::Map(ref m) = v {
                        m.iter().find(|(k, _)| {
                            if let smartpack::Value::String(s) = k { s == "size" } else { false }
                        }).and_then(|(_, v)| {
                            if let smartpack::Value::UInt64(n) = v { Some(*n as usize) } else { None }
                        })
                    } else {
                        None
                    }
                })
                .unwrap_or(0);

            let new_offset = match whence {
                SEEK_SET => offset as usize,
                SEEK_CUR => (*file_offset as i64 + offset) as usize,
                SEEK_END => (file_size as i64 + offset) as usize,
                _ => return Err("Invalid whence"),
            };

            *file_offset = new_offset;
            Ok(new_offset)
        }
        _ => Err("Cannot seek on this FD type"),
    }
}

// ═══════════════════════════════════════════════════════════════
//  open_with_flags — POSIX-style open
// ═══════════════════════════════════════════════════════════════

/// Open a file with POSIX-style flags. Returns (FdKind, FdFlags).
pub fn open_with_flags(path: &str, flags: usize) -> Result<(super::fd::FdKind, super::fd::FdFlags), &'static str> {
    use super::fd::{FdKind, FdFlags};

    let access = flags & 0x3; // O_RDONLY=0, O_WRONLY=1, O_RDWR=2
    let create = flags & O_CREAT != 0;
    let trunc = flags & O_TRUNC != 0;

    // Try to open existing file
    let exists = crate::vfs::stat(path).is_ok();

    if !exists && create {
        // Create the file
        crate::vfs::create_and_write(path, b"")?;
    } else if !exists {
        return Err("File not found");
    }

    if trunc && exists {
        // Truncate by rewriting with empty content
        crate::vfs::create_and_write(path, b"")?;
    }

    let fd_flags = match access {
        O_RDONLY => FdFlags::read_only(),
        O_WRONLY => FdFlags::write_only(),
        O_RDWR => FdFlags::read_write(),
        _ => FdFlags::read_only(),
    };

    let kind = FdKind::File {
        path: String::from(path),
        offset: 0,
    };

    Ok((kind, fd_flags))
}

// ═══════════════════════════════════════════════════════════════
//  select — poll FDs for readability
// ═══════════════════════════════════════════════════════════════

/// Simplified select: returns which of the given FDs are readable.
/// Files are always ready. Pipes check buffer. TCP sockets check recv_buf.
pub fn select_fds(pid: u64, fds: &[usize], _timeout_ms: u64) -> Vec<usize> {
    use super::fd::{PROC_FDS, FdKind};

    let table = PROC_FDS.lock();
    let fdt = match table.get(&pid) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut ready = Vec::new();
    for &fd_num in fds {
        if let Some(entry) = fdt.fds.get(&fd_num) {
            let is_ready = match &entry.kind {
                FdKind::File { .. } => true,  // files are always ready
                FdKind::Stdio => true,         // stdio always ready
                FdKind::PipeRead { pipe_id } => {
                    // Check if pipe has data
                    crate::process::pipe::pipe_readable(*pipe_id)
                }
                FdKind::PipeWrite { .. } => true, // write end always ready
                FdKind::TcpSocket { conn_id } => {
                    // Check if TCP socket has received data
                    crate::net::tcp::has_recv_data(*conn_id)
                }
            };
            if is_ready {
                ready.push(fd_num);
            }
        }
    }
    ready
}

// ═══════════════════════════════════════════════════════════════
//  Sockets — POSIX Networking
// ═══════════════════════════════════════════════════════════════

pub const AF_INET: usize = 2;
pub const SOCK_STREAM: usize = 1;

/// Create a socket file descriptor.
pub fn socket(pid: u64, domain: usize, type_: usize, _protocol: usize) -> Result<usize, &'static str> {
    if domain != AF_INET || type_ != SOCK_STREAM {
        return Err("Only AF_INET / SOCK_STREAM supported");
    }

    use super::fd::{PROC_FDS, FdKind, FdFlags};
    let mut table = PROC_FDS.lock();
    let fdt = table.get_mut(&pid).ok_or("No FD table for process")?;
    
    // Pick an ephemeral local port
    let _local_port = crate::net::tcp::alloc_ephemeral_port();

    // A newly created socket isn't connected yet, but we store a placeholder
    // connection ID of 0 until connect() is called.
    let fd_num = fdt.alloc_fd(FdKind::TcpSocket { conn_id: 0 }, FdFlags::read_write());

    Ok(fd_num)
}

/// Connect a socket to a remote address.
pub fn connect(pid: u64, fd: usize, ip: [u8; 4], port: u16) -> Result<(), &'static str> {
    use super::fd::{PROC_FDS, FdKind};
    
    let local_port = crate::net::tcp::alloc_ephemeral_port();
    let conn_id = crate::net::tcp::connect(ip, port, local_port)?;

    let mut table = PROC_FDS.lock();
    let fdt = table.get_mut(&pid).ok_or("No FD table for process")?;
    let entry = fdt.get_mut(fd).ok_or("Invalid file descriptor")?;

    match &mut entry.kind {
        FdKind::TcpSocket { conn_id: cid } => {
            *cid = conn_id;
            Ok(())
        }
        _ => Err("Not a socket"),
    }
}

/// Send data over a socket.
pub fn send(pid: u64, fd: usize, buf: &[u8]) -> Result<usize, &'static str> {
    use super::fd::{PROC_FDS, FdKind};
    let table = PROC_FDS.lock();
    let fdt = table.get(&pid).ok_or("No FD table for process")?;
    let entry = fdt.get(fd).ok_or("Invalid file descriptor")?;

    match &entry.kind {
        FdKind::TcpSocket { conn_id } => {
            if *conn_id == 0 {
                return Err("Socket not connected");
            }
            crate::net::tcp::send(*conn_id, buf)
        }
        _ => Err("Not a socket"),
    }
}

/// Receive data from a socket.
pub fn recv(pid: u64, fd: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
    use super::fd::{PROC_FDS, FdKind};
    let table = PROC_FDS.lock();
    let fdt = table.get(&pid).ok_or("No FD table for process")?;
    let entry = fdt.get(fd).ok_or("Invalid file descriptor")?;

    match &entry.kind {
        FdKind::TcpSocket { conn_id } => {
            if *conn_id == 0 {
                return Err("Socket not connected");
            }
            crate::net::tcp::recv(*conn_id, buf)
        }
        _ => Err("Not a socket"),
    }
}

/// Initialize the POSIX compatibility layer.
pub fn init() {
    serial_println!("[posix] POSIX compatibility layer initialized.");
}

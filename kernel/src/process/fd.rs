/// Per-process file descriptor table for Smart OS.
///
/// Phase 11: Each process gets its own FD table. fork() clones it,
/// exec() preserves it (Unix semantics). FDs 0/1/2 are reserved for
/// stdin/stdout/stderr and handled specially (serial I/O).

use alloc::collections::BTreeMap;
use alloc::string::String;
use spin::Mutex;
use crate::serial_println;

/// Kind of file descriptor.
#[derive(Debug, Clone)]
pub enum FdKind {
    /// A file in the VFS with its current read/write offset.
    File { path: String, offset: usize },
    /// Read end of a pipe.
    PipeRead { pipe_id: u64 },
    /// Write end of a pipe.
    PipeWrite { pipe_id: u64 },
    /// Standard I/O (stdin=0, stdout=1, stderr=2) — routed to serial.
    Stdio,
    /// TCP socket (for select/poll support).
    TcpSocket { conn_id: u64 },
}

/// Flags for a file descriptor.
#[derive(Debug, Clone, Copy)]
pub struct FdFlags {
    pub readable: bool,
    pub writable: bool,
}

impl FdFlags {
    pub fn read_only() -> Self {
        Self { readable: true, writable: false }
    }
    pub fn write_only() -> Self {
        Self { readable: false, writable: true }
    }
    pub fn read_write() -> Self {
        Self { readable: true, writable: true }
    }
}

/// A single file descriptor entry.
#[derive(Debug, Clone)]
pub struct ProcessFd {
    pub kind: FdKind,
    pub flags: FdFlags,
}

/// Per-process FD table.
#[derive(Debug, Clone)]
pub struct ProcessFdTable {
    /// File descriptors indexed by FD number.
    pub fds: BTreeMap<usize, ProcessFd>,
    /// Next FD number to allocate.
    next_fd: usize,
}

impl ProcessFdTable {
    /// Create a new FD table with stdin/stdout/stderr pre-allocated.
    pub fn new() -> Self {
        let mut fds = BTreeMap::new();
        // FD 0 = stdin
        fds.insert(0, ProcessFd {
            kind: FdKind::Stdio,
            flags: FdFlags::read_only(),
        });
        // FD 1 = stdout
        fds.insert(1, ProcessFd {
            kind: FdKind::Stdio,
            flags: FdFlags::write_only(),
        });
        // FD 2 = stderr
        fds.insert(2, ProcessFd {
            kind: FdKind::Stdio,
            flags: FdFlags::write_only(),
        });
        Self { fds, next_fd: 3 }
    }

    /// Allocate a new FD with the given kind. Returns the FD number.
    pub fn alloc_fd(&mut self, kind: FdKind, flags: FdFlags) -> usize {
        let fd = self.next_fd;
        self.fds.insert(fd, ProcessFd { kind, flags });
        self.next_fd = fd + 1;
        fd
    }

    /// Get a reference to an FD entry.
    pub fn get(&self, fd: usize) -> Option<&ProcessFd> {
        self.fds.get(&fd)
    }

    /// Get a mutable reference to an FD entry.
    pub fn get_mut(&mut self, fd: usize) -> Option<&mut ProcessFd> {
        self.fds.get_mut(&fd)
    }

    /// Close an FD.
    pub fn close(&mut self, fd: usize) -> Option<ProcessFd> {
        self.fds.remove(&fd)
    }

    /// Duplicate an FD: copy old_fd to new_fd (closing new_fd if open).
    pub fn dup2(&mut self, old_fd: usize, new_fd: usize) -> Result<(), &'static str> {
        let entry = self.fds.get(&old_fd).cloned().ok_or("Bad fd")?;
        self.fds.insert(new_fd, entry);
        if new_fd >= self.next_fd {
            self.next_fd = new_fd + 1;
        }
        Ok(())
    }

    /// Deep clone this FD table (for fork).
    pub fn clone_table(&self) -> Self {
        Self {
            fds: self.fds.clone(),
            next_fd: self.next_fd,
        }
    }

    /// Close all FDs.
    pub fn close_all(&mut self) {
        self.fds.clear();
    }

    /// Number of open FDs.
    pub fn count(&self) -> usize {
        self.fds.len()
    }

    /// List all open FDs as (fd_num, description).
    pub fn list(&self) -> alloc::vec::Vec<(usize, String)> {
        self.fds.iter().map(|(&fd, entry)| {
            let desc = match &entry.kind {
                FdKind::File { path, offset } => alloc::format!("file:{} @{}", path, offset),
                FdKind::PipeRead { pipe_id } => alloc::format!("pipe-r:{}", pipe_id),
                FdKind::PipeWrite { pipe_id } => alloc::format!("pipe-w:{}", pipe_id),
                FdKind::Stdio => String::from("stdio"),
                FdKind::TcpSocket { conn_id } => alloc::format!("tcp:{}", conn_id),
            };
            (fd, desc)
        }).collect()
    }
}

/// Global table of per-process FD tables, keyed by PID.
pub(crate) static PROC_FDS: Mutex<BTreeMap<u64, ProcessFdTable>> = Mutex::new(BTreeMap::new());

/// Create a new FD table for a process (with stdin/stdout/stderr).
pub fn create_fd_table(pid: u64) {
    PROC_FDS.lock().insert(pid, ProcessFdTable::new());
}

/// Destroy a process's FD table.
pub fn destroy_fd_table(pid: u64) {
    PROC_FDS.lock().remove(&pid);
}

/// Clone a parent's FD table to a child process.
pub fn clone_fd_table(src_pid: u64, dst_pid: u64) {
    let table = PROC_FDS.lock();
    if let Some(src) = table.get(&src_pid) {
        let cloned = src.clone_table();
        drop(table);
        PROC_FDS.lock().insert(dst_pid, cloned);
    }
}

/// Execute a closure with a process's FD table.
pub fn with_fd_table<F, R>(pid: u64, f: F) -> Result<R, &'static str>
where
    F: FnOnce(&mut ProcessFdTable) -> R,
{
    let mut tables = PROC_FDS.lock();
    let table = tables.get_mut(&pid).ok_or("No FD table for process")?;
    Ok(f(table))
}

/// Get a snapshot of FD info for a process (for debugging).
pub fn fd_list(pid: u64) -> alloc::vec::Vec<(usize, String)> {
    let tables = PROC_FDS.lock();
    if let Some(table) = tables.get(&pid) {
        table.list()
    } else {
        alloc::vec::Vec::new()
    }
}

/// Initialize the FD subsystem.
pub fn init() {
    // Create FD table for kernel process (pid=0)
    create_fd_table(0);
    serial_println!("[fd] Per-process file descriptor tables initialized.");
}

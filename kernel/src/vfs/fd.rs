/// File descriptor table.
///
/// Maps integer file descriptors to open file state.

use alloc::collections::BTreeMap;
use alloc::string::String;
use super::inode::InodeId;

/// An open file descriptor.
#[derive(Debug, Clone)]
pub struct FileDescriptor {
    pub inode_id: InodeId,
    pub path: String,
    pub offset: usize,
    pub readable: bool,
    pub writable: bool,
}

/// File descriptor table (one per thread, but global for Phase 2).
pub struct FdTable {
    fds: BTreeMap<usize, FileDescriptor>,
    next_fd: usize,
}

impl FdTable {
    pub fn new() -> Self {
        Self {
            fds: BTreeMap::new(),
            next_fd: 3, // 0=stdin, 1=stdout, 2=stderr (reserved)
        }
    }

    /// Open a file and return a file descriptor number.
    pub fn open(&mut self, inode_id: InodeId, path: &str) -> usize {
        let fd_num = self.next_fd;
        self.next_fd += 1;
        self.fds.insert(fd_num, FileDescriptor {
            inode_id,
            path: String::from(path),
            offset: 0,
            readable: true,
            writable: true,
        });
        fd_num
    }

    /// Close a file descriptor.
    pub fn close(&mut self, fd_num: usize) -> Option<FileDescriptor> {
        self.fds.remove(&fd_num)
    }

    /// Get a file descriptor (immutable).
    pub fn get(&self, fd_num: usize) -> Option<&FileDescriptor> {
        self.fds.get(&fd_num)
    }

    /// Get a file descriptor (mutable).
    pub fn get_mut(&mut self, fd_num: usize) -> Option<&mut FileDescriptor> {
        self.fds.get_mut(&fd_num)
    }
}

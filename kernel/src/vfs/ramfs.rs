/// In-memory RAM filesystem.
///
/// A simple tree-structured filesystem that lives entirely in kernel heap.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use super::inode::{Inode, InodeId, InodeType};
use core::sync::atomic::{AtomicU64, Ordering};

static NEXT_INODE: AtomicU64 = AtomicU64::new(1);

fn alloc_inode_id() -> InodeId {
    NEXT_INODE.fetch_add(1, Ordering::Relaxed)
}

/// The RAM filesystem.
pub struct RamFs {
    /// All inodes, indexed by ID.
    inodes: BTreeMap<InodeId, Inode>,
    /// Path → inode ID mapping for fast lookups.
    path_map: BTreeMap<String, InodeId>,
    /// Root inode ID.
    root: Option<InodeId>,
}

impl RamFs {
    pub fn new() -> Self {
        Self {
            inodes: BTreeMap::new(),
            path_map: BTreeMap::new(),
            root: None,
        }
    }

    /// Create a directory at the given path.
    pub fn mkdir(&mut self, path: &str) -> Result<(), &'static str> {
        if self.path_map.contains_key(path) {
            return Ok(()); // Already exists
        }

        let name = path_filename(path);
        let id = alloc_inode_id();
        let inode = Inode::new_directory(id, name);

        // Add to parent directory
        if let Some(parent_path) = path_parent(path) {
            if let Some(&parent_id) = self.path_map.get(parent_path) {
                if let Some(parent) = self.inodes.get_mut(&parent_id) {
                    parent.children.push(id);
                }
            }
        }

        self.inodes.insert(id, inode);
        self.path_map.insert(String::from(path), id);

        if self.root.is_none() {
            self.root = Some(id);
        }

        Ok(())
    }

    /// Create a file at the given path with initial content.
    pub fn create_file(&mut self, path: &str, data: &[u8]) -> Result<(), &'static str> {
        if self.path_map.contains_key(path) {
            return Err("File already exists");
        }

        let name = path_filename(path);
        let id = alloc_inode_id();
        let inode = Inode::new_file(id, name, data);

        // Add to parent directory
        if let Some(parent_path) = path_parent(path) {
            if let Some(&parent_id) = self.path_map.get(parent_path) {
                if let Some(parent) = self.inodes.get_mut(&parent_id) {
                    parent.children.push(id);
                }
            }
        }

        self.inodes.insert(id, inode);
        self.path_map.insert(String::from(path), id);
        Ok(())
    }

    /// Look up a path and return the inode ID.
    pub fn lookup(&self, path: &str) -> Option<InodeId> {
        self.path_map.get(path).copied()
    }

    /// Read data from an inode.
    pub fn read(&self, inode_id: InodeId, offset: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
        let inode = self.inodes.get(&inode_id).ok_or("Inode not found")?;
        if inode.inode_type != InodeType::File {
            return Err("Not a file");
        }
        if offset >= inode.data.len() {
            return Ok(0);
        }
        let available = inode.data.len() - offset;
        let to_read = buf.len().min(available);
        buf[..to_read].copy_from_slice(&inode.data[offset..offset + to_read]);
        Ok(to_read)
    }

    /// Write data to an inode (replaces content).
    pub fn write(&mut self, inode_id: InodeId, data: &[u8]) -> Result<(), &'static str> {
        let inode = self.inodes.get_mut(&inode_id).ok_or("Inode not found")?;
        if inode.inode_type != InodeType::File {
            return Err("Not a file");
        }
        inode.data = data.to_vec();
        inode.size = data.len() as u64;
        inode.modified_at = crate::drivers::timer::ticks();
        Ok(())
    }

    /// List directory contents.
    pub fn readdir(&self, path: &str) -> Result<Vec<String>, &'static str> {
        let &inode_id = self.path_map.get(path).ok_or("Path not found")?;
        let inode = self.inodes.get(&inode_id).ok_or("Inode not found")?;
        if inode.inode_type != InodeType::Directory {
            return Err("Not a directory");
        }

        let mut names = Vec::new();
        for &child_id in &inode.children {
            if let Some(child) = self.inodes.get(&child_id) {
                names.push(child.name.clone());
            }
        }
        Ok(names)
    }

    /// Get file/directory metadata as SmartPack Value.
    pub fn stat(&self, path: &str) -> Result<smartpack::Value, &'static str> {
        let &inode_id = self.path_map.get(path).ok_or("Path not found")?;
        let inode = self.inodes.get(&inode_id).ok_or("Inode not found")?;
        Ok(inode.to_smartpack())
    }
}

/// Extract the filename from a path.
fn path_filename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Get the parent path.
fn path_parent(path: &str) -> Option<&str> {
    if path == "/" {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => Some("/"),
        Some(pos) => Some(&trimmed[..pos]),
        None => None,
    }
}

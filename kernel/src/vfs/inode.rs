/// Inode — the core filesystem metadata structure.
///
/// Each file/directory is represented by an inode containing SmartPack metadata.

use alloc::string::String;
use alloc::vec::Vec;
use smartpack::Value;

/// Unique inode identifier.
pub type InodeId = u64;

/// Type of filesystem entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeType {
    File,
    Directory,
    Device,
}

/// An inode stores metadata and data for a filesystem object.
#[derive(Debug, Clone)]
pub struct Inode {
    pub id: InodeId,
    pub name: String,
    pub inode_type: InodeType,
    /// File content (for regular files).
    pub data: Vec<u8>,
    /// Child inode IDs (for directories).
    pub children: Vec<InodeId>,
    /// Size in bytes.
    pub size: u64,
    /// Creation timestamp (tick count).
    pub created_at: u64,
    /// Last modification timestamp.
    pub modified_at: u64,
}

impl Inode {
    pub fn new_file(id: InodeId, name: &str, data: &[u8]) -> Self {
        let now = crate::drivers::timer::ticks();
        Self {
            id,
            name: String::from(name),
            inode_type: InodeType::File,
            data: data.to_vec(),
            children: Vec::new(),
            size: data.len() as u64,
            created_at: now,
            modified_at: now,
        }
    }

    pub fn new_directory(id: InodeId, name: &str) -> Self {
        let now = crate::drivers::timer::ticks();
        Self {
            id,
            name: String::from(name),
            inode_type: InodeType::Directory,
            data: Vec::new(),
            children: Vec::new(),
            size: 0,
            created_at: now,
            modified_at: now,
        }
    }

    /// Convert inode metadata to a SmartPack Value.
    pub fn to_smartpack(&self) -> Value {
        Value::Map(alloc::vec![
            (Value::String(String::from("name")), Value::String(self.name.clone())),
            (Value::String(String::from("type")), Value::String(String::from(match self.inode_type {
                InodeType::File => "file",
                InodeType::Directory => "directory",
                InodeType::Device => "device",
            }))),
            (Value::String(String::from("size")), Value::UInt64(self.size)),
            (Value::String(String::from("created")), Value::Timestamp(self.created_at)),
            (Value::String(String::from("modified")), Value::Timestamp(self.modified_at)),
            (Value::String(String::from("children")), Value::UInt32(self.children.len() as u32)),
        ])
    }
}

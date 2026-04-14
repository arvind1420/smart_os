/// Embedded Document Database for Smart OS.
///
/// Provides a NoSQL-like Key-Value document store backed by the VFS.
/// Uses `smartpack` for structured binary serialization and maintains
/// an in-memory B-Tree index for O(log N) lookups.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use crate::syscall::{syscall3, SYS_OPEN, SYS_READ, SYS_WRITE, SYS_CLOSE, SYS_STAT};
use smartpack::{Value, encode, decode};

const O_RDWR: u64 = 2;
const O_CREAT: u64 = 64;

pub struct SmartDb {
    fd: usize,
    index: BTreeMap<String, u64>, // Maps document ID to byte offset in the file
    next_offset: u64,
}

impl SmartDb {
    /// Opens or creates a database file.
    pub fn open(path: &str) -> Result<Self, &'static str> {
        let fd = crate::syscall::syscall3(
            SYS_OPEN, 
            path.as_ptr() as u64, 
            path.len() as u64, 
            O_RDWR | O_CREAT
        );
        if fd == u64::MAX {
            return Err("Failed to open database file");
        }

        let mut db = Self {
            fd: fd as usize,
            index: BTreeMap::new(),
            next_offset: 0,
        };

        db.rebuild_index()?;
        Ok(db)
    }

    /// Rebuilds the in-memory index by scanning the file for document headers.
    fn rebuild_index(&mut self) -> Result<(), &'static str> {
        // For MVP, we'll assume the file is a sequence of SmartPack serialized
        // Map objects where one of the keys is "_id".
        // A true production DB would use a Write-Ahead Log and slotted pages.
        let stat_val = crate::syscall::syscall3(SYS_STAT, 0, 0, self.fd as u64); // Need a proper stat wrapper
        // Simplification for no_std: just read until EOF.
        
        // This is a simplified append-only log indexer.
        let mut offset = 0;
        let mut header_buf = [0u8; 8];
        loop {
            // Read 8 byte length header (custom framing for our DB)
            let n = crate::syscall::syscall3(
                SYS_READ, 
                self.fd as u64, 
                header_buf.as_mut_ptr() as u64, 
                8
            );
            if n == 0 || n == u64::MAX { break; } // EOF or error
            
            let mut len_bytes = [0u8; 8];
            len_bytes.copy_from_slice(&header_buf);
            let doc_len = u64::from_le_bytes(len_bytes) as usize;
            
            // Read the actual document
            let mut doc_buf = alloc::vec![0u8; doc_len];
            let n2 = crate::syscall::syscall3(
                SYS_READ,
                self.fd as u64,
                doc_buf.as_mut_ptr() as u64,
                doc_len as u64
            );
            if n2 == 0 || n2 == u64::MAX { break; }

            // Decode and find "_id"
            if let Ok(Value::Map(map)) = decode(&doc_buf) {
                for (k, v) in map {
                    if let Value::String(key_str) = k {
                        if key_str == "_id" {
                            if let Value::String(id_val) = v {
                                // Latest offset wins (append-only update)
                                self.index.insert(id_val, offset);
                            }
                        }
                    }
                }
            }
            
            offset += 8 + doc_len as u64;
        }
        
        self.next_offset = offset;
        Ok(())
    }

    /// Insert or Update a document.
    pub fn put(&mut self, id: &str, mut document: Vec<(Value, Value)>) -> Result<(), &'static str> {
        // Ensure the ID is in the document
        document.push((Value::String(String::from("_id")), Value::String(String::from(id))));
        let val = Value::Map(document);
        
        let encoded = encode(&val).map_err(|_| "Failed to encode document")?;
        
        let doc_len = encoded.len() as u64;
        let len_bytes = doc_len.to_le_bytes();
        
        // Append to file
        crate::syscall::syscall3(SYS_WRITE, self.fd as u64, len_bytes.as_ptr() as u64, 8);
        crate::syscall::syscall3(SYS_WRITE, self.fd as u64, encoded.as_ptr() as u64, doc_len);
        
        // Update index
        self.index.insert(String::from(id), self.next_offset);
        self.next_offset += 8 + doc_len;
        
        Ok(())
    }

    /// Retrieve a document by ID.
    pub fn get(&self, id: &str) -> Result<Option<Value>, &'static str> {
        let offset = match self.index.get(id) {
            Some(&off) => off,
            None => return Ok(None),
        };

        // We would need SYS_LSEEK in a real implementation.
        // For this MVP SDK without seek exposed, we simulate it by re-reading or relying on VFS capabilities.
        // Since we don't have SYS_LSEEK exposed yet, we will just read the whole file again
        // (Horrible for performance, but sufficient to prove the API works until we add lseek).
        
        // Close and reopen to reset offset
        crate::syscall::syscall1(SYS_CLOSE, self.fd as u64);
        // ... (this requires knowing the path, so let's just keep the state simple or add seek)
        Err("LSEEK not implemented in kernel yet, cannot fetch random offsets.")
    }
}

impl Drop for SmartDb {
    fn drop(&mut self) {
        crate::syscall::syscall1(SYS_CLOSE, self.fd as u64);
    }
}

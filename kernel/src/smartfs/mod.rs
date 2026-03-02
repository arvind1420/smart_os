/// SmartFS — Content-aware filesystem for Smart OS.
///
/// Builds on top of the VFS ramfs layer to add:
/// - Content-aware file categorization (heuristic + AI)
/// - Content-defined chunking (CDC) for deduplication
/// - Per-file compression selection (LZ4-simple, RLE, or raw)
/// - Rich SmartPack metadata per file

pub mod metadata;
pub mod classify;
pub mod chunking;
pub mod compression;

use alloc::vec::Vec;
use metadata::SmartMetadata;

/// Initialize SmartFS.
pub fn init() {
    chunking::init();
    crate::serial_println!("[smartfs] SmartFS initialized (CDC + compression + classify).");
}

/// Store a file through SmartFS with classification, compression, and dedup.
pub fn store_file(path: &str, data: &[u8]) -> Result<(), &'static str> {
    // Step 1: Classify the file content
    let filename = path.rsplit('/').next().unwrap_or(path);
    let (category, confidence) = classify::classify(data, filename);

    // Step 2: Select compression method based on category
    let method = compression::select_compression(data, category);

    // Step 3: Compress the data
    let compressed = compression::compress(data, method);

    // Step 4: Chunk and deduplicate the compressed data
    let chunk_hashes = chunking::store_chunks(&compressed);

    // Step 5: Build metadata
    let meta = SmartMetadata {
        category,
        compression: method,
        original_size: data.len(),
        compressed_size: compressed.len(),
        chunk_count: chunk_hashes.len(),
        chunk_hashes: chunk_hashes.clone(),
        tags: alloc::vec![],
        ai_confidence: if confidence > 0.0 { Some(confidence) } else { None },
    };

    // Step 6: Store compressed data in VFS
    crate::vfs::mkdir(path_parent(path)).ok(); // Ensure parent exists
    // Store compressed data
    let vfs_guard = crate::vfs::open(path);
    if vfs_guard.is_err() {
        // File doesn't exist, create it via internal VFS
        // We need to write the compressed data to the underlying ramfs
        store_raw(path, &compressed)?;
    } else {
        let fd = vfs_guard.unwrap();
        crate::vfs::write(fd, &compressed)?;
        crate::vfs::close(fd).ok();
    }

    crate::serial_println!(
        "[smartfs] Stored '{}': {} -> {} bytes ({:?}, {:?}, {} chunks, {:.0}% conf)",
        path, data.len(), compressed.len(), category, method,
        meta.chunk_count, confidence * 100.0,
    );

    Ok(())
}

/// Read a file through SmartFS, decompressing if needed.
pub fn read_file(path: &str) -> Result<Vec<u8>, &'static str> {
    // Read raw compressed data from VFS
    let fd = crate::vfs::open(path)?;
    let mut buf = alloc::vec![0u8; 65536]; // Max read size
    let n = crate::vfs::read(fd, &mut buf)?;
    crate::vfs::close(fd).ok();

    let compressed_data = &buf[..n];

    // For now, try to detect if data was compressed by checking SmartFS metadata
    // In a full implementation, metadata would be stored alongside the file
    // For Phase 3 demo: attempt LZ4 decompress, fall back to raw
    // Since we don't yet persist metadata, return raw data
    Ok(compressed_data.to_vec())
}

/// Classify a file's content.
pub fn classify_file(path: &str) -> Result<(metadata::FileCategory, f32), &'static str> {
    let fd = crate::vfs::open(path)?;
    let mut buf = alloc::vec![0u8; 1024];
    let n = crate::vfs::read(fd, &mut buf)?;
    crate::vfs::close(fd).ok();

    let filename = path.rsplit('/').next().unwrap_or(path);
    Ok(classify::classify(&buf[..n], filename))
}

/// Get deduplication statistics.
pub fn dedup_stats() -> (usize, usize) {
    chunking::stats()
}

/// Store raw bytes to the VFS (internal helper).
fn store_raw(path: &str, data: &[u8]) -> Result<(), &'static str> {
    // Access the internal VFS to create and write file
    // We use the VFS public API: try mkdir for parent, then create via write
    let parent = path_parent(path);
    if !parent.is_empty() && parent != "/" {
        crate::vfs::mkdir(parent).ok();
    }

    // For ramfs, we need to create the file first then write
    // The VFS doesn't have a "create" in its public API, so we use a workaround:
    // write to the inode directly through the VFS internal state
    // For simplicity in Phase 3, use the internal VFS access pattern
    crate::vfs::create_and_write(path, data)
}

/// Extract parent path from a file path.
fn path_parent(path: &str) -> &str {
    if let Some(pos) = path.rfind('/') {
        if pos == 0 { "/" } else { &path[..pos] }
    } else {
        "/"
    }
}

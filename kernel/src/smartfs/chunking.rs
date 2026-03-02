/// Content-defined chunking (CDC) for SmartFS deduplication.
///
/// Uses a rolling hash to find natural chunk boundaries in data.
/// Chunks are stored in a global dedup store keyed by FNV-1a hash.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

/// Minimum chunk size in bytes.
const MIN_CHUNK: usize = 256;
/// Maximum chunk size in bytes.
const MAX_CHUNK: usize = 8192;
/// Target average chunk size (~2K).
const CHUNK_MODULUS: u32 = 2048;
/// Pattern to match for chunk boundary.
const CHUNK_PATTERN: u32 = 0;
/// Rolling hash window size.
const WINDOW_SIZE: usize = 32;

/// A content-defined chunk.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub offset: usize,
    pub length: usize,
    pub hash: u32,
}

/// Global chunk store for deduplication.
struct ChunkStore {
    /// hash → chunk data.
    chunks: BTreeMap<u32, Vec<u8>>,
    /// Total chunks encountered.
    total_seen: usize,
    /// Deduplicated hits.
    dedup_hits: usize,
}

impl ChunkStore {
    const fn new() -> Self {
        Self {
            chunks: BTreeMap::new(),
            total_seen: 0,
            dedup_hits: 0,
        }
    }
}

static STORE: Mutex<ChunkStore> = Mutex::new(ChunkStore::new());

/// Initialize the chunk store.
pub fn init() {
    // Already initialized as const; nothing to do
}

/// Split data into content-defined chunks using a rolling hash.
pub fn chunk_data(data: &[u8]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    if data.is_empty() {
        return chunks;
    }

    let mut start = 0;
    let mut hash: u32 = 0;

    let mut i = 0;
    while i < data.len() {
        // Update rolling hash
        hash = hash.wrapping_mul(31).wrapping_add(data[i] as u32);

        // Remove oldest byte from window if past window size
        let chunk_len = i - start + 1;
        if chunk_len > WINDOW_SIZE {
            let old_idx = i - WINDOW_SIZE;
            // Approximate removal (not exact, but good enough for chunking)
            hash = hash.wrapping_sub(
                (data[old_idx] as u32).wrapping_mul(31u32.wrapping_pow(WINDOW_SIZE as u32))
            );
        }

        // Check for chunk boundary
        let at_boundary = chunk_len >= MIN_CHUNK && (hash % CHUNK_MODULUS == CHUNK_PATTERN);
        let at_max = chunk_len >= MAX_CHUNK;
        let at_end = i == data.len() - 1;

        if at_boundary || at_max || at_end {
            let chunk_hash = fnv1a(&data[start..=i]);
            chunks.push(Chunk {
                offset: start,
                length: chunk_len,
                hash: chunk_hash,
            });
            start = i + 1;
            hash = 0;
        }

        i += 1;
    }

    chunks
}

/// Store chunks with deduplication. Returns the list of chunk hashes (references).
pub fn store_chunks(data: &[u8]) -> Vec<u32> {
    let chunks = chunk_data(data);
    let mut hashes = Vec::with_capacity(chunks.len());
    let mut store = STORE.lock();

    for chunk in &chunks {
        store.total_seen += 1;
        hashes.push(chunk.hash);

        if store.chunks.contains_key(&chunk.hash) {
            store.dedup_hits += 1;
        } else {
            let chunk_data = data[chunk.offset..chunk.offset + chunk.length].to_vec();
            store.chunks.insert(chunk.hash, chunk_data);
        }
    }

    hashes
}

/// Reassemble data from a list of chunk hashes.
pub fn reassemble(chunk_hashes: &[u32]) -> Result<Vec<u8>, &'static str> {
    let store = STORE.lock();
    let mut result = Vec::new();

    for &hash in chunk_hashes {
        let chunk = store.chunks.get(&hash).ok_or("Chunk not found in store")?;
        result.extend_from_slice(chunk);
    }

    Ok(result)
}

/// Get deduplication statistics: (unique_chunks, total_chunks_seen).
pub fn stats() -> (usize, usize) {
    let store = STORE.lock();
    (store.chunks.len(), store.total_seen)
}

/// FNV-1a hash (32-bit) — simple, fast, good distribution.
fn fnv1a(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c9dc5; // FNV offset basis
    for &byte in data {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x01000193); // FNV prime
    }
    hash
}

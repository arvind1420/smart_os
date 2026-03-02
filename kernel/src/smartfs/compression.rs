/// Compression algorithms for SmartFS.
///
/// Provides two simple compressors suitable for no_std:
/// - RLE (Run-Length Encoding): great for files with repeated bytes
/// - LZ4-simple: simplified LZ4-like compressor for repetitive text
///
/// Both are designed for simplicity and correctness over maximum compression ratio.

use alloc::vec::Vec;
use super::metadata::{FileCategory, CompressionMethod};

/// Select the best compression method based on file content and category.
pub fn select_compression(data: &[u8], category: FileCategory) -> CompressionMethod {
    if data.len() < 64 {
        // Too small to benefit from compression
        return CompressionMethod::None;
    }

    match category {
        // Already-compressed formats
        FileCategory::Image | FileCategory::Archive => CompressionMethod::None,

        // Text-like content benefits from LZ4
        FileCategory::Text | FileCategory::SourceCode | FileCategory::Config => {
            CompressionMethod::Lz4Simple
        }

        // Binary: check for repeated bytes (RLE candidate)
        FileCategory::Binary | FileCategory::Data => {
            if has_long_runs(data) {
                CompressionMethod::Rle
            } else {
                CompressionMethod::Lz4Simple
            }
        }

        FileCategory::Unknown => {
            // Quick entropy check
            if quick_entropy(data) > 7.0 {
                CompressionMethod::None // High entropy = likely compressed/encrypted
            } else {
                CompressionMethod::Lz4Simple
            }
        }
    }
}

/// Compress data with the given method.
pub fn compress(data: &[u8], method: CompressionMethod) -> Vec<u8> {
    match method {
        CompressionMethod::None => data.to_vec(),
        CompressionMethod::Rle => rle_compress(data),
        CompressionMethod::Lz4Simple => lz4_simple_compress(data),
    }
}

/// Decompress data with the given method.
pub fn decompress(data: &[u8], method: CompressionMethod, original_size: usize) -> Result<Vec<u8>, &'static str> {
    match method {
        CompressionMethod::None => Ok(data.to_vec()),
        CompressionMethod::Rle => rle_decompress(data, original_size),
        CompressionMethod::Lz4Simple => lz4_simple_decompress(data, original_size),
    }
}

// ═══════════════════════════════════════════════════════════════
//  RLE: Run-Length Encoding
// ═══════════════════════════════════════════════════════════════

/// RLE format: [0xFF, byte, count_high, count_low] for runs of 4+ bytes,
/// or literal bytes for non-runs. 0xFF is escaped as [0xFF, 0xFF, 0x00, 0x01].
pub fn rle_compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;

    while i < data.len() {
        let byte = data[i];
        let mut run_len = 1usize;

        // Count consecutive identical bytes
        while i + run_len < data.len() && data[i + run_len] == byte && run_len < 65535 {
            run_len += 1;
        }

        if run_len >= 4 {
            // Encode as RLE sequence
            out.push(0xFF);
            out.push(byte);
            out.push((run_len >> 8) as u8);
            out.push((run_len & 0xFF) as u8);
            i += run_len;
        } else {
            // Literal byte
            if byte == 0xFF {
                // Escape 0xFF
                out.push(0xFF);
                out.push(0xFF);
                out.push(0x00);
                out.push(0x01);
            } else {
                out.push(byte);
            }
            i += 1;
        }
    }

    out
}

/// RLE decompression.
pub fn rle_decompress(data: &[u8], max_size: usize) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::with_capacity(max_size.min(65536));
    let mut i = 0;

    while i < data.len() {
        if data[i] == 0xFF {
            if i + 3 >= data.len() {
                return Err("Truncated RLE sequence");
            }
            let byte = data[i + 1];
            let count = ((data[i + 2] as usize) << 8) | (data[i + 3] as usize);

            if out.len() + count > max_size {
                return Err("RLE decompression overflow");
            }

            for _ in 0..count {
                out.push(byte);
            }
            i += 4;
        } else {
            out.push(data[i]);
            i += 1;
        }

        if out.len() > max_size {
            return Err("RLE decompression overflow");
        }
    }

    Ok(out)
}

// ═══════════════════════════════════════════════════════════════
//  LZ4-simple: Simplified LZ4-like compressor
// ═══════════════════════════════════════════════════════════════

/// Hash table size for match finding (must be power of 2).
const HASH_TABLE_SIZE: usize = 4096;
/// Minimum match length.
const MIN_MATCH: usize = 4;
/// Maximum match offset (window size).
const MAX_OFFSET: usize = 65535;

/// LZ4-simple format per sequence:
/// [token_byte] [literal_bytes...] [offset_low, offset_high] (if match)
///
/// Token byte:
///   High nibble = literal length (0-14; 15 = extended length follows)
///   Low nibble  = match length - MIN_MATCH (0-14; 15 = extended length follows)
///
/// If literal length >= 15: additional bytes follow (each adds value, 255 means continue)
/// If match length >= 15+MIN_MATCH: additional bytes follow similarly
pub fn lz4_simple_compress(input: &[u8]) -> Vec<u8> {
    if input.len() < MIN_MATCH {
        // Too small — just output as literals
        return encode_literals_only(input);
    }

    let mut out = Vec::with_capacity(input.len());
    let mut hash_table = [0u32; HASH_TABLE_SIZE];
    let mut anchor = 0; // Start of unmatched literals
    let mut pos = 0;

    while pos + MIN_MATCH <= input.len() {
        // Hash current 4-byte sequence
        let h = hash4(&input[pos..]) % HASH_TABLE_SIZE;
        let candidate = hash_table[h] as usize;
        hash_table[h] = pos as u32;

        // Check for match
        let offset = pos - candidate;
        if candidate < pos && offset > 0 && offset <= MAX_OFFSET
            && pos + MIN_MATCH <= input.len()
            && candidate + MIN_MATCH <= input.len()
            && input[candidate..candidate + MIN_MATCH] == input[pos..pos + MIN_MATCH]
        {
            // Found a match! Extend it
            let mut match_len = MIN_MATCH;
            while pos + match_len < input.len()
                && candidate + match_len < input.len()
                && input[candidate + match_len] == input[pos + match_len]
                && match_len < 65535
            {
                match_len += 1;
            }

            // Emit literals before the match
            let lit_len = pos - anchor;
            emit_sequence(&mut out, &input[anchor..pos], lit_len, match_len, offset);

            pos += match_len;
            anchor = pos;
        } else {
            pos += 1;
        }
    }

    // Emit remaining literals
    if anchor < input.len() {
        let remaining = &input[anchor..];
        emit_sequence(&mut out, remaining, remaining.len(), 0, 0);
    }

    // Only use compressed version if it's actually smaller
    if out.len() >= input.len() {
        encode_literals_only(input)
    } else {
        out
    }
}

/// LZ4-simple decompression.
pub fn lz4_simple_decompress(data: &[u8], max_size: usize) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::with_capacity(max_size.min(65536));
    let mut pos = 0;

    while pos < data.len() {
        if out.len() >= max_size {
            break;
        }

        let token = data[pos];
        pos += 1;

        // Decode literal length
        let mut lit_len = ((token >> 4) & 0x0F) as usize;
        if lit_len == 15 {
            loop {
                if pos >= data.len() { return Err("Truncated LZ4 literal length"); }
                let extra = data[pos] as usize;
                pos += 1;
                lit_len += extra;
                if extra < 255 { break; }
            }
        }

        // Copy literals
        if pos + lit_len > data.len() {
            return Err("Truncated LZ4 literals");
        }
        out.extend_from_slice(&data[pos..pos + lit_len]);
        pos += lit_len;

        // Check if we're at the end (last sequence has no match)
        if pos >= data.len() {
            break;
        }

        // Decode match offset
        if pos + 2 > data.len() {
            return Err("Truncated LZ4 offset");
        }
        let offset = (data[pos] as usize) | ((data[pos + 1] as usize) << 8);
        pos += 2;

        if offset == 0 {
            return Err("Invalid LZ4 zero offset");
        }

        // Decode match length
        let mut match_len = ((token & 0x0F) as usize) + MIN_MATCH;
        if (token & 0x0F) == 15 {
            loop {
                if pos >= data.len() { return Err("Truncated LZ4 match length"); }
                let extra = data[pos] as usize;
                pos += 1;
                match_len += extra;
                if extra < 255 { break; }
            }
        }

        // Copy match (byte-by-byte to handle overlapping matches)
        if offset > out.len() {
            return Err("LZ4 offset beyond output buffer");
        }
        let match_start = out.len() - offset;
        for i in 0..match_len {
            let byte = out[match_start + i];
            out.push(byte);
        }
    }

    Ok(out)
}

/// Encode input as a single literal-only sequence (no compression).
fn encode_literals_only(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + 8);
    emit_sequence(&mut out, input, input.len(), 0, 0);
    out
}

/// Emit a LZ4 sequence (literals + optional match).
fn emit_sequence(out: &mut Vec<u8>, literals: &[u8], lit_len: usize, match_len: usize, offset: usize) {
    // Build token byte
    let lit_nibble = if lit_len >= 15 { 15 } else { lit_len as u8 };
    let match_nibble = if match_len == 0 {
        0
    } else {
        let ml = match_len - MIN_MATCH;
        if ml >= 15 { 15 } else { ml as u8 }
    };
    let token = (lit_nibble << 4) | match_nibble;
    out.push(token);

    // Extended literal length
    if lit_len >= 15 {
        let mut remaining = lit_len - 15;
        loop {
            let extra = remaining.min(255) as u8;
            out.push(extra);
            remaining -= extra as usize;
            if extra < 255 { break; }
        }
    }

    // Literal bytes
    out.extend_from_slice(literals);

    // Match offset + extended match length (only if there's a match)
    if match_len > 0 {
        out.push((offset & 0xFF) as u8);
        out.push(((offset >> 8) & 0xFF) as u8);

        if match_len - MIN_MATCH >= 15 {
            let mut remaining = match_len - MIN_MATCH - 15;
            loop {
                let extra = remaining.min(255) as u8;
                out.push(extra);
                remaining -= extra as usize;
                if extra < 255 { break; }
            }
        }
    }
}

/// Hash 4 bytes for the hash table.
fn hash4(data: &[u8]) -> usize {
    let v = (data[0] as u32)
        | ((data[1] as u32) << 8)
        | ((data[2] as u32) << 16)
        | ((data[3] as u32) << 24);
    // Fibonacci hashing
    ((v.wrapping_mul(2654435761)) >> 20) as usize
}

/// Check if data has long runs of repeated bytes (RLE candidate).
fn has_long_runs(data: &[u8]) -> bool {
    let sample = &data[..data.len().min(256)];
    let mut max_run = 0u32;
    let mut current_run = 1u32;
    for i in 1..sample.len() {
        if sample[i] == sample[i - 1] {
            current_run += 1;
            max_run = max_run.max(current_run);
        } else {
            current_run = 1;
        }
    }
    max_run >= 8
}

/// Quick entropy estimate (0.0 = uniform, 8.0 = random).
fn quick_entropy(data: &[u8]) -> f32 {
    let sample = &data[..data.len().min(256)];
    let n = sample.len() as f32;
    let mut freq = [0u32; 256];
    for &b in sample {
        freq[b as usize] += 1;
    }

    let mut entropy: f32 = 0.0;
    for &f in &freq {
        if f > 0 {
            let p = f as f32 / n;
            // log2(p) ≈ log2_approx
            entropy -= p * log2_approx(p);
        }
    }
    entropy
}

/// Approximate log2 for entropy calculation.
fn log2_approx(x: f32) -> f32 {
    if x <= 0.0 { return 0.0; }
    let bits = x.to_bits();
    let exp = ((bits >> 23) & 0xFF) as f32 - 127.0;
    let mantissa = f32::from_bits((bits & 0x007FFFFF) | 0x3F800000) - 1.0;
    exp + mantissa * (1.0 - mantissa * 0.333)
}

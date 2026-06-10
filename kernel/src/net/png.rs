//! Minimal PNG decoder for the browser.
//!
//! Supports:
//!   • Truecolor (RGB)            — colour type 2, 8 bit/channel
//!   • Truecolor + alpha (RGBA)   — colour type 6, 8 bit/channel
//!   • Greyscale                  — colour type 0, 8 bit/channel
//!   • Greyscale + alpha          — colour type 4, 8 bit/channel
//!   • Indexed (PLTE)             — colour type 3, 1/2/4/8 bit
//!   • zlib + deflate (uncompressed, fixed Huffman, dynamic Huffman)
//!   • PNG filters 0..4 per scan line
//!
//! Out of scope:
//!   • 16-bit samples (downconverted to 8-bit if encountered)
//!   • Interlacing (Adam7) — interlaced files return Err
//!   • Animated PNG / tRNS for palette indices
//!
//! This is enough for typical "favicon", logo, and inline content imagery.

#![allow(dead_code)]

use alloc::vec;
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
//  Public surface
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum PngError {
    BadMagic,
    BadCrc,
    Truncated,
    Unsupported(&'static str),
    Zlib(&'static str),
    BadFilter(u8),
    BadChunk,
}

/// Decoded RGBA image.  `data` is row-major, 4 bytes/pixel, top-down.
pub struct Image {
    pub width:  u32,
    pub height: u32,
    pub data:   Vec<u8>, // RGBA8888, len = width * height * 4
}

const PNG_MAGIC: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

pub fn decode(bytes: &[u8]) -> Result<Image, PngError> {
    if bytes.len() < 8 || bytes[..8] != PNG_MAGIC {
        return Err(PngError::BadMagic);
    }

    let mut chunks = Chunks::new(&bytes[8..]);

    // 1. Read IHDR.
    let hdr = chunks.next()?.ok_or(PngError::Truncated)?;
    if hdr.kind != *b"IHDR" || hdr.data.len() != 13 {
        return Err(PngError::Unsupported("missing IHDR"));
    }
    let width  = u32::from_be_bytes([hdr.data[0], hdr.data[1], hdr.data[2], hdr.data[3]]);
    let height = u32::from_be_bytes([hdr.data[4], hdr.data[5], hdr.data[6], hdr.data[7]]);
    let bit_depth   = hdr.data[8];
    let color_type  = hdr.data[9];
    let compression = hdr.data[10];
    let filter      = hdr.data[11];
    let interlace   = hdr.data[12];

    if compression != 0 { return Err(PngError::Unsupported("compression method")); }
    if filter      != 0 { return Err(PngError::Unsupported("filter method")); }
    if interlace   != 0 { return Err(PngError::Unsupported("interlaced PNG")); }

    let (channels, supported_depth) = match color_type {
        0 => (1, &[1u8, 2, 4, 8][..]),
        2 => (3, &[8u8][..]),
        3 => (1, &[1u8, 2, 4, 8][..]),
        4 => (2, &[8u8][..]),
        6 => (4, &[8u8][..]),
        _ => return Err(PngError::Unsupported("colour type")),
    };
    if !supported_depth.contains(&bit_depth) {
        return Err(PngError::Unsupported("bit depth"));
    }

    // 2. Collect IDAT data, optional PLTE.
    let mut plte: Option<Vec<u8>> = None;
    let mut zdata: Vec<u8> = Vec::new();
    loop {
        let chunk = match chunks.next()? {
            Some(c) => c,
            None    => break,
        };
        match &chunk.kind {
            b"PLTE" => plte = Some(chunk.data.to_vec()),
            b"IDAT" => zdata.extend_from_slice(chunk.data),
            b"IEND" => break,
            _ => {} // ignore ancillary chunks (tRNS, tEXt, etc.)
        }
    }

    if zdata.is_empty() {
        return Err(PngError::Truncated);
    }

    // 3. zlib-inflate.
    let raw = inflate_zlib(&zdata)?;

    // 4. Un-filter scan lines into samples.
    let bpp = match color_type {
        0 => (bit_depth as usize + 7) / 8,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => unreachable!(),
    };
    let bits_per_row = width as usize * channels as usize * bit_depth as usize;
    let row_bytes    = (bits_per_row + 7) / 8;

    let unfiltered = unfilter_scanlines(&raw, height as usize, row_bytes, bpp.max(1))?;

    // 5. Expand to RGBA8.
    let rgba = expand_to_rgba(&unfiltered, width, height, bit_depth, color_type, plte.as_deref())?;

    Ok(Image { width, height, data: rgba })
}

// ─────────────────────────────────────────────────────────────────────────────
//  Chunk parser
// ─────────────────────────────────────────────────────────────────────────────

struct Chunks<'a> {
    src: &'a [u8],
    pos: usize,
}

struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
}

impl<'a> Chunks<'a> {
    fn new(src: &'a [u8]) -> Self { Self { src, pos: 0 } }

    fn next(&mut self) -> Result<Option<Chunk<'a>>, PngError> {
        if self.pos >= self.src.len() { return Ok(None); }
        if self.pos + 8 > self.src.len() { return Err(PngError::Truncated); }
        let len = u32::from_be_bytes([
            self.src[self.pos], self.src[self.pos + 1],
            self.src[self.pos + 2], self.src[self.pos + 3],
        ]) as usize;
        let kind = [
            self.src[self.pos + 4], self.src[self.pos + 5],
            self.src[self.pos + 6], self.src[self.pos + 7],
        ];
        let data_start = self.pos + 8;
        let data_end = data_start + len;
        if data_end + 4 > self.src.len() { return Err(PngError::Truncated); }
        // CRC is not verified (we trust the network layer).
        let data = &self.src[data_start..data_end];
        self.pos = data_end + 4;
        Ok(Some(Chunk { kind, data }))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Scan-line un-filter (PNG filter types 0..4)
// ─────────────────────────────────────────────────────────────────────────────

fn unfilter_scanlines(
    raw: &[u8], height: usize, row_bytes: usize, bpp: usize,
) -> Result<Vec<u8>, PngError> {
    let stride = row_bytes + 1;
    if raw.len() < stride * height { return Err(PngError::Truncated); }

    let mut out = vec![0u8; row_bytes * height];
    let mut prev_row: Vec<u8> = vec![0u8; row_bytes];

    for y in 0..height {
        let src = &raw[y * stride .. (y + 1) * stride];
        let filt = src[0];
        let scan = &src[1..];
        let mut cur = vec![0u8; row_bytes];

        match filt {
            0 => { cur.copy_from_slice(scan); }
            1 => {
                // Sub: x + a (left)
                for i in 0..row_bytes {
                    let a = if i >= bpp { cur[i - bpp] } else { 0 };
                    cur[i] = scan[i].wrapping_add(a);
                }
            }
            2 => {
                // Up: x + b (above)
                for i in 0..row_bytes {
                    cur[i] = scan[i].wrapping_add(prev_row[i]);
                }
            }
            3 => {
                // Average: x + floor((a + b) / 2)
                for i in 0..row_bytes {
                    let a = if i >= bpp { cur[i - bpp] as u16 } else { 0 };
                    let b = prev_row[i] as u16;
                    cur[i] = scan[i].wrapping_add(((a + b) / 2) as u8);
                }
            }
            4 => {
                // Paeth
                for i in 0..row_bytes {
                    let a = if i >= bpp { cur[i - bpp] as i32 } else { 0 };
                    let b = prev_row[i] as i32;
                    let c = if i >= bpp { prev_row[i - bpp] as i32 } else { 0 };
                    let p = a + b - c;
                    let pa = (p - a).unsigned_abs();
                    let pb = (p - b).unsigned_abs();
                    let pc = (p - c).unsigned_abs();
                    let pred = if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c };
                    cur[i] = scan[i].wrapping_add(pred as u8);
                }
            }
            other => return Err(PngError::BadFilter(other)),
        }

        out[y * row_bytes .. (y + 1) * row_bytes].copy_from_slice(&cur);
        prev_row = cur;
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Sample-format expansion → RGBA8
// ─────────────────────────────────────────────────────────────────────────────

fn expand_to_rgba(
    samples: &[u8], width: u32, height: u32,
    bit_depth: u8, color_type: u8, palette: Option<&[u8]>,
) -> Result<Vec<u8>, PngError> {
    let n = width as usize * height as usize;
    let mut out = vec![0u8; n * 4];

    match color_type {
        0 => {
            // Greyscale.
            let step = bit_depth as usize;
            for i in 0..n {
                let v = sample_at(samples, i, step) as u8;
                let p = v << (8 - step);
                let p = if step < 8 { p | (p >> step) } else { v };
                out[i * 4 + 0] = p;
                out[i * 4 + 1] = p;
                out[i * 4 + 2] = p;
                out[i * 4 + 3] = 255;
            }
        }
        2 => {
            // RGB.
            for i in 0..n {
                out[i * 4 + 0] = samples[i * 3 + 0];
                out[i * 4 + 1] = samples[i * 3 + 1];
                out[i * 4 + 2] = samples[i * 3 + 2];
                out[i * 4 + 3] = 255;
            }
        }
        3 => {
            // Indexed.
            let plt = palette.ok_or(PngError::Unsupported("missing PLTE"))?;
            let step = bit_depth as usize;
            for i in 0..n {
                let idx = sample_at(samples, i, step) as usize;
                let base = idx * 3;
                if base + 2 >= plt.len() {
                    out[i * 4 + 0..i * 4 + 4].copy_from_slice(&[0, 0, 0, 255]);
                } else {
                    out[i * 4 + 0] = plt[base];
                    out[i * 4 + 1] = plt[base + 1];
                    out[i * 4 + 2] = plt[base + 2];
                    out[i * 4 + 3] = 255;
                }
            }
        }
        4 => {
            // Greyscale + alpha.
            for i in 0..n {
                let v = samples[i * 2];
                let a = samples[i * 2 + 1];
                out[i * 4 + 0] = v;
                out[i * 4 + 1] = v;
                out[i * 4 + 2] = v;
                out[i * 4 + 3] = a;
            }
        }
        6 => {
            // RGBA.
            out.copy_from_slice(&samples[..n * 4]);
        }
        _ => return Err(PngError::Unsupported("colour type")),
    }
    Ok(out)
}

/// Read a packed sub-byte sample (1/2/4 bit) or full byte from `samples`.
fn sample_at(samples: &[u8], i: usize, step_bits: usize) -> u32 {
    if step_bits >= 8 {
        return samples[i * (step_bits / 8)] as u32;
    }
    let samples_per_byte = 8 / step_bits;
    let byte_idx = i / samples_per_byte;
    let sub_idx  = i % samples_per_byte;
    let shift    = 8 - step_bits * (sub_idx + 1);
    let mask     = (1u32 << step_bits) - 1;
    ((samples[byte_idx] >> shift) as u32) & mask
}

// ─────────────────────────────────────────────────────────────────────────────
//  zlib / deflate inflate
//  Implements RFC 1950 wrapper + RFC 1951 block formats (stored, fixed
//  Huffman, dynamic Huffman).
// ─────────────────────────────────────────────────────────────────────────────

fn inflate_zlib(input: &[u8]) -> Result<Vec<u8>, PngError> {
    if input.len() < 6 { return Err(PngError::Zlib("zlib header truncated")); }
    let cmf = input[0];
    let flg = input[1];
    if (cmf & 0x0F) != 8 { return Err(PngError::Zlib("not deflate")); }
    if ((cmf as u16) * 256 + flg as u16) % 31 != 0 {
        return Err(PngError::Zlib("bad FCHECK"));
    }
    if (flg & 0x20) != 0 { return Err(PngError::Zlib("FDICT not supported")); }

    let mut br = BitReader::new(&input[2..input.len() - 4]); // strip 4-byte ADLER32
    let mut out = Vec::with_capacity(input.len() * 4);

    loop {
        let bfinal = br.read_bits(1)?;
        let btype  = br.read_bits(2)?;
        match btype {
            0 => inflate_stored(&mut br, &mut out)?,
            1 => inflate_huff(&mut br, &mut out, &fixed_litlen(), &fixed_dist())?,
            2 => {
                let (lit, dist) = read_dynamic_codes(&mut br)?;
                inflate_huff(&mut br, &mut out, &lit, &dist)?;
            }
            _ => return Err(PngError::Zlib("reserved block type")),
        }
        if bfinal == 1 { break; }
    }
    Ok(out)
}

fn inflate_stored(br: &mut BitReader, out: &mut Vec<u8>) -> Result<(), PngError> {
    br.align_to_byte();
    let len  = br.read_u16_le()?;
    let nlen = br.read_u16_le()?;
    if len ^ 0xFFFF != nlen { return Err(PngError::Zlib("stored LEN/NLEN mismatch")); }
    for _ in 0..len {
        out.push(br.read_byte()?);
    }
    Ok(())
}

fn inflate_huff(br: &mut BitReader, out: &mut Vec<u8>, litlen: &HuffTable, dist: &HuffTable) -> Result<(), PngError> {
    loop {
        let sym = decode_symbol(br, litlen)?;
        if sym < 256 {
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(()); // end of block
        } else {
            // 257..285 → length code.
            let length = read_length(br, sym)?;
            let dsym   = decode_symbol(br, dist)?;
            let backd  = read_distance(br, dsym)?;
            if backd == 0 || backd > out.len() {
                return Err(PngError::Zlib("bad back reference"));
            }
            let start = out.len() - backd;
            for i in 0..length {
                let b = out[start + i];
                out.push(b);
            }
        }
    }
}

// ── Length & distance tables (RFC 1951 §3.2.5) ───────────────────────────────

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31,
    35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2,
    3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193,
    257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6,
    7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];

fn read_length(br: &mut BitReader, sym: u32) -> Result<usize, PngError> {
    let i = (sym - 257) as usize;
    if i >= 29 { return Err(PngError::Zlib("bad length symbol")); }
    let extra = LEN_EXTRA[i] as u32;
    let base  = LEN_BASE[i] as u32;
    Ok((base + if extra > 0 { br.read_bits(extra)? } else { 0 }) as usize)
}

fn read_distance(br: &mut BitReader, sym: u32) -> Result<usize, PngError> {
    let i = sym as usize;
    if i >= 30 { return Err(PngError::Zlib("bad distance symbol")); }
    let extra = DIST_EXTRA[i] as u32;
    let base  = DIST_BASE[i] as u32;
    Ok((base + if extra > 0 { br.read_bits(extra)? } else { 0 }) as usize)
}

// ── Fixed Huffman tables (RFC 1951 §3.2.6) ───────────────────────────────────

fn fixed_litlen() -> HuffTable {
    let mut lengths = [0u8; 288];
    for i in 0..=143  { lengths[i] = 8; }
    for i in 144..=255 { lengths[i] = 9; }
    for i in 256..=279 { lengths[i] = 7; }
    for i in 280..=287 { lengths[i] = 8; }
    build_huff(&lengths)
}

fn fixed_dist() -> HuffTable {
    build_huff(&[5u8; 30])
}

// ── Dynamic Huffman tables (RFC 1951 §3.2.7) ─────────────────────────────────

const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn read_dynamic_codes(br: &mut BitReader) -> Result<(HuffTable, HuffTable), PngError> {
    let hlit  = br.read_bits(5)? + 257;
    let hdist = br.read_bits(5)? + 1;
    let hclen = br.read_bits(4)? + 4;

    let mut cl_lengths = [0u8; 19];
    for i in 0..hclen as usize {
        cl_lengths[CL_ORDER[i]] = br.read_bits(3)? as u8;
    }
    let cl_table = build_huff(&cl_lengths);

    let total = (hlit + hdist) as usize;
    let mut lengths = vec![0u8; total];
    let mut i = 0;
    while i < total {
        let sym = decode_symbol(br, &cl_table)?;
        match sym {
            0..=15 => { lengths[i] = sym as u8; i += 1; }
            16 => {
                if i == 0 { return Err(PngError::Zlib("repeat with no prior")); }
                let n = (br.read_bits(2)? + 3) as usize;
                let prev = lengths[i - 1];
                for _ in 0..n {
                    if i >= total { return Err(PngError::Zlib("CL overflow")); }
                    lengths[i] = prev; i += 1;
                }
            }
            17 => {
                let n = (br.read_bits(3)? + 3) as usize;
                for _ in 0..n {
                    if i >= total { return Err(PngError::Zlib("CL overflow")); }
                    lengths[i] = 0; i += 1;
                }
            }
            18 => {
                let n = (br.read_bits(7)? + 11) as usize;
                for _ in 0..n {
                    if i >= total { return Err(PngError::Zlib("CL overflow")); }
                    lengths[i] = 0; i += 1;
                }
            }
            _ => return Err(PngError::Zlib("bad CL symbol")),
        }
    }

    let lit  = build_huff(&lengths[..hlit as usize]);
    let dist = build_huff(&lengths[hlit as usize..]);
    Ok((lit, dist))
}

// ── Huffman table & decoder ──────────────────────────────────────────────────

/// Maps a numeric code → symbol, indexed by (code-length, code-value).
/// We use a simple two-level: per-length sorted symbol list + first-code table.
struct HuffTable {
    /// Maximum code length present (or 0 if empty).
    max_len: u32,
    /// For each length 1..=15: the first code value of that length.
    first_code: [u32; 16],
    /// For each length 1..=15: the starting offset into `syms` for that length.
    sym_offset: [usize; 16],
    /// Flat list of symbols, grouped by code length in ascending order.
    syms: Vec<u32>,
}

fn build_huff(lengths: &[u8]) -> HuffTable {
    let mut bl_count = [0u32; 16];
    for &l in lengths {
        if l > 0 && (l as usize) < bl_count.len() {
            bl_count[l as usize] += 1;
        }
    }
    let mut next_code = [0u32; 16];
    let mut code = 0u32;
    for bits in 1..16 {
        code = (code + bl_count[bits - 1]) << 1;
        next_code[bits] = code;
    }

    // Sort symbols by (length, value).
    let mut entries: Vec<(u8, u32)> = lengths.iter().enumerate()
        .filter(|(_, l)| **l > 0)
        .map(|(i, l)| (*l, i as u32))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut first_code = [0u32; 16];
    let mut sym_offset = [0usize; 16];
    let mut syms: Vec<u32> = Vec::with_capacity(entries.len());

    let mut prev_len = 0u8;
    let mut idx = 0usize;
    for (l, s) in &entries {
        if *l != prev_len {
            for fill in (prev_len as usize + 1) ..= (*l as usize) {
                sym_offset[fill] = idx;
                first_code[fill] = next_code[fill];
            }
            prev_len = *l;
        }
        syms.push(*s);
        idx += 1;
    }

    let max_len = entries.last().map(|(l, _)| *l as u32).unwrap_or(0);
    HuffTable { max_len, first_code, sym_offset, syms }
}

fn decode_symbol(br: &mut BitReader, table: &HuffTable) -> Result<u32, PngError> {
    if table.max_len == 0 {
        return Err(PngError::Zlib("empty Huffman table"));
    }
    let mut code = 0u32;
    for length in 1..=table.max_len as usize {
        let bit = br.read_bits(1)?;
        code = (code << 1) | bit;
        // Convert to slot index within this length.
        let first = table.first_code[length];
        let count = if length < 15 {
            table.sym_offset[length + 1].saturating_sub(table.sym_offset[length])
        } else {
            table.syms.len().saturating_sub(table.sym_offset[length])
        } as u32;
        if code >= first && code < first + count {
            let idx = table.sym_offset[length] + (code - first) as usize;
            return Ok(table.syms[idx]);
        }
    }
    Err(PngError::Zlib("symbol not in tree"))
}

// ── LSB-first bit reader ─────────────────────────────────────────────────────

struct BitReader<'a> {
    src: &'a [u8],
    pos: usize,    // byte position
    buf: u32,      // bits read but not yet consumed
    bits: u32,     // count of valid bits in `buf`
}

impl<'a> BitReader<'a> {
    fn new(src: &'a [u8]) -> Self { Self { src, pos: 0, buf: 0, bits: 0 } }

    fn read_bits(&mut self, n: u32) -> Result<u32, PngError> {
        while self.bits < n {
            if self.pos >= self.src.len() { return Err(PngError::Zlib("EOF mid-stream")); }
            self.buf |= (self.src[self.pos] as u32) << self.bits;
            self.pos += 1;
            self.bits += 8;
        }
        let mask = (1u32 << n) - 1;
        let v = self.buf & mask;
        self.buf >>= n;
        self.bits -= n;
        Ok(v)
    }

    fn align_to_byte(&mut self) {
        let extra = self.bits & 7;
        if extra > 0 {
            self.buf >>= extra;
            self.bits -= extra;
        }
    }

    fn read_u16_le(&mut self) -> Result<u32, PngError> {
        let lo = self.read_byte()? as u32;
        let hi = self.read_byte()? as u32;
        Ok(lo | (hi << 8))
    }

    fn read_byte(&mut self) -> Result<u8, PngError> {
        // align_to_byte should have been called first by caller for stored blocks
        Ok(self.read_bits(8)? as u8)
    }
}

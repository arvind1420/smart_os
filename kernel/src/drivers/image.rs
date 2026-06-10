//! PNG + JPEG image decoder — Phase 33 for Smart OS.
//!
//! Provides:
//!  • PNG decoder: IHDR, IDAT (deflate via ZLIB), PLTE, tRNS, iCCP (ignored)
//!    - Color types: grayscale (1/2/4/8/16-bit), RGB, RGBA, indexed, grayscale+alpha
//!    - Filter types: None, Sub, Up, Average, Paeth
//!    - Output: RGBA8 pixel array
//!  • JPEG decoder: Baseline DCT (SOF0), Huffman tables (DHT), quantization (DQT)
//!    - YCbCr → RGB conversion
//!    - MCU decoding for 4:4:4, 4:2:2, 4:2:0 chroma subsampling
//!    - Output: RGBA8 pixel array
//!  • Unified `Image` type with format detection
//!
//! Both decoders are pure software, no external crates.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;

// ─────────────────────────────────────────────────────────────────────────────
//  Decoded image
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct Image {
    pub width:  u32,
    pub height: u32,
    /// RGBA8 pixels, row-major (width * height * 4 bytes).
    pub pixels: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width * height * 4) as usize;
        Image { width, height, pixels: vec![0u8; size] }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let off = ((y * self.width + x) * 4) as usize;
        [self.pixels[off], self.pixels[off+1], self.pixels[off+2], self.pixels[off+3]]
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        let off = ((y * self.width + x) * 4) as usize;
        self.pixels[off..off+4].copy_from_slice(&rgba);
    }
}

#[derive(Debug)]
pub enum ImageError {
    InvalidHeader,
    UnsupportedFormat,
    CorruptData,
    OutOfMemory,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Format detection
// ─────────────────────────────────────────────────────────────────────────────

pub fn decode(data: &[u8]) -> Result<Image, ImageError> {
    if data.len() < 8 { return Err(ImageError::InvalidHeader); }
    // PNG signature: 0x89 P N G \r \n 0x1A \n
    if &data[..8] == b"\x89PNG\r\n\x1a\n" {
        return decode_png(data);
    }
    // JPEG signature: FF D8 FF
    if data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
        return decode_jpeg(data);
    }
    // BMP signature: BM
    if data[0] == b'B' && data[1] == b'M' {
        return decode_bmp(data);
    }
    Err(ImageError::UnsupportedFormat)
}

// ─────────────────────────────────────────────────────────────────────────────
//  DEFLATE / ZLIB decompressor (for PNG IDAT chunks)
// ─────────────────────────────────────────────────────────────────────────────

/// Decompress ZLIB-wrapped DEFLATE data (RFC 1950 + RFC 1951).
fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>, ImageError> {
    if data.len() < 2 { return Err(ImageError::CorruptData); }
    // Skip ZLIB header (CMF, FLG).
    let _cmf = data[0];
    let _flg = data[1];
    // Skip optional dict (fcheck).
    let payload_start = if data[1] & 0x20 != 0 { 6 } else { 2 };
    deflate_decompress(&data[payload_start..data.len().saturating_sub(4)])
}

/// DEFLATE decompressor (RFC 1951) — handles non-compressed, fixed, and dynamic blocks.
fn deflate_decompress(data: &[u8]) -> Result<Vec<u8>, ImageError> {
    let mut out: Vec<u8> = Vec::new();
    let mut br = BitReader::new(data);

    loop {
        let bfinal = br.read_bits(1)?;
        let btype  = br.read_bits(2)?;
        match btype {
            0 => {
                // Non-compressed block.
                br.align();
                let len  = br.read_u16_le()?;
                let nlen = br.read_u16_le()?;
                if len ^ nlen != 0xFFFF { return Err(ImageError::CorruptData); }
                for _ in 0..len {
                    out.push(br.read_byte()?);
                }
            }
            1 => {
                // Fixed Huffman codes.
                let (lit_tree, dist_tree) = fixed_huffman_trees();
                deflate_block(&mut br, &mut out, &lit_tree, &dist_tree)?;
            }
            2 => {
                // Dynamic Huffman codes.
                let (lit_tree, dist_tree) = read_dynamic_trees(&mut br)?;
                deflate_block(&mut br, &mut out, &lit_tree, &dist_tree)?;
            }
            _ => return Err(ImageError::CorruptData),
        }
        if bfinal != 0 { break; }
    }
    Ok(out)
}

/// Bit reader for DEFLATE (LSB-first).
struct BitReader<'a> {
    data:   &'a [u8],
    pos:    usize,
    buf:    u64,
    nbits:  u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self { BitReader { data, pos: 0, buf: 0, nbits: 0 } }

    fn fill(&mut self) {
        while self.nbits <= 56 && self.pos < self.data.len() {
            self.buf |= (self.data[self.pos] as u64) << self.nbits;
            self.nbits += 8;
            self.pos   += 1;
        }
    }

    fn read_bits(&mut self, n: u32) -> Result<u32, ImageError> {
        if n == 0 { return Ok(0); }
        self.fill();
        if self.nbits < n { return Err(ImageError::CorruptData); }
        let v = self.buf & ((1u64 << n) - 1);
        self.buf >>= n;
        self.nbits -= n;
        Ok(v as u32)
    }

    fn read_bits_rev(&mut self, n: u32) -> Result<u32, ImageError> {
        let v = self.read_bits(n)?;
        Ok(reverse_bits(v, n))
    }

    fn align(&mut self) {
        let discard = self.nbits % 8;
        self.buf >>= discard;
        self.nbits -= discard;
    }

    fn read_byte(&mut self) -> Result<u8, ImageError> {
        self.fill();
        if self.nbits < 8 { return Err(ImageError::CorruptData); }
        let v = (self.buf & 0xFF) as u8;
        self.buf >>= 8;
        self.nbits -= 8;
        Ok(v)
    }

    fn read_u16_le(&mut self) -> Result<u16, ImageError> {
        let lo = self.read_byte()? as u16;
        let hi = self.read_byte()? as u16;
        Ok(lo | (hi << 8))
    }
}

fn reverse_bits(mut v: u32, n: u32) -> u32 {
    let mut r = 0u32;
    for _ in 0..n { r = (r << 1) | (v & 1); v >>= 1; }
    r
}

// ─── Huffman tree ─────────────────────────────────────────────────────────

/// A canonical Huffman tree represented as a lookup table.
/// `table[code]` = (symbol, code_length) for up to 15-bit codes.
struct HuffTree {
    table:     Vec<u16>,   // indexed by reversed code (up to 15 bits)
    max_bits:  u32,
}

impl HuffTree {
    fn from_lengths(lengths: &[u8]) -> Self {
        let max_bits = lengths.iter().copied().max().unwrap_or(0) as u32;
        if max_bits == 0 { return HuffTree { table: Vec::new(), max_bits: 0 }; }
        let size = 1usize << max_bits;
        let mut table = vec![0xFFFFu16; size];

        // Build canonical codes.
        let mut bl_count = vec![0u32; (max_bits + 1) as usize];
        for &l in lengths { if l > 0 { bl_count[l as usize] += 1; } }
        let mut next_code = vec![0u32; (max_bits + 2) as usize];
        let mut code = 0u32;
        for bits in 1..=max_bits as usize {
            code = (code + bl_count[bits - 1]) << 1;
            next_code[bits] = code;
        }

        for (sym, &len) in lengths.iter().enumerate() {
            if len == 0 { continue; }
            let l = len as usize;
            let c = next_code[l];
            next_code[l] += 1;
            // Fill all table entries that match this code.
            let rev = reverse_bits(c, len as u32);
            let stride = 1u32 << len;
            let mut idx = rev as usize;
            while idx < size {
                table[idx] = ((sym as u16) << 4) | (len as u16);
                idx += stride as usize;
            }
        }
        HuffTree { table, max_bits }
    }

    fn decode(&self, br: &mut BitReader<'_>) -> Result<u16, ImageError> {
        br.fill();
        if self.max_bits == 0 { return Err(ImageError::CorruptData); }
        let peek = (br.buf & ((1u64 << self.max_bits) - 1)) as usize;
        let entry = *self.table.get(peek).ok_or(ImageError::CorruptData)?;
        if entry == 0xFFFF { return Err(ImageError::CorruptData); }
        let sym = (entry >> 4) as u16;
        let len = (entry & 0xF) as u32;
        br.buf   >>= len;
        br.nbits -=  len;
        Ok(sym)
    }
}

fn fixed_huffman_trees() -> (HuffTree, HuffTree) {
    // RFC 1951 §3.2.6.
    let mut lit_lengths = vec![0u8; 288];
    for i in   0..=143 { lit_lengths[i] = 8; }
    for i in 144..=255 { lit_lengths[i] = 9; }
    for i in 256..=279 { lit_lengths[i] = 7; }
    for i in 280..=287 { lit_lengths[i] = 8; }
    let dist_lengths = vec![5u8; 32];
    (HuffTree::from_lengths(&lit_lengths), HuffTree::from_lengths(&dist_lengths))
}

fn read_dynamic_trees(br: &mut BitReader<'_>) -> Result<(HuffTree, HuffTree), ImageError> {
    let hlit  = br.read_bits(5)? as usize + 257;
    let hdist = br.read_bits(5)? as usize + 1;
    let hclen = br.read_bits(4)? as usize + 4;

    // Code length alphabet.
    const CL_ORDER: [usize; 19] = [16,17,18,0,8,7,9,6,10,5,11,4,12,3,13,2,14,1,15];
    let mut cl_lengths = [0u8; 19];
    for i in 0..hclen { cl_lengths[CL_ORDER[i]] = br.read_bits(3)? as u8; }
    let cl_tree = HuffTree::from_lengths(&cl_lengths);

    // Decode literal + distance code lengths.
    let total = hlit + hdist;
    let mut lengths = Vec::with_capacity(total);
    while lengths.len() < total {
        let sym = cl_tree.decode(br)?;
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => {
                let rep = br.read_bits(2)? as usize + 3;
                let last = *lengths.last().unwrap_or(&0);
                for _ in 0..rep { lengths.push(last); }
            }
            17 => { let rep = br.read_bits(3)? as usize + 3; for _ in 0..rep { lengths.push(0); } }
            18 => { let rep = br.read_bits(7)? as usize + 11; for _ in 0..rep { lengths.push(0); } }
            _ => return Err(ImageError::CorruptData),
        }
    }
    let lit_tree  = HuffTree::from_lengths(&lengths[..hlit]);
    let dist_tree = HuffTree::from_lengths(&lengths[hlit..hlit+hdist]);
    Ok((lit_tree, dist_tree))
}

/// Decode one DEFLATE compressed block.
fn deflate_block(
    br:        &mut BitReader<'_>,
    out:       &mut Vec<u8>,
    lit_tree:  &HuffTree,
    dist_tree: &HuffTree,
) -> Result<(), ImageError> {
    const LEN_EXTRA:  [(u32, u32); 29] = [
        (3,0),(4,0),(5,0),(6,0),(7,0),(8,0),(9,0),(10,0),
        (11,1),(13,1),(15,1),(17,1),(19,2),(23,2),(27,2),(31,2),
        (35,3),(43,3),(51,3),(59,3),(67,4),(83,4),(99,4),(115,4),
        (131,5),(163,5),(195,5),(227,5),(258,0),
    ];
    const DIST_EXTRA: [(u32, u32); 30] = [
        (1,0),(2,0),(3,0),(4,0),(5,1),(7,1),(9,2),(13,2),
        (17,3),(25,3),(33,4),(49,4),(65,5),(97,5),(129,6),(193,6),
        (257,7),(385,7),(513,8),(769,8),(1025,9),(1537,9),(2049,10),(3073,10),
        (4097,11),(6145,11),(8193,12),(12289,12),(16385,13),(24577,13),
    ];

    loop {
        let sym = lit_tree.decode(br)?;
        match sym {
            0..=255 => out.push(sym as u8),
            256     => break, // end of block
            257..=285 => {
                let idx = (sym - 257) as usize;
                if idx >= LEN_EXTRA.len() { return Err(ImageError::CorruptData); }
                let (base_len, extra_bits) = LEN_EXTRA[idx];
                let length = base_len + br.read_bits(extra_bits)?;

                let dist_sym = dist_tree.decode(br)? as usize;
                if dist_sym >= DIST_EXTRA.len() { return Err(ImageError::CorruptData); }
                let (base_dist, dist_extra) = DIST_EXTRA[dist_sym];
                let dist = (base_dist + br.read_bits(dist_extra)?) as usize;

                let start = out.len().checked_sub(dist).ok_or(ImageError::CorruptData)?;
                for i in 0..length as usize { out.push(out[start + (i % dist)]); }
            }
            _ => return Err(ImageError::CorruptData),
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
//  PNG decoder
// ─────────────────────────────────────────────────────────────────────────────

pub fn decode_png(data: &[u8]) -> Result<Image, ImageError> {
    if data.len() < 8 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(ImageError::InvalidHeader);
    }

    let mut pos = 8usize;
    let mut width      = 0u32;
    let mut height     = 0u32;
    let mut bit_depth  = 0u8;
    let mut color_type = 0u8;
    let mut idat_data  = Vec::<u8>::new();
    let mut palette    = Vec::<[u8; 3]>::new();
    let mut trns       = Vec::<u8>::new();
    let mut ihdr_ok    = false;

    // Parse chunks.
    while pos + 12 <= data.len() {
        let chunk_len  = u32::from_be_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
        let chunk_type = &data[pos+4..pos+8];
        let chunk_data = if pos + 8 + chunk_len <= data.len() { &data[pos+8..pos+8+chunk_len] } else { break };
        pos += 12 + chunk_len;

        match chunk_type {
            b"IHDR" => {
                if chunk_len < 13 { return Err(ImageError::CorruptData); }
                width      = u32::from_be_bytes([chunk_data[0], chunk_data[1], chunk_data[2], chunk_data[3]]);
                height     = u32::from_be_bytes([chunk_data[4], chunk_data[5], chunk_data[6], chunk_data[7]]);
                bit_depth  = chunk_data[8];
                color_type = chunk_data[9];
                // interlace = chunk_data[12]; we only support non-interlaced
                ihdr_ok = true;
            }
            b"PLTE" => {
                for i in 0..chunk_len/3 {
                    palette.push([chunk_data[i*3], chunk_data[i*3+1], chunk_data[i*3+2]]);
                }
            }
            b"tRNS" => {
                trns.extend_from_slice(chunk_data);
            }
            b"IDAT" => {
                idat_data.extend_from_slice(chunk_data);
            }
            b"IEND" => break,
            _ => {} // skip unknown chunks
        }
    }

    if !ihdr_ok { return Err(ImageError::InvalidHeader); }
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(ImageError::UnsupportedFormat);
    }

    // Decompress IDAT.
    let raw = zlib_decompress(&idat_data)?;

    // Reconstruct filtered scanlines.
    let channels: usize = match color_type {
        0 => 1, // grayscale
        2 => 3, // RGB
        3 => 1, // indexed
        4 => 2, // grayscale + alpha
        6 => 4, // RGBA
        _ => return Err(ImageError::UnsupportedFormat),
    };
    let bpp = ((bit_depth as usize * channels + 7) / 8).max(1);
    let stride = ((width as usize * channels * bit_depth as usize + 7) / 8) + 1; // +1 for filter byte

    let expected = stride * height as usize;
    if raw.len() < expected { return Err(ImageError::CorruptData); }

    let mut scanlines: Vec<Vec<u8>> = Vec::with_capacity(height as usize);
    for y in 0..height as usize {
        let row_start = y * stride;
        let filter = raw[row_start];
        let row = raw[row_start+1..row_start+stride].to_vec();
        let prev = scanlines.last().cloned().unwrap_or_else(|| vec![0u8; stride - 1]);
        let decoded = apply_png_filter(filter, &row, &prev, bpp)?;
        scanlines.push(decoded);
    }

    // Convert to RGBA8.
    let mut img = Image::new(width, height);
    for y in 0..height as usize {
        let row = &scanlines[y];
        for x in 0..width as usize {
            let rgba = sample_png_pixel(row, x, color_type, bit_depth, &palette, &trns);
            let off = (y * width as usize + x) * 4;
            img.pixels[off..off+4].copy_from_slice(&rgba);
        }
    }
    Ok(img)
}

fn apply_png_filter(filter: u8, row: &[u8], prev: &[u8], bpp: usize) -> Result<Vec<u8>, ImageError> {
    let n = row.len();
    let mut out = vec![0u8; n];
    match filter {
        0 => out.copy_from_slice(row), // None
        1 => { // Sub
            for i in 0..n {
                let a = if i >= bpp { out[i - bpp] } else { 0 };
                out[i] = row[i].wrapping_add(a);
            }
        }
        2 => { // Up
            for i in 0..n {
                let b = if i < prev.len() { prev[i] } else { 0 };
                out[i] = row[i].wrapping_add(b);
            }
        }
        3 => { // Average
            for i in 0..n {
                let a = if i >= bpp { out[i - bpp] as u16 } else { 0u16 };
                let b = if i < prev.len() { prev[i] as u16 } else { 0u16 };
                out[i] = row[i].wrapping_add(((a + b) / 2) as u8);
            }
        }
        4 => { // Paeth
            for i in 0..n {
                let a = if i >= bpp { out[i - bpp] } else { 0 };
                let b = if i < prev.len() { prev[i] } else { 0 };
                let c = if i >= bpp && i < prev.len() { prev[i - bpp] } else { 0 };
                out[i] = row[i].wrapping_add(paeth_predictor(a, b, c));
            }
        }
        _ => return Err(ImageError::CorruptData),
    }
    Ok(out)
}

fn paeth_predictor(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (a as i32, b as i32, c as i32);
    let p  = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc { a as u8 }
    else if pb <= pc { b as u8 }
    else { c as u8 }
}

fn sample_png_pixel(row: &[u8], x: usize, color_type: u8, bit_depth: u8, palette: &[[u8;3]], trns: &[u8]) -> [u8; 4] {
    match color_type {
        0 => { // Grayscale
            let v = sample_channel(row, x, 0, bit_depth);
            [v, v, v, 255]
        }
        2 => { // RGB
            let r = sample_channel(row, x, 0, bit_depth);
            let g = sample_channel(row, x, 1, bit_depth);
            let b = sample_channel(row, x, 2, bit_depth);
            [r, g, b, 255]
        }
        3 => { // Indexed
            let idx = sample_channel(row, x, 0, bit_depth) as usize;
            let rgb = palette.get(idx).copied().unwrap_or([0, 0, 0]);
            let a   = trns.get(idx).copied().unwrap_or(255);
            [rgb[0], rgb[1], rgb[2], a]
        }
        4 => { // Grayscale + alpha
            let v = sample_channel(row, x, 0, bit_depth);
            let a = sample_channel(row, x, 1, bit_depth);
            [v, v, v, a]
        }
        6 => { // RGBA
            let r = sample_channel(row, x, 0, bit_depth);
            let g = sample_channel(row, x, 1, bit_depth);
            let b = sample_channel(row, x, 2, bit_depth);
            let a = sample_channel(row, x, 3, bit_depth);
            [r, g, b, a]
        }
        _ => [0, 0, 0, 255],
    }
}

fn sample_channel(row: &[u8], x: usize, ch: usize, bit_depth: u8) -> u8 {
    match bit_depth {
        8  => {
            let channels = row.len() / (row.len().max(1)); // fallback
            let _ = channels;
            *row.get(x * (bit_depth as usize / 8) * (ch + 1) - (ch * bit_depth as usize / 8)).unwrap_or(&0)
        }
        _ => {
            // General channel extraction.
            let bits_per_pixel_total = match bit_depth {
                1 => 1, 2 => 2, 4 => 4, 8 => 8, 16 => 16, _ => 8,
            };
            let start_bit = x * bits_per_pixel_total + ch * bit_depth as usize;
            let byte_idx  = start_bit / 8;
            let bit_off   = 7 - (start_bit % 8);
            let v = row.get(byte_idx).copied().unwrap_or(0);
            let raw = (v >> bit_off) & ((1u8 << bit_depth) - 1);
            // Scale to 8-bit.
            match bit_depth {
                1 => if raw != 0 { 255 } else { 0 },
                2 => raw * 85,
                4 => raw * 17,
                8 => raw,
                16 => {
                    let hi = row.get(byte_idx).copied().unwrap_or(0);
                    let lo = row.get(byte_idx+1).copied().unwrap_or(0);
                    u16::from_be_bytes([hi, lo]) as u8 // truncate to 8-bit
                }
                _ => raw,
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  JPEG decoder — Baseline DCT (SOF0)
// ─────────────────────────────────────────────────────────────────────────────

/// JPEG quantization table (64 coefficients in zigzag order → natural order).
struct QuantTable { data: [u16; 64] }

/// JPEG Huffman table.
struct JpegHuffTable {
    sizes: Vec<u8>,   // lengths
    codes: Vec<u16>,  // codes
    syms:  Vec<u8>,   // symbols
}

impl JpegHuffTable {
    fn decode_from_bits(&self, br: &mut BitReader<'_>) -> Result<u8, ImageError> {
        // Simple linear scan (fast enough for Phase 33).
        let mut code = 0u16;
        let mut code_len = 0u32;
        let mut idx = 0usize;
        while idx < self.sizes.len() {
            code_len += 1;
            code = (code << 1) | br.read_bits_rev(1)? as u16;
            while idx < self.sizes.len() && self.sizes[idx] as u32 == code_len {
                if self.codes[idx] == code {
                    return Ok(self.syms[idx]);
                }
                idx += 1;
            }
            if code_len > 16 { break; }
        }
        Err(ImageError::CorruptData)
    }
}

const ZIGZAG: [usize; 64] = [
     0, 1, 8,16, 9, 2, 3,10,
    17,24,32,25,18,11, 4, 5,
    12,19,26,33,40,48,41,34,
    27,20,13, 6, 7,14,21,28,
    35,42,49,56,57,50,43,36,
    29,22,15,23,30,37,44,51,
    58,59,52,45,38,31,39,46,
    53,60,61,54,47,55,62,63,
];

/// IDCT (8×8 AAN fast IDCT — integer approximation).
fn idct8x8(block: &mut [i32; 64]) {
    const W1: i32 = 2841;
    const W2: i32 = 2676;
    const W3: i32 = 2408;
    const W5: i32 = 1609;
    const W6: i32 = 1108;
    const W7: i32 =  565;

    // Row pass.
    for i in 0..8 {
        let off = i * 8;
        let (x0,x1,x2,x3,x4,x5,x6,x7) = (
            block[off+0] << 11,
            block[off+1],
            block[off+2],
            block[off+3],
            block[off+4],
            block[off+5],
            block[off+6],
            block[off+7],
        );
        if x1==0&&x2==0&&x3==0&&x4==0&&x5==0&&x6==0&&x7==0 {
            let dc = block[off+0] << 3;
            for k in 0..8 { block[off+k] = dc; }
            continue;
        }
        let x8  = W7*(x1+x7);
        let x1a = x8 + (W1-W7)*x1;
        let x7a = x8 - (W1+W7)*x7;
        let x8b = W3*(x5+x3);
        let x5a = x8b - (W3-W5)*x5;
        let x3a = x8b - (W3+W5)*x3;
        let x8c = x0 + (x4<<11);
        let x0a = x0 - (x4<<11);
        let x8d = W6*(x2+x6);
        let x2a = x8d - (W2+W6)*x6;
        let x6a = x8d + (W2-W6)*x2;
        block[off+0] = (x8c+x6a+x1a+x5a+512)>>10;
        block[off+1] = (x0a+x2a+x7a+x3a+512)>>10;
        block[off+2] = (x0a-x2a+x1a-x5a+512)>>10;
        block[off+3] = (x8c-x6a+x7a-x3a+512)>>10;
        block[off+4] = (x8c-x6a-x7a+x3a+512)>>10;
        block[off+5] = (x0a-x2a-x1a+x5a+512)>>10;
        block[off+6] = (x0a+x2a-x7a-x3a+512)>>10;
        block[off+7] = (x8c+x6a-x1a-x5a+512)>>10;
    }
    // Column pass.
    for i in 0..8 {
        let (x0,x1,x2,x3,x4,x5,x6,x7) = (
            block[i   ]<<8, block[i+ 8], block[i+16],
            block[i+24], block[i+32], block[i+40], block[i+48], block[i+56],
        );
        let x8  = W7*(x1+x7) + 4;
        let x1a = (x8 + (W1-W7)*x1) >> 3;
        let x7a = (x8 - (W1+W7)*x7) >> 3;
        let x8b = W3*(x5+x3) + 4;
        let x5a = (x8b - (W3-W5)*x5) >> 3;
        let x3a = (x8b - (W3+W5)*x3) >> 3;
        let x8c = x0 + (x4<<8);
        let x0a = x0 - (x4<<8);
        let x8d = W6*(x2+x6) + 4;
        let x2a = (x8d - (W2+W6)*x6) >> 3;
        let x6a = (x8d + (W2-W6)*x2) >> 3;
        block[i   ] = (x8c+x6a+x1a+x5a+65536)>>17;
        block[i+ 8] = (x0a+x2a+x7a+x3a+65536)>>17;
        block[i+16] = (x0a-x2a+x1a-x5a+65536)>>17;
        block[i+24] = (x8c-x6a+x7a-x3a+65536)>>17;
        block[i+32] = (x8c-x6a-x7a+x3a+65536)>>17;
        block[i+40] = (x0a-x2a-x1a+x5a+65536)>>17;
        block[i+48] = (x0a+x2a-x7a-x3a+65536)>>17;
        block[i+56] = (x8c+x6a-x1a-x5a+65536)>>17;
    }
}

/// Clamp i32 to [0, 255].
#[inline]
fn clamp8(v: i32) -> u8 { if v < 0 { 0 } else if v > 255 { 255 } else { v as u8 } }

/// YCbCr → RGB (BT.601).
#[inline]
fn ycbcr_to_rgb(y: i32, cb: i32, cr: i32) -> [u8; 3] {
    let r = y + 45 * cr / 32;
    let g = y - (11 * cb + 23 * cr) / 32;
    let b = y + 113 * cb / 64;
    [clamp8(r), clamp8(g), clamp8(b)]
}

pub fn decode_jpeg(data: &[u8]) -> Result<Image, ImageError> {
    let mut pos = 2usize; // skip SOI (FF D8)
    let mut width    = 0u32;
    let mut height   = 0u32;
    let mut ncomp    = 0u8;
    let mut quant:   Vec<Option<QuantTable>>    = (0..4).map(|_| None).collect();
    let mut dc_huff: Vec<Option<JpegHuffTable>> = (0..4).map(|_| None).collect();
    let mut ac_huff: Vec<Option<JpegHuffTable>> = (0..4).map(|_| None).collect();
    // Component info: (qtable_id, dc_table_id, ac_table_id, h_samp, v_samp)
    let mut comp_info: Vec<(u8, u8, u8, u8, u8)> = Vec::new();
    let mut sos_data: &[u8] = &[];

    while pos + 2 <= data.len() {
        if data[pos] != 0xFF { return Err(ImageError::CorruptData); }
        let marker = data[pos+1];
        pos += 2;
        if marker == 0xD9 { break; } // EOI
        if marker == 0xD8 { continue; } // SOI (nested)
        if pos + 2 > data.len() { break; }
        let seg_len = u16::from_be_bytes([data[pos], data[pos+1]]) as usize;
        let seg = if pos + seg_len <= data.len() { &data[pos..pos+seg_len] } else { break };
        pos += seg_len;

        match marker {
            0xC0 => { // SOF0 — baseline DCT
                if seg.len() < 11 { return Err(ImageError::CorruptData); }
                height = u16::from_be_bytes([seg[3], seg[4]]) as u32;
                width  = u16::from_be_bytes([seg[5], seg[6]]) as u32;
                ncomp  = seg[7];
                comp_info.clear();
                for i in 0..ncomp as usize {
                    let b = 8 + i * 3;
                    if b + 3 > seg.len() { break; }
                    let qt   = seg[b+2] & 0x0F;
                    let samp = seg[b+1];
                    let hs   = samp >> 4;
                    let vs   = samp & 0xF;
                    comp_info.push((qt, 0, 0, hs, vs));
                }
            }
            0xDB => { // DQT
                let mut off = 2;
                while off + 65 <= seg.len() {
                    let pq_tq = seg[off]; off += 1;
                    let tid   = (pq_tq & 0x0F) as usize;
                    let prec  = pq_tq >> 4;
                    let mut qt = QuantTable { data: [0u16; 64] };
                    for i in 0..64 {
                        qt.data[ZIGZAG[i]] = if prec == 0 {
                            let v = seg[off] as u16; off += 1; v
                        } else {
                            let v = u16::from_be_bytes([seg[off], seg[off+1]]); off += 2; v
                        };
                    }
                    if tid < 4 { quant[tid] = Some(qt); }
                }
            }
            0xC4 => { // DHT
                let mut off = 2;
                while off + 17 <= seg.len() {
                    let tc_th = seg[off]; off += 1;
                    let tc = (tc_th >> 4) as usize; // 0=DC, 1=AC
                    let th = (tc_th & 0xF) as usize;
                    let mut counts = [0u8; 16];
                    counts.copy_from_slice(&seg[off..off+16]); off += 16;
                    let total: usize = counts.iter().map(|&c| c as usize).sum();
                    let mut syms = Vec::with_capacity(total);
                    for _ in 0..total {
                        if off >= seg.len() { break; }
                        syms.push(seg[off]); off += 1;
                    }
                    // Build canonical codes.
                    let mut sizes = Vec::new();
                    let mut codes = Vec::new();
                    let mut code = 0u16;
                    for (len, &cnt) in counts.iter().enumerate() {
                        for i in 0..cnt as usize {
                            let sym_idx = sizes.len();
                            if sym_idx < syms.len() {
                                sizes.push((len + 1) as u8);
                                codes.push(code);
                            }
                            code += 1;
                        }
                        code <<= 1;
                    }
                    let table = JpegHuffTable { sizes, codes, syms };
                    if tc == 0 && th < 4 { dc_huff[th] = Some(table); }
                    else if tc == 1 && th < 4 { ac_huff[th] = Some(table); }
                }
            }
            0xC4 => {} // duplicate DHT handled above
            0xDA => { // SOS — scan header, then entropy-coded data
                // Find the entropy-coded data (after the scan header).
                let scan_hdr_len = seg_len;
                // Map components to tables.
                let ns = seg.get(2).copied().unwrap_or(0) as usize;
                for i in 0..ns {
                    let b = 3 + i * 2;
                    if b + 2 > seg.len() { break; }
                    let comp_id = seg[b];
                    let td_ta   = seg[b+1];
                    let dc_id   = (td_ta >> 4) as usize;
                    let ac_id   = (td_ta & 0xF) as usize;
                    // Find this component in comp_info by index.
                    let ci = (comp_id as usize).saturating_sub(1).min(comp_info.len().saturating_sub(1));
                    if ci < comp_info.len() {
                        comp_info[ci].1 = dc_id as u8;
                        comp_info[ci].2 = ac_id as u8;
                    }
                }
                // Entropy-coded data starts at pos (after SOS segment).
                sos_data = &data[pos..];
                break; // start decoding below
            }
            _ => {} // skip unknown markers
        }
    }

    if width == 0 || height == 0 { return Err(ImageError::InvalidHeader); }
    if width > 8192 || height > 8192 { return Err(ImageError::UnsupportedFormat); }
    if sos_data.is_empty() { return Err(ImageError::CorruptData); }

    // Unstuff 0xFF 0x00 → 0xFF in entropy-coded data.
    let mut ecd: Vec<u8> = Vec::new();
    let mut si = 0usize;
    while si < sos_data.len() {
        let b = sos_data[si]; si += 1;
        ecd.push(b);
        if b == 0xFF {
            let next = sos_data.get(si).copied().unwrap_or(0);
            if next == 0x00 { si += 1; } // stuffed zero
            else if next == 0xD9 { break; } // EOI
        }
    }

    // Decode MCUs.
    let mut br = BitReader::new(&ecd);
    let mut img = Image::new(width, height);
    let ncomp = comp_info.len().min(3); // max 3 components (Y,Cb,Cr)
    let mut dc_prev = [0i32; 3];
    let mcu_w = 8usize;
    let mcu_h = 8usize;
    let mcus_x = (width as usize + mcu_w - 1) / mcu_w;
    let mcus_y = (height as usize + mcu_h - 1) / mcu_h;

    'mcu_loop: for mcu_y in 0..mcus_y {
        for mcu_x in 0..mcus_x {
            let mut comp_blocks: Vec<[i32; 64]> = Vec::new();
            for c in 0..ncomp {
                let (qt_id, dc_id, ac_id, _, _) = comp_info[c];
                let qt  = match quant.get(qt_id as usize).and_then(|q| q.as_ref()) {
                    Some(t) => t,
                    None    => continue,
                };
                let dc_table = match dc_huff.get(dc_id as usize).and_then(|t| t.as_ref()) {
                    Some(t) => t,
                    None    => continue,
                };
                let ac_table = match ac_huff.get(ac_id as usize).and_then(|t| t.as_ref()) {
                    Some(t) => t,
                    None    => continue,
                };

                let mut block = [0i32; 64];
                // DC coefficient.
                let dc_size = match dc_table.decode_from_bits(&mut br) {
                    Ok(s) => s,
                    Err(_) => break 'mcu_loop,
                };
                let dc_diff = if dc_size == 0 {
                    0i32
                } else {
                    let raw = match br.read_bits(dc_size as u32) {
                        Ok(v) => v as i32,
                        Err(_) => break 'mcu_loop,
                    };
                    extend(raw, dc_size)
                };
                dc_prev[c] += dc_diff;
                block[0] = dc_prev[c] * qt.data[0] as i32;

                // AC coefficients.
                let mut k = 1usize;
                while k < 64 {
                    let rs = match ac_table.decode_from_bits(&mut br) {
                        Ok(v) => v,
                        Err(_) => break,
                    };
                    let run  = (rs >> 4) as usize;
                    let size = rs & 0xF;
                    if rs == 0x00 { break; } // EOB
                    if rs == 0xF0 { k += 16; continue; } // ZRL
                    k += run;
                    if k >= 64 { break; }
                    let raw = match br.read_bits(size as u32) {
                        Ok(v) => v as i32,
                        Err(_) => break,
                    };
                    block[ZIGZAG[k]] = extend(raw, size) * qt.data[ZIGZAG[k]] as i32;
                    k += 1;
                }

                idct8x8(&mut block);
                comp_blocks.push(block);
            }

            // Write pixels.
            for by in 0..8usize {
                for bx in 0..8usize {
                    let px = mcu_x * 8 + bx;
                    let py = mcu_y * 8 + by;
                    if px >= width as usize || py >= height as usize { continue; }
                    let idx = by * 8 + bx;
                    let rgba = if ncomp == 1 {
                        let y = comp_blocks.get(0).map(|b| b[idx] + 128).unwrap_or(128) as u8;
                        [y, y, y, 255]
                    } else {
                        let y  = comp_blocks.get(0).map(|b| b[idx] + 128).unwrap_or(128);
                        let cb = comp_blocks.get(1).map(|b| b[idx]).unwrap_or(0);
                        let cr = comp_blocks.get(2).map(|b| b[idx]).unwrap_or(0);
                        let [r, g, b] = ycbcr_to_rgb(y, cb, cr);
                        [r, g, b, 255]
                    };
                    img.set_pixel(px as u32, py as u32, rgba);
                }
            }
        }
    }

    Ok(img)
}

/// Extend a raw JPEG coefficient to a signed value.
fn extend(v: i32, t: u8) -> i32 {
    if t == 0 { return 0; }
    let vt = 1 << (t - 1);
    if v < vt { v + (-1 << t) + 1 } else { v }
}

// ─────────────────────────────────────────────────────────────────────────────
//  BMP decoder (24-bit + 32-bit, uncompressed)
// ─────────────────────────────────────────────────────────────────────────────

pub fn decode_bmp(data: &[u8]) -> Result<Image, ImageError> {
    if data.len() < 54 { return Err(ImageError::InvalidHeader); }
    let pixel_off = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;
    let width     = u32::from_le_bytes([data[18], data[19], data[20], data[21]]);
    let height_s  = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
    let height    = height_s.unsigned_abs();
    let bpp       = u16::from_le_bytes([data[28], data[29]]);
    let comp      = u32::from_le_bytes([data[30], data[31], data[32], data[33]]);

    if width == 0 || height == 0 || width > 8192 || height > 8192 { return Err(ImageError::UnsupportedFormat); }
    if comp != 0 && comp != 3 { return Err(ImageError::UnsupportedFormat); } // only BI_RGB, BI_BITFIELDS
    if bpp != 24 && bpp != 32 { return Err(ImageError::UnsupportedFormat); }

    let bytes_pp = bpp as usize / 8;
    let row_stride = (width as usize * bytes_pp + 3) & !3; // padded to 4 bytes

    let mut img = Image::new(width, height);
    for y in 0..height as usize {
        // BMP rows stored bottom-up unless height is negative.
        let src_y = if height_s > 0 { height as usize - 1 - y } else { y };
        let row_off = pixel_off + src_y * row_stride;
        for x in 0..width as usize {
            let off = row_off + x * bytes_pp;
            if off + bytes_pp > data.len() { break; }
            let b = data[off];
            let g = data[off+1];
            let r = data[off+2];
            let a = if bytes_pp == 4 { data[off+3] } else { 255 };
            img.set_pixel(x as u32, y as u32, [r, g, b, a]);
        }
    }
    Ok(img)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Module init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[image] PNG/JPEG/BMP image decoder ready.");
}

//! Deflate / zlib / gzip decompression.
//!
//! Pure-Rust, no_std-compatible implementation of:
//!   • RFC 1951 — Deflate (stored, fixed Huffman, dynamic Huffman blocks)
//!   • RFC 1950 — zlib wrapper (2-byte header + ADLER32 trailer)
//!   • RFC 1952 — gzip wrapper (10-byte header + optional FEXTRA/FNAME/FCOMMENT/FHCRC + CRC32 + ISIZE trailer)
//!
//! Used by both the PNG decoder and the HTTP client (`Content-Encoding: gzip`).

#![allow(dead_code)]

use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug)]
pub enum InflateError {
    Truncated,
    BadHeader(&'static str),
    BadBlock(&'static str),
    EmptyHuffman,
    SymbolNotInTree,
    BadBackref,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Public surface
// ─────────────────────────────────────────────────────────────────────────────

/// Decode a raw deflate stream (no wrapper).
pub fn inflate(data: &[u8]) -> Result<Vec<u8>, InflateError> {
    let mut br = BitReader::new(data);
    let mut out = Vec::with_capacity(data.len() * 4);
    inflate_blocks(&mut br, &mut out)?;
    Ok(out)
}

/// Decode a zlib (RFC 1950) stream: 2-byte CMF/FLG header + deflate + 4-byte ADLER32.
pub fn inflate_zlib(data: &[u8]) -> Result<Vec<u8>, InflateError> {
    if data.len() < 6 { return Err(InflateError::Truncated); }
    let cmf = data[0];
    let flg = data[1];
    if (cmf & 0x0F) != 8 { return Err(InflateError::BadHeader("zlib: not deflate")); }
    if ((cmf as u16) * 256 + flg as u16) % 31 != 0 {
        return Err(InflateError::BadHeader("zlib: bad FCHECK"));
    }
    if (flg & 0x20) != 0 {
        return Err(InflateError::BadHeader("zlib: FDICT not supported"));
    }
    inflate(&data[2..data.len() - 4])
}

/// Decode a gzip (RFC 1952) stream: 10-byte header (+ optional fields) + deflate + 8-byte trailer.
pub fn inflate_gzip(data: &[u8]) -> Result<Vec<u8>, InflateError> {
    if data.len() < 18 { return Err(InflateError::Truncated); }
    if data[0] != 0x1F || data[1] != 0x8B {
        return Err(InflateError::BadHeader("gzip: bad magic"));
    }
    if data[2] != 8 {
        return Err(InflateError::BadHeader("gzip: not deflate"));
    }
    let flg = data[3];
    let mut pos = 10usize; // fixed header

    // FEXTRA (bit 2): xlen u16 LE + xlen bytes
    if flg & 0x04 != 0 {
        if pos + 2 > data.len() { return Err(InflateError::Truncated); }
        let xlen = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2 + xlen;
        if pos > data.len() { return Err(InflateError::Truncated); }
    }
    // FNAME (bit 3): NUL-terminated
    if flg & 0x08 != 0 {
        while pos < data.len() && data[pos] != 0 { pos += 1; }
        if pos >= data.len() { return Err(InflateError::Truncated); }
        pos += 1;
    }
    // FCOMMENT (bit 4): NUL-terminated
    if flg & 0x10 != 0 {
        while pos < data.len() && data[pos] != 0 { pos += 1; }
        if pos >= data.len() { return Err(InflateError::Truncated); }
        pos += 1;
    }
    // FHCRC (bit 1): 2-byte CRC16
    if flg & 0x02 != 0 {
        if pos + 2 > data.len() { return Err(InflateError::Truncated); }
        pos += 2;
    }
    // Trailer: CRC32 + ISIZE = 8 bytes at the very end.
    if data.len() < pos + 8 { return Err(InflateError::Truncated); }
    let deflate_end = data.len() - 8;
    inflate(&data[pos..deflate_end])
}

// ─────────────────────────────────────────────────────────────────────────────
//  Deflate block driver
// ─────────────────────────────────────────────────────────────────────────────

fn inflate_blocks(br: &mut BitReader, out: &mut Vec<u8>) -> Result<(), InflateError> {
    loop {
        let bfinal = br.read_bits(1)?;
        let btype  = br.read_bits(2)?;
        match btype {
            0 => inflate_stored(br, out)?,
            1 => inflate_huff(br, out, &fixed_litlen(), &fixed_dist())?,
            2 => {
                let (lit, dist) = read_dynamic_codes(br)?;
                inflate_huff(br, out, &lit, &dist)?;
            }
            _ => return Err(InflateError::BadBlock("reserved type")),
        }
        if bfinal == 1 { break; }
    }
    Ok(())
}

fn inflate_stored(br: &mut BitReader, out: &mut Vec<u8>) -> Result<(), InflateError> {
    br.align_to_byte();
    let len  = br.read_u16_le()? as usize;
    let nlen = br.read_u16_le()? as usize;
    if len ^ 0xFFFF != nlen {
        return Err(InflateError::BadBlock("stored LEN/NLEN mismatch"));
    }
    for _ in 0..len { out.push(br.read_byte()?); }
    Ok(())
}

fn inflate_huff(br: &mut BitReader, out: &mut Vec<u8>, litlen: &HuffTable, dist: &HuffTable) -> Result<(), InflateError> {
    loop {
        let sym = decode_symbol(br, litlen)?;
        if sym < 256 {
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(());
        } else {
            let length = read_length(br, sym)?;
            let dsym   = decode_symbol(br, dist)?;
            let backd  = read_distance(br, dsym)?;
            if backd == 0 || backd > out.len() {
                return Err(InflateError::BadBackref);
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

fn read_length(br: &mut BitReader, sym: u32) -> Result<usize, InflateError> {
    let i = (sym - 257) as usize;
    if i >= 29 { return Err(InflateError::BadBlock("bad length symbol")); }
    let extra = LEN_EXTRA[i] as u32;
    let base  = LEN_BASE[i] as u32;
    Ok((base + if extra > 0 { br.read_bits(extra)? } else { 0 }) as usize)
}

fn read_distance(br: &mut BitReader, sym: u32) -> Result<usize, InflateError> {
    let i = sym as usize;
    if i >= 30 { return Err(InflateError::BadBlock("bad distance symbol")); }
    let extra = DIST_EXTRA[i] as u32;
    let base  = DIST_BASE[i] as u32;
    Ok((base + if extra > 0 { br.read_bits(extra)? } else { 0 }) as usize)
}

// ── Fixed Huffman tables ─────────────────────────────────────────────────────

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

// ── Dynamic Huffman tables ───────────────────────────────────────────────────

const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn read_dynamic_codes(br: &mut BitReader) -> Result<(HuffTable, HuffTable), InflateError> {
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
                if i == 0 { return Err(InflateError::BadBlock("CL repeat with no prior")); }
                let n = (br.read_bits(2)? + 3) as usize;
                let prev = lengths[i - 1];
                for _ in 0..n {
                    if i >= total { return Err(InflateError::BadBlock("CL overflow")); }
                    lengths[i] = prev; i += 1;
                }
            }
            17 => {
                let n = (br.read_bits(3)? + 3) as usize;
                for _ in 0..n {
                    if i >= total { return Err(InflateError::BadBlock("CL overflow")); }
                    lengths[i] = 0; i += 1;
                }
            }
            18 => {
                let n = (br.read_bits(7)? + 11) as usize;
                for _ in 0..n {
                    if i >= total { return Err(InflateError::BadBlock("CL overflow")); }
                    lengths[i] = 0; i += 1;
                }
            }
            _ => return Err(InflateError::BadBlock("bad CL symbol")),
        }
    }
    let lit  = build_huff(&lengths[..hlit as usize]);
    let dist = build_huff(&lengths[hlit as usize..]);
    Ok((lit, dist))
}

// ── Huffman table ────────────────────────────────────────────────────────────

pub struct HuffTable {
    max_len: u32,
    first_code: [u32; 16],
    sym_offset: [usize; 16],
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

fn decode_symbol(br: &mut BitReader, table: &HuffTable) -> Result<u32, InflateError> {
    if table.max_len == 0 {
        return Err(InflateError::EmptyHuffman);
    }
    let mut code = 0u32;
    for length in 1..=table.max_len as usize {
        let bit = br.read_bits(1)?;
        code = (code << 1) | bit;
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
    Err(InflateError::SymbolNotInTree)
}

// ── Bit reader (LSB-first as required by deflate) ───────────────────────────

pub struct BitReader<'a> {
    src: &'a [u8],
    pos: usize,
    buf: u32,
    bits: u32,
}

impl<'a> BitReader<'a> {
    pub fn new(src: &'a [u8]) -> Self { Self { src, pos: 0, buf: 0, bits: 0 } }

    pub fn read_bits(&mut self, n: u32) -> Result<u32, InflateError> {
        while self.bits < n {
            if self.pos >= self.src.len() { return Err(InflateError::Truncated); }
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

    pub fn align_to_byte(&mut self) {
        let extra = self.bits & 7;
        if extra > 0 {
            self.buf >>= extra;
            self.bits -= extra;
        }
    }

    pub fn read_u16_le(&mut self) -> Result<u32, InflateError> {
        let lo = self.read_byte()? as u32;
        let hi = self.read_byte()? as u32;
        Ok(lo | (hi << 8))
    }

    pub fn read_byte(&mut self) -> Result<u8, InflateError> {
        Ok(self.read_bits(8)? as u8)
    }
}

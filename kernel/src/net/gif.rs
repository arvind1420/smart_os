//! Minimal GIF decoder for the browser.
//!
//! Supports:
//!   • GIF87a / GIF89a signatures
//!   • Global Color Table + Logical Screen Descriptor
//!   • Image Descriptor (frame 0 only — later frames ignored)
//!   • LZW-compressed image data with code-size escalation + CLEAR/EOI codes
//!   • Local Color Table (overrides global if present)
//!   • Interlaced frames (Adam7-style 4-pass)
//!   • Transparency via Graphic Control Extension
//!
//! Out of scope:
//!   • Animation (we always return frame 0)
//!   • Disposal methods (single-frame output)
//!   • Plain Text Extension
//!
//! Output is RGBA8888 row-major.

#![allow(dead_code)]

use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug)]
pub enum GifError {
    BadMagic,
    Truncated,
    Unsupported(&'static str),
    LzwError(&'static str),
}

pub struct Image {
    pub width:  u32,
    pub height: u32,
    pub data:   Vec<u8>, // RGBA8 row-major
}

pub fn decode(bytes: &[u8]) -> Result<Image, GifError> {
    if bytes.len() < 13 { return Err(GifError::Truncated); }
    let magic = &bytes[..6];
    if magic != b"GIF87a" && magic != b"GIF89a" {
        return Err(GifError::BadMagic);
    }

    // Logical screen descriptor (7 bytes after the signature).
    let lsd = &bytes[6..13];
    let screen_w = u16::from_le_bytes([lsd[0], lsd[1]]) as u32;
    let screen_h = u16::from_le_bytes([lsd[2], lsd[3]]) as u32;
    let packed   = lsd[4];
    let bg_index = lsd[5];
    let _aspect  = lsd[6];

    let global_color_table_flag = packed & 0x80 != 0;
    let color_resolution = (packed >> 4) & 0x07;
    let _sort_flag = packed & 0x08 != 0;
    let gct_size_exp = (packed & 0x07) as u32;
    let gct_entries = if global_color_table_flag { 1u32 << (gct_size_exp + 1) } else { 0 };

    let mut pos = 13usize;
    let mut global_table = Vec::new();
    if gct_entries > 0 {
        let end = pos + (gct_entries as usize) * 3;
        if bytes.len() < end { return Err(GifError::Truncated); }
        global_table = bytes[pos..end].to_vec();
        pos = end;
    }
    let _ = (color_resolution, bg_index);

    // Parse blocks until we find an image descriptor.  Track the most recent
    // Graphic Control Extension for transparency.
    let mut transparent: Option<u8> = None;

    loop {
        if pos >= bytes.len() { return Err(GifError::Truncated); }
        let intro = bytes[pos]; pos += 1;
        match intro {
            0x21 => {
                // Extension introducer.
                if pos >= bytes.len() { return Err(GifError::Truncated); }
                let label = bytes[pos]; pos += 1;
                if label == 0xF9 {
                    // Graphic Control Extension — 1-byte block size then 4 data bytes + terminator.
                    if pos + 6 > bytes.len() { return Err(GifError::Truncated); }
                    let block_size = bytes[pos] as usize;
                    if block_size != 4 { return Err(GifError::Unsupported("GCE block size")); }
                    let packed_g = bytes[pos + 1];
                    let _delay  = u16::from_le_bytes([bytes[pos + 2], bytes[pos + 3]]);
                    let t_index = bytes[pos + 4];
                    let trailing = bytes[pos + 5];
                    if trailing != 0 { return Err(GifError::Unsupported("GCE terminator")); }
                    if packed_g & 0x01 != 0 { transparent = Some(t_index); }
                    pos += 6;
                } else {
                    // Skip unknown extension by consuming its sub-blocks.
                    pos = skip_sub_blocks(bytes, pos)?;
                }
            }
            0x2C => {
                // Image descriptor — decode this frame, then stop.
                if pos + 9 > bytes.len() { return Err(GifError::Truncated); }
                let _left = u16::from_le_bytes([bytes[pos    ], bytes[pos + 1]]);
                let _top  = u16::from_le_bytes([bytes[pos + 2], bytes[pos + 3]]);
                let frame_w = u16::from_le_bytes([bytes[pos + 4], bytes[pos + 5]]) as u32;
                let frame_h = u16::from_le_bytes([bytes[pos + 6], bytes[pos + 7]]) as u32;
                let img_packed = bytes[pos + 8];
                pos += 9;

                let lct_flag = img_packed & 0x80 != 0;
                let interlace = img_packed & 0x40 != 0;
                let lct_size_exp = (img_packed & 0x07) as u32;
                let palette = if lct_flag {
                    let n = (1u32 << (lct_size_exp + 1)) as usize;
                    let end = pos + n * 3;
                    if bytes.len() < end { return Err(GifError::Truncated); }
                    let p = bytes[pos..end].to_vec();
                    pos = end;
                    p
                } else {
                    global_table.clone()
                };
                if palette.is_empty() {
                    return Err(GifError::Unsupported("no palette"));
                }

                // LZW minimum code size.
                if pos >= bytes.len() { return Err(GifError::Truncated); }
                let min_code_size = bytes[pos]; pos += 1;
                if min_code_size < 2 || min_code_size > 8 {
                    return Err(GifError::Unsupported("LZW min code size"));
                }

                // Gather sub-blocks of compressed data.
                let mut lzw_buf: Vec<u8> = Vec::new();
                loop {
                    if pos >= bytes.len() { return Err(GifError::Truncated); }
                    let blk_len = bytes[pos] as usize; pos += 1;
                    if blk_len == 0 { break; }
                    if pos + blk_len > bytes.len() { return Err(GifError::Truncated); }
                    lzw_buf.extend_from_slice(&bytes[pos..pos + blk_len]);
                    pos += blk_len;
                }

                let indices = lzw_decode(&lzw_buf, min_code_size, (frame_w * frame_h) as usize)?;

                // Convert indices to RGBA8.
                let mut data = vec![0u8; (frame_w * frame_h * 4) as usize];
                if interlace {
                    // Deinterlace — 4 passes: rows 0,8,16,...  4,12,20,...  2,6,10,...  1,3,5,...
                    let passes: [(u32, u32); 4] = [(0, 8), (4, 8), (2, 4), (1, 2)];
                    let mut src_y = 0u32;
                    for (start, step) in passes {
                        let mut y = start;
                        while y < frame_h {
                            for x in 0..frame_w {
                                let idx_pos = (src_y * frame_w + x) as usize;
                                if idx_pos >= indices.len() { break; }
                                let pi = indices[idx_pos] as usize;
                                write_pixel(&mut data, x, y, frame_w, &palette, pi, transparent);
                            }
                            src_y += 1;
                            y += step;
                        }
                    }
                } else {
                    for y in 0..frame_h {
                        for x in 0..frame_w {
                            let idx_pos = (y * frame_w + x) as usize;
                            if idx_pos >= indices.len() { break; }
                            let pi = indices[idx_pos] as usize;
                            write_pixel(&mut data, x, y, frame_w, &palette, pi, transparent);
                        }
                    }
                }

                let _ = screen_w; let _ = screen_h;
                return Ok(Image { width: frame_w, height: frame_h, data });
            }
            0x3B => return Err(GifError::Truncated), // trailer with no image
            _    => return Err(GifError::Unsupported("unknown block")),
        }
    }
}

fn skip_sub_blocks(bytes: &[u8], mut pos: usize) -> Result<usize, GifError> {
    loop {
        if pos >= bytes.len() { return Err(GifError::Truncated); }
        let n = bytes[pos] as usize; pos += 1;
        if n == 0 { break; }
        pos += n;
        if pos > bytes.len() { return Err(GifError::Truncated); }
    }
    Ok(pos)
}

fn write_pixel(data: &mut [u8], x: u32, y: u32, w: u32, palette: &[u8], idx: usize, transparent: Option<u8>) {
    let dst = ((y * w + x) * 4) as usize;
    if let Some(t) = transparent {
        if idx as u8 == t {
            // Transparent — leave alpha 0.
            data[dst + 3] = 0;
            return;
        }
    }
    let base = idx * 3;
    if base + 2 < palette.len() {
        data[dst    ] = palette[base    ];
        data[dst + 1] = palette[base + 1];
        data[dst + 2] = palette[base + 2];
        data[dst + 3] = 255;
    } else {
        data[dst + 3] = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  LZW decoder
// ─────────────────────────────────────────────────────────────────────────────

fn lzw_decode(input: &[u8], min_code_size: u8, expected: usize) -> Result<Vec<u8>, GifError> {
    let clear_code = 1u16 << min_code_size;
    let end_code   = clear_code + 1;
    let mut code_size = min_code_size as u32 + 1;

    // Dictionary: code → (prefix code, suffix byte).
    // Codes 0..clear_code are roots (single bytes).
    let mut dict_prefix: Vec<u16> = Vec::with_capacity(4096);
    let mut dict_suffix: Vec<u8>  = Vec::with_capacity(4096);

    let mut out = Vec::with_capacity(expected);
    let mut br = BitReaderLsb::new(input);

    // Build initial dictionary.
    let init = |dp: &mut Vec<u16>, ds: &mut Vec<u8>| {
        dp.clear();
        ds.clear();
        for i in 0..clear_code {
            dp.push(0);
            ds.push(i as u8);
        }
        dp.push(0); ds.push(0); // CLEAR
        dp.push(0); ds.push(0); // EOI
    };
    init(&mut dict_prefix, &mut dict_suffix);

    let mut prev_code: Option<u16> = None;

    loop {
        let code = match br.read_bits(code_size) {
            Some(c) => c as u16,
            None    => break,
        };
        if code == clear_code {
            code_size = min_code_size as u32 + 1;
            init(&mut dict_prefix, &mut dict_suffix);
            prev_code = None;
            continue;
        }
        if code == end_code { break; }

        // Decode entry into out via stack.
        let mut stack: Vec<u8> = Vec::new();
        let mut current = code;
        if (current as usize) < dict_prefix.len() {
            while current >= clear_code {
                stack.push(dict_suffix[current as usize]);
                current = dict_prefix[current as usize];
            }
            stack.push(dict_suffix[current as usize]);
        } else {
            // Special "KwK" case: code == next-to-be-added.  Use previous string + its first byte.
            let pc = prev_code.ok_or(GifError::LzwError("KwK without previous"))?;
            let mut c2 = pc;
            while c2 >= clear_code {
                stack.push(dict_suffix[c2 as usize]);
                c2 = dict_prefix[c2 as usize];
            }
            stack.push(dict_suffix[c2 as usize]);
            let first = *stack.last().unwrap();
            // Output previous string then first byte.
            for &b in stack.iter().rev() { out.push(b); }
            out.push(first);
            stack.clear();
            // Add (prev, first) to dict.
            if dict_prefix.len() < 4096 {
                dict_prefix.push(pc);
                dict_suffix.push(first);
            }
            prev_code = Some(code);
            if dict_prefix.len() == (1 << code_size) && code_size < 12 {
                code_size += 1;
            }
            continue;
        }

        let first = *stack.last().unwrap();
        for &b in stack.iter().rev() { out.push(b); }

        // Add (prev, first) to dict.
        if let Some(pc) = prev_code {
            if dict_prefix.len() < 4096 {
                dict_prefix.push(pc);
                dict_suffix.push(first);
            }
        }
        prev_code = Some(code);

        // Grow code size when we fill the current span.
        if dict_prefix.len() == (1 << code_size) && code_size < 12 {
            code_size += 1;
        }
    }

    let _ = expected;
    Ok(out)
}

// ── LSB-first bit reader for LZW ─────────────────────────────────────────────

struct BitReaderLsb<'a> {
    src:  &'a [u8],
    pos:  usize,
    buf:  u32,
    bits: u32,
}

impl<'a> BitReaderLsb<'a> {
    fn new(src: &'a [u8]) -> Self { Self { src, pos: 0, buf: 0, bits: 0 } }
    fn read_bits(&mut self, n: u32) -> Option<u32> {
        while self.bits < n {
            if self.pos >= self.src.len() { return None; }
            self.buf |= (self.src[self.pos] as u32) << self.bits;
            self.pos += 1;
            self.bits += 8;
        }
        let mask = (1u32 << n) - 1;
        let v = self.buf & mask;
        self.buf >>= n;
        self.bits -= n;
        Some(v)
    }
}

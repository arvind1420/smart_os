//! Minimal baseline JPEG decoder.
//!
//! Supports:
//!   • SOI / EOI markers
//!   • APPn / DRI / DAC / COM (skipped, length-prefixed)
//!   • DQT — quantisation tables (8-bit precision only)
//!   • DHT — Huffman tables
//!   • SOF0 — baseline DCT
//!   • SOS — scan data with restart markers
//!   • IDCT (integer AAN approximation)
//!   • YCbCr → RGB conversion (BT.601)
//!   • Chroma subsampling: 4:4:4, 4:2:2, 4:2:0
//!
//! Not supported:
//!   • Progressive (SOF2), arithmetic coded (SOF1/3/etc.)
//!   • 12-bit precision
//!   • Lossless / hierarchical
//!   • CMYK / YCCK
//!
//! Output is row-major RGBA8888 (alpha forced to 255).

#![allow(dead_code)]

use alloc::vec;
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
//  Public API
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum JpegError {
    BadMagic,
    Truncated,
    UnsupportedMarker(u8),
    Unsupported(&'static str),
    HuffmanOverflow,
    BadHuffman,
    BadComponent,
}

pub struct Image {
    pub width:  u32,
    pub height: u32,
    pub data:   Vec<u8>, // RGBA8888 row-major
}

const SOI:  u8 = 0xD8;
const EOI:  u8 = 0xD9;
const SOF0: u8 = 0xC0;
const SOF1: u8 = 0xC1;
const SOF2: u8 = 0xC2;
const DHT:  u8 = 0xC4;
const DQT:  u8 = 0xDB;
const SOS:  u8 = 0xDA;
const DRI:  u8 = 0xDD;
const COM:  u8 = 0xFE;

pub fn decode(bytes: &[u8]) -> Result<Image, JpegError> {
    if bytes.len() < 2 || bytes[0] != 0xFF || bytes[1] != SOI {
        return Err(JpegError::BadMagic);
    }

    let mut dec = Decoder::new(bytes);
    dec.pos = 2;

    loop {
        let m = dec.read_marker()?;
        match m {
            SOF0 => dec.parse_sof()?,
            SOF1 | SOF2 => return Err(JpegError::Unsupported("non-baseline JPEG")),
            DHT  => dec.parse_dht()?,
            DQT  => dec.parse_dqt()?,
            DRI  => dec.parse_dri()?,
            SOS  => { dec.parse_sos_and_scan()?; break; }
            EOI  => break,
            0xE0..=0xEF | COM => dec.skip_segment()?,
            other => {
                // Pass unknown but valid app/skippable markers.
                if (0xC0..=0xCF).contains(&other) || (0xD0..=0xD7).contains(&other) {
                    return Err(JpegError::UnsupportedMarker(other));
                }
                dec.skip_segment()?;
            }
        }
    }

    dec.finish()
}

// ─────────────────────────────────────────────────────────────────────────────
//  Internal state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct Component {
    id:    u8,
    h:     u8,        // horizontal sampling factor
    v:     u8,        // vertical sampling factor
    tq:    u8,        // quantisation table index
    td:    u8,        // DC Huffman table index
    ta:    u8,        // AC Huffman table index
    pred:  i32,       // DC predictor across MCUs
}

#[derive(Default, Clone)]
struct HuffTable {
    // Direct decode: max_code[len] = largest code of that length;
    // val_offset[len] = first slot in `values` for that length;
    // values = symbols in canonical order.
    max_code:   [i32; 17],
    val_offset: [i32; 17],
    values:     Vec<u8>,
}

struct Decoder<'a> {
    src:        &'a [u8],
    pos:        usize,

    width:      u32,
    height:     u32,
    n_comps:    usize,
    components: [Component; 4],

    quant_tabs: [[i32; 64]; 4],
    huff_dc:    [HuffTable; 4],
    huff_ac:    [HuffTable; 4],
    restart_interval: u32,

    output:     Vec<u8>, // RGBA, populated by parse_sos_and_scan
}

impl<'a> Decoder<'a> {
    fn new(src: &'a [u8]) -> Self {
        Self {
            src, pos: 0,
            width: 0, height: 0, n_comps: 0,
            components: Default::default(),
            quant_tabs: [[0; 64]; 4],
            huff_dc: Default::default(),
            huff_ac: Default::default(),
            restart_interval: 0,
            output: Vec::new(),
        }
    }

    fn read_byte(&mut self) -> Result<u8, JpegError> {
        if self.pos >= self.src.len() { return Err(JpegError::Truncated); }
        let b = self.src[self.pos]; self.pos += 1; Ok(b)
    }

    fn read_u16(&mut self) -> Result<u16, JpegError> {
        let hi = self.read_byte()? as u16;
        let lo = self.read_byte()? as u16;
        Ok((hi << 8) | lo)
    }

    fn read_marker(&mut self) -> Result<u8, JpegError> {
        // A marker is 0xFF followed by a non-zero, non-0xFF byte.
        loop {
            let b = self.read_byte()?;
            if b != 0xFF { continue; }
            // Skip any fill 0xFFs.
            let mut next = self.read_byte()?;
            while next == 0xFF { next = self.read_byte()?; }
            if next != 0x00 {
                return Ok(next);
            }
        }
    }

    fn skip_segment(&mut self) -> Result<(), JpegError> {
        let len = self.read_u16()? as usize;
        if len < 2 || self.pos + len - 2 > self.src.len() {
            return Err(JpegError::Truncated);
        }
        self.pos += len - 2;
        Ok(())
    }

    fn finish(self) -> Result<Image, JpegError> {
        if self.output.is_empty() {
            return Err(JpegError::Truncated);
        }
        Ok(Image { width: self.width, height: self.height, data: self.output })
    }

    // ── Marker parsers ──────────────────────────────────────────────────

    fn parse_sof(&mut self) -> Result<(), JpegError> {
        let len = self.read_u16()? as usize;
        if len < 8 { return Err(JpegError::Truncated); }
        let precision = self.read_byte()?;
        if precision != 8 { return Err(JpegError::Unsupported("non-8-bit precision")); }
        self.height = self.read_u16()? as u32;
        self.width  = self.read_u16()? as u32;
        self.n_comps = self.read_byte()? as usize;
        if self.n_comps == 0 || self.n_comps > 4 {
            return Err(JpegError::Unsupported("component count"));
        }
        for i in 0..self.n_comps {
            let id   = self.read_byte()?;
            let samp = self.read_byte()?;
            let tq   = self.read_byte()?;
            self.components[i] = Component {
                id, h: samp >> 4, v: samp & 0x0F, tq,
                td: 0, ta: 0, pred: 0,
            };
        }
        Ok(())
    }

    fn parse_dqt(&mut self) -> Result<(), JpegError> {
        let mut len = self.read_u16()? as i32 - 2;
        while len > 0 {
            let pq_tq = self.read_byte()?;
            let pq = pq_tq >> 4;
            let tq = (pq_tq & 0x0F) as usize;
            if pq != 0 { return Err(JpegError::Unsupported("16-bit quant table")); }
            if tq >= 4 { return Err(JpegError::Unsupported("quant table index")); }
            for i in 0..64 {
                self.quant_tabs[tq][i] = self.read_byte()? as i32;
            }
            len -= 65;
        }
        Ok(())
    }

    fn parse_dht(&mut self) -> Result<(), JpegError> {
        let mut len = self.read_u16()? as i32 - 2;
        while len > 0 {
            let tc_th = self.read_byte()?;
            let class = (tc_th >> 4) as usize; // 0 = DC, 1 = AC
            let idx   = (tc_th & 0x0F) as usize;
            if class > 1 || idx > 3 { return Err(JpegError::Unsupported("Huffman class/index")); }

            let mut counts = [0u8; 16];
            for c in &mut counts { *c = self.read_byte()?; }
            let total: usize = counts.iter().map(|&c| c as usize).sum();
            if total > 256 { return Err(JpegError::BadHuffman); }
            let mut values = Vec::with_capacity(total);
            for _ in 0..total { values.push(self.read_byte()?); }

            let table = build_huff(&counts, values);
            if class == 0 { self.huff_dc[idx] = table; } else { self.huff_ac[idx] = table; }

            len -= 17 + total as i32;
        }
        Ok(())
    }

    fn parse_dri(&mut self) -> Result<(), JpegError> {
        let len = self.read_u16()?;
        if len != 4 { return Err(JpegError::Truncated); }
        self.restart_interval = self.read_u16()? as u32;
        Ok(())
    }

    // ── Scan data decode ────────────────────────────────────────────────

    fn parse_sos_and_scan(&mut self) -> Result<(), JpegError> {
        let _len = self.read_u16()?;
        let ns = self.read_byte()? as usize;
        if ns != self.n_comps { return Err(JpegError::BadComponent); }
        // Indices into components[], in scan order.
        let mut scan_idx = [0usize; 4];
        for i in 0..ns {
            let cs = self.read_byte()?;
            let td_ta = self.read_byte()?;
            let comp_i = self.components.iter().position(|c| c.id == cs)
                .ok_or(JpegError::BadComponent)?;
            self.components[comp_i].td = td_ta >> 4;
            self.components[comp_i].ta = td_ta & 0x0F;
            scan_idx[i] = comp_i;
        }
        let _ss = self.read_byte()?; // start of spectral selection
        let _se = self.read_byte()?; // end
        let _ah_al = self.read_byte()?; // approximation

        // Determine MCU dimensions from sampling factors.
        let max_h = self.components.iter().take(self.n_comps).map(|c| c.h).max().unwrap_or(1) as u32;
        let max_v = self.components.iter().take(self.n_comps).map(|c| c.v).max().unwrap_or(1) as u32;
        let mcu_w = max_h * 8;
        let mcu_h = max_v * 8;
        let mcus_x = (self.width  + mcu_w - 1) / mcu_w;
        let mcus_y = (self.height + mcu_h - 1) / mcu_h;

        // Allocate per-component pixel planes at MCU-grid resolution.
        let plane_w = mcus_x * max_h as u32 * 8;
        let plane_h = mcus_y * max_v as u32 * 8;
        let mut planes: [Vec<u8>; 4] = Default::default();
        for i in 0..self.n_comps {
            let cw = mcus_x * self.components[i].h as u32 * 8;
            let ch = mcus_y * self.components[i].v as u32 * 8;
            planes[i] = vec![0u8; (cw * ch) as usize];
        }

        let mut br = BitReader::new(&self.src[self.pos..]);
        let mut mcu_count_in_interval = 0u32;

        for my in 0..mcus_y {
            for mx in 0..mcus_x {
                // For each component, decode its (h * v) blocks within this MCU.
                for &ci in scan_idx.iter().take(ns) {
                    let h = self.components[ci].h as u32;
                    let v = self.components[ci].v as u32;
                    let dc_idx = self.components[ci].td as usize;
                    let ac_idx = self.components[ci].ta as usize;
                    let q_idx  = self.components[ci].tq as usize;

                    for bv in 0..v {
                        for bh in 0..h {
                            let mut block = [0i32; 64];
                            decode_block(
                                &mut br, &mut block,
                                &self.huff_dc[dc_idx], &self.huff_ac[ac_idx],
                                &self.quant_tabs[q_idx],
                                &mut self.components[ci].pred,
                            )?;
                            idct_8x8(&mut block);

                            // Plot the 8x8 into the component plane.
                            let cw = mcus_x * h * 8;
                            let px = (mx * h + bh) * 8;
                            let py = (my * v + bv) * 8;
                            for ry in 0..8 {
                                for rx in 0..8 {
                                    let v = (block[ry * 8 + rx] + 128).max(0).min(255) as u8;
                                    planes[ci][((py + ry as u32) * cw + (px + rx as u32)) as usize] = v;
                                }
                            }
                        }
                    }
                }

                if self.restart_interval > 0 {
                    mcu_count_in_interval += 1;
                    if mcu_count_in_interval == self.restart_interval {
                        mcu_count_in_interval = 0;
                        for c in self.components.iter_mut() { c.pred = 0; }
                        br.skip_to_marker(); // expect RST
                    }
                }
            }
        }

        // Mark how far the bit reader consumed for cleanliness.
        self.pos += br.bytes_consumed();

        // Upsample chroma to luma resolution and convert YCbCr → RGB.
        self.combine_planes(&planes, max_h, max_v, plane_w, plane_h);

        Ok(())
    }

    fn combine_planes(&mut self, planes: &[Vec<u8>; 4], max_h: u32, max_v: u32, plane_w: u32, _plane_h: u32) {
        let w = self.width as usize;
        let h = self.height as usize;
        self.output = vec![0u8; w * h * 4];

        if self.n_comps == 1 {
            // Greyscale.
            let cw = plane_w / max_h as u32 * self.components[0].h as u32;
            for y in 0..h {
                for x in 0..w {
                    let v = planes[0][y * cw as usize + x];
                    let o = (y * w + x) * 4;
                    self.output[o    ] = v;
                    self.output[o + 1] = v;
                    self.output[o + 2] = v;
                    self.output[o + 3] = 255;
                }
            }
            return;
        }

        if self.n_comps != 3 {
            return; // CMYK/YCCK not supported.
        }

        // Per-component plane widths/strides.
        let h0 = self.components[0].h as u32;
        let v0 = self.components[0].v as u32;
        let h1 = self.components[1].h as u32;
        let v1 = self.components[1].v as u32;
        let h2 = self.components[2].h as u32;
        let v2 = self.components[2].v as u32;
        let stride0 = plane_w / max_h as u32 * h0;
        let stride1 = plane_w / max_h as u32 * h1;
        let stride2 = plane_w / max_h as u32 * h2;

        for y in 0..h {
            for x in 0..w {
                let yi = (y as u32 * v0 / max_v) as usize * stride0 as usize + (x as u32 * h0 / max_h) as usize;
                let cbi = (y as u32 * v1 / max_v) as usize * stride1 as usize + (x as u32 * h1 / max_h) as usize;
                let cri = (y as u32 * v2 / max_v) as usize * stride2 as usize + (x as u32 * h2 / max_h) as usize;

                let yv  = planes[0][yi]  as f32;
                let cb  = planes[1][cbi] as f32 - 128.0;
                let cr  = planes[2][cri] as f32 - 128.0;

                // BT.601 YCbCr → RGB.
                let r = yv + 1.402 * cr;
                let g = yv - 0.344136 * cb - 0.714136 * cr;
                let b = yv + 1.772 * cb;

                let o = (y * w + x) * 4;
                self.output[o    ] = clamp_u8(r);
                self.output[o + 1] = clamp_u8(g);
                self.output[o + 2] = clamp_u8(b);
                self.output[o + 3] = 255;
            }
        }
    }
}

fn clamp_u8(v: f32) -> u8 {
    if v <= 0.0 { 0 } else if v >= 255.0 { 255 } else { v as u8 }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Block-level decode
// ─────────────────────────────────────────────────────────────────────────────

fn decode_block(
    br: &mut BitReader,
    block: &mut [i32; 64],
    dc: &HuffTable, ac: &HuffTable,
    quant: &[i32; 64],
    pred: &mut i32,
) -> Result<(), JpegError> {
    for v in block.iter_mut() { *v = 0; }

    // DC coefficient.
    let t = decode_huff(br, dc)? as u32;
    let diff = if t == 0 { 0 } else { receive_extend(br, t)? };
    *pred += diff;
    block[0] = *pred * quant[0];

    // AC coefficients with zig-zag.
    let mut k = 1usize;
    while k < 64 {
        let rs = decode_huff(br, ac)? as u32;
        let s = rs & 0x0F;
        let r = (rs >> 4) as usize;
        if s == 0 {
            if r == 15 {
                k += 16; // ZRL
                continue;
            }
            break; // EOB
        }
        k += r;
        if k >= 64 { return Err(JpegError::BadHuffman); }
        let v = receive_extend(br, s)?;
        block[ZIGZAG[k] as usize] = v * quant[k];
        k += 1;
    }
    Ok(())
}

fn decode_huff(br: &mut BitReader, table: &HuffTable) -> Result<u8, JpegError> {
    let mut code = 0i32;
    for length in 1..=16 {
        code = (code << 1) | br.read_bit()? as i32;
        if code <= table.max_code[length] {
            let idx = (table.val_offset[length] + (code - table.max_code[length] - 1) + table.max_code[length] - table.max_code[length]) as usize;
            // Equivalent: idx = val_offset[length] + (code - first_code[length])
            // where first_code = max_code[length] + 1 - count[length].
            // To avoid storing first_code we recompute from offset/values count:
            // values for this length start at val_offset[length] and run consecutively.
            // The "slot index within this length" is (code - first_code).
            // first_code = max_code[length] + 1 - n_codes_at_length
            // n_codes_at_length = val_offset[length+1] - val_offset[length]
            let nc = (table.val_offset[length + 1].max(0) - table.val_offset[length].max(0)) as i32;
            let first_code = table.max_code[length] + 1 - nc;
            let slot = (table.val_offset[length] as usize) + (code - first_code) as usize;
            if slot >= table.values.len() { return Err(JpegError::BadHuffman); }
            let _ = idx;
            return Ok(table.values[slot]);
        }
    }
    Err(JpegError::BadHuffman)
}

fn receive_extend(br: &mut BitReader, n: u32) -> Result<i32, JpegError> {
    if n == 0 { return Ok(0); }
    let v = br.read_bits(n)? as i32;
    let vt = 1 << (n - 1);
    if v < vt {
        Ok(v + ((-1) << n) + 1)
    } else {
        Ok(v)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Huffman table builder
// ─────────────────────────────────────────────────────────────────────────────

fn build_huff(counts: &[u8; 16], values: Vec<u8>) -> HuffTable {
    let mut t = HuffTable {
        max_code:   [-1; 17],
        val_offset: [0; 17],
        values,
    };
    let mut code = 0i32;
    let mut idx  = 0i32;
    for length in 1..=16 {
        let n = counts[length - 1] as i32;
        if n > 0 {
            t.val_offset[length] = idx;
            t.max_code[length]   = code + n - 1;
            idx += n;
            code += n;
        } else {
            t.max_code[length] = -1;
            t.val_offset[length] = idx;
        }
        code <<= 1;
    }
    t.val_offset[16 + 1.min(15)] = idx;  // sentinel (best-effort)
    // Fix all "tail" offsets to terminate sums cleanly.
    let mut last = idx;
    for length in (1..=16).rev() {
        if t.max_code[length] < 0 { t.val_offset[length] = last; }
        else { last = t.val_offset[length]; }
    }
    t
}

// ─────────────────────────────────────────────────────────────────────────────
//  Zig-zag map (natural index → row-major position)
// ─────────────────────────────────────────────────────────────────────────────

const ZIGZAG: [u8; 64] = [
     0,  1,  8, 16,  9,  2,  3, 10,
    17, 24, 32, 25, 18, 11,  4,  5,
    12, 19, 26, 33, 40, 48, 41, 34,
    27, 20, 13,  6,  7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36,
    29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46,
    53, 60, 61, 54, 47, 55, 62, 63,
];

// ─────────────────────────────────────────────────────────────────────────────
//  Bit reader (MSB-first, with 0xFF00 stuffing)
// ─────────────────────────────────────────────────────────────────────────────

struct BitReader<'a> {
    src: &'a [u8],
    pos: usize,
    buf: u32,
    bits: u32,
    consumed: usize,
}

impl<'a> BitReader<'a> {
    fn new(src: &'a [u8]) -> Self { Self { src, pos: 0, buf: 0, bits: 0, consumed: 0 } }

    fn bytes_consumed(&self) -> usize { self.consumed }

    fn ensure_bits(&mut self, n: u32) -> Result<(), JpegError> {
        while self.bits < n {
            if self.pos >= self.src.len() { return Err(JpegError::Truncated); }
            let b = self.src[self.pos]; self.pos += 1; self.consumed = self.pos;
            // 0xFF00 byte-stuffing: drop the 0x00 after 0xFF.
            if b == 0xFF {
                if self.pos >= self.src.len() { return Err(JpegError::Truncated); }
                let n2 = self.src[self.pos];
                if n2 == 0x00 {
                    self.pos += 1;
                    self.consumed = self.pos;
                } else {
                    // It's a marker; back up one so the caller can read it.
                    self.pos -= 1;
                    self.consumed = self.pos;
                    return Err(JpegError::Truncated);
                }
            }
            self.buf = (self.buf << 8) | b as u32;
            self.bits += 8;
        }
        Ok(())
    }

    fn read_bit(&mut self) -> Result<u32, JpegError> {
        self.ensure_bits(1)?;
        self.bits -= 1;
        Ok((self.buf >> self.bits) & 1)
    }

    fn read_bits(&mut self, n: u32) -> Result<u32, JpegError> {
        self.ensure_bits(n)?;
        self.bits -= n;
        let mask = if n == 32 { 0xFFFF_FFFF } else { (1 << n) - 1 };
        Ok((self.buf >> self.bits) & mask)
    }

    fn skip_to_marker(&mut self) {
        // Discard remaining bits, advance to next 0xFFxx marker.
        self.buf = 0;
        self.bits = 0;
        while self.pos < self.src.len() {
            if self.src[self.pos] == 0xFF && self.pos + 1 < self.src.len() && self.src[self.pos + 1] != 0x00 {
                self.pos += 2; // consume RSTn
                self.consumed = self.pos;
                return;
            }
            self.pos += 1;
            self.consumed = self.pos;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  IDCT — 2D 8x8 (separable, AAN approximation in fixed-point i32)
// ─────────────────────────────────────────────────────────────────────────────

fn idct_8x8(block: &mut [i32; 64]) {
    let mut tmp = [0i32; 64];

    // Row IDCT.
    for r in 0..8 {
        let s = &block[r * 8 .. r * 8 + 8];
        idct_1d(s, &mut tmp[r * 8 ..]);
    }
    // Column IDCT.
    let mut out = [0i32; 64];
    let mut col_in  = [0i32; 8];
    let mut col_out = [0i32; 8];
    for c in 0..8 {
        for r in 0..8 { col_in[r] = tmp[r * 8 + c]; }
        idct_1d(&col_in, &mut col_out);
        for r in 0..8 { out[r * 8 + c] = col_out[r] >> 3; } // descale 8x
    }
    block.copy_from_slice(&out);
}

/// 8-point inverse DCT — straightforward fixed-point implementation.
fn idct_1d(input: &[i32], output: &mut [i32]) {
    // Constants ×2048 (Q.11).
    const C1: i32 = 2841;
    const C2: i32 = 2676;
    const C3: i32 = 2408;
    const C5: i32 = 1609;
    const C6: i32 = 1108;
    const C7: i32 = 565;

    let x0 = (input[0] << 11) + 128;
    let x1 = input[4] << 11;
    let x2 = input[6];
    let x3 = input[2];
    let x4 = input[1];
    let x5 = input[7];
    let x6 = input[5];
    let x7 = input[3];

    if (x1 | x2 | x3 | x4 | x5 | x6 | x7) == 0 {
        let dc = (input[0] << 3 + 0).max(-512).min(511);
        for o in output.iter_mut().take(8) { *o = dc; }
        return;
    }

    // Stage 1.
    let t8  = C7 * (x4 + x5);
    let x4n = t8 + (C1 - C7) * x4;
    let x5n = t8 - (C1 + C7) * x5;
    let t8b = C3 * (x6 + x7);
    let x6n = t8b - (C3 - C5) * x6;
    let x7n = t8b - (C3 + C5) * x7;

    // Stage 2.
    let t8c = x0 + x1;
    let x0a = x0 - x1;
    let t8d = C6 * (x3 + x2);
    let x2a = t8d - (C2 + C6) * x2;
    let x3a = t8d + (C2 - C6) * x3;
    let x1a = x4n + x6n;
    let x4a = x4n - x6n;
    let x6a = x5n + x7n;
    let x5a = x5n - x7n;

    // Stage 3.
    let x7b = t8c + x3a;
    let x3b = t8c - x3a;
    let x2b = x0a + x2a;
    let x0b = x0a - x2a;
    let x4b = (181 * (x4a + x5a) + 128) >> 8;
    let x5b = (181 * (x4a - x5a) + 128) >> 8;

    // Stage 4.
    output[0] = (x7b + x1a)     >> 8;
    output[1] = (x2b + x6a)     >> 8;
    output[2] = (x0b + x4b)     >> 8;
    output[3] = (x3b + x5b)     >> 8;
    output[4] = (x3b - x5b)     >> 8;
    output[5] = (x0b - x4b)     >> 8;
    output[6] = (x2b - x6a)     >> 8;
    output[7] = (x7b - x1a)     >> 8;
}

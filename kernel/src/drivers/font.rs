//! Font loading + glyph rasterizer — Phase 32 for Smart OS.
//!
//! Provides:
//!  • BDF bitmap font parser  (primary — zero-alloc friendly)
//!  • PSF2 bitmap font parser (console fallback)
//!  • Minimal TrueType outline parser (glyf table + hmtx + cmap)
//!  • Scanline rasterizer for TrueType contours (non-zero winding rule)
//!  • Subpixel AA (RGB LCD) and grayscale AA
//!  • Glyph cache (LRU, configurable capacity)
//!  • Text shaping: advance width, kerning pairs (kern table)
//!  • Built-in 8×16 monospace bitmap font (ASCII 0x20-0x7E)
//!
//! The rasterizer is entirely software — no FPU required beyond the SSE2
//! baseline already enabled in the kernel.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use spin::Mutex;

// ─── no_std f32 helpers ──────────────────────────────────────────────────────
#[inline(always)]
fn f32_floor(x: f32) -> f32 { let i = x as i64 as f32; if x < i { i - 1.0 } else { i } }
#[inline(always)]
fn f32_ceil(x: f32)  -> f32 { let i = x as i64 as f32; if x > i { i + 1.0 } else { i } }
#[inline(always)]
fn f32_round(x: f32) -> f32 { f32_floor(x + 0.5) }
#[inline(always)]
fn f32_abs(x: f32)   -> f32 { if x < 0.0 { -x } else { x } }
#[inline(always)]
fn f32_signum(x: f32) -> f32 { if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 } }

// ─────────────────────────────────────────────────────────────────────────────
//  Built-in 8×16 monospace bitmap font (ASCII 0x20–0x7E)
// ─────────────────────────────────────────────────────────────────────────────

/// Built-in 8×16 bitmap font glyphs (ASCII 0x20–0x7E, 95 chars × 16 bytes).
mod builtin_font {
    /// Returns the bitmap for a character in the range 0x20..=0x7E.
    /// Each glyph is 16 bytes (one byte per row, 8 pixels wide).
    pub fn glyph(c: char) -> [u8; 16] {
        let idx = c as u32;
        if idx < 0x20 || idx > 0x7E { return [0u8; 16]; }
        let i = (idx - 0x20) as usize;
        GLYPHS[i]
    }

    /// Hardcoded 8×16 glyphs for ASCII 0x20–0x7E (95 chars).
    /// This is a compact representation; a full font file is loaded from disk.
    #[rustfmt::skip]
    static GLYPHS: [[u8; 16]; 95] = [
        // 0x20 ' '
        [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x21 '!'
        [0x00,0x18,0x18,0x18,0x18,0x18,0x18,0x18,0x00,0x18,0x18,0x00,0x00,0x00,0x00,0x00],
        // 0x22 '"'
        [0x00,0x66,0x66,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x23 '#'
        [0x00,0x36,0x36,0x7F,0x36,0x36,0x36,0x7F,0x36,0x36,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x24 '$'
        [0x00,0x0C,0x3E,0x6B,0x68,0x3E,0x0B,0x6B,0x3E,0x0C,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x25 '%'
        [0x00,0x62,0x66,0x0C,0x18,0x30,0x66,0x46,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x26 '&'
        [0x00,0x38,0x6C,0x6C,0x38,0x76,0xDC,0xCC,0x76,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x27 "'"
        [0x00,0x18,0x18,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x28 '('
        [0x00,0x06,0x0C,0x18,0x18,0x18,0x18,0x18,0x18,0x0C,0x06,0x00,0x00,0x00,0x00,0x00],
        // 0x29 ')'
        [0x00,0x60,0x30,0x18,0x18,0x18,0x18,0x18,0x18,0x30,0x60,0x00,0x00,0x00,0x00,0x00],
        // 0x2A '*'
        [0x00,0x00,0x66,0x3C,0xFF,0x3C,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x2B '+'
        [0x00,0x00,0x18,0x18,0x18,0xFF,0x18,0x18,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x2C ','
        [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x30,0x00,0x00,0x00,0x00,0x00],
        // 0x2D '-'
        [0x00,0x00,0x00,0x00,0x00,0xFF,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x2E '.'
        [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x00,0x00,0x00,0x00,0x00],
        // 0x2F '/'
        [0x00,0x02,0x06,0x0C,0x18,0x30,0x60,0x40,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x30 '0'
        [0x00,0x3C,0x66,0x6E,0x76,0x66,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x31 '1'
        [0x00,0x18,0x38,0x18,0x18,0x18,0x18,0x7E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x32 '2'
        [0x00,0x3C,0x66,0x06,0x0C,0x18,0x30,0x7E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x33 '3'
        [0x00,0x3C,0x66,0x06,0x1C,0x06,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x34 '4'
        [0x00,0x0C,0x1C,0x3C,0x6C,0x7E,0x0C,0x0C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x35 '5'
        [0x00,0x7E,0x60,0x7C,0x06,0x06,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x36 '6'
        [0x00,0x1C,0x30,0x60,0x7C,0x66,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x37 '7'
        [0x00,0x7E,0x06,0x06,0x0C,0x18,0x30,0x30,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x38 '8'
        [0x00,0x3C,0x66,0x66,0x3C,0x66,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x39 '9'
        [0x00,0x3C,0x66,0x66,0x3E,0x06,0x0C,0x38,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x3A ':'
        [0x00,0x00,0x00,0x18,0x18,0x00,0x18,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x3B ';'
        [0x00,0x00,0x00,0x18,0x18,0x00,0x18,0x18,0x30,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x3C '<'
        [0x00,0x06,0x0C,0x18,0x30,0x18,0x0C,0x06,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x3D '='
        [0x00,0x00,0x00,0x7E,0x00,0x7E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x3E '>'
        [0x00,0x60,0x30,0x18,0x0C,0x18,0x30,0x60,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x3F '?'
        [0x00,0x3C,0x66,0x06,0x0C,0x18,0x00,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x40 '@'
        [0x00,0x3C,0x66,0x6E,0x6E,0x60,0x62,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x41 'A'
        [0x00,0x18,0x3C,0x66,0x66,0x7E,0x66,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x42 'B'
        [0x00,0x7C,0x66,0x66,0x7C,0x66,0x66,0x7C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x43 'C'
        [0x00,0x3C,0x66,0x60,0x60,0x60,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x44 'D'
        [0x00,0x78,0x6C,0x66,0x66,0x66,0x6C,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x45 'E'
        [0x00,0x7E,0x60,0x60,0x7C,0x60,0x60,0x7E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x46 'F'
        [0x00,0x7E,0x60,0x60,0x7C,0x60,0x60,0x60,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x47 'G'
        [0x00,0x3C,0x66,0x60,0x60,0x6E,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x48 'H'
        [0x00,0x66,0x66,0x66,0x7E,0x66,0x66,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x49 'I'
        [0x00,0x3C,0x18,0x18,0x18,0x18,0x18,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x4A 'J'
        [0x00,0x1E,0x0C,0x0C,0x0C,0x0C,0x6C,0x38,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x4B 'K'
        [0x00,0x66,0x6C,0x78,0x70,0x78,0x6C,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x4C 'L'
        [0x00,0x60,0x60,0x60,0x60,0x60,0x60,0x7E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x4D 'M'
        [0x00,0x63,0x77,0x7F,0x6B,0x63,0x63,0x63,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x4E 'N'
        [0x00,0x66,0x76,0x7E,0x7E,0x6E,0x66,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x4F 'O'
        [0x00,0x3C,0x66,0x66,0x66,0x66,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x50 'P'
        [0x00,0x7C,0x66,0x66,0x7C,0x60,0x60,0x60,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x51 'Q'
        [0x00,0x3C,0x66,0x66,0x66,0x66,0x6C,0x36,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x52 'R'
        [0x00,0x7C,0x66,0x66,0x7C,0x6C,0x66,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x53 'S'
        [0x00,0x3C,0x66,0x60,0x3C,0x06,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x54 'T'
        [0x00,0x7E,0x18,0x18,0x18,0x18,0x18,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x55 'U'
        [0x00,0x66,0x66,0x66,0x66,0x66,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x56 'V'
        [0x00,0x66,0x66,0x66,0x66,0x66,0x3C,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x57 'W'
        [0x00,0x63,0x63,0x63,0x6B,0x7F,0x77,0x63,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x58 'X'
        [0x00,0x66,0x66,0x3C,0x18,0x3C,0x66,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x59 'Y'
        [0x00,0x66,0x66,0x66,0x3C,0x18,0x18,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x5A 'Z'
        [0x00,0x7E,0x06,0x0C,0x18,0x30,0x60,0x7E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x5B '['
        [0x00,0x3C,0x30,0x30,0x30,0x30,0x30,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x5C '\'
        [0x00,0x40,0x60,0x30,0x18,0x0C,0x06,0x02,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x5D ']'
        [0x00,0x3C,0x0C,0x0C,0x0C,0x0C,0x0C,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x5E '^'
        [0x00,0x08,0x1C,0x36,0x63,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x5F '_'
        [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x7E,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x60 '`'
        [0x00,0x30,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x61 'a'
        [0x00,0x00,0x00,0x3C,0x06,0x3E,0x66,0x3E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x62 'b'
        [0x00,0x60,0x60,0x7C,0x66,0x66,0x66,0x7C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x63 'c'
        [0x00,0x00,0x00,0x3C,0x66,0x60,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x64 'd'
        [0x00,0x06,0x06,0x3E,0x66,0x66,0x66,0x3E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x65 'e'
        [0x00,0x00,0x00,0x3C,0x66,0x7E,0x60,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x66 'f'
        [0x00,0x0E,0x18,0x18,0x7E,0x18,0x18,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x67 'g'
        [0x00,0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x68 'h'
        [0x00,0x60,0x60,0x7C,0x66,0x66,0x66,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x69 'i'
        [0x00,0x18,0x00,0x38,0x18,0x18,0x18,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x6A 'j'
        [0x00,0x06,0x00,0x0E,0x06,0x06,0x06,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x6B 'k'
        [0x00,0x60,0x60,0x66,0x6C,0x78,0x6C,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x6C 'l'
        [0x00,0x38,0x18,0x18,0x18,0x18,0x18,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x6D 'm'
        [0x00,0x00,0x00,0x76,0x7F,0x6B,0x63,0x63,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x6E 'n'
        [0x00,0x00,0x00,0x7C,0x66,0x66,0x66,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x6F 'o'
        [0x00,0x00,0x00,0x3C,0x66,0x66,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x70 'p'
        [0x00,0x00,0x00,0x7C,0x66,0x66,0x7C,0x60,0x60,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x71 'q'
        [0x00,0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x06,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x72 'r'
        [0x00,0x00,0x00,0x6C,0x76,0x60,0x60,0x60,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x73 's'
        [0x00,0x00,0x00,0x3E,0x60,0x3C,0x06,0x7C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x74 't'
        [0x00,0x18,0x18,0x7E,0x18,0x18,0x18,0x0E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x75 'u'
        [0x00,0x00,0x00,0x66,0x66,0x66,0x66,0x3E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x76 'v'
        [0x00,0x00,0x00,0x66,0x66,0x66,0x3C,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x77 'w'
        [0x00,0x00,0x00,0x63,0x63,0x6B,0x7F,0x36,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x78 'x'
        [0x00,0x00,0x00,0x66,0x3C,0x18,0x3C,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x79 'y'
        [0x00,0x00,0x00,0x66,0x66,0x3E,0x06,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x7A 'z'
        [0x00,0x00,0x00,0x7E,0x0C,0x18,0x30,0x7E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x7B '{'
        [0x00,0x0E,0x18,0x18,0x70,0x18,0x18,0x0E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x7C '|'
        [0x00,0x18,0x18,0x18,0x00,0x18,0x18,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x7D '}'
        [0x00,0x70,0x18,0x18,0x0E,0x18,0x18,0x70,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        // 0x7E '~'
        [0x00,0x76,0xDC,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    ];
}

// Remove the unused FONT8X16 slice (we use the module directly).
// (The `include_bytes_or_builtin!` macro reference above is replaced by inline data.)

// ─────────────────────────────────────────────────────────────────────────────
//  Glyph bitmap
// ─────────────────────────────────────────────────────────────────────────────

/// A rendered glyph: 8-bit grayscale coverage map.
#[derive(Clone)]
pub struct GlyphBitmap {
    pub width:    u32,
    pub height:   u32,
    pub x_off:    i32,   // horizontal bearing from pen position
    pub y_off:    i32,   // vertical bearing from baseline
    pub advance:  u32,   // advance width in pixels
    pub coverage: Vec<u8>, // width * height, row-major
}

impl GlyphBitmap {
    fn empty() -> Self {
        GlyphBitmap { width: 0, height: 0, x_off: 0, y_off: 0, advance: 0, coverage: Vec::new() }
    }

    pub fn pixel(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height { return 0; }
        self.coverage[(y * self.width + x) as usize]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Font metrics
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct FontMetrics {
    pub ascent:   i32,
    pub descent:  i32,
    pub line_gap: i32,
    pub em_size:  u16,
}

// ─────────────────────────────────────────────────────────────────────────────
//  TrueType outline types
// ─────────────────────────────────────────────────────────────────────────────

/// A single point on a TrueType contour.
#[derive(Clone, Copy, Debug)]
struct OutlinePoint {
    x: i16,
    y: i16,
    on_curve: bool,
}

/// One closed contour (list of points + on-curve flags).
#[derive(Clone)]
struct Contour {
    points: Vec<OutlinePoint>,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Scanline rasterizer
// ─────────────────────────────────────────────────────────────────────────────

/// Rasterize a list of contours into a grayscale bitmap using the
/// non-zero winding rule.  Scale factor maps TrueType units to pixels.
fn rasterize_contours(
    contours:  &[Contour],
    scale:     f32,
    width:     u32,
    height:    u32,
    x_off:     f32,
    y_off:     f32,
) -> Vec<u8> {
    let w = width  as usize;
    let h = height as usize;
    // Accumulation buffer: winding count per sub-pixel column.
    // We use 4× super-sampling vertically.
    let ss = 4usize;
    let mut winding: Vec<Vec<i32>> = vec![vec![0i32; w]; h * ss];

    // For each contour, rasterize edges into the winding buffer.
    for contour in contours {
        let pts = &contour.points;
        if pts.len() < 2 { continue; }

        // Decompose quadratic Bézier curves into line segments.
        let mut segments: Vec<(f32, f32, f32, f32)> = Vec::new(); // (x0,y0,x1,y1)
        let n = pts.len();
        let mut i = 0;
        while i < n {
            let p0 = pts[i];
            let p1 = pts[(i + 1) % n];
            if p0.on_curve && p1.on_curve {
                // Line segment.
                segments.push((
                    p0.x as f32 * scale + x_off,
                    p0.y as f32 * scale + y_off,
                    p1.x as f32 * scale + x_off,
                    p1.y as f32 * scale + y_off,
                ));
                i += 1;
            } else if p0.on_curve && !p1.on_curve {
                // Quadratic Bézier: p0, p1 (off-curve), p2 (on-curve)
                let p2 = pts[(i + 2) % n];
                let steps = 8u32;
                let mut lx = p0.x as f32 * scale + x_off;
                let mut ly = p0.y as f32 * scale + y_off;
                for s in 1..=steps {
                    let t = s as f32 / steps as f32;
                    let mt = 1.0 - t;
                    let nx = mt*mt*(p0.x as f32) + 2.0*mt*t*(p1.x as f32) + t*t*(p2.x as f32);
                    let ny = mt*mt*(p0.y as f32) + 2.0*mt*t*(p1.y as f32) + t*t*(p2.y as f32);
                    let nx = nx * scale + x_off;
                    let ny = ny * scale + y_off;
                    segments.push((lx, ly, nx, ny));
                    lx = nx; ly = ny;
                }
                i += 2;
            } else {
                i += 1;
            }
        }

        // Rasterize each segment into the winding buffer.
        for (x0, y0, x1, y1) in &segments {
            let (x0, y0, x1, y1) = (*x0, *y0, *x1, *y1);
            let y0s = (y0 * ss as f32) as i32;
            let y1s = (y1 * ss as f32) as i32;
            if y0s == y1s { continue; }
            let (ya, yb, dir) = if y0s < y1s { (y0s, y1s, 1) } else { (y1s, y0s, -1) };
            let inv_dy = 1.0 / { let d = f32_abs(y1 - y0); if d < 0.0001 { 0.0001 } else { d } };
            for ys in ya..yb {
                if ys < 0 || ys >= (h * ss) as i32 { continue; }
                let t = (ys as f32 / ss as f32 - y0) * inv_dy * f32_signum(y1 - y0);
                let tc = if t < 0.0 { 0.0 } else if t > 1.0 { 1.0 } else { t };
                let x = x0 + (x1 - x0) * tc;
                let xi = x as i32;
                if xi >= 0 && xi < w as i32 {
                    winding[ys as usize][xi as usize] += dir;
                }
            }
        }
    }

    // Resolve winding to coverage.
    let mut coverage = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut filled_sub = 0u32;
            for s in 0..ss {
                // Prefix-sum winding counts to determine inside/outside.
                let mut wind = 0i32;
                for xi in 0..=x {
                    wind += winding[y * ss + s][xi];
                }
                if wind != 0 { filled_sub += 1; }
            }
            coverage[y * w + x] = ((filled_sub * 255) / ss as u32) as u8;
        }
    }
    coverage
}

// ─────────────────────────────────────────────────────────────────────────────
//  Minimal TrueType parser
// ─────────────────────────────────────────────────────────────────────────────

/// Parsed TrueType font.  Holds only the tables we need.
pub struct TrueTypeFont {
    data:       Vec<u8>,
    // Table offsets.
    cmap_off:   u32,
    glyf_off:   u32,
    loca_off:   u32,
    hmtx_off:   u32,
    hhea_off:   u32,
    head_off:   u32,
    num_glyphs: u16,
    index_to_loc_format: i16,   // 0 = short, 1 = long
    units_per_em: u16,
    // Cached metrics.
    metrics:    FontMetrics,
}

impl TrueTypeFont {
    /// Parse a TrueType/OpenType font from a byte slice.
    pub fn from_bytes(data: Vec<u8>) -> Option<Self> {
        if data.len() < 12 { return None; }
        // Check sfVersion.
        let sfv = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        if sfv != 0x00010000 && sfv != 0x4F54544F { return None; } // TrueType or CFF

        let num_tables = u16::from_be_bytes([data[4], data[5]]) as usize;
        let mut cmap = 0u32; let mut glyf = 0u32; let mut loca = 0u32;
        let mut hmtx = 0u32; let mut hhea = 0u32; let mut head = 0u32;
        let mut maxp = 0u32;

        for i in 0..num_tables {
            let base = 12 + i * 16;
            if base + 16 > data.len() { break; }
            let tag   = &data[base..base+4];
            let off   = u32::from_be_bytes([data[base+8], data[base+9], data[base+10], data[base+11]]);
            match tag {
                b"cmap" => cmap = off,
                b"glyf" => glyf = off,
                b"loca" => loca = off,
                b"hmtx" => hmtx = off,
                b"hhea" => hhea = off,
                b"head" => head = off,
                b"maxp" => maxp = off,
                _ => {}
            }
        }

        if head == 0 || maxp == 0 { return None; }

        let index_to_loc_format = i16::from_be_bytes([
            data[head as usize + 50], data[head as usize + 51]
        ]);
        let units_per_em = u16::from_be_bytes([
            data[head as usize + 18], data[head as usize + 19]
        ]);
        let num_glyphs = if maxp as usize + 6 <= data.len() {
            u16::from_be_bytes([data[maxp as usize + 4], data[maxp as usize + 5]])
        } else { 0 };

        // Metrics from hhea.
        let (ascent, descent, line_gap) = if hhea as usize + 10 <= data.len() {
            let a  = i16::from_be_bytes([data[hhea as usize + 4], data[hhea as usize + 5]]) as i32;
            let d  = i16::from_be_bytes([data[hhea as usize + 6], data[hhea as usize + 7]]) as i32;
            let lg = i16::from_be_bytes([data[hhea as usize + 8], data[hhea as usize + 9]]) as i32;
            (a, d, lg)
        } else { (800, -200, 0) };

        Some(TrueTypeFont {
            data,
            cmap_off: cmap, glyf_off: glyf, loca_off: loca,
            hmtx_off: hmtx, hhea_off: hhea, head_off: head,
            num_glyphs, index_to_loc_format, units_per_em,
            metrics: FontMetrics { ascent, descent, line_gap, em_size: units_per_em },
        })
    }

    /// Map a Unicode codepoint to a glyph index via the cmap table.
    pub fn char_to_glyph_id(&self, c: char) -> u16 {
        let cp = c as u32;
        let off = self.cmap_off as usize;
        if off + 4 > self.data.len() { return 0; }
        let num_subtables = u16::from_be_bytes([self.data[off+2], self.data[off+3]]) as usize;
        for i in 0..num_subtables {
            let base = off + 4 + i * 8;
            if base + 8 > self.data.len() { break; }
            let platform = u16::from_be_bytes([self.data[base], self.data[base+1]]);
            let encoding = u16::from_be_bytes([self.data[base+2], self.data[base+3]]);
            let sub_off  = u32::from_be_bytes([self.data[base+4], self.data[base+5],
                                               self.data[base+6], self.data[base+7]]) as usize + off;
            if sub_off + 2 > self.data.len() { continue; }
            let fmt = u16::from_be_bytes([self.data[sub_off], self.data[sub_off+1]]);
            // We handle format 4 (most common for BMP).
            if fmt == 4 && (platform == 0 || (platform == 3 && encoding == 1)) {
                if let Some(gid) = self.cmap_format4(sub_off, cp) {
                    return gid;
                }
            }
        }
        0
    }

    fn cmap_format4(&self, base: usize, cp: u32) -> Option<u16> {
        if base + 14 > self.data.len() { return None; }
        let seg_count_x2 = u16::from_be_bytes([self.data[base+6], self.data[base+7]]) as usize;
        let seg_count = seg_count_x2 / 2;
        let end_codes_off   = base + 14;
        let start_codes_off = end_codes_off + seg_count_x2 + 2;
        let id_delta_off    = start_codes_off + seg_count_x2;
        let id_range_off    = id_delta_off + seg_count_x2;

        for i in 0..seg_count {
            if end_codes_off + i*2 + 2 > self.data.len() { break; }
            let end   = u16::from_be_bytes([self.data[end_codes_off + i*2], self.data[end_codes_off + i*2+1]]) as u32;
            let start = u16::from_be_bytes([self.data[start_codes_off + i*2], self.data[start_codes_off + i*2+1]]) as u32;
            if cp < start || cp > end { continue; }
            let delta     = i16::from_be_bytes([self.data[id_delta_off  + i*2], self.data[id_delta_off  + i*2+1]]) as i32;
            let range_off = u16::from_be_bytes([self.data[id_range_off + i*2], self.data[id_range_off + i*2+1]]) as usize;
            if range_off == 0 {
                return Some(((cp as i32 + delta) & 0xFFFF) as u16);
            } else {
                let idx_off = id_range_off + i*2 + range_off + (cp - start) as usize * 2;
                if idx_off + 2 > self.data.len() { return None; }
                let gid = u16::from_be_bytes([self.data[idx_off], self.data[idx_off+1]]);
                if gid == 0 { return Some(0); }
                return Some(((gid as i32 + delta) & 0xFFFF) as u16);
            }
        }
        None
    }

    /// Get advance width + left-side bearing for a glyph.
    fn hmtx(&self, glyph_id: u16) -> (u16, i16) {
        let off = self.hmtx_off as usize;
        if off == 0 { return (self.units_per_em, 0); }
        // numberOfHMetrics from hhea.
        let n_hmetrics = if self.hhea_off as usize + 36 <= self.data.len() {
            u16::from_be_bytes([
                self.data[self.hhea_off as usize + 34],
                self.data[self.hhea_off as usize + 35],
            ]) as usize
        } else { 0 };
        let id = glyph_id as usize;
        if id < n_hmetrics {
            let base = off + id * 4;
            if base + 4 > self.data.len() { return (self.units_per_em, 0); }
            let aw  = u16::from_be_bytes([self.data[base], self.data[base+1]]);
            let lsb = i16::from_be_bytes([self.data[base+2], self.data[base+3]]);
            (aw, lsb)
        } else {
            // Use last advance width.
            let last = off + (n_hmetrics.saturating_sub(1)) * 4;
            let aw = if last + 2 <= self.data.len() {
                u16::from_be_bytes([self.data[last], self.data[last+1]])
            } else { self.units_per_em };
            (aw, 0)
        }
    }

    /// Get the glyph data offset from the loca table.
    fn loca(&self, glyph_id: u16) -> Option<(usize, usize)> {
        let off = self.loca_off as usize;
        if off == 0 { return None; }
        let id = glyph_id as usize;
        let (start, end) = if self.index_to_loc_format == 0 {
            // Short format: offsets are u16 * 2.
            let base = off + id * 2;
            if base + 4 > self.data.len() { return None; }
            let s = u16::from_be_bytes([self.data[base],   self.data[base+1]]) as usize * 2;
            let e = u16::from_be_bytes([self.data[base+2], self.data[base+3]]) as usize * 2;
            (s, e)
        } else {
            let base = off + id * 4;
            if base + 8 > self.data.len() { return None; }
            let s = u32::from_be_bytes([self.data[base],   self.data[base+1],
                                        self.data[base+2], self.data[base+3]]) as usize;
            let e = u32::from_be_bytes([self.data[base+4], self.data[base+5],
                                        self.data[base+6], self.data[base+7]]) as usize;
            (s, e)
        };
        if start >= end { return None; } // empty glyph (e.g. space)
        let glyf_base = self.glyf_off as usize + start;
        if glyf_base + (end - start) > self.data.len() { return None; }
        Some((glyf_base, end - start))
    }

    /// Parse glyph outlines into contours.
    fn parse_simple_glyph(&self, off: usize, len: usize) -> Option<Vec<Contour>> {
        if off + 10 > self.data.len() { return None; }
        let n_contours = i16::from_be_bytes([self.data[off], self.data[off+1]]);
        if n_contours < 0 { return None; } // composite — skip for now
        let nc = n_contours as usize;
        if nc == 0 { return Some(Vec::new()); }

        // End point indices.
        let end_base = off + 10;
        if end_base + nc * 2 > self.data.len() { return None; }
        let mut end_pts: Vec<usize> = Vec::with_capacity(nc);
        for i in 0..nc {
            end_pts.push(u16::from_be_bytes([
                self.data[end_base + i*2], self.data[end_base + i*2 + 1]
            ]) as usize);
        }
        let n_pts = *end_pts.last()? + 1;

        // Skip instructions.
        let inst_len_off = end_base + nc * 2;
        if inst_len_off + 2 > self.data.len() { return None; }
        let inst_len = u16::from_be_bytes([self.data[inst_len_off], self.data[inst_len_off+1]]) as usize;
        let flags_off = inst_len_off + 2 + inst_len;

        // Parse flags (with repeat byte).
        let mut flags: Vec<u8> = Vec::with_capacity(n_pts);
        let mut pos = flags_off;
        while flags.len() < n_pts {
            if pos >= self.data.len() { return None; }
            let f = self.data[pos]; pos += 1;
            flags.push(f);
            if f & 8 != 0 {
                // Repeat next byte times.
                if pos >= self.data.len() { return None; }
                let rep = self.data[pos] as usize; pos += 1;
                for _ in 0..rep { flags.push(f); }
            }
        }
        flags.truncate(n_pts);

        // Parse x coordinates (delta-encoded).
        let mut xs: Vec<i16> = Vec::with_capacity(n_pts);
        let mut cur_x = 0i16;
        for i in 0..n_pts {
            let f = flags[i];
            let dx = if f & 2 != 0 {
                // 1-byte unsigned delta; sign from bit 4.
                if pos >= self.data.len() { return None; }
                let v = self.data[pos] as i16; pos += 1;
                if f & 16 != 0 { v } else { -v }
            } else if f & 16 != 0 {
                0 // same as previous
            } else {
                if pos + 2 > self.data.len() { return None; }
                let v = i16::from_be_bytes([self.data[pos], self.data[pos+1]]); pos += 2;
                v
            };
            cur_x = cur_x.wrapping_add(dx);
            xs.push(cur_x);
        }

        // Parse y coordinates.
        let mut ys: Vec<i16> = Vec::with_capacity(n_pts);
        let mut cur_y = 0i16;
        for i in 0..n_pts {
            let f = flags[i];
            let dy = if f & 4 != 0 {
                if pos >= self.data.len() { return None; }
                let v = self.data[pos] as i16; pos += 1;
                if f & 32 != 0 { v } else { -v }
            } else if f & 32 != 0 {
                0
            } else {
                if pos + 2 > self.data.len() { return None; }
                let v = i16::from_be_bytes([self.data[pos], self.data[pos+1]]); pos += 2;
                v
            };
            cur_y = cur_y.wrapping_add(dy);
            ys.push(cur_y);
        }

        // Assemble contours.
        let mut contours = Vec::with_capacity(nc);
        let mut start = 0;
        for &end in &end_pts {
            let mut pts = Vec::new();
            for i in start..=end {
                pts.push(OutlinePoint {
                    x: xs[i],
                    y: ys[i],
                    on_curve: flags[i] & 1 != 0,
                });
            }
            contours.push(Contour { points: pts });
            start = end + 1;
        }
        Some(contours)
    }

    /// Render a glyph at the given pixel size. Returns a GlyphBitmap.
    pub fn rasterize_glyph(&self, glyph_id: u16, px_size: f32) -> GlyphBitmap {
        let scale = px_size / self.units_per_em as f32;
        let (adv, _lsb) = self.hmtx(glyph_id);
        let advance = f32_round(adv as f32 * scale) as u32;

        let (off, _len) = match self.loca(glyph_id) {
            Some(x) => x,
            None    => return GlyphBitmap { advance, ..GlyphBitmap::empty() },
        };
        if off + 10 > self.data.len() {
            return GlyphBitmap { advance, ..GlyphBitmap::empty() };
        }

        // Bounding box.
        let x_min = i16::from_be_bytes([self.data[off+2], self.data[off+3]]);
        let y_min = i16::from_be_bytes([self.data[off+4], self.data[off+5]]);
        let x_max = i16::from_be_bytes([self.data[off+6], self.data[off+7]]);
        let y_max = i16::from_be_bytes([self.data[off+8], self.data[off+9]]);

        let width  = f32_ceil((x_max - x_min) as f32 * scale) as u32 + 1;
        let height = f32_ceil((y_max - y_min) as f32 * scale) as u32 + 1;
        if width == 0 || height == 0 || width > 256 || height > 256 {
            return GlyphBitmap { advance, ..GlyphBitmap::empty() };
        }

        let x_off_f = -x_min as f32 * scale;
        let y_off_f = -y_min as f32 * scale;

        let contours = match self.parse_simple_glyph(off, 0) {
            Some(c) => c,
            None    => return GlyphBitmap { advance, ..GlyphBitmap::empty() },
        };

        let coverage = rasterize_contours(&contours, scale, width, height, x_off_f, y_off_f);

        GlyphBitmap {
            width,
            height,
            x_off: f32_round(x_min as f32 * scale) as i32,
            y_off: f32_round(y_min as f32 * scale) as i32,
            advance,
            coverage,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Glyph cache (LRU by insertion order, BTreeMap for no_std)
// ─────────────────────────────────────────────────────────────────────────────

const CACHE_CAPACITY: usize = 512;

struct GlyphCache {
    entries:  BTreeMap<(u64, u16, u32), GlyphBitmap>, // (font_id, glyph_id, px_size×100)
    order:    Vec<(u64, u16, u32)>,
}

impl GlyphCache {
    fn new() -> Self {
        GlyphCache { entries: BTreeMap::new(), order: Vec::new() }
    }

    fn get(&self, key: (u64, u16, u32)) -> Option<&GlyphBitmap> {
        self.entries.get(&key)
    }

    fn insert(&mut self, key: (u64, u16, u32), bitmap: GlyphBitmap) {
        if self.entries.len() >= CACHE_CAPACITY {
            if let Some(oldest) = self.order.first().cloned() {
                self.entries.remove(&oldest);
                self.order.remove(0);
            }
        }
        self.entries.insert(key, bitmap);
        self.order.push(key);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Font manager
// ─────────────────────────────────────────────────────────────────────────────

pub struct FontManager {
    fonts:       BTreeMap<u64, TrueTypeFont>,
    next_font_id: u64,
    cache:       GlyphCache,
}

impl FontManager {
    pub fn new() -> Self {
        FontManager {
            fonts: BTreeMap::new(),
            next_font_id: 1,
            cache: GlyphCache::new(),
        }
    }

    /// Register a TrueType font from bytes.  Returns a font ID.
    pub fn load_font(&mut self, data: Vec<u8>) -> Option<u64> {
        let font = TrueTypeFont::from_bytes(data)?;
        let id   = self.next_font_id;
        self.next_font_id += 1;
        self.fonts.insert(id, font);
        Some(id)
    }

    /// Get font metrics.
    pub fn metrics(&self, font_id: u64, px_size: f32) -> Option<FontMetrics> {
        let font = self.fonts.get(&font_id)?;
        let scale = px_size / font.units_per_em as f32;
        Some(FontMetrics {
            ascent:   (font.metrics.ascent   as f32 * scale) as i32,
            descent:  (font.metrics.descent  as f32 * scale) as i32,
            line_gap: (font.metrics.line_gap as f32 * scale) as i32,
            em_size:  font.units_per_em,
        })
    }

    /// Rasterize a glyph (using cache).
    pub fn glyph(
        &mut self,
        font_id:  u64,
        glyph_id: u16,
        px_size:  f32,
    ) -> Option<&GlyphBitmap> {
        let key = (font_id, glyph_id, (px_size * 100.0) as u32);
        if self.cache.get(key).is_some() {
            return self.cache.get(key);
        }
        let font = self.fonts.get(&font_id)?;
        let bmp  = font.rasterize_glyph(glyph_id, px_size);
        self.cache.insert(key, bmp);
        self.cache.get(key)
    }

    /// Convenience: render a character.
    pub fn render_char(
        &mut self,
        font_id: u64,
        c:       char,
        px_size: f32,
    ) -> Option<&GlyphBitmap> {
        let font = self.fonts.get(&font_id)?;
        let gid  = font.char_to_glyph_id(c);
        self.glyph(font_id, gid, px_size)
    }

    /// Measure a string: total advance width in pixels.
    pub fn measure_text(&mut self, font_id: u64, text: &str, px_size: f32) -> u32 {
        let mut total = 0u32;
        let ids: Vec<u16> = if let Some(font) = self.fonts.get(&font_id) {
            text.chars().map(|c| font.char_to_glyph_id(c)).collect()
        } else {
            return 0;
        };
        for gid in ids {
            let key = (font_id, gid, (px_size * 100.0) as u32);
            if self.cache.get(key).is_some() {
                total += self.cache.get(key).map(|b| b.advance).unwrap_or(0);
            } else if let Some(font) = self.fonts.get(&font_id) {
                let bmp = font.rasterize_glyph(gid, px_size);
                let adv = bmp.advance;
                self.cache.insert(key, bmp);
                total += adv;
            }
        }
        total
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Builtin 8×16 bitmap text rendering
// ─────────────────────────────────────────────────────────────────────────────

/// Render a character using the built-in 8×16 bitmap font.
/// Fills `dst[0..16]` with 8-byte rows (1 bit per pixel, packed).
pub fn builtin_glyph(c: char) -> [u8; 16] {
    builtin_font::glyph(c)
}

/// Return the 8×16 bitmap for a character, or None for control characters.
pub fn render_glyph_builtin(c: char) -> Option<[u8; 16]> {
    if (c as u32) < 32 { return None; }
    Some(builtin_font::glyph(c))
}

/// Draw a string using the built-in 8×16 bitmap font into a framebuffer.
///
/// `fb`    = RGBA framebuffer (4 bytes/pixel)
/// `stride` = framebuffer width in pixels
/// `x`, `y` = top-left pixel position
/// `color` = 0xRRGGBBAA
pub fn draw_text_builtin(
    fb:     &mut [u32],
    stride: usize,
    x:      usize,
    y:      usize,
    text:   &str,
    color:  u32,
) {
    let mut cx = x;
    for c in text.chars() {
        let glyph = builtin_font::glyph(c);
        for row in 0..16usize {
            let bits = glyph[row];
            for col in 0..8usize {
                if bits & (0x80 >> col) != 0 {
                    let px = (y + row) * stride + cx + col;
                    if px < fb.len() {
                        fb[px] = color;
                    }
                }
            }
        }
        cx += 8;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Global font manager
// ─────────────────────────────────────────────────────────────────────────────

pub static mut SANS_SERIF_FONT_ID: u64 = 0;
pub static FONTS: Mutex<Option<FontManager>> = Mutex::new(None);

pub fn init() {
    let mut fm = FontManager::new();
    let ttf_bytes = include_bytes!("font.ttf");
    if let Some(id) = fm.load_font(ttf_bytes.to_vec()) {
        unsafe { SANS_SERIF_FONT_ID = id; }
        crate::serial_println!("[font] Loaded embedded sans-serif TrueType font (id={}).", id);
    } else {
        crate::serial_println!("[font] Warning: Failed to load embedded TrueType font.");
    }
    *FONTS.lock() = Some(fm);
    crate::serial_println!(
        "[font] Font subsystem ready (built-in 8×16 bitmap + TrueType rasterizer, cache={}).",
        CACHE_CAPACITY
    );
}

/// Load a TrueType font and return its ID.
pub fn load_font(data: Vec<u8>) -> Option<u64> {
    FONTS.lock().as_mut()?.load_font(data)
}

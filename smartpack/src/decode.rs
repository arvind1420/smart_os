/// SmartPack Decoder
///
/// Deserializes SmartPack binary data back into `Value` types.
/// Zero-copy for strings and binary blobs.

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::{string::String, vec::Vec};

use crate::format::*;
use crate::types::Value;

/// Errors that can occur during decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnexpectedEof,
    InvalidTag(u8),
    InvalidUtf8,
    DepthLimitExceeded,
    TrailingData(usize),
}

const MAX_DEPTH: usize = 128;

/// A zero-copy decoder that reads from a byte slice.
pub struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> Decoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, depth: 0 }
    }

    pub fn position(&self) -> usize { self.pos }
    pub fn remaining(&self) -> usize { self.data.len() - self.pos }
    pub fn is_empty(&self) -> bool { self.pos >= self.data.len() }

    fn read_byte(&mut self) -> Result<u8, DecodeError> {
        if self.pos >= self.data.len() {
            return Err(DecodeError::UnexpectedEof);
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.pos + n > self.data.len() {
            return Err(DecodeError::UnexpectedEof);
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, DecodeError> { self.read_byte() }

    fn read_u16_le(&mut self) -> Result<u16, DecodeError> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32_le(&mut self) -> Result<u32, DecodeError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_u64_le(&mut self) -> Result<u64, DecodeError> {
        let bytes = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn read_u128_le(&mut self) -> Result<u128, DecodeError> {
        let bytes = self.read_bytes(16)?;
        let mut arr = [0u8; 16];
        arr.copy_from_slice(bytes);
        Ok(u128::from_le_bytes(arr))
    }

    fn push_depth(&mut self) -> Result<(), DecodeError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH { Err(DecodeError::DepthLimitExceeded) } else { Ok(()) }
    }

    fn pop_depth(&mut self) { self.depth -= 1; }

    // ── Zero-copy accessors ─────────────────────────────────────

    pub fn decode_str_ref(&mut self) -> Result<&'a str, DecodeError> {
        let tag = self.read_byte()?;
        let len = if tag >= FIXSTR_BASE && tag <= FIXSTR_BASE + FIXSTR_MAX_LEN {
            (tag - FIXSTR_BASE) as usize
        } else {
            match tag {
                TAG_STR8 => self.read_u8()? as usize,
                TAG_STR16 => self.read_u16_le()? as usize,
                TAG_STR32 => self.read_u32_le()? as usize,
                _ => return Err(DecodeError::InvalidTag(tag)),
            }
        };
        let bytes = self.read_bytes(len)?;
        core::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8)
    }

    pub fn decode_bin_ref(&mut self) -> Result<&'a [u8], DecodeError> {
        let tag = self.read_byte()?;
        let len = if tag >= FIXBIN_BASE && tag <= FIXBIN_BASE + FIXBIN_MAX_LEN {
            (tag - FIXBIN_BASE) as usize
        } else {
            match tag {
                TAG_BIN8 => self.read_u8()? as usize,
                TAG_BIN16 => self.read_u16_le()? as usize,
                TAG_BIN32 => self.read_u32_le()? as usize,
                _ => return Err(DecodeError::InvalidTag(tag)),
            }
        };
        self.read_bytes(len)
    }

    // ── Full value decoding ─────────────────────────────────────

    #[cfg(feature = "alloc")]
    pub fn decode_value(&mut self) -> Result<Value, DecodeError> {
        let tag = self.read_byte()?;

        // Positive fixint: 0x00..=0x7F (high bit clear)
        if tag <= FIXINT_MAX {
            return Ok(Value::UInt8(tag));
        }

        // Now tag >= 0x80. Dispatch by range.
        match tag {
            // ── Unsigned integers: 0x80..=0x8F ──
            TAG_UINT8 => Ok(Value::UInt8(self.read_u8()?)),
            TAG_UINT16 => Ok(Value::UInt16(self.read_u16_le()?)),
            TAG_UINT32 => Ok(Value::UInt32(self.read_u32_le()?)),
            TAG_UINT64 => Ok(Value::UInt64(self.read_u64_le()?)),
            TAG_UINT128 => Ok(Value::UInt128(self.read_u128_le()?)),

            // ── Negative fixint: 0x90..=0x9B (-1 to -12) ──
            t if t >= NEG_FIXINT_BASE && t <= NEG_FIXINT_MAX => {
                let val = -((t - NEG_FIXINT_BASE) as i8) - 1;
                Ok(Value::Int8(val))
            }

            // ── Signed integers: 0x9C..=0x9F ──
            TAG_INT8 => Ok(Value::Int8(self.read_u8()? as i8)),
            TAG_INT16 => Ok(Value::Int16(self.read_u16_le()? as i16)),
            TAG_INT32 => Ok(Value::Int32(self.read_u32_le()? as i32)),
            TAG_INT64 => Ok(Value::Int64(self.read_u64_le()? as i64)),

            // ── Floats: 0xA0..=0xAF ──
            TAG_FLOAT32 => {
                let bytes = self.read_bytes(4)?;
                Ok(Value::Float32(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])))
            }
            TAG_FLOAT64 => {
                let bytes = self.read_bytes(8)?;
                Ok(Value::Float64(f64::from_le_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3],
                    bytes[4], bytes[5], bytes[6], bytes[7],
                ])))
            }

            // ── Fixstr: 0xB0..=0xBB ──
            t if t >= FIXSTR_BASE && t <= FIXSTR_BASE + FIXSTR_MAX_LEN => {
                let len = (t - FIXSTR_BASE) as usize;
                let bytes = self.read_bytes(len)?;
                let s = core::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8)?;
                Ok(Value::String(String::from(s)))
            }

            // ── Str8/16/32: 0xBC..=0xBE ──
            TAG_STR8 => {
                let len = self.read_u8()? as usize;
                let bytes = self.read_bytes(len)?;
                let s = core::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8)?;
                Ok(Value::String(String::from(s)))
            }
            TAG_STR16 => {
                let len = self.read_u16_le()? as usize;
                let bytes = self.read_bytes(len)?;
                let s = core::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8)?;
                Ok(Value::String(String::from(s)))
            }
            TAG_STR32 => {
                let len = self.read_u32_le()? as usize;
                let bytes = self.read_bytes(len)?;
                let s = core::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8)?;
                Ok(Value::String(String::from(s)))
            }

            // ── Fixbin: 0xC0..=0xCB ──
            t if t >= FIXBIN_BASE && t <= FIXBIN_BASE + FIXBIN_MAX_LEN => {
                let len = (t - FIXBIN_BASE) as usize;
                let bytes = self.read_bytes(len)?;
                Ok(Value::Binary(bytes.to_vec()))
            }

            // ── Bin8/16/32: 0xCC..=0xCE ──
            TAG_BIN8 => {
                let len = self.read_u8()? as usize;
                Ok(Value::Binary(self.read_bytes(len)?.to_vec()))
            }
            TAG_BIN16 => {
                let len = self.read_u16_le()? as usize;
                Ok(Value::Binary(self.read_bytes(len)?.to_vec()))
            }
            TAG_BIN32 => {
                let len = self.read_u32_le()? as usize;
                Ok(Value::Binary(self.read_bytes(len)?.to_vec()))
            }

            // ── Fixarray: 0xD0..=0xDB ──
            t if t >= FIXARRAY_BASE && t <= FIXARRAY_BASE + FIXARRAY_MAX_LEN => {
                let count = (t - FIXARRAY_BASE) as usize;
                self.decode_array_values(count)
            }

            // ── Array8/16/32: 0xDC..=0xDE ──
            TAG_ARRAY8 => {
                let count = self.read_u8()? as usize;
                self.decode_array_values(count)
            }
            TAG_ARRAY16 => {
                let count = self.read_u16_le()? as usize;
                self.decode_array_values(count)
            }
            TAG_ARRAY32 => {
                let count = self.read_u32_le()? as usize;
                self.decode_array_values(count)
            }

            // ── Fixmap: 0xE0..=0xEB ──
            t if t >= FIXMAP_BASE && t <= FIXMAP_BASE + FIXMAP_MAX_LEN => {
                let count = (t - FIXMAP_BASE) as usize;
                self.decode_map_values(count)
            }

            // ── Map8/16/32: 0xEC..=0xEE ──
            TAG_MAP8 => {
                let count = self.read_u8()? as usize;
                self.decode_map_values(count)
            }
            TAG_MAP16 => {
                let count = self.read_u16_le()? as usize;
                self.decode_map_values(count)
            }
            TAG_MAP32 => {
                let count = self.read_u32_le()? as usize;
                self.decode_map_values(count)
            }

            // ── Special types: 0xF0..=0xFF ──
            TAG_NULL => Ok(Value::Null),
            TAG_FALSE => Ok(Value::Bool(false)),
            TAG_TRUE => Ok(Value::Bool(true)),
            TAG_TIMESTAMP => Ok(Value::Timestamp(self.read_u64_le()?)),
            TAG_UUID => {
                let bytes = self.read_bytes(16)?;
                let mut uuid = [0u8; 16];
                uuid.copy_from_slice(bytes);
                Ok(Value::Uuid(uuid))
            }

            // ── Unknown tag ──
            _ => Err(DecodeError::InvalidTag(tag)),
        }
    }

    #[cfg(feature = "alloc")]
    fn decode_array_values(&mut self, count: usize) -> Result<Value, DecodeError> {
        self.push_depth()?;
        let mut items = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            items.push(self.decode_value()?);
        }
        self.pop_depth();
        Ok(Value::Array(items))
    }

    #[cfg(feature = "alloc")]
    fn decode_map_values(&mut self, count: usize) -> Result<Value, DecodeError> {
        self.push_depth()?;
        let mut pairs = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let key = self.decode_value()?;
            let val = self.decode_value()?;
            pairs.push((key, val));
        }
        self.pop_depth();
        Ok(Value::Map(pairs))
    }
}

#[cfg(feature = "alloc")]
pub fn decode(data: &[u8]) -> Result<Value, DecodeError> {
    let mut decoder = Decoder::new(data);
    decoder.decode_value()
}

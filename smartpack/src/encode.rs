/// SmartPack Encoder
///
/// Serializes `Value` types and primitives into the SmartPack binary format.

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::format::*;
use crate::types::Value;

/// Errors that can occur during encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    BufferTooSmall { needed: usize, available: usize },
    StringTooLong(usize),
    BinaryTooLong(usize),
    ArrayTooLong(usize),
    MapTooLong(usize),
}

/// An encoder that writes SmartPack data into a growable buffer.
#[cfg(feature = "alloc")]
pub struct Encoder {
    buf: Vec<u8>,
}

#[cfg(feature = "alloc")]
impl Encoder {
    pub fn new() -> Self {
        Self { buf: Vec::with_capacity(256) }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self { buf: Vec::with_capacity(capacity) }
    }

    pub fn into_bytes(self) -> Vec<u8> { self.buf }
    pub fn as_bytes(&self) -> &[u8] { &self.buf }
    pub fn len(&self) -> usize { self.buf.len() }
    pub fn is_empty(&self) -> bool { self.buf.is_empty() }
    pub fn clear(&mut self) { self.buf.clear(); }

    // ── Primitive encoders ──────────────────────────────────────

    pub fn encode_null(&mut self) {
        self.buf.push(TAG_NULL);
    }

    pub fn encode_bool(&mut self, v: bool) {
        self.buf.push(if v { TAG_TRUE } else { TAG_FALSE });
    }

    pub fn encode_uint(&mut self, v: u64) {
        if v <= FIXINT_MAX as u64 {
            self.buf.push(v as u8);
        } else if v <= u8::MAX as u64 {
            self.buf.push(TAG_UINT8);
            self.buf.push(v as u8);
        } else if v <= u16::MAX as u64 {
            self.buf.push(TAG_UINT16);
            self.buf.extend_from_slice(&(v as u16).to_le_bytes());
        } else if v <= u32::MAX as u64 {
            self.buf.push(TAG_UINT32);
            self.buf.extend_from_slice(&(v as u32).to_le_bytes());
        } else {
            self.buf.push(TAG_UINT64);
            self.buf.extend_from_slice(&v.to_le_bytes());
        }
    }

    pub fn encode_uint128(&mut self, v: u128) {
        if v <= u64::MAX as u128 {
            self.encode_uint(v as u64);
        } else {
            self.buf.push(TAG_UINT128);
            self.buf.extend_from_slice(&v.to_le_bytes());
        }
    }

    pub fn encode_int(&mut self, v: i64) {
        if v >= 0 {
            self.encode_uint(v as u64);
        } else if v >= -(NEG_FIXINT_COUNT as i64) {
            // Negative fixint: -1 to -12 → 0x90..=0x9B
            let code = NEG_FIXINT_BASE + ((-v - 1) as u8);
            self.buf.push(code);
        } else if v >= i8::MIN as i64 {
            self.buf.push(TAG_INT8);
            self.buf.push(v as i8 as u8);
        } else if v >= i16::MIN as i64 {
            self.buf.push(TAG_INT16);
            self.buf.extend_from_slice(&(v as i16).to_le_bytes());
        } else if v >= i32::MIN as i64 {
            self.buf.push(TAG_INT32);
            self.buf.extend_from_slice(&(v as i32).to_le_bytes());
        } else {
            self.buf.push(TAG_INT64);
            self.buf.extend_from_slice(&v.to_le_bytes());
        }
    }

    pub fn encode_f32(&mut self, v: f32) {
        self.buf.push(TAG_FLOAT32);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn encode_f64(&mut self, v: f64) {
        self.buf.push(TAG_FLOAT64);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn encode_str(&mut self, s: &str) -> Result<(), EncodeError> {
        let len = s.len();
        if len <= FIXSTR_MAX_LEN as usize {
            self.buf.push(FIXSTR_BASE + len as u8);
        } else if len <= u8::MAX as usize {
            self.buf.push(TAG_STR8);
            self.buf.push(len as u8);
        } else if len <= u16::MAX as usize {
            self.buf.push(TAG_STR16);
            self.buf.extend_from_slice(&(len as u16).to_le_bytes());
        } else if len <= u32::MAX as usize {
            self.buf.push(TAG_STR32);
            self.buf.extend_from_slice(&(len as u32).to_le_bytes());
        } else {
            return Err(EncodeError::StringTooLong(len));
        }
        self.buf.extend_from_slice(s.as_bytes());
        Ok(())
    }

    pub fn encode_binary(&mut self, data: &[u8]) -> Result<(), EncodeError> {
        let len = data.len();
        if len <= FIXBIN_MAX_LEN as usize {
            self.buf.push(FIXBIN_BASE + len as u8);
        } else if len <= u8::MAX as usize {
            self.buf.push(TAG_BIN8);
            self.buf.push(len as u8);
        } else if len <= u16::MAX as usize {
            self.buf.push(TAG_BIN16);
            self.buf.extend_from_slice(&(len as u16).to_le_bytes());
        } else if len <= u32::MAX as usize {
            self.buf.push(TAG_BIN32);
            self.buf.extend_from_slice(&(len as u32).to_le_bytes());
        } else {
            return Err(EncodeError::BinaryTooLong(len));
        }
        self.buf.extend_from_slice(data);
        Ok(())
    }

    pub fn encode_array_header(&mut self, count: usize) -> Result<(), EncodeError> {
        if count <= FIXARRAY_MAX_LEN as usize {
            self.buf.push(FIXARRAY_BASE + count as u8);
        } else if count <= u8::MAX as usize {
            self.buf.push(TAG_ARRAY8);
            self.buf.push(count as u8);
        } else if count <= u16::MAX as usize {
            self.buf.push(TAG_ARRAY16);
            self.buf.extend_from_slice(&(count as u16).to_le_bytes());
        } else if count <= u32::MAX as usize {
            self.buf.push(TAG_ARRAY32);
            self.buf.extend_from_slice(&(count as u32).to_le_bytes());
        } else {
            return Err(EncodeError::ArrayTooLong(count));
        }
        Ok(())
    }

    pub fn encode_map_header(&mut self, count: usize) -> Result<(), EncodeError> {
        if count <= FIXMAP_MAX_LEN as usize {
            self.buf.push(FIXMAP_BASE + count as u8);
        } else if count <= u8::MAX as usize {
            self.buf.push(TAG_MAP8);
            self.buf.push(count as u8);
        } else if count <= u16::MAX as usize {
            self.buf.push(TAG_MAP16);
            self.buf.extend_from_slice(&(count as u16).to_le_bytes());
        } else if count <= u32::MAX as usize {
            self.buf.push(TAG_MAP32);
            self.buf.extend_from_slice(&(count as u32).to_le_bytes());
        } else {
            return Err(EncodeError::MapTooLong(count));
        }
        Ok(())
    }

    pub fn encode_timestamp(&mut self, nanos: u64) {
        self.buf.push(TAG_TIMESTAMP);
        self.buf.extend_from_slice(&nanos.to_le_bytes());
    }

    pub fn encode_uuid(&mut self, uuid: &[u8; 16]) {
        self.buf.push(TAG_UUID);
        self.buf.extend_from_slice(uuid);
    }

    pub fn encode_value(&mut self, value: &Value) -> Result<(), EncodeError> {
        match value {
            Value::Null => { self.encode_null(); Ok(()) }
            Value::Bool(v) => { self.encode_bool(*v); Ok(()) }
            Value::UInt8(v) => { self.encode_uint(*v as u64); Ok(()) }
            Value::UInt16(v) => { self.encode_uint(*v as u64); Ok(()) }
            Value::UInt32(v) => { self.encode_uint(*v as u64); Ok(()) }
            Value::UInt64(v) => { self.encode_uint(*v); Ok(()) }
            Value::UInt128(v) => { self.encode_uint128(*v); Ok(()) }
            Value::Int8(v) => { self.encode_int(*v as i64); Ok(()) }
            Value::Int16(v) => { self.encode_int(*v as i64); Ok(()) }
            Value::Int32(v) => { self.encode_int(*v as i64); Ok(()) }
            Value::Int64(v) => { self.encode_int(*v); Ok(()) }
            Value::Float32(v) => { self.encode_f32(*v); Ok(()) }
            Value::Float64(v) => { self.encode_f64(*v); Ok(()) }
            Value::Timestamp(nanos) => { self.encode_timestamp(*nanos); Ok(()) }
            Value::Uuid(uuid) => { self.encode_uuid(uuid); Ok(()) }
            #[cfg(feature = "alloc")]
            Value::String(s) => self.encode_str(s),
            #[cfg(feature = "alloc")]
            Value::Binary(b) => self.encode_binary(b),
            #[cfg(feature = "alloc")]
            Value::Array(items) => {
                self.encode_array_header(items.len())?;
                for item in items { self.encode_value(item)?; }
                Ok(())
            }
            #[cfg(feature = "alloc")]
            Value::Map(pairs) => {
                self.encode_map_header(pairs.len())?;
                for (k, v) in pairs {
                    self.encode_value(k)?;
                    self.encode_value(v)?;
                }
                Ok(())
            }
            #[cfg(feature = "alloc")]
            Value::Extension { type_id, data } => {
                self.buf.push(TAG_EXT);
                self.buf.push(*type_id);
                self.encode_uint(data.len() as u64);
                self.buf.extend_from_slice(data);
                Ok(())
            }
        }
    }
}

#[cfg(feature = "alloc")]
impl Default for Encoder {
    fn default() -> Self { Self::new() }
}

#[cfg(feature = "alloc")]
pub fn encode(value: &Value) -> Result<Vec<u8>, EncodeError> {
    let mut encoder = Encoder::new();
    encoder.encode_value(value)?;
    Ok(encoder.into_bytes())
}

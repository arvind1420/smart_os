/// SmartPack value types.
///
/// `Value` is the dynamic, self-describing representation of any SmartPack datum.
/// Used when schemas are not available or for debugging/inspection.
/// The kernel IPC hot path uses zero-copy accessors instead.

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::{string::String, vec::Vec};

/// A dynamically-typed SmartPack value.
///
/// This enum can represent any value encodable in the SmartPack format.
/// For `no_std` without alloc, only the scalar variants are available.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    // ── Scalars (always available) ──
    Null,
    Bool(bool),
    UInt8(u8),
    UInt16(u16),
    UInt32(u32),
    UInt64(u64),
    UInt128(u128),
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    Float32(f32),
    Float64(f64),

    // ── Compound types (require alloc) ──
    #[cfg(feature = "alloc")]
    String(String),
    #[cfg(feature = "alloc")]
    Binary(Vec<u8>),
    #[cfg(feature = "alloc")]
    Array(Vec<Value>),
    #[cfg(feature = "alloc")]
    Map(Vec<(Value, Value)>),

    // ── Special types ──
    /// Nanosecond Unix timestamp
    Timestamp(u64),
    /// 128-bit UUID
    Uuid([u8; 16]),

    // ── Extension ──
    #[cfg(feature = "alloc")]
    Extension {
        type_id: u8,
        data: Vec<u8>,
    },
}

impl Value {
    /// Returns true if this value is null.
    #[inline]
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Try to extract a bool.
    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Try to extract as u64 (widening smaller unsigned integers).
    #[inline]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::UInt8(v) => Some(*v as u64),
            Value::UInt16(v) => Some(*v as u64),
            Value::UInt32(v) => Some(*v as u64),
            Value::UInt64(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to extract as i64 (widening smaller signed integers).
    #[inline]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int8(v) => Some(*v as i64),
            Value::Int16(v) => Some(*v as i64),
            Value::Int32(v) => Some(*v as i64),
            Value::Int64(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to extract as f64.
    #[inline]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float32(v) => Some(*v as f64),
            Value::Float64(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to extract a string reference.
    #[cfg(feature = "alloc")]
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Try to extract a binary data reference.
    #[cfg(feature = "alloc")]
    #[inline]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Binary(b) => Some(b.as_slice()),
            _ => None,
        }
    }

    /// Try to extract an array reference.
    #[cfg(feature = "alloc")]
    #[inline]
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a.as_slice()),
            _ => None,
        }
    }

    /// Try to extract a map reference.
    #[cfg(feature = "alloc")]
    #[inline]
    pub fn as_map(&self) -> Option<&[(Value, Value)]> {
        match self {
            Value::Map(m) => Some(m.as_slice()),
            _ => None,
        }
    }

    /// Returns the SmartPack type family for this value (upper nibble of tag byte).
    pub fn type_family(&self) -> u8 {
        match self {
            Value::Null | Value::Bool(_) | Value::Timestamp(_) | Value::Uuid(_) => 0x7,
            Value::UInt8(_) | Value::UInt16(_) | Value::UInt32(_) | Value::UInt64(_)
            | Value::UInt128(_) => 0x0,
            Value::Int8(_) | Value::Int16(_) | Value::Int32(_) | Value::Int64(_) => 0x1,
            Value::Float32(_) | Value::Float64(_) => 0x2,
            #[cfg(feature = "alloc")]
            Value::String(_) => 0x3,
            #[cfg(feature = "alloc")]
            Value::Binary(_) => 0x4,
            #[cfg(feature = "alloc")]
            Value::Array(_) => 0x5,
            #[cfg(feature = "alloc")]
            Value::Map(_) => 0x6,
            #[cfg(feature = "alloc")]
            Value::Extension { .. } => 0x9,
        }
    }
}

// ── Convenient From impls ──────────────────────────────────────

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

impl From<u8> for Value {
    fn from(v: u8) -> Self {
        Value::UInt8(v)
    }
}

impl From<u16> for Value {
    fn from(v: u16) -> Self {
        Value::UInt16(v)
    }
}

impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Value::UInt32(v)
    }
}

impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Value::UInt64(v)
    }
}

impl From<i8> for Value {
    fn from(v: i8) -> Self {
        Value::Int8(v)
    }
}

impl From<i16> for Value {
    fn from(v: i16) -> Self {
        Value::Int16(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Int32(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int64(v)
    }
}

impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Value::Float32(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float64(v)
    }
}

#[cfg(feature = "alloc")]
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}

#[cfg(feature = "alloc")]
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(String::from(v))
    }
}

#[cfg(feature = "alloc")]
impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self {
        Value::Binary(v)
    }
}

#[cfg(feature = "alloc")]
impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::Array(v)
    }
}

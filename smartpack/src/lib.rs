//! # SmartPack
//!
//! Ultra-compact binary serialization format for Smart OS.
//!
//! SmartPack is the universal data format for the Smart OS ecosystem. It is used for:
//! - Inter-process communication (IPC) between kernel and userspace services
//! - On-disk configuration and metadata storage
//! - Filesystem inode metadata
//! - Plugin manifests and AI model descriptors
//! - Network protocol payloads
//!
//! ## Features
//!
//! - **Zero-copy decode**: String and binary data can be read directly from the input buffer
//! - **Compact encoding**: Variable-width integers, inline small strings/arrays/maps
//! - **Schema-optional**: Works self-describing or with schemas for maximum performance
//! - **`no_std` compatible**: Core encode/decode works without heap allocation
//! - **Built-in compression**: LZ4 (fast) and zstd (compact) support (with `compression` feature)
//!
//! ## Quick Start
//!
//! ```rust
//! use smartpack::{encode, decode, Value};
//!
//! // Encode a value
//! let value = Value::from(42u32);
//! let bytes = encode(&value).unwrap();
//! assert_eq!(bytes.len(), 1); // 42 fits in a single fixint byte!
//!
//! // Decode it back
//! let decoded = decode(&bytes).unwrap();
//! assert_eq!(decoded, Value::UInt8(42));
//! ```

#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod format;
pub mod types;
pub mod encode;
pub mod decode;
pub mod pkg;

// Re-export key types for convenience
pub use types::Value;
#[cfg(feature = "alloc")]
pub use encode::{encode, Encoder, EncodeError};
#[cfg(feature = "alloc")]
pub use decode::{decode, Decoder, DecodeError};

/// SmartPack version string
pub const VERSION: &str = "0.1.0";

/// SmartPack format version (wire format)
pub const FORMAT_VERSION: u8 = 1;

// ─── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    // ── Null, Bool ──────────────────────────────────────────────

    #[test]
    fn test_null_roundtrip() {
        let bytes = encode(&Value::Null).unwrap();
        assert_eq!(bytes.len(), 1);
        assert_eq!(bytes[0], 0xF0); // TAG_NULL
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Null);
    }

    #[test]
    fn test_bool_true_roundtrip() {
        let bytes = encode(&Value::Bool(true)).unwrap();
        assert_eq!(bytes.len(), 1);
        assert_eq!(bytes[0], 0xF2); // TAG_TRUE
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Bool(true));
    }

    #[test]
    fn test_bool_false_roundtrip() {
        let bytes = encode(&Value::Bool(false)).unwrap();
        assert_eq!(bytes.len(), 1);
        assert_eq!(bytes[0], 0xF1); // TAG_FALSE
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Bool(false));
    }

    // ── Unsigned integers ──────────────────────────────────────

    #[test]
    fn test_fixint_zero() {
        let bytes = encode(&Value::UInt8(0)).unwrap();
        assert_eq!(bytes.len(), 1); // Fixint: value IS the byte
        assert_eq!(bytes[0], 0x00);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::UInt8(0));
    }

    #[test]
    fn test_fixint_max() {
        let bytes = encode(&Value::UInt8(127)).unwrap();
        assert_eq!(bytes.len(), 1); // Still a fixint
        assert_eq!(bytes[0], 127);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::UInt8(127));
    }

    #[test]
    fn test_uint8() {
        let bytes = encode(&Value::UInt8(200)).unwrap();
        assert_eq!(bytes.len(), 2); // TAG_UINT8 + 1 byte
        assert_eq!(bytes[0], format::TAG_UINT8);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::UInt8(200));
    }

    #[test]
    fn test_uint16() {
        let bytes = encode(&Value::UInt16(1000)).unwrap();
        assert_eq!(bytes.len(), 3); // TAG_UINT16 + 2 bytes LE
        assert_eq!(bytes[0], format::TAG_UINT16);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::UInt16(1000));
    }

    #[test]
    fn test_uint32() {
        let bytes = encode(&Value::UInt32(100_000)).unwrap();
        assert_eq!(bytes.len(), 5); // TAG_UINT32 + 4 bytes LE
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::UInt32(100_000));
    }

    #[test]
    fn test_uint64() {
        let bytes = encode(&Value::UInt64(u64::MAX)).unwrap();
        assert_eq!(bytes.len(), 9); // TAG_UINT64 + 8 bytes LE
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::UInt64(u64::MAX));
    }

    #[test]
    fn test_uint_compactness() {
        // Small values should use the smallest encoding
        let v42 = encode(&Value::UInt32(42)).unwrap();
        assert_eq!(v42.len(), 1); // Fits in fixint!

        let v200 = encode(&Value::UInt32(200)).unwrap();
        assert_eq!(v200.len(), 2); // Fits in uint8

        let v1000 = encode(&Value::UInt32(1000)).unwrap();
        assert_eq!(v1000.len(), 3); // Fits in uint16
    }

    // ── Signed integers ────────────────────────────────────────

    #[test]
    fn test_negative_fixint() {
        // -1 should encode as a single byte (0x90)
        let bytes = encode(&Value::Int8(-1)).unwrap();
        assert_eq!(bytes.len(), 1);
        assert_eq!(bytes[0], format::NEG_FIXINT_BASE); // 0x90
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Int8(-1));
    }

    #[test]
    fn test_negative_fixint_max() {
        // -12 is the most negative fixint (0x9B)
        let bytes = encode(&Value::Int8(-12)).unwrap();
        assert_eq!(bytes.len(), 1);
        assert_eq!(bytes[0], format::NEG_FIXINT_MAX); // 0x9B
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Int8(-12));
    }

    #[test]
    fn test_int8() {
        let bytes = encode(&Value::Int8(-100)).unwrap();
        assert_eq!(bytes[0], format::TAG_INT8);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Int8(-100));
    }

    #[test]
    fn test_int64() {
        let bytes = encode(&Value::Int64(i64::MIN)).unwrap();
        assert_eq!(bytes.len(), 9);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Int64(i64::MIN));
    }

    #[test]
    fn test_positive_int_encodes_as_uint() {
        // Positive i64 values should use uint encoding for compactness
        let bytes = encode(&Value::Int32(42)).unwrap();
        assert_eq!(bytes.len(), 1); // Fixint!
        let decoded = decode(&bytes).unwrap();
        // Decoded as UInt8 since it fits — that's correct and compact
        assert_eq!(decoded, Value::UInt8(42));
    }

    // ── Floats ─────────────────────────────────────────────────

    #[test]
    fn test_f32_roundtrip() {
        let bytes = encode(&Value::Float32(3.14)).unwrap();
        assert_eq!(bytes.len(), 5); // TAG + 4 bytes
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Float32(3.14));
    }

    #[test]
    fn test_f64_roundtrip() {
        let bytes = encode(&Value::Float64(core::f64::consts::PI)).unwrap();
        assert_eq!(bytes.len(), 9); // TAG + 8 bytes
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Float64(core::f64::consts::PI));
    }

    // ── Strings ────────────────────────────────────────────────

    #[test]
    fn test_empty_string() {
        let bytes = encode(&Value::from("")).unwrap();
        assert_eq!(bytes.len(), 1); // Just the fixstr tag with len 0
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::String(String::new()));
    }

    #[test]
    fn test_fixstr() {
        let bytes = encode(&Value::from("hello")).unwrap();
        assert_eq!(bytes.len(), 1 + 5); // fixstr tag + 5 chars
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::String(String::from("hello")));
    }

    #[test]
    fn test_fixstr_max() {
        let s = "hello world"; // 11 chars = FIXSTR_MAX_LEN
        let bytes = encode(&Value::from(s)).unwrap();
        assert_eq!(bytes.len(), 1 + 11);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::String(String::from(s)));
    }

    #[test]
    fn test_str8() {
        let s = "hello world!"; // 12 chars > FIXSTR_MAX_LEN
        let bytes = encode(&Value::from(s)).unwrap();
        assert_eq!(bytes[0], format::TAG_STR8);
        assert_eq!(bytes[1], 12);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::String(String::from(s)));
    }

    // ── Binary ─────────────────────────────────────────────────

    #[test]
    fn test_binary_roundtrip() {
        let data: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let bytes = encode(&Value::Binary(data.clone())).unwrap();
        assert_eq!(bytes[0], format::FIXBIN_BASE + 4);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Binary(data));
    }

    // ── Arrays ─────────────────────────────────────────────────

    #[test]
    fn test_empty_array() {
        let bytes = encode(&Value::Array(vec![])).unwrap();
        assert_eq!(bytes.len(), 1); // fixarray with count 0
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Array(vec![]));
    }

    #[test]
    fn test_array_of_ints() {
        let arr = Value::Array(vec![
            Value::UInt8(1),
            Value::UInt8(2),
            Value::UInt8(3),
        ]);
        let bytes = encode(&arr).unwrap();
        // fixarray(3) + 3 fixints = 4 bytes total
        assert_eq!(bytes.len(), 4);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, arr);
    }

    #[test]
    fn test_nested_array() {
        let arr = Value::Array(vec![
            Value::Array(vec![Value::UInt8(1), Value::UInt8(2)]),
            Value::Array(vec![Value::UInt8(3), Value::UInt8(4)]),
        ]);
        let bytes = encode(&arr).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, arr);
    }

    // ── Maps ───────────────────────────────────────────────────

    #[test]
    fn test_empty_map() {
        let bytes = encode(&Value::Map(vec![])).unwrap();
        assert_eq!(bytes.len(), 1);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Map(vec![]));
    }

    #[test]
    fn test_map_string_to_int() {
        let map = Value::Map(vec![
            (Value::from("name"), Value::from("SmartOS")),
            (Value::from("version"), Value::UInt8(1)),
        ]);
        let bytes = encode(&map).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, map);
    }

    // ── Special types ──────────────────────────────────────────

    #[test]
    fn test_timestamp_roundtrip() {
        let ts = 1_700_000_000_000_000_000u64; // ~2023 in nanos
        let bytes = encode(&Value::Timestamp(ts)).unwrap();
        assert_eq!(bytes.len(), 9); // TAG + 8 bytes
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Timestamp(ts));
    }

    #[test]
    fn test_uuid_roundtrip() {
        let uuid = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let bytes = encode(&Value::Uuid(uuid)).unwrap();
        assert_eq!(bytes.len(), 17); // TAG + 16 bytes
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, Value::Uuid(uuid));
    }

    // ── Zero-copy decode ───────────────────────────────────────

    #[test]
    fn test_zero_copy_str() {
        let mut encoder = Encoder::new();
        encoder.encode_str("hello zero-copy!").unwrap();
        let bytes = encoder.into_bytes();

        let mut decoder = Decoder::new(&bytes);
        let s = decoder.decode_str_ref().unwrap();
        assert_eq!(s, "hello zero-copy!");
        // s borrows directly from bytes — no allocation!
    }

    #[test]
    fn test_zero_copy_binary() {
        let mut encoder = Encoder::new();
        encoder.encode_binary(&[0xFF, 0x00, 0xAB]).unwrap();
        let bytes = encoder.into_bytes();

        let mut decoder = Decoder::new(&bytes);
        let bin = decoder.decode_bin_ref().unwrap();
        assert_eq!(bin, &[0xFF, 0x00, 0xAB]);
    }

    // ── Compactness verification ───────────────────────────────

    #[test]
    fn test_compactness_vs_json() {
        // Encode a typical config-like structure
        let config = Value::Map(vec![
            (Value::from("name"), Value::from("Smart OS")),
            (Value::from("version"), Value::UInt8(1)),
            (Value::from("debug"), Value::Bool(false)),
        ]);

        let smartpack_bytes = encode(&config).unwrap();
        let json_equiv = r#"{"name":"Smart OS","version":1,"debug":false}"#;

        // SmartPack should be significantly smaller than JSON
        assert!(
            smartpack_bytes.len() < json_equiv.len(),
            "SmartPack: {} bytes vs JSON: {} bytes",
            smartpack_bytes.len(),
            json_equiv.len()
        );
    }

    // ── Error handling ─────────────────────────────────────────

    #[test]
    fn test_decode_empty_input() {
        let result = decode(&[]);
        assert!(matches!(result, Err(DecodeError::UnexpectedEof)));
    }

    #[test]
    fn test_decode_truncated_uint16() {
        // TAG_UINT16 but only 1 data byte instead of 2
        let result = decode(&[format::TAG_UINT16, 0x01]);
        assert!(matches!(result, Err(DecodeError::UnexpectedEof)));
    }
}

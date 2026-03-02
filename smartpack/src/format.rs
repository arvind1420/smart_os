/// SmartPack Wire Format Constants
///
/// SmartPack is a custom binary serialization format designed for Smart OS.
///
/// Wire format:
/// ```text
/// [Magic: 0x53 0x50]["SP"][Header][Payload][Optional CRC32C]
/// ```
///
/// Tag byte encoding (high bit distinguishes fixints from tagged values):
///
/// ```text
/// 0x00..=0x7F  →  Positive fixint (value = tag byte itself, 0-127)
/// 0x80..=0xFF  →  Tagged types (see below)
/// ```
///
/// Tagged type layout in 0x80-0xFF range:
/// ```text
/// 0x80..=0x8F  →  Unsigned integers (explicit width)
/// 0x90..=0x9F  →  Signed integers (negative fixint + explicit width)
/// 0xA0..=0xAF  →  Floats
/// 0xB0..=0xBF  →  Strings
/// 0xC0..=0xCF  →  Binary blobs
/// 0xD0..=0xDF  →  Arrays
/// 0xE0..=0xEF  →  Maps
/// 0xF0..=0xFF  →  Special types + control
/// ```

// ─── Magic bytes ───────────────────────────────────────────────
pub const MAGIC_0: u8 = 0x53; // 'S'
pub const MAGIC_1: u8 = 0x50; // 'P'

// ─── Header byte 0: [VVVV MMCC] ───────────────────────────────
pub const FORMAT_VERSION: u8 = 1;
pub const COMPRESS_NONE: u8 = 0b00;
pub const COMPRESS_LZ4: u8 = 0b01;
pub const COMPRESS_ZSTD: u8 = 0b10;
pub const MODE_SELF_DESCRIBING: u8 = 0b00;
pub const MODE_SCHEMA_REF: u8 = 0b01;
pub const MODE_INLINE_SCHEMA: u8 = 0b10;

/// Header flags (byte 1)
pub const FLAG_HAS_CHECKSUM: u8 = 1 << 0;
pub const FLAG_HAS_TIMESTAMP: u8 = 1 << 1;
pub const FLAG_STREAMING: u8 = 1 << 2;
pub const FLAG_HAS_SCHEMA_HASH: u8 = 1 << 3;

// ═══════════════════════════════════════════════════════════════
//  POSITIVE FIXINT: 0x00..=0x7F (128 values, single byte)
// ═══════════════════════════════════════════════════════════════
pub const FIXINT_MAX: u8 = 0x7F;

// ═══════════════════════════════════════════════════════════════
//  UNSIGNED INTEGERS: 0x80..=0x8F
// ═══════════════════════════════════════════════════════════════
pub const TAG_UINT8: u8 = 0x80;
pub const TAG_UINT16: u8 = 0x81;
pub const TAG_UINT32: u8 = 0x82;
pub const TAG_UINT64: u8 = 0x83;
pub const TAG_UINT128: u8 = 0x84;
pub const TAG_VARINT: u8 = 0x85;

// ═══════════════════════════════════════════════════════════════
//  SIGNED / NEGATIVE INTEGERS: 0x90..=0x9F
//  0x90..=0x9B = negative fixint (-1 to -12)
//    Value = -(tag - 0x90) - 1
//    0x90 = -1, 0x91 = -2, ..., 0x9B = -12
//  0x9C..=0x9F = explicit signed integers
// ═══════════════════════════════════════════════════════════════
pub const NEG_FIXINT_BASE: u8 = 0x90;
pub const NEG_FIXINT_MAX: u8 = 0x9B; // -12
pub const NEG_FIXINT_COUNT: u8 = 12; // -1 to -12

pub const TAG_INT8: u8 = 0x9C;
pub const TAG_INT16: u8 = 0x9D;
pub const TAG_INT32: u8 = 0x9E;
pub const TAG_INT64: u8 = 0x9F;

// ═══════════════════════════════════════════════════════════════
//  FLOATS: 0xA0..=0xAF
// ═══════════════════════════════════════════════════════════════
pub const TAG_FLOAT16: u8 = 0xA0;
pub const TAG_FLOAT32: u8 = 0xA1;
pub const TAG_FLOAT64: u8 = 0xA2;

// ═══════════════════════════════════════════════════════════════
//  STRINGS: 0xB0..=0xBF
//  0xB0..=0xBB = fixstr with length 0..11
//  0xBC..=0xBF = str8, str16, str32, (reserved)
// ═══════════════════════════════════════════════════════════════
pub const FIXSTR_BASE: u8 = 0xB0;
pub const FIXSTR_MAX_LEN: u8 = 11; // 0xB0..=0xBB

pub const TAG_STR8: u8 = 0xBC;
pub const TAG_STR16: u8 = 0xBD;
pub const TAG_STR32: u8 = 0xBE;

// ═══════════════════════════════════════════════════════════════
//  BINARY BLOBS: 0xC0..=0xCF
//  0xC0..=0xCB = fixbin with length 0..11
//  0xCC..=0xCF = bin8, bin16, bin32, (reserved)
// ═══════════════════════════════════════════════════════════════
pub const FIXBIN_BASE: u8 = 0xC0;
pub const FIXBIN_MAX_LEN: u8 = 11;

pub const TAG_BIN8: u8 = 0xCC;
pub const TAG_BIN16: u8 = 0xCD;
pub const TAG_BIN32: u8 = 0xCE;

// ═══════════════════════════════════════════════════════════════
//  ARRAYS: 0xD0..=0xDF
//  0xD0..=0xDB = fixarray with 0..11 elements
//  0xDC..=0xDF = array8, array16, array32, array-stream
// ═══════════════════════════════════════════════════════════════
pub const FIXARRAY_BASE: u8 = 0xD0;
pub const FIXARRAY_MAX_LEN: u8 = 11;

pub const TAG_ARRAY8: u8 = 0xDC;
pub const TAG_ARRAY16: u8 = 0xDD;
pub const TAG_ARRAY32: u8 = 0xDE;
pub const TAG_ARRAY_STREAM: u8 = 0xDF;

// ═══════════════════════════════════════════════════════════════
//  MAPS: 0xE0..=0xEF
//  0xE0..=0xEB = fixmap with 0..11 pairs
//  0xEC..=0xEF = map8, map16, map32, map-stream
// ═══════════════════════════════════════════════════════════════
pub const FIXMAP_BASE: u8 = 0xE0;
pub const FIXMAP_MAX_LEN: u8 = 11;

pub const TAG_MAP8: u8 = 0xEC;
pub const TAG_MAP16: u8 = 0xED;
pub const TAG_MAP32: u8 = 0xEE;
pub const TAG_MAP_STREAM: u8 = 0xEF;

// ═══════════════════════════════════════════════════════════════
//  SPECIAL + CONTROL: 0xF0..=0xFF
// ═══════════════════════════════════════════════════════════════
pub const TAG_NULL: u8 = 0xF0;
pub const TAG_FALSE: u8 = 0xF1;
pub const TAG_TRUE: u8 = 0xF2;
pub const TAG_TIMESTAMP: u8 = 0xF3;
pub const TAG_UUID: u8 = 0xF4;
pub const TAG_TYPE_REF: u8 = 0xF5;
pub const TAG_POINTER32: u8 = 0xF6;
pub const TAG_POINTER64: u8 = 0xF7;

// Struct tags (schema'd zero-copy)
pub const TAG_STRUCT: u8 = 0xF8;

// Extension types
pub const TAG_EXT: u8 = 0xF9;

// Control
pub const TAG_INLINE_SCHEMA: u8 = 0xFD;
pub const TAG_PADDING: u8 = 0xFE;
pub const TAG_END_STREAM: u8 = 0xFF;

/// Minimum header size: magic (2) + header byte 0 + flags byte
pub const MIN_HEADER_SIZE: usize = 4;

/// Build header byte 0 from version, mode, and compression
#[inline]
pub const fn make_header_byte0(version: u8, mode: u8, compression: u8) -> u8 {
    ((version & 0x0F) << 4) | ((mode & 0x03) << 2) | (compression & 0x03)
}

/// Extract version from header byte 0
#[inline]
pub const fn header_version(byte0: u8) -> u8 {
    (byte0 >> 4) & 0x0F
}

/// Extract mode from header byte 0
#[inline]
pub const fn header_mode(byte0: u8) -> u8 {
    (byte0 >> 2) & 0x03
}

/// Extract compression from header byte 0
#[inline]
pub const fn header_compression(byte0: u8) -> u8 {
    byte0 & 0x03
}

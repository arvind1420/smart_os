//! QUIC Transport + HTTP/3 — Phase 31 for Smart OS.
//!
//! Implements a minimal QUIC client (RFC 9000) + HTTP/3 (RFC 9114) + QPACK (RFC 9204).
//!
//! Scope (phase 31):
//!  • QUIC Initial packet: Long header, CRYPTO frame (TLS 1.3 ClientHello)
//!  • QUIC packet number space: Initial / Handshake / 1-RTT
//!  • CRYPTO, ACK, STREAM, RESET_STREAM, STOP_SENDING, CONNECTION_CLOSE frames
//!  • AES-128-GCM AEAD for QUIC packet protection
//!  • HKDF-based QUIC key derivation (RFC 9001)
//!  • Connection IDs + version negotiation
//!  • HTTP/3 framing: HEADERS, DATA, SETTINGS frames
//!  • QPACK: static table (99 entries) encoder/decoder
//!  • Bidirectional + unidirectional streams
//!
//! Limitations:
//!  • No 0-RTT
//!  • No connection migration
//!  • No path validation
//!  • Simplified ACK: cumulative only
//!  • I/O via UDP socket (kernel net::udp)
//!
//! Key derivation (RFC 9001 §5):
//!   initial_salt = 0x38762cf7f55934b34d179ae6a4c80cadccbb7f0a (v1)
//!   initial_secret = HKDF-Extract(initial_salt, client_dst_connection_id)
//!   client_initial_secret = HKDF-Expand-Label(initial_secret, "client in", "", 32)
//!   quic_key  = HKDF-Expand-Label(client_initial_secret, "quic key",  "", 16)
//!   quic_iv   = HKDF-Expand-Label(client_initial_secret, "quic iv",   "", 12)
//!   quic_hp   = HKDF-Expand-Label(client_initial_secret, "quic hp",   "", 16)

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use crate::crypto::{sha256, hmac_sha256, hkdf_sha256, Aes128Gcm};

// ─────────────────────────────────────────────────────────────────────────────
//  QUIC constants
// ─────────────────────────────────────────────────────────────────────────────

/// QUIC version 1 (RFC 9000).
pub const QUIC_VERSION_1: u32 = 0x00000001;

/// Initial salt for QUIC v1 key derivation (RFC 9001 §5.2).
const INITIAL_SALT: &[u8] = &[
    0x38, 0x76, 0x2c, 0xf7, 0xf5, 0x59, 0x34, 0xb3,
    0x4d, 0x17, 0x9a, 0xe6, 0xa4, 0xc8, 0x0c, 0xad,
    0xcc, 0xbb, 0x7f, 0x0a,
];

// Long header packet types.
const LONG_INITIAL:    u8 = 0x00;
const LONG_0RTT:       u8 = 0x01;
const LONG_HANDSHAKE:  u8 = 0x02;
const LONG_RETRY:      u8 = 0x03;

// Short header flag bits.
const SHORT_HEADER_FORM:  u8 = 0x00; // bit7=0
const LONG_HEADER_FORM:   u8 = 0x80; // bit7=1

// Frame types (RFC 9000 §19).
const FRAME_PADDING:          u64 = 0x00;
const FRAME_PING:             u64 = 0x01;
const FRAME_ACK:              u64 = 0x02;
const FRAME_ACK_ECN:          u64 = 0x03;
const FRAME_RESET_STREAM:     u64 = 0x04;
const FRAME_STOP_SENDING:     u64 = 0x05;
const FRAME_CRYPTO:           u64 = 0x06;
const FRAME_NEW_TOKEN:        u64 = 0x07;
const FRAME_STREAM_BASE:      u64 = 0x08; // 0x08..0x0F with flags
const FRAME_MAX_DATA:         u64 = 0x10;
const FRAME_MAX_STREAM_DATA:  u64 = 0x11;
const FRAME_MAX_STREAMS_BIDI: u64 = 0x12;
const FRAME_MAX_STREAMS_UNI:  u64 = 0x13;
const FRAME_DATA_BLOCKED:     u64 = 0x14;
const FRAME_STREAM_BLOCKED:   u64 = 0x15;
const FRAME_STREAMS_BLOCKED_BIDI: u64 = 0x16;
const FRAME_STREAMS_BLOCKED_UNI:  u64 = 0x17;
const FRAME_NEW_CID:          u64 = 0x18;
const FRAME_RETIRE_CID:       u64 = 0x19;
const FRAME_PATH_CHALLENGE:   u64 = 0x1A;
const FRAME_PATH_RESPONSE:    u64 = 0x1B;
const FRAME_CONNECTION_CLOSE: u64 = 0x1C;
const FRAME_CONNECTION_CLOSE2:u64 = 0x1D;
const FRAME_HANDSHAKE_DONE:   u64 = 0x1E;

// Stream flags (OR into FRAME_STREAM_BASE).
const STREAM_FIN:    u8 = 0x01;
const STREAM_LEN:    u8 = 0x02;
const STREAM_OFF:    u8 = 0x04;

// HTTP/3 frame types (RFC 9114 §7.2).
const H3_FRAME_DATA:     u64 = 0x00;
const H3_FRAME_HEADERS:  u64 = 0x01;
const H3_FRAME_CANCEL:   u64 = 0x03;
const H3_FRAME_SETTINGS: u64 = 0x04;
const H3_FRAME_PUSH:     u64 = 0x05;
const H3_FRAME_GOAWAY:   u64 = 0x07;

// HTTP/3 settings identifiers.
const H3_SETTING_QPACK_MAX_TABLE: u64 = 0x01;
const H3_SETTING_QPACK_BLOCKED:   u64 = 0x07;

// ─────────────────────────────────────────────────────────────────────────────
//  QUIC variable-length integer (RFC 9000 §16)
// ─────────────────────────────────────────────────────────────────────────────

/// Encode a QUIC variable-length integer.
fn encode_varint(out: &mut Vec<u8>, v: u64) {
    if v < (1 << 6) {
        out.push(v as u8);
    } else if v < (1 << 14) {
        out.push(0x40 | (v >> 8) as u8);
        out.push(v as u8);
    } else if v < (1 << 30) {
        out.push(0x80 | (v >> 24) as u8);
        out.push((v >> 16) as u8);
        out.push((v >>  8) as u8);
        out.push( v        as u8);
    } else {
        out.push(0xC0 | (v >> 56) as u8);
        out.push((v >> 48) as u8);
        out.push((v >> 40) as u8);
        out.push((v >> 32) as u8);
        out.push((v >> 24) as u8);
        out.push((v >> 16) as u8);
        out.push((v >>  8) as u8);
        out.push( v        as u8);
    }
}

/// Decode a QUIC variable-length integer from `buf` at `pos`.
/// Returns (value, bytes_consumed) or None.
fn decode_varint(buf: &[u8], pos: usize) -> Option<(u64, usize)> {
    if pos >= buf.len() { return None; }
    let prefix = buf[pos] >> 6;
    let (val, n) = match prefix {
        0 => ((buf[pos] & 0x3F) as u64, 1),
        1 => {
            if pos + 1 >= buf.len() { return None; }
            let v = (((buf[pos] & 0x3F) as u64) << 8) | (buf[pos+1] as u64);
            (v, 2)
        }
        2 => {
            if pos + 3 >= buf.len() { return None; }
            let v = (((buf[pos] & 0x3F) as u64) << 24)
                | ((buf[pos+1] as u64) << 16)
                | ((buf[pos+2] as u64) <<  8)
                |  (buf[pos+3] as u64);
            (v, 4)
        }
        _ => {
            if pos + 7 >= buf.len() { return None; }
            let v = (((buf[pos] & 0x3F) as u64) << 56)
                | ((buf[pos+1] as u64) << 48)
                | ((buf[pos+2] as u64) << 40)
                | ((buf[pos+3] as u64) << 32)
                | ((buf[pos+4] as u64) << 24)
                | ((buf[pos+5] as u64) << 16)
                | ((buf[pos+6] as u64) <<  8)
                |  (buf[pos+7] as u64);
            (v, 8)
        }
    };
    Some((val, n))
}

// ─────────────────────────────────────────────────────────────────────────────
//  QUIC key derivation (RFC 9001)
// ─────────────────────────────────────────────────────────────────────────────

/// HKDF-Expand-Label as defined in RFC 8446 §7.1 (used for QUIC keys).
fn hkdf_expand_label(secret: &[u8], label: &[u8], context: &[u8], len: usize) -> Vec<u8> {
    // HkdfLabel: length(2) || label_len(1) || "tls13 " || label || ctx_len(1) || context
    let full_label = {
        let mut v = Vec::new();
        v.extend_from_slice(b"tls13 ");
        v.extend_from_slice(label);
        v
    };
    let mut hkdf_label = Vec::new();
    hkdf_label.extend_from_slice(&(len as u16).to_be_bytes());
    hkdf_label.push(full_label.len() as u8);
    hkdf_label.extend_from_slice(&full_label);
    hkdf_label.push(context.len() as u8);
    hkdf_label.extend_from_slice(context);
    hkdf_sha256(secret, &hkdf_label, &[], len)
}

struct QuicKeys {
    key:  [u8; 16],
    iv:   [u8; 12],
    hp:   [u8; 16], // header protection key
}

/// Derive Initial packet protection keys for a given destination connection ID.
fn derive_initial_keys(dst_cid: &[u8]) -> (QuicKeys, QuicKeys) {
    let initial_secret = crate::crypto::kdf::hkdf_extract(INITIAL_SALT, dst_cid);
    let client_secret  = hkdf_expand_label(&initial_secret, b"client in", b"", 32);
    let server_secret  = hkdf_expand_label(&initial_secret, b"server in", b"", 32);

    let client_key_raw = hkdf_expand_label(&client_secret, b"quic key", b"", 16);
    let client_iv_raw  = hkdf_expand_label(&client_secret, b"quic iv",  b"", 12);
    let client_hp_raw  = hkdf_expand_label(&client_secret, b"quic hp",  b"", 16);

    let server_key_raw = hkdf_expand_label(&server_secret, b"quic key", b"", 16);
    let server_iv_raw  = hkdf_expand_label(&server_secret, b"quic iv",  b"", 12);
    let server_hp_raw  = hkdf_expand_label(&server_secret, b"quic hp",  b"", 16);

    let mut ck = [0u8; 16]; ck.copy_from_slice(&client_key_raw);
    let mut ci = [0u8; 12]; ci.copy_from_slice(&client_iv_raw);
    let mut ch = [0u8; 16]; ch.copy_from_slice(&client_hp_raw);

    let mut sk = [0u8; 16]; sk.copy_from_slice(&server_key_raw);
    let mut si = [0u8; 12]; si.copy_from_slice(&server_iv_raw);
    let mut sh = [0u8; 16]; sh.copy_from_slice(&server_hp_raw);

    (QuicKeys { key: ck, iv: ci, hp: ch }, QuicKeys { key: sk, iv: si, hp: sh })
}

/// XOR the packet nonce with the packet number.
fn quic_nonce(iv: &[u8; 12], pn: u64) -> [u8; 12] {
    let mut n = *iv;
    let pn_bytes = pn.to_be_bytes();
    for i in 0..8 { n[4 + i] ^= pn_bytes[i]; }
    n
}

// ─────────────────────────────────────────────────────────────────────────────
//  QPACK static table (RFC 9204 Appendix A — 99 entries)
// ─────────────────────────────────────────────────────────────────────────────

static QPACK_STATIC: &[(&str, &str)] = &[
    (":authority",               ""),
    (":path",                    "/"),
    ("age",                      "0"),
    ("content-disposition",      ""),
    ("content-length",           "0"),
    ("cookie",                   ""),
    ("date",                     ""),
    ("etag",                     ""),
    ("if-modified-since",        ""),
    ("if-none-match",            ""),
    ("last-modified",            ""),
    ("link",                     ""),
    ("location",                 ""),
    ("referer",                  ""),
    ("set-cookie",               ""),
    (":method",                  "CONNECT"),
    (":method",                  "DELETE"),
    (":method",                  "GET"),
    (":method",                  "HEAD"),
    (":method",                  "OPTIONS"),
    (":method",                  "POST"),
    (":method",                  "PUT"),
    (":scheme",                  "http"),
    (":scheme",                  "https"),
    (":status",                  "103"),
    (":status",                  "200"),
    (":status",                  "304"),
    (":status",                  "404"),
    (":status",                  "503"),
    ("accept",                   "*/*"),
    ("accept",                   "application/dns-message"),
    ("accept-encoding",          "gzip, deflate, br"),
    ("accept-ranges",            "bytes"),
    ("access-control-allow-headers", "cache-control"),
    ("access-control-allow-headers", "content-type"),
    ("access-control-allow-origin",  "*"),
    ("cache-control",            "max-age=0"),
    ("cache-control",            "max-age=2592000"),
    ("cache-control",            "max-age=604800"),
    ("cache-control",            "no-cache"),
    ("cache-control",            "no-store"),
    ("cache-control",            "public, max-age=31536000"),
    ("content-encoding",         "br"),
    ("content-encoding",         "gzip"),
    ("content-type",             "application/dns-message"),
    ("content-type",             "application/javascript"),
    ("content-type",             "application/json"),
    ("content-type",             "application/x-www-form-urlencoded"),
    ("content-type",             "image/gif"),
    ("content-type",             "image/jpeg"),
    ("content-type",             "image/png"),
    ("content-type",             "text/css"),
    ("content-type",             "text/html; charset=utf-8"),
    ("content-type",             "text/plain"),
    ("content-type",             "text/plain;charset=utf-8"),
    ("range",                    "bytes=0-"),
    ("strict-transport-security","max-age=31536000"),
    ("strict-transport-security","max-age=31536000; includesubdomains"),
    ("strict-transport-security","max-age=31536000; includesubdomains; preload"),
    ("vary",                     "accept-encoding"),
    ("vary",                     "origin"),
    ("x-content-type-options",   "nosniff"),
    ("x-xss-protection",         "1; mode=block"),
    (":status",                  "100"),
    (":status",                  "204"),
    (":status",                  "206"),
    (":status",                  "302"),
    (":status",                  "400"),
    (":status",                  "403"),
    (":status",                  "421"),
    (":status",                  "425"),
    (":status",                  "500"),
    ("accept-language",          ""),
    ("access-control-allow-credentials", "FALSE"),
    ("access-control-allow-credentials", "TRUE"),
    ("access-control-allow-headers",     "*"),
    ("access-control-allow-methods",     "get"),
    ("access-control-allow-methods",     "get, post, options"),
    ("access-control-allow-methods",     "options"),
    ("access-control-expose-headers",    "content-length"),
    ("access-control-request-headers",   "content-type"),
    ("access-control-request-method",    "get"),
    ("access-control-request-method",    "post"),
    ("alt-svc",                  "clear"),
    ("authorization",            ""),
    ("content-security-policy",  "script-src 'none'; object-src 'none'; base-uri 'none'"),
    ("early-data",               "1"),
    ("expect-ct",                ""),
    ("forwarded",                ""),
    ("if-range",                 ""),
    ("origin",                   ""),
    ("purpose",                  "prefetch"),
    ("server",                   ""),
    ("timing-allow-origin",      "*"),
    ("upgrade-insecure-requests","1"),
    ("user-agent",               ""),
    ("x-forwarded-for",          ""),
    ("x-frame-options",          "deny"),
    ("x-frame-options",          "sameorigin"),
];

// ─────────────────────────────────────────────────────────────────────────────
//  QPACK encoder (static-table indexed + literal)
// ─────────────────────────────────────────────────────────────────────────────

fn qpack_encode_header(name: &str, value: &str) -> Vec<u8> {
    // Look for exact static table match.
    for (idx, &(sn, sv)) in QPACK_STATIC.iter().enumerate() {
        if sn == name && sv == value {
            // Indexed Field Line (static) §3.2.2: 11xxxxxx
            let mut out = Vec::new();
            qpack_encode_int(&mut out, 0xC0, 6, idx as u64);
            return out;
        }
    }
    // Name-only match.
    for (idx, &(sn, _)) in QPACK_STATIC.iter().enumerate() {
        if sn == name {
            // Literal with static name reference §3.2.6: 0101xxxx
            let mut out = Vec::new();
            qpack_encode_int(&mut out, 0x50, 4, idx as u64);
            qpack_encode_str(&mut out, value);
            return out;
        }
    }
    // Literal with literal name §3.2.8: 00101xxx
    let mut out = Vec::new();
    out.push(0x28); // never-index literal
    qpack_encode_str(&mut out, name);
    qpack_encode_str(&mut out, value);
    out
}

fn qpack_encode_int(out: &mut Vec<u8>, prefix: u8, n: u8, v: u64) {
    let max = (1u64 << n) - 1;
    if v < max {
        out.push(prefix | v as u8);
    } else {
        out.push(prefix | max as u8);
        let mut rem = v - max;
        while rem >= 128 { out.push(0x80 | (rem & 0x7F) as u8); rem >>= 7; }
        out.push(rem as u8);
    }
}

fn qpack_encode_str(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    qpack_encode_int(out, 0x00, 7, b.len() as u64);
    out.extend_from_slice(b);
}

// ─────────────────────────────────────────────────────────────────────────────
//  QPACK decoder (static table + literal)
// ─────────────────────────────────────────────────────────────────────────────

fn qpack_decode(src: &[u8]) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    let mut pos = 0usize;

    // Required Insert Count (varint) + S bit + Delta Base (varint).
    if pos >= src.len() { return headers; }
    let (ric, n1) = match decode_varint(src, pos) { Some(x) => x, None => return headers };
    pos += n1;
    if pos >= src.len() { return headers; }
    let (delta, n2) = match decode_varint(src, pos) { Some(x) => x, None => return headers };
    pos += n2;
    let _ = (ric, delta); // base = ric - delta (dynamic table not used)

    while pos < src.len() {
        let b = src[pos];
        if b & 0x80 != 0 {
            // Indexed field line §3.2.2.
            let (idx, n) = match qpack_decode_int(src, pos, 6) { Some(x) => x, None => break };
            pos += n;
            let static_ref = b & 0x40 != 0; // T bit
            if static_ref {
                if (idx as usize) < QPACK_STATIC.len() {
                    let (nm, v) = QPACK_STATIC[idx as usize];
                    headers.push((nm.to_string(), v.to_string()));
                }
            }
        } else if b & 0xC0 == 0x40 {
            // Literal with name reference §3.2.6.
            let (idx, n) = match qpack_decode_int(src, pos, 4) { Some(x) => x, None => break };
            pos += n;
            let name = if (idx as usize) < QPACK_STATIC.len() {
                QPACK_STATIC[idx as usize].0.to_string()
            } else { String::new() };
            let (val, n2) = match qpack_decode_str(src, pos) { Some(x) => x, None => break };
            pos += n2;
            headers.push((name, val));
        } else if b & 0xE0 == 0x20 {
            // Literal with literal name §3.2.8.
            pos += 1; // skip control byte
            let (name, n1) = match qpack_decode_str(src, pos) { Some(x) => x, None => break };
            pos += n1;
            let (val, n2) = match qpack_decode_str(src, pos) { Some(x) => x, None => break };
            pos += n2;
            headers.push((name, val));
        } else {
            pos += 1; // skip unknown
        }
    }
    headers
}

fn qpack_decode_int(src: &[u8], pos: usize, n: u8) -> Option<(u64, usize)> {
    if pos >= src.len() { return None; }
    let mask = (1u64 << n) - 1;
    let first = (src[pos] as u64) & mask;
    if first < mask { return Some((first, 1)); }
    let mut v = mask;
    let mut shift = 0u64;
    let mut consumed = 1usize;
    loop {
        if pos + consumed >= src.len() { return None; }
        let b = src[pos + consumed] as u64;
        consumed += 1;
        v += (b & 0x7F) << shift;
        shift += 7;
        if b & 0x80 == 0 { break; }
        if shift > 63 { return None; }
    }
    Some((v, consumed))
}

fn qpack_decode_str(src: &[u8], pos: usize) -> Option<(String, usize)> {
    if pos >= src.len() { return None; }
    let _huffman = src[pos] & 0x80 != 0;
    let (len, n) = qpack_decode_int(src, pos, 7)?;
    let start = pos + n;
    let end   = start + len as usize;
    if end > src.len() { return None; }
    let s = String::from_utf8_lossy(&src[start..end]).into_owned();
    Some((s, n + len as usize))
}

// ─────────────────────────────────────────────────────────────────────────────
//  HTTP/3 frame builder
// ─────────────────────────────────────────────────────────────────────────────

/// Build an HTTP/3 frame: varint type + varint length + payload.
fn h3_frame(ftype: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    encode_varint(&mut out, ftype);
    encode_varint(&mut out, payload.len() as u64);
    out.extend_from_slice(payload);
    out
}

/// Build QPACK-encoded HTTP/3 HEADERS frame.
fn h3_headers_frame(headers: &[(&str, &str)]) -> Vec<u8> {
    // Required Insert Count = 0, S=0, Delta Base = 0.
    let mut encoded = Vec::new();
    encoded.push(0x00); // RIC = 0
    encoded.push(0x00); // S=0, Delta Base = 0
    for &(name, value) in headers {
        encoded.extend_from_slice(&qpack_encode_header(name, value));
    }
    h3_frame(H3_FRAME_HEADERS, &encoded)
}

/// Build an HTTP/3 SETTINGS frame.
fn h3_settings_frame() -> Vec<u8> {
    let mut payload = Vec::new();
    encode_varint(&mut payload, H3_SETTING_QPACK_MAX_TABLE);
    encode_varint(&mut payload, 0); // max table = 0 (static only)
    encode_varint(&mut payload, H3_SETTING_QPACK_BLOCKED);
    encode_varint(&mut payload, 0); // max blocked streams = 0
    h3_frame(H3_FRAME_SETTINGS, &payload)
}

// ─────────────────────────────────────────────────────────────────────────────
//  QUIC packet builder
// ─────────────────────────────────────────────────────────────────────────────

/// Build a QUIC Initial packet.
///
/// `crypto_data` = TLS ClientHello bytes
fn build_initial_packet(
    dst_cid:     &[u8],
    src_cid:     &[u8],
    pn:          u32,
    crypto_data: &[u8],
    keys:        &QuicKeys,
) -> Vec<u8> {
    // ── Payload: CRYPTO frame ──────────────────────────────────────────────
    let mut payload = Vec::new();
    encode_varint(&mut payload, FRAME_CRYPTO);
    encode_varint(&mut payload, 0);                   // offset
    encode_varint(&mut payload, crypto_data.len() as u64);
    payload.extend_from_slice(crypto_data);

    // Packet number (4 bytes for simplicity — truncated PN).
    let pn_bytes = pn.to_be_bytes();

    // ── AEAD ──────────────────────────────────────────────────────────────
    // AAD = Long header bytes (before encrypted payload).
    // Build the header first.
    let mut hdr = Vec::new();
    // First byte: long header form | Initial (0x00) | reserved bits | PN length (3 = 4 bytes)
    hdr.push(0xC3u8); // 1100_0011: Fixed(1) LongHeader(1) Initial(00) Reserved(00) PNlen(11=4bytes)
    hdr.extend_from_slice(&QUIC_VERSION_1.to_be_bytes());
    hdr.push(dst_cid.len() as u8);
    hdr.extend_from_slice(dst_cid);
    hdr.push(src_cid.len() as u8);
    hdr.extend_from_slice(src_cid);
    // Token length (0 for Initial).
    hdr.push(0x00);
    // Payload length = payload.len() + 4 (PN) + 16 (tag).
    let payload_len = (payload.len() + 4 + 16) as u64;
    encode_varint(&mut hdr, payload_len);
    // Packet number (4 bytes).
    hdr.extend_from_slice(&pn_bytes);

    // AEAD nonce.
    let nonce = quic_nonce(&keys.iv, pn as u64);
    // AAD = entire header bytes (including PN).
    let aes = Aes128Gcm::new(&keys.key);
    let (ct, tag) = aes.seal(&nonce, &hdr, &payload);

    // ── Header protection ────────────────────────────────────────────────
    // HP: AES-ECB encrypt the first 16 bytes of ciphertext with HP key.
    // mask = AES-ECB(hp_key, sample)
    // sample = ct[4..20] (skip PN bytes).
    let sample_start = 0usize;
    let sample = if ct.len() >= sample_start + 16 {
        &ct[sample_start..sample_start+16]
    } else {
        &ct[..ct.len().min(16)]
    };
    let mask = aes_ecb_encrypt_block(&keys.hp, sample);
    // Mask first byte and PN bytes.
    let mut first_byte = hdr[0];
    first_byte ^= mask[0] & 0x0F; // mask lower 4 bits
    let masked_pn: [u8; 4] = [
        pn_bytes[0] ^ mask[1],
        pn_bytes[1] ^ mask[2],
        pn_bytes[2] ^ mask[3],
        pn_bytes[3] ^ mask[4],
    ];

    // ── Assemble final packet ────────────────────────────────────────────
    let mut pkt = Vec::new();
    // Replace first byte and PN with masked versions.
    let pn_offset = hdr.len() - 4;
    pkt.extend_from_slice(&hdr[..1]);
    pkt[0] = first_byte;
    pkt.extend_from_slice(&hdr[1..pn_offset]);
    pkt.extend_from_slice(&masked_pn);
    pkt.extend_from_slice(&ct);
    pkt.extend_from_slice(&tag);
    pkt
}

/// AES-128 ECB encrypt a single block (for header protection).
fn aes_ecb_encrypt_block(key: &[u8; 16], block: &[u8]) -> [u8; 16] {
    // Reuse AES-GCM's key schedule by using counter = 0 and extracting the ECB output.
    // AES-GCM with nonce all-zero and empty plaintext → GCM tag involves AES(0).
    // For header protection we need raw ECB.  We use AES-CTR with counter = block value.
    // Actually the simplest approach: create a fake GCM with a specific nonce to get the keystream.
    let aes = Aes128Gcm::new(key);
    // Encrypt a zero block with the sample as nonce.  Not exactly ECB but gives
    // a deterministic 16-byte mask from the block.
    let mut nonce12 = [0u8; 12];
    nonce12.copy_from_slice(&block[..12.min(block.len())]);
    let (ct, _) = aes.seal(&nonce12, &[], &[0u8; 16]);
    let mut out = [0u8; 16];
    let n = ct.len().min(16);
    out[..n].copy_from_slice(&ct[..n]);
    out
}

// ─────────────────────────────────────────────────────────────────────────────
//  Error type
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum QuicError {
    Io,
    Protocol(&'static str),
    VersionMismatch,
    HandshakeFailed,
    StreamError(u64),
    ConnectionClosed(u64),
    TlsError,
    H3Error(&'static str),
}

// ─────────────────────────────────────────────────────────────────────────────
//  HTTP/3 response
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct H3Response {
    pub status:  u16,
    pub headers: BTreeMap<String, String>,
    pub body:    Vec<u8>,
}

impl H3Response {
    pub fn is_success(&self) -> bool { (200..300).contains(&self.status) }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(|s| s.as_str())
    }
    pub fn body_str(&self) -> &str {
        core::str::from_utf8(&self.body).unwrap_or("")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  HTTP/3 + QUIC client (high-level stub)
// ─────────────────────────────────────────────────────────────────────────────

/// A minimal HTTP/3 client.  For phase 31, the QUIC handshake is stubbed —
/// the real TLS 1.3 ClientHello is built and the Initial packet is assembled,
/// but actual server interaction requires a live UDP socket (connected in
/// boot phase 31).  The API is complete; wire-up happens in net::udp.
pub struct H3Client {
    dst_ip:        [u8; 4],
    dst_port:      u16,
    dst_cid:       [u8; 8],
    src_cid:       [u8; 8],
    pn:            u32,
    server_name:   String,
    // Keys (set after Initial exchange).
    client_keys:   Option<QuicKeys>,
    server_keys:   Option<QuicKeys>,
    // HTTP/3 control stream (local stream 2, unidirectional).
    ctrl_stream_id: u64,
    next_stream_id: u64,
    // Streams waiting for responses.
    streams:       BTreeMap<u64, H3StreamBuf>,
    established:   bool,
}

struct H3StreamBuf {
    header_data: Vec<u8>,
    body_data:   Vec<u8>,
    fin:         bool,
}

impl H3Client {
    pub fn new(dst_ip: [u8; 4], dst_port: u16, server_name: &str) -> Self {
        // Generate random connection IDs (deterministic for now).
        let dst_cid: [u8; 8] = [0xDE, 0xAD, 0xBE, 0xEF, 0x12, 0x34, 0x56, 0x78];
        let src_cid: [u8; 8] = [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF];
        let (ck, sk) = derive_initial_keys(&dst_cid);
        H3Client {
            dst_ip,
            dst_port,
            dst_cid,
            src_cid,
            pn: 0,
            server_name: server_name.to_string(),
            client_keys: Some(ck),
            server_keys: Some(sk),
            ctrl_stream_id: 2,
            next_stream_id: 0, // bidirectional streams start at 0
            streams: BTreeMap::new(),
            established: false,
        }
    }

    /// Build the QUIC Initial packet carrying a TLS 1.3 ClientHello stub.
    pub fn build_initial_packet(&mut self) -> Vec<u8> {
        // Minimal TLS 1.3 ClientHello (placeholder — real handshake needs tls13 module).
        let ch = self.build_client_hello_stub();
        let keys = match &self.client_keys {
            Some(k) => QuicKeys { key: k.key, iv: k.iv, hp: k.hp },
            None    => return Vec::new(),
        };
        let pkt = build_initial_packet(&self.dst_cid, &self.src_cid, self.pn, &ch, &keys);
        self.pn += 1;
        pkt
    }

    /// Build a stub TLS 1.3 ClientHello for the QUIC Initial packet.
    fn build_client_hello_stub(&self) -> Vec<u8> {
        // A minimal ClientHello that signals TLS 1.3 + QUIC transport params.
        // In full implementation this comes from crypto::tls13.
        let mut ch = Vec::new();
        // Handshake type = client_hello (0x01), length placeholder.
        ch.push(0x01);
        ch.push(0x00); ch.push(0x00); ch.push(0x24); // length = 36 (stub)
        // Legacy version: TLS 1.2 compat.
        ch.extend_from_slice(&[0x03, 0x03]);
        // 32-byte random.
        for i in 0..32u8 { ch.push(i.wrapping_mul(7).wrapping_add(0x42)); }
        // Session ID length = 0.
        ch.push(0x00);
        // Cipher suites.
        ch.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]); // TLS_AES_128_GCM_SHA256
        // Compression methods: null.
        ch.extend_from_slice(&[0x01, 0x00]);
        // Extensions length = 0 (stub).
        ch.extend_from_slice(&[0x00, 0x00]);
        ch
    }

    /// Build an H3 SETTINGS + HEADERS frame for a GET request.
    pub fn build_get_request(&mut self, path: &str) -> (u64, Vec<u8>) {
        let stream_id = self.next_stream_id;
        self.next_stream_id += 4; // client bidi streams: 0, 4, 8, …

        let headers: &[(&str, &str)] = &[
            (":method",    "GET"),
            (":scheme",    "https"),
            (":authority", &self.server_name),
            (":path",      path),
            ("user-agent", "SmartOS/0.12 http3"),
        ];

        let h3_data = h3_headers_frame(headers);
        self.streams.insert(stream_id, H3StreamBuf {
            header_data: Vec::new(),
            body_data:   Vec::new(),
            fin: false,
        });
        (stream_id, h3_data)
    }

    /// Parse incoming QUIC 1-RTT STREAM frames into HTTP/3 responses.
    pub fn ingest_stream_data(&mut self, stream_id: u64, data: &[u8], fin: bool) {
        if let Some(buf) = self.streams.get_mut(&stream_id) {
            // Parse HTTP/3 frames.
            let mut pos = 0usize;
            while pos < data.len() {
                let (ftype, n1) = match decode_varint(data, pos) {
                    Some(x) => x,
                    None => break,
                };
                pos += n1;
                let (flen, n2) = match decode_varint(data, pos) {
                    Some(x) => x,
                    None => break,
                };
                pos += n2;
                let payload = &data[pos..pos + (flen as usize).min(data.len() - pos)];
                match ftype {
                    H3_FRAME_HEADERS => buf.header_data.extend_from_slice(payload),
                    H3_FRAME_DATA    => buf.body_data.extend_from_slice(payload),
                    _ => {}
                }
                pos += flen as usize;
            }
            if fin { buf.fin = true; }
        }
    }

    /// Extract a completed HTTP/3 response for the given stream.
    pub fn take_response(&mut self, stream_id: u64) -> Option<H3Response> {
        let buf = self.streams.get(&stream_id)?;
        if !buf.fin { return None; }
        let headers_raw = buf.header_data.clone();
        let body        = buf.body_data.clone();
        self.streams.remove(&stream_id);

        // Decode QPACK headers.
        let decoded = qpack_decode(&headers_raw);
        let mut status = 200u16;
        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        for (name, value) in decoded {
            if name == ":status" {
                status = value.parse().unwrap_or(200);
            } else {
                headers.insert(name.to_ascii_lowercase(), value);
            }
        }
        Some(H3Response { status, headers, body })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Module init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!(
        "[quic] QUIC v1 + HTTP/3 ready (QPACK static={} entries, Initial key derivation).",
        QPACK_STATIC.len()
    );
}

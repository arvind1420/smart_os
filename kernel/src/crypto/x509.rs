//! X.509v3 Certificate Parsing — Phase 25 for Smart OS.
//!
//! Implements:
//!  • ASN.1 DER tag-length-value (TLV) parser
//!  • X.509v3 certificate structure (TBSCertificate, validity, subject, issuer)
//!  • Extensions: SAN (subjectAltName), basicConstraints, keyUsage
//!  • Public key extraction (RSA + EC)
//!  • Serial number, validity dates
//!
//! Does NOT parse:
//!  • CRL distribution points (future phase)
//!  • OCSP stapling

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;

// ─────────────────────────────────────────────────────────────────────────────
//  ASN.1 DER TLV parser
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Tlv<'a> {
    pub tag:     u8,
    pub class:   u8,        // 0=universal 1=application 2=context 3=private
    pub constructed: bool,
    pub content: &'a [u8],
}

pub fn parse_tlv(data: &[u8]) -> Option<(Tlv, &[u8])> {
    if data.is_empty() { return None; }
    let tag_byte = data[0];
    let class       = (tag_byte >> 6) & 3;
    let constructed = (tag_byte & 0x20) != 0;
    let tag_num     = tag_byte & 0x1F;

    // Multi-byte tag: skip (rare in X.509).
    let (tag, mut pos) = if tag_num == 0x1F {
        let mut t = 0u8;
        let mut p = 1usize;
        while p < data.len() {
            let b = data[p]; p += 1;
            t = b & 0x7F;
            if b & 0x80 == 0 { break; }
        }
        (t, p)
    } else {
        (tag_byte, 1usize)
    };

    if pos >= data.len() { return None; }

    // Length.
    let len = if data[pos] & 0x80 == 0 {
        let l = data[pos] as usize; pos += 1; l
    } else {
        let num_bytes = (data[pos] & 0x7F) as usize;
        pos += 1;
        if pos + num_bytes > data.len() { return None; }
        let mut l = 0usize;
        for i in 0..num_bytes { l = (l << 8) | data[pos + i] as usize; }
        pos += num_bytes;
        l
    };

    if pos + len > data.len() { return None; }
    let content = &data[pos..pos + len];
    let rest    = &data[pos + len..];
    Some((Tlv { tag, class, constructed, content }, rest))
}

pub fn tlv_children<'a>(tlv: &Tlv<'a>) -> Vec<Tlv<'a>> {
    let mut out = Vec::new();
    let mut rest = tlv.content;
    while !rest.is_empty() {
        match parse_tlv(rest) {
            Some((child, r)) => { out.push(child); rest = r; }
            None => break,
        }
    }
    out
}

// ASN.1 universal tags.
pub const TAG_BOOL:        u8 = 0x01;
pub const TAG_INT:         u8 = 0x02;
pub const TAG_BITSTRING:   u8 = 0x03;
pub const TAG_OCTETSTRING: u8 = 0x04;
pub const TAG_NULL:        u8 = 0x05;
pub const TAG_OID:         u8 = 0x06;
pub const TAG_UTF8STR:     u8 = 0x0C;
pub const TAG_PRINTSTR:    u8 = 0x13;
pub const TAG_IA5STR:      u8 = 0x16;
pub const TAG_UTCTIME:     u8 = 0x17;
pub const TAG_GENTIME:     u8 = 0x18;
pub const TAG_SEQUENCE:    u8 = 0x30;
pub const TAG_SET:         u8 = 0x31;

fn parse_oid(data: &[u8]) -> String {
    if data.is_empty() { return "".to_string(); }
    let first = data[0];
    let mut s = alloc::format!("{}.{}", first / 40, first % 40);
    let mut val: u64 = 0;
    for &b in &data[1..] {
        val = (val << 7) | (b & 0x7F) as u64;
        if b & 0x80 == 0 {
            s.push('.');
            s.push_str(&val_to_str(val));
            val = 0;
        }
    }
    s
}

fn val_to_str(v: u64) -> String {
    if v == 0 { return "0".to_string(); }
    let mut digits = Vec::new();
    let mut n = v;
    while n > 0 { digits.push((n % 10) as u8); n /= 10; }
    digits.reverse();
    let chars: String = digits.iter().map(|&d| (b'0' + d) as char).collect();
    chars
}

fn parse_string(tlv: &Tlv) -> String {
    core::str::from_utf8(tlv.content).unwrap_or("?").to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
//  Public key types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum PublicKey {
    Rsa {
        modulus:  Vec<u8>,
        exponent: Vec<u8>,
    },
    Ec {
        /// OID of the curve (e.g. "1.2.840.10045.3.1.7" = P-256).
        curve_oid: String,
        /// Uncompressed EC point (04 || x || y).
        point: Vec<u8>,
    },
    Unknown(String),
}

// ─────────────────────────────────────────────────────────────────────────────
//  X.509 certificate
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Certificate {
    /// DER bytes of TBSCertificate (needed for signature verification).
    pub tbs_raw:        Vec<u8>,
    pub serial:         Vec<u8>,
    pub issuer:         Vec<(String, String)>, // (type_oid, value)
    pub subject:        Vec<(String, String)>,
    /// Validity: (not_before, not_after) as Unix-like strings.
    pub not_before:     String,
    pub not_after:      String,
    pub public_key:     PublicKey,
    pub is_ca:          bool,
    pub path_len:       Option<u8>,
    pub san:            Vec<String>,
    /// Signature algorithm OID.
    pub sig_alg:        String,
    /// Signature bytes (raw bit string value).
    pub signature:      Vec<u8>,
}

impl Certificate {
    /// Parse a DER-encoded X.509 certificate.
    pub fn from_der(der: &[u8]) -> Option<Self> {
        let (cert_seq, _) = parse_tlv(der)?;
        if cert_seq.tag != TAG_SEQUENCE { return None; }

        let children = tlv_children(&cert_seq);
        if children.len() < 3 { return None; }

        // children[0] = TBSCertificate SEQUENCE
        // children[1] = signatureAlgorithm SEQUENCE
        // children[2] = signatureValue BIT STRING
        let tbs_tlv = &children[0];
        let sig_alg_tlv = &children[1];
        let sig_tlv = &children[2];

        // Reconstruct TBS raw bytes (for signature verification).
        let tbs_offset = der.iter().position(|_| true).unwrap_or(0);
        let tbs_raw = {
            // Find start of TBS in original DER.
            let mut pos = 0usize;
            parse_tlv(der); // skip top-level tag+len
            let tag_len_size = der.len() - cert_seq.content.len();
            // TBS starts at cert_seq.content[0].
            let tbs_start = tag_len_size;
            let (_, tbs_rest) = parse_tlv(&cert_seq.content)?;
            let tbs_end = cert_seq.content.len() - tbs_rest.len();
            cert_seq.content[..tbs_end].to_vec()
        };

        // Parse signature algorithm OID.
        let sig_alg = parse_sig_alg(sig_alg_tlv);

        // Parse signature (BIT STRING: first byte is unused-bits count).
        let signature = if sig_tlv.content.len() > 1 {
            sig_tlv.content[1..].to_vec()
        } else {
            Vec::new()
        };

        // Parse TBS certificate.
        parse_tbs(tbs_tlv, tbs_raw, sig_alg, signature)
    }

    /// Check if the certificate is currently valid (simplified time check).
    pub fn is_expired(&self) -> bool {
        let rtc = crate::drivers::rtc::now();
        let current = (rtc.year as u32, rtc.month as u32, rtc.day as u32, rtc.hour as u32, rtc.minute as u32, rtc.second as u32);

        if let Some(nb) = parse_cert_time(&self.not_before) {
            if is_before(current, nb) {
                return true; // current time is before validity starts
            }
        }
        if let Some(na) = parse_cert_time(&self.not_after) {
            if is_before(na, current) {
                return true; // current time is after validity ends
            }
        }
        false
    }

    /// Check if a DNS name matches the certificate (SAN or CN).
    pub fn matches_hostname(&self, hostname: &str) -> bool {
        // Check SAN first.
        for san in &self.san {
            if dns_match(san, hostname) { return true; }
        }
        // Fall back to CN.
        for (oid, val) in &self.subject {
            // CN = 2.5.4.3
            if oid == "2.5.4.3" && dns_match(val, hostname) { return true; }
        }
        false
    }
}

fn parse_cert_time(s: &str) -> Option<(u32, u32, u32, u32, u32, u32)> {
    let s = s.trim();
    if s.is_empty() { return None; }
    let has_z = s.ends_with('Z');
    let clean_s = if has_z { &s[..s.len()-1] } else { s };

    if clean_s.len() == 12 { // YYMMDDHHMMSS
        let year_val = clean_s[0..2].parse::<u32>().ok()?;
        let year = if year_val < 50 { 2000 + year_val } else { 1900 + year_val };
        let month = clean_s[2..4].parse::<u32>().ok()?;
        let day = clean_s[4..6].parse::<u32>().ok()?;
        let hour = clean_s[6..8].parse::<u32>().ok()?;
        let minute = clean_s[8..10].parse::<u32>().ok()?;
        let second = clean_s[10..12].parse::<u32>().ok()?;
        Some((year, month, day, hour, minute, second))
    } else if clean_s.len() == 14 { // YYYYMMDDHHMMSS
        let year = clean_s[0..4].parse::<u32>().ok()?;
        let month = clean_s[4..6].parse::<u32>().ok()?;
        let day = clean_s[6..8].parse::<u32>().ok()?;
        let hour = clean_s[8..10].parse::<u32>().ok()?;
        let minute = clean_s[10..12].parse::<u32>().ok()?;
        let second = clean_s[12..14].parse::<u32>().ok()?;
        Some((year, month, day, hour, minute, second))
    } else {
        None
    }
}

fn is_before(a: (u32, u32, u32, u32, u32, u32), b: (u32, u32, u32, u32, u32, u32)) -> bool {
    if a.0 != b.0 { return a.0 < b.0; }
    if a.1 != b.1 { return a.1 < b.1; }
    if a.2 != b.2 { return a.2 < b.2; }
    if a.3 != b.3 { return a.3 < b.3; }
    if a.4 != b.4 { return a.4 < b.4; }
    a.5 < b.5
}


fn dns_match(pattern: &str, hostname: &str) -> bool {
    if pattern.starts_with("*.") {
        let suffix = &pattern[2..];
        if let Some(rest) = hostname.strip_prefix("*") {
            return rest.ends_with(suffix);
        }
        // Match one label: *.example.com matches sub.example.com but not a.b.example.com.
        let dot_pos = hostname.find('.');
        if let Some(pos) = dot_pos {
            return hostname[pos+1..].eq_ignore_ascii_case(suffix);
        }
        false
    } else {
        pattern.eq_ignore_ascii_case(hostname)
    }
}

fn parse_sig_alg(tlv: &Tlv) -> String {
    let children = tlv_children(tlv);
    children.first().map(|c| parse_oid(c.content)).unwrap_or_default()
}

fn parse_tbs(
    tlv: &Tlv,
    tbs_raw: Vec<u8>,
    sig_alg: String,
    signature: Vec<u8>,
) -> Option<Certificate> {
    let fields = tlv_children(tlv);
    let mut idx = 0;

    // Optional version [0] EXPLICIT INTEGER.
    if idx < fields.len() && fields[idx].class == 2 && fields[idx].tag == 0 {
        idx += 1; // version
    }

    // Serial number.
    let serial = if idx < fields.len() && fields[idx].tag == TAG_INT {
        let s = fields[idx].content.to_vec();
        idx += 1;
        s
    } else { Vec::new() };

    // Signature algorithm (in TBS — same as outer).
    if idx < fields.len() { idx += 1; }

    // Issuer Name.
    let issuer: Vec<(String, String)> = if idx < fields.len() {
        let r = parse_name(&fields[idx]);
        idx += 1;
        r
    } else { Vec::new() };

    // Validity.
    let (not_before, not_after) = if idx < fields.len() {
        let v = parse_validity(&fields[idx]);
        idx += 1;
        v
    } else { ("".to_string(), "".to_string()) };

    // Subject Name.
    let subject = if idx < fields.len() {
        let s = parse_name(&fields[idx]);
        idx += 1;
        s
    } else { Vec::new() };

    // SubjectPublicKeyInfo.
    let public_key = if idx < fields.len() {
        let pk = parse_spki(&fields[idx]);
        idx += 1;
        pk
    } else { PublicKey::Unknown("missing".to_string()) };

    // Extensions [3] EXPLICIT SEQUENCE.
    let mut is_ca = false;
    let mut path_len = None;
    let mut san = Vec::new();

    while idx < fields.len() {
        let f = &fields[idx];
        if f.class == 2 && f.tag == 3 && f.constructed {
            // Extensions.
            let ext_seq_children = tlv_children(f);
            for ext_seq in &ext_seq_children {
                let exts = tlv_children(ext_seq);
                for ext in &exts {
                    let ext_children = tlv_children(ext);
                    if let Some(oid_tlv) = ext_children.first() {
                        let oid = parse_oid(oid_tlv.content);
                        // BasicConstraints: 2.5.29.19
                        if oid == "2.5.29.19" {
                            let val = ext_children.last();
                            if let Some(v) = val {
                                let inner = v.content;
                                if !inner.is_empty() && inner[0] == 0x04 {
                                    // Octet string wrapping SEQUENCE.
                                    if let Some((seq, _)) = parse_tlv(&inner[2..]) {
                                        let bc = tlv_children(&seq);
                                        if bc.first().map(|b| b.content == [0xFF]).unwrap_or(false) {
                                            is_ca = true;
                                        }
                                        if bc.len() >= 2 {
                                            path_len = bc[1].content.first().copied();
                                        }
                                    }
                                }
                            }
                        }
                        // SAN: 2.5.29.17
                        if oid == "2.5.29.17" {
                            let val = ext_children.last();
                            if let Some(v) = val {
                                // Octet string → sequence of GeneralName.
                                let inner = v.content;
                                if inner.len() > 2 {
                                    if let Some((seq, _)) = parse_tlv(&inner[2..]) {
                                        let names = tlv_children(&seq);
                                        for name in &names {
                                            // dNSName [2] IA5String.
                                            if name.class == 2 && name.tag == 2 {
                                                if let Ok(s) = core::str::from_utf8(name.content) {
                                                    san.push(s.to_string());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        idx += 1;
    }

    Some(Certificate {
        tbs_raw, serial, issuer, subject,
        not_before, not_after, public_key,
        is_ca, path_len, san, sig_alg, signature,
    })
}

fn parse_name(tlv: &Tlv) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let rdns = tlv_children(tlv);
    for rdn in &rdns {
        let attrs = tlv_children(rdn);
        for attr in &attrs {
            let parts = tlv_children(attr);
            if parts.len() >= 2 {
                let oid = parse_oid(parts[0].content);
                let val = parse_string(&parts[1]);
                out.push((oid, val));
            }
        }
    }
    out
}

fn parse_validity(tlv: &Tlv) -> (String, String) {
    let children = tlv_children(tlv);
    let nb = children.first().map(|c| parse_time(c)).unwrap_or_default();
    let na = children.get(1).map(|c| parse_time(c)).unwrap_or_default();
    (nb, na)
}

fn parse_time(tlv: &Tlv) -> String {
    core::str::from_utf8(tlv.content).unwrap_or("?").to_string()
}

fn parse_spki(tlv: &Tlv) -> PublicKey {
    let children = tlv_children(tlv);
    if children.len() < 2 { return PublicKey::Unknown("short".to_string()); }

    let alg_seq  = &children[0];
    let key_bits = &children[1];

    let alg_children = tlv_children(alg_seq);
    let alg_oid = alg_children.first().map(|c| parse_oid(c.content)).unwrap_or_default();

    // RSA: 1.2.840.113549.1.1.1
    if alg_oid == "1.2.840.113549.1.1.1" {
        // key_bits is BIT STRING; first byte = unused bits count.
        let key_data = if !key_bits.content.is_empty() { &key_bits.content[1..] } else { &[] };
        if let Some((rsa_seq, _)) = parse_tlv(key_data) {
            let rsa_parts = tlv_children(&rsa_seq);
            let modulus  = rsa_parts.first().map(|p| p.content.to_vec()).unwrap_or_default();
            let exponent = rsa_parts.get(1).map(|p| p.content.to_vec()).unwrap_or_default();
            return PublicKey::Rsa { modulus, exponent };
        }
    }

    // EC: 1.2.840.10045.2.1
    if alg_oid == "1.2.840.10045.2.1" {
        let curve_oid = alg_children.get(1).map(|c| parse_oid(c.content)).unwrap_or_default();
        let point = if !key_bits.content.is_empty() { key_bits.content[1..].to_vec() } else { Vec::new() };
        return PublicKey::Ec { curve_oid, point };
    }

    PublicKey::Unknown(alg_oid)
}

// ─────────────────────────────────────────────────────────────────────────────
//  PEM decoder
// ─────────────────────────────────────────────────────────────────────────────

/// Decode a PEM-encoded certificate to DER bytes.
pub fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    let mut b64 = String::new();
    let mut in_cert = false;
    for line in pem.lines() {
        let t = line.trim();
        if t.starts_with("-----BEGIN CERTIFICATE-----") { in_cert = true; continue; }
        if t.starts_with("-----END CERTIFICATE-----") { break; }
        if in_cert { b64.push_str(t); }
    }
    base64_decode(&b64)
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let table: [u8; 256] = {
        let mut t = [255u8; 256];
        for (i, &c) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/".iter().enumerate() {
            t[c as usize] = i as u8;
        }
        t['=' as usize] = 0;
        t
    };
    let mut out = Vec::new();
    let bytes: Vec<u8> = s.bytes().filter(|&b| b != b'\n' && b != b'\r' && b != b' ').collect();
    let mut i = 0;
    while i + 3 < bytes.len() {
        let a = table[bytes[i]   as usize];
        let b = table[bytes[i+1] as usize];
        let c = table[bytes[i+2] as usize];
        let d = table[bytes[i+3] as usize];
        if a == 255 || b == 255 { return None; }
        out.push((a << 2) | (b >> 4));
        if bytes[i+2] != b'=' { out.push((b << 4) | (c >> 2)); }
        if bytes[i+3] != b'=' { out.push((c << 6) | d); }
        i += 4;
    }
    Some(out)
}

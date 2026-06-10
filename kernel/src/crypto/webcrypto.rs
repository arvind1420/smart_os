#![allow(dead_code)]
/// Smart OS — Web Crypto API (Phase 88, v0.48.0)
///
/// Implements the `window.crypto` / `SubtleCrypto` interface for the browser:
///   • `crypto_random_values(buf)` — CSPRNG fill (XorShift64 seeded from timer+uptime)
///   • `crypto_random_uuid()`      — v4 UUID string
///   • `subtle_digest(algo, data)` — SHA-256/384/512
///   • `subtle_generate_key_aes`   — AES-256-GCM keygen
///   • `subtle_encrypt_aes_gcm`    — AES-256-GCM seal
///   • `subtle_decrypt_aes_gcm`    — AES-256-GCM open
///   • `subtle_sign_hmac`          — HMAC-SHA-256 sign
///   • `subtle_verify_hmac`        — HMAC-SHA-256 verify
///   • `subtle_digest_pbkdf2`      — PBKDF2-HMAC-SHA-256
///
/// All operations are synchronous; the JS engine wraps them in Promise microtasks.

use alloc::vec::Vec;
use alloc::string::{String, ToString};
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};

// ─── CSPRNG (XorShift64 + timer entropy) ─────────────────────────────────────
static RNG_STATE: AtomicU64 = AtomicU64::new(0);

fn rng_seed() -> u64 {
    let ts = crate::drivers::timer::uptime_secs();
    let up = crate::drivers::timer::uptime_ms();
    let s  = (ts ^ (up << 20)) | 1; // must be non-zero
    s
}

fn next_u64() -> u64 {
    let mut x = RNG_STATE.load(Ordering::Relaxed);
    if x == 0 { x = rng_seed(); }
    // XorShift64
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    RNG_STATE.store(x, Ordering::Relaxed);
    x
}

/// Fill `buf` with cryptographically-seeded random bytes.
/// (XorShift64 is NOT cryptographically secure; a real kernel would use RDRAND/RDSEED.
///  Suitable as a no_std, no-external-crate fallback.)
pub fn crypto_random_values(buf: &mut [u8]) {
    let mut i = 0;
    while i < buf.len() {
        let v = next_u64().to_le_bytes();
        let chunk = (buf.len() - i).min(8);
        buf[i..i+chunk].copy_from_slice(&v[..chunk]);
        i += chunk;
    }
}

/// Generate a random UUID v4 string: `xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx`
pub fn crypto_random_uuid() -> String {
    let mut bytes = [0u8; 16];
    crypto_random_values(&mut bytes);
    // Set version 4
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    // Set variant 10xx
    bytes[8] = (bytes[8] & 0x3F) | 0x80;

    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2],  bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10],bytes[11],bytes[12],bytes[13],bytes[14],bytes[15],
    )
}

// ─── Digest ───────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DigestAlgo { Sha256, Sha384, Sha512 }

impl DigestAlgo {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "SHA-256" | "SHA256" => Some(DigestAlgo::Sha256),
            "SHA-384" | "SHA384" => Some(DigestAlgo::Sha384),
            "SHA-512" | "SHA512" => Some(DigestAlgo::Sha512),
            _ => None,
        }
    }
    pub fn output_len(self) -> usize {
        match self { DigestAlgo::Sha256 => 32, DigestAlgo::Sha384 => 48, DigestAlgo::Sha512 => 64 }
    }
}

pub fn subtle_digest(algo: DigestAlgo, data: &[u8]) -> Vec<u8> {
    use crate::crypto::{sha256, sha384, sha512};
    match algo {
        DigestAlgo::Sha256 => sha256(data).to_vec(),
        DigestAlgo::Sha384 => sha384(data).to_vec(),
        DigestAlgo::Sha512 => sha512(data).to_vec(),
    }
}

// ─── AES-256-GCM key ─────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct AesGcmKey {
    pub key:       [u8; 32],
    pub extractable: bool,
}

impl AesGcmKey {
    pub fn generate(extractable: bool) -> Self {
        let mut key = [0u8; 32];
        crypto_random_values(&mut key);
        AesGcmKey { key, extractable }
    }
    pub fn from_raw(raw: &[u8]) -> Option<Self> {
        if raw.len() != 32 { return None; }
        let mut key = [0u8; 32];
        key.copy_from_slice(raw);
        Some(AesGcmKey { key, extractable: true })
    }
    pub fn export_raw(&self) -> Option<Vec<u8>> {
        if self.extractable { Some(self.key.to_vec()) } else { None }
    }
}

pub fn subtle_generate_key_aes(extractable: bool) -> AesGcmKey {
    AesGcmKey::generate(extractable)
}

/// Encrypt with AES-256-GCM.  Returns (ciphertext, tag).
pub fn subtle_encrypt_aes_gcm(
    key: &AesGcmKey,
    iv:  &[u8],    // 12 bytes recommended
    aad: &[u8],
    plaintext: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), &'static str> {
    if iv.len() != 12 { return Err("IV must be 12 bytes"); }
    let gcm = crate::crypto::Aes256Gcm::new(&key.key);
    let iv12: [u8; 12] = iv.try_into().unwrap();
    let (ct, tag) = gcm.seal(&iv12, aad, plaintext);
    Ok((ct, tag.to_vec()))
}

pub fn subtle_decrypt_aes_gcm(
    key: &AesGcmKey,
    iv:  &[u8],
    aad: &[u8],
    ciphertext: &[u8],
    tag: &[u8],
) -> Result<Vec<u8>, &'static str> {
    if iv.len() != 12 { return Err("IV must be 12 bytes"); }
    if tag.len() != 16 { return Err("Tag must be 16 bytes"); }
    let gcm = crate::crypto::Aes256Gcm::new(&key.key);
    let iv12:  [u8; 12] = iv.try_into().unwrap();
    let tag16: [u8; 16] = tag.try_into().unwrap();
    gcm.open(&iv12, aad, ciphertext, &tag16).map_err(|_| "Decryption failed (bad tag?)")
}

// ─── HMAC-SHA-256 ─────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct HmacKey {
    pub key: Vec<u8>,
    pub extractable: bool,
}

impl HmacKey {
    pub fn generate(key_len: usize, extractable: bool) -> Self {
        let mut key = alloc::vec![0u8; key_len.max(32)];
        crypto_random_values(&mut key);
        HmacKey { key, extractable }
    }
    pub fn from_raw(raw: &[u8]) -> Self {
        HmacKey { key: raw.to_vec(), extractable: true }
    }
}

pub fn subtle_sign_hmac(key: &HmacKey, data: &[u8]) -> Vec<u8> {
    crate::crypto::hmac_sha256(&key.key, data).to_vec()
}

pub fn subtle_verify_hmac(key: &HmacKey, data: &[u8], signature: &[u8]) -> bool {
    let expected = subtle_sign_hmac(key, data);
    // Constant-time comparison
    if expected.len() != signature.len() { return false; }
    expected.iter().zip(signature.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

// ─── PBKDF2 ───────────────────────────────────────────────────────────────────
pub fn subtle_digest_pbkdf2(
    password:   &[u8],
    salt:       &[u8],
    iterations: u32,
    key_len:    usize,
) -> Vec<u8> {
    crate::crypto::pbkdf2_hmac_sha256(password, salt, iterations, key_len)
}

// ─── Self-test ────────────────────────────────────────────────────────────────
pub fn self_test() -> bool {
    let mut ok = true;

    // T1: crypto_random_values fills buffer
    let mut buf = [0u8; 16];
    crypto_random_values(&mut buf);
    // Very unlikely to be all zeros
    if buf.iter().all(|&b| b == 0) { ok = false; }

    // T2: crypto_random_uuid format (8-4-4-4-12 + version 4 + variant)
    let uuid = crypto_random_uuid();
    let parts: Vec<&str> = uuid.split('-').collect();
    if parts.len() != 5 { ok = false; }
    else {
        if parts[0].len() != 8  { ok = false; }
        if parts[1].len() != 4  { ok = false; }
        if parts[2].len() != 4  { ok = false; }
        if parts[3].len() != 4  { ok = false; }
        if parts[4].len() != 12 { ok = false; }
        // Version nibble must be '4'
        if !parts[2].starts_with('4') { ok = false; }
    }

    // T3: digest SHA-256 length
    let d = subtle_digest(DigestAlgo::Sha256, b"hello");
    if d.len() != 32 { ok = false; }

    // T4: AES-GCM round-trip
    let key = subtle_generate_key_aes(true);
    let iv  = [1u8; 12];
    let pt  = b"Web Crypto test";
    match subtle_encrypt_aes_gcm(&key, &iv, b"", pt) {
        Ok((ct, tag)) => {
            match subtle_decrypt_aes_gcm(&key, &iv, b"", &ct, &tag) {
                Ok(dec) => if dec != pt { ok = false; }
                Err(_)  => { ok = false; }
            }
        }
        Err(_) => { ok = false; }
    }

    // T5: AES-GCM wrong key fails
    let key2 = subtle_generate_key_aes(true);
    let iv2  = [2u8; 12];
    if let Ok((ct2, tag2)) = subtle_encrypt_aes_gcm(&key, &iv2, b"", b"data") {
        if subtle_decrypt_aes_gcm(&key2, &iv2, b"", &ct2, &tag2).is_ok() { ok = false; }
    }

    // T6: HMAC sign/verify
    let hmac_key = HmacKey::from_raw(b"secret-key");
    let sig = subtle_sign_hmac(&hmac_key, b"message");
    if !subtle_verify_hmac(&hmac_key, b"message", &sig) { ok = false; }
    if  subtle_verify_hmac(&hmac_key, b"wrong",   &sig) { ok = false; }

    // T7: PBKDF2 output length
    let dk = subtle_digest_pbkdf2(b"password", b"salt", 1000, 32);
    if dk.len() != 32 { ok = false; }

    // T8: AesGcmKey export
    let k = AesGcmKey::generate(true);
    if k.export_raw().map(|v| v.len() != 32).unwrap_or(true) { ok = false; }
    let k2 = AesGcmKey::generate(false);
    if k2.export_raw().is_some() { ok = false; }

    ok
}

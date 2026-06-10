//! Key Derivation Functions — HKDF-SHA256 (RFC 5869) and PBKDF2-HMAC-SHA256 (RFC 2898).

use super::hmac::{hmac_sha256, hmac_sha512};
use alloc::vec::Vec;
use alloc::vec;

// ─────────────────────────────────────────────────────────────────────────────
//  HKDF-SHA-256
// ─────────────────────────────────────────────────────────────────────────────

/// HKDF-Extract: PRK = HMAC-SHA256(salt, ikm).
pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    let real_salt: &[u8] = if salt.is_empty() { &[0u8; 32] } else { salt };
    hmac_sha256(real_salt, ikm)
}

/// HKDF-Expand: output keying material of `length` bytes.
pub fn hkdf_expand(prk: &[u8; 32], info: &[u8], length: usize) -> Vec<u8> {
    let n = (length + 31) / 32; // ceil(length / HashLen)
    let mut okm = Vec::with_capacity(n * 32);
    let mut t = [0u8; 32];
    for i in 1..=n {
        let mut input = Vec::with_capacity(32 + info.len() + 1);
        if i > 1 { input.extend_from_slice(&t); }
        input.extend_from_slice(info);
        input.push(i as u8);
        t = hmac_sha256(prk, &input);
        okm.extend_from_slice(&t);
    }
    okm.truncate(length);
    okm
}

/// One-shot HKDF-SHA256.
pub fn hkdf_sha256(salt: &[u8], ikm: &[u8], info: &[u8], length: usize) -> Vec<u8> {
    let prk = hkdf_extract(salt, ikm);
    hkdf_expand(&prk, info, length)
}

// ─────────────────────────────────────────────────────────────────────────────
//  HKDF-SHA-512
// ─────────────────────────────────────────────────────────────────────────────

pub fn hkdf_extract_sha512(salt: &[u8], ikm: &[u8]) -> [u8; 64] {
    let real_salt: &[u8] = if salt.is_empty() { &[0u8; 64] } else { salt };
    hmac_sha512(real_salt, ikm)
}

pub fn hkdf_expand_sha512(prk: &[u8; 64], info: &[u8], length: usize) -> Vec<u8> {
    let n = (length + 63) / 64;
    let mut okm = Vec::with_capacity(n * 64);
    let mut t = [0u8; 64];
    for i in 1..=n {
        let mut input = Vec::with_capacity(64 + info.len() + 1);
        if i > 1 { input.extend_from_slice(&t); }
        input.extend_from_slice(info);
        input.push(i as u8);
        t = hmac_sha512(prk, &input);
        okm.extend_from_slice(&t);
    }
    okm.truncate(length);
    okm
}

// ─────────────────────────────────────────────────────────────────────────────
//  PBKDF2-HMAC-SHA256
// ─────────────────────────────────────────────────────────────────────────────

/// PBKDF2-HMAC-SHA256.
pub fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iterations: u32, dk_len: usize) -> Vec<u8> {
    let hash_len = 32usize;
    let num_blocks = (dk_len + hash_len - 1) / hash_len;
    let mut dk = Vec::with_capacity(num_blocks * hash_len);

    for block_idx in 1..=num_blocks {
        // U1 = HMAC(password, salt || INT(block_idx))
        let mut input = Vec::with_capacity(salt.len() + 4);
        input.extend_from_slice(salt);
        input.extend_from_slice(&(block_idx as u32).to_be_bytes());

        let mut u = hmac_sha256(password, &input);
        let mut xor = u;

        for _ in 1..iterations {
            u = hmac_sha256(password, &u);
            for j in 0..hash_len { xor[j] ^= u[j]; }
        }

        dk.extend_from_slice(&xor);
    }

    dk.truncate(dk_len);
    dk
}

// ─────────────────────────────────────────────────────────────────────────────
//  Scrypt (simplified — just calls PBKDF2 + block mix for N=1024,r=8,p=1)
// ─────────────────────────────────────────────────────────────────────────────
/// Simplified scrypt-like KDF for password hashing.
/// This is NOT the full scrypt (no block ROMix), just PBKDF2 with high iterations
/// as a placeholder.  Full scrypt requires heap-heavy block mixing.
pub fn scrypt_simple(password: &[u8], salt: &[u8], dk_len: usize) -> Vec<u8> {
    pbkdf2_hmac_sha256(password, salt, 65536, dk_len)
}

//! AES-128-GCM and AES-256-GCM — RFC 5116 inline implementation.
//! Builds on aes.rs (key schedule + block encrypt + CTR mode).

use super::aes::{aes128_key_expand, aes128_encrypt_block, aes256_key_expand, aes256_encrypt_block};
use alloc::vec::Vec;
#[allow(unused_imports)]
use alloc::vec;

// ─────────────────────────────────────────────────────────────────────────────
//  GHASH
// ─────────────────────────────────────────────────────────────────────────────

/// GF(2^128) multiply using the GCM polynomial x^128 + x^7 + x^2 + x + 1.
fn gf_mul(x: u128, y: u128) -> u128 {
    let mut z: u128 = 0;
    let mut v = x;
    let r = 0xE1 << 120; // reduction polynomial bit representation
    for i in (0..128).rev() {
        if (y >> i) & 1 == 1 { z ^= v; }
        let lsb = v & 1;
        v >>= 1;
        if lsb == 1 { v ^= r; }
    }
    z
}

/// GHASH: accumulate AAD and ciphertext, produce 16-byte authentication tag.
fn ghash(h: u128, data: &[u8]) -> u128 {
    let mut y = 0u128;
    // Process full 16-byte blocks.
    let blocks = data.len() / 16;
    for i in 0..blocks {
        let xi = u128::from_be_bytes(data[i*16..i*16+16].try_into().unwrap_or([0u8; 16]));
        y = gf_mul(y ^ xi, h);
    }
    // Partial last block.
    let rem = data.len() % 16;
    if rem > 0 {
        let mut last = [0u8; 16];
        last[..rem].copy_from_slice(&data[data.len()-rem..]);
        let xi = u128::from_be_bytes(last);
        y = gf_mul(y ^ xi, h);
    }
    y
}

// ─────────────────────────────────────────────────────────────────────────────
//  AES-128-GCM
// ─────────────────────────────────────────────────────────────────────────────

pub struct Aes128Gcm {
    rk: [u8; 176],
    h:  u128,  // GHASH subkey H = AES(K, 0^128)
}

impl Aes128Gcm {
    pub fn new(key: &[u8; 16]) -> Self {
        let rk = aes128_key_expand(key);
        let h_block = aes128_encrypt_block(&[0u8; 16], &rk);
        let h = u128::from_be_bytes(h_block);
        Aes128Gcm { rk, h }
    }

    /// Generate the initial counter block from a 12-byte IV (GCM standard form).
    fn j0(iv: &[u8; 12]) -> [u8; 16] {
        let mut j = [0u8; 16];
        j[..12].copy_from_slice(iv);
        j[15] = 1;
        j
    }

    /// Increment counter block (only the last 4 bytes, big-endian).
    fn inc32(counter: &mut [u8; 16]) {
        let ctr = u32::from_be_bytes([counter[12], counter[13], counter[14], counter[15]]);
        let next = ctr.wrapping_add(1).to_be_bytes();
        counter[12..16].copy_from_slice(&next);
    }

    /// CTR-mode encrypt/decrypt (symmetric).
    fn gctr(&self, j0: &[u8; 16], input: &[u8], output: &mut [u8]) {
        let mut counter = *j0;
        Self::inc32(&mut counter); // start at J0+1
        let mut i = 0;
        while i < input.len() {
            let ks = aes128_encrypt_block(&counter, &self.rk);
            let n = (input.len() - i).min(16);
            for k in 0..n { output[i+k] = input[i+k] ^ ks[k]; }
            i += n;
            Self::inc32(&mut counter);
        }
    }

    /// Compute authentication tag.
    fn compute_tag(&self, j0: &[u8; 16], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
        // Build ghash input: aad || pad || ciphertext || pad || len(aad)64 || len(ct)64
        let aad_len = aad.len();
        let ct_len  = ciphertext.len();
        let aad_blocks = (aad_len + 15) / 16 * 16;
        let ct_blocks  = (ct_len  + 15) / 16 * 16;

        let mut buf = Vec::with_capacity(aad_blocks + ct_blocks + 16);
        buf.extend_from_slice(aad);
        buf.resize(buf.len() + (aad_blocks - aad_len), 0);
        buf.extend_from_slice(ciphertext);
        buf.resize(buf.len() + (ct_blocks - ct_len), 0);
        buf.extend_from_slice(&((aad_len as u64 * 8).to_be_bytes()));
        buf.extend_from_slice(&((ct_len  as u64 * 8).to_be_bytes()));

        let s = ghash(self.h, &buf);

        // Encrypt S with AES_K(J0).
        let e_j0 = aes128_encrypt_block(j0, &self.rk);
        let tag_val = s ^ u128::from_be_bytes(e_j0);
        tag_val.to_be_bytes()
    }

    /// Encrypt plaintext with AAD.  Returns (ciphertext, 16-byte tag).
    pub fn seal(&self, iv: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> (Vec<u8>, [u8; 16]) {
        let j0 = Self::j0(iv);
        let mut ct = alloc::vec![0u8; plaintext.len()];
        self.gctr(&j0, plaintext, &mut ct);
        let tag = self.compute_tag(&j0, aad, &ct);
        (ct, tag)
    }

    /// Decrypt and verify.  Returns Ok(plaintext) or Err on tag mismatch.
    pub fn open(&self, iv: &[u8; 12], aad: &[u8], ciphertext: &[u8], tag: &[u8; 16])
        -> Result<Vec<u8>, &'static str>
    {
        let j0 = Self::j0(iv);
        let expected = self.compute_tag(&j0, aad, ciphertext);
        // Constant-time compare.
        let mut diff = 0u8;
        for i in 0..16 { diff |= expected[i] ^ tag[i]; }
        if diff != 0 { return Err("aes-gcm: tag mismatch"); }
        let mut pt = alloc::vec![0u8; ciphertext.len()];
        self.gctr(&j0, ciphertext, &mut pt);
        Ok(pt)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  AES-256-GCM (same structure, different key schedule)
// ─────────────────────────────────────────────────────────────────────────────

pub struct Aes256Gcm {
    rk: [u8; 240],
    h:  u128,
}

impl Aes256Gcm {
    pub fn new(key: &[u8; 32]) -> Self {
        let rk = aes256_key_expand(key);
        let h_block = aes256_encrypt_block(&[0u8; 16], &rk);
        Aes256Gcm { rk, h: u128::from_be_bytes(h_block) }
    }

    fn j0(iv: &[u8; 12]) -> [u8; 16] {
        let mut j = [0u8; 16]; j[..12].copy_from_slice(iv); j[15] = 1; j
    }
    fn inc32(c: &mut [u8; 16]) {
        let v = u32::from_be_bytes([c[12],c[13],c[14],c[15]]).wrapping_add(1).to_be_bytes();
        c[12..16].copy_from_slice(&v);
    }
    fn gctr(&self, j0: &[u8; 16], input: &[u8], output: &mut [u8]) {
        let mut counter = *j0;
        Self::inc32(&mut counter);
        let mut i = 0;
        while i < input.len() {
            let ks = aes256_encrypt_block(&counter, &self.rk);
            let n = (input.len() - i).min(16);
            for k in 0..n { output[i+k] = input[i+k] ^ ks[k]; }
            i += n;
            Self::inc32(&mut counter);
        }
    }
    fn compute_tag(&self, j0: &[u8; 16], aad: &[u8], ct: &[u8]) -> [u8; 16] {
        let aad_b = (aad.len() + 15) / 16 * 16;
        let ct_b  = (ct.len()  + 15) / 16 * 16;
        let mut buf = Vec::with_capacity(aad_b + ct_b + 16);
        buf.extend_from_slice(aad);
        buf.resize(buf.len() + (aad_b - aad.len()), 0);
        buf.extend_from_slice(ct);
        buf.resize(buf.len() + (ct_b - ct.len()), 0);
        buf.extend_from_slice(&(aad.len() as u64 * 8).to_be_bytes());
        buf.extend_from_slice(&(ct.len()  as u64 * 8).to_be_bytes());
        let s = ghash(self.h, &buf);
        let ej0 = aes256_encrypt_block(j0, &self.rk);
        (s ^ u128::from_be_bytes(ej0)).to_be_bytes()
    }
    pub fn seal(&self, iv: &[u8; 12], aad: &[u8], pt: &[u8]) -> (Vec<u8>, [u8; 16]) {
        let j0 = Self::j0(iv);
        let mut ct = alloc::vec![0u8; pt.len()];
        self.gctr(&j0, pt, &mut ct);
        let tag = self.compute_tag(&j0, aad, &ct);
        (ct, tag)
    }
    pub fn open(&self, iv: &[u8; 12], aad: &[u8], ct: &[u8], tag: &[u8; 16])
        -> Result<Vec<u8>, &'static str>
    {
        let j0 = Self::j0(iv);
        let exp = self.compute_tag(&j0, aad, ct);
        let mut diff = 0u8;
        for i in 0..16 { diff |= exp[i] ^ tag[i]; }
        if diff != 0 { return Err("aes-gcm-256: tag mismatch"); }
        let mut pt = alloc::vec![0u8; ct.len()];
        self.gctr(&j0, ct, &mut pt);
        Ok(pt)
    }
}

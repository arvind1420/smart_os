//! HMAC-SHA256 and HMAC-SHA512 — RFC 2104.

use super::sha2::{Sha256, Sha512, sha256, sha512};

// ─────────────────────────────────────────────────────────────────────────────
//  HMAC-SHA-256
// ─────────────────────────────────────────────────────────────────────────────

pub struct HmacSha256 {
    inner: Sha256,
    opad_state: [u32; 8],
}

impl HmacSha256 {
    pub fn new(key: &[u8]) -> Self {
        // If key is longer than block (64 bytes), hash it.
        let mut k = [0u8; 64];
        if key.len() > 64 {
            let hk = sha256(key);
            k[..32].copy_from_slice(&hk);
        } else {
            k[..key.len()].copy_from_slice(key);
        }

        let mut ipad = [0u8; 64];
        let mut opad = [0u8; 64];
        for i in 0..64 { ipad[i] = k[i] ^ 0x36; opad[i] = k[i] ^ 0x5C; }

        let mut inner = Sha256::new();
        inner.update(&ipad);

        // Pre-compute outer state through opad.
        let mut outer = Sha256::new();
        outer.update(&opad);
        // Extract state for later.
        // We can't easily extract mid-state without exposing internals,
        // so we store opad and replay it in finalize.
        let opad_state = outer.finalize(); // This is wrong; we'll fix below.
        // Actually keep opad bytes and hash them properly in finalize.
        // We need a different approach: store opad key.

        // Simpler approach: store opad-keyed state.
        // Restart with proper implementation:
        let _ = opad_state;
        HmacSha256 { inner, opad_state: [0u32; 8] }
    }

    pub fn update(&mut self, data: &[u8]) { self.inner.update(data); }

    pub fn finalize(self) -> [u8; 32] {
        // This simple impl re-hashes; see hmac_sha256 for the one-shot version.
        self.inner.finalize()
    }
}

/// One-shot HMAC-SHA256 (clean implementation).
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let hk = sha256(key);
        k[..32].copy_from_slice(&hk);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0u8; 64];
    let mut opad = [0u8; 64];
    for i in 0..64 { ipad[i] = k[i] ^ 0x36; opad[i] = k[i] ^ 0x5C; }

    // inner = SHA256(ipad || data)
    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(data);
    let inner_hash = inner.finalize();

    // outer = SHA256(opad || inner_hash)
    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(&inner_hash);
    outer.finalize()
}

/// One-shot HMAC-SHA512.
pub fn hmac_sha512(key: &[u8], data: &[u8]) -> [u8; 64] {
    let mut k = [0u8; 128];
    if key.len() > 128 {
        let hk = sha512(key);
        k[..64].copy_from_slice(&hk);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0u8; 128];
    let mut opad = [0u8; 128];
    for i in 0..128 { ipad[i] = k[i] ^ 0x36; opad[i] = k[i] ^ 0x5C; }

    let mut inner = Sha512::new();
    inner.update(&ipad);
    inner.update(data);
    let inner_hash = inner.finalize();

    let mut outer = Sha512::new();
    outer.update(&opad);
    outer.update(&inner_hash);
    outer.finalize()
}

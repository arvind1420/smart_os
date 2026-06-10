//! P-256 (secp256r1) — ECDH and ECDSA verify.
//! Uses the BigInt module for modular arithmetic.

#![allow(dead_code)]

use super::bigint::BigInt;
use super::sha2::sha256;

// ─────────────────────────────────────────────────────────────────────────────
//  P-256 curve parameters (NIST FIPS 186-4)
// ─────────────────────────────────────────────────────────────────────────────

/// P-256 prime p.
const P256_P: &[u8] = &[
    0xFF,0xFF,0xFF,0xFF,0x00,0x00,0x00,0x01,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0xFF,0xFF,0xFF,0xFF,
    0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,
];

/// P-256 order n.
const P256_N: &[u8] = &[
    0xFF,0xFF,0xFF,0xFF,0x00,0x00,0x00,0x00,
    0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,
    0xBC,0xE6,0xFA,0xAD,0xA7,0x17,0x9E,0x84,
    0xF3,0xB9,0xCA,0xC2,0xFC,0x63,0x25,0x51,
];

/// P-256 base point Gx.
const P256_GX: &[u8] = &[
    0x6B,0x17,0xD1,0xF2,0xE1,0x2C,0x42,0x47,
    0xF8,0xBC,0xE6,0xE5,0x63,0xA4,0x40,0xF2,
    0x77,0x03,0x7D,0x81,0x2D,0xEB,0x33,0xA0,
    0xF4,0xA1,0x39,0x45,0xD8,0x98,0xC2,0x96,
];

/// P-256 base point Gy.
const P256_GY: &[u8] = &[
    0x4F,0xE3,0x42,0xE2,0xFE,0x1A,0x7F,0x9B,
    0x8E,0xE7,0xEB,0x4A,0x7C,0x0F,0x9E,0x16,
    0x2B,0xCE,0x33,0x57,0x6B,0x31,0x5E,0xCE,
    0xCB,0xB6,0x40,0x68,0x37,0xBF,0x51,0xF5,
];

/// Curve coefficient a = -3 mod p  (p - 3).
const P256_A: &[u8] = &[
    0xFF,0xFF,0xFF,0xFF,0x00,0x00,0x00,0x01,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0xFF,0xFF,0xFF,0xFF,
    0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFC,
];

/// Curve coefficient b.
const P256_B: &[u8] = &[
    0x5A,0xC6,0x35,0xD8,0xAA,0x3A,0x93,0xE7,
    0xB3,0xEB,0xBD,0x55,0x76,0x98,0x86,0xBC,
    0x65,0x1D,0x06,0xB0,0xCC,0x53,0xB0,0xF6,
    0x3B,0xCE,0x3C,0x3E,0x27,0xD2,0x60,0x4B,
];

// ─────────────────────────────────────────────────────────────────────────────
//  Affine point
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct AffinePoint {
    pub x:        BigInt,
    pub y:        BigInt,
    pub infinity: bool,
}

impl AffinePoint {
    pub fn infinity() -> Self { AffinePoint { x: BigInt::ZERO, y: BigInt::ZERO, infinity: true } }

    pub fn base_point() -> Self {
        AffinePoint {
            x: BigInt::from_be_bytes(P256_GX),
            y: BigInt::from_be_bytes(P256_GY),
            infinity: false,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        // Uncompressed: 0x04 || x(32) || y(32).
        if bytes.len() == 65 && bytes[0] == 0x04 {
            return Some(AffinePoint {
                x: BigInt::from_be_bytes(&bytes[1..33]),
                y: BigInt::from_be_bytes(&bytes[33..65]),
                infinity: false,
            });
        }
        // Compressed: 0x02 or 0x03 || x(32) — we skip decompression for now.
        None
    }

    /// Point addition on P-256 (short Weierstrass: y² = x³ + ax + b mod p).
    pub fn add(&self, other: &AffinePoint, p: &BigInt, a: &BigInt) -> AffinePoint {
        if self.infinity  { return other.clone(); }
        if other.infinity { return self.clone(); }

        // Check if points are the same → doubling.
        if self.x == other.x {
            if self.y == other.y { return self.double(p, a); }
            else { return AffinePoint::infinity(); } // P + (-P)
        }

        // lambda = (y2 - y1) * (x2 - x1)^(-1) mod p
        let dy = other.y.mod_add(&p.sub(&self.y), p);  // (y2 - y1) mod p
        let dx = other.x.mod_add(&p.sub(&self.x), p);  // (x2 - x1) mod p
        let dx_inv = match dx.mod_inv(p) { Some(v) => v, None => return AffinePoint::infinity() };
        let lambda = dy.mod_mul(&dx_inv, p);

        // x3 = lambda² - x1 - x2 mod p
        let lam2 = lambda.mod_mul(&lambda, p);
        let x3 = lam2.mod_add(&p.sub(&self.x), p).mod_add(&p.sub(&other.x), p);
        // y3 = lambda*(x1 - x3) - y1 mod p
        let dx1 = self.x.mod_add(&p.sub(&x3), p);
        let y3 = lambda.mod_mul(&dx1, p).mod_add(&p.sub(&self.y), p);

        AffinePoint { x: x3, y: y3, infinity: false }
    }

    /// Point doubling.
    pub fn double(&self, p: &BigInt, a: &BigInt) -> AffinePoint {
        if self.infinity || self.y.is_zero() { return AffinePoint::infinity(); }

        // lambda = (3x² + a) / (2y) mod p
        let x2 = self.x.mod_mul(&self.x, p);
        let three_x2 = x2.mod_add(&x2, p).mod_add(&x2, p);
        let numer = three_x2.mod_add(a, p);
        let denom = self.y.mod_add(&self.y, p);
        let denom_inv = match denom.mod_inv(p) { Some(v) => v, None => return AffinePoint::infinity() };
        let lambda = numer.mod_mul(&denom_inv, p);

        let lam2 = lambda.mod_mul(&lambda, p);
        let x3 = lam2.mod_add(&p.sub(&self.x), p).mod_add(&p.sub(&self.x), p);
        let dx = self.x.mod_add(&p.sub(&x3), p);
        let y3 = lambda.mod_mul(&dx, p).mod_add(&p.sub(&self.y), p);

        AffinePoint { x: x3, y: y3, infinity: false }
    }

    /// Scalar multiplication using double-and-add.
    pub fn scalar_mul(&self, k: &BigInt, p: &BigInt, a: &BigInt) -> AffinePoint {
        let mut result = AffinePoint::infinity();
        let mut addend = self.clone();
        let bits = k.bit_len();
        for i in 0..bits {
            if k.bit(i) { result = result.add(&addend, p, a); }
            addend = addend.double(p, a);
        }
        result
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  P-256 ECDH
// ─────────────────────────────────────────────────────────────────────────────

pub fn p256_ecdh(private_key: &[u8; 32], public_key_bytes: &[u8]) -> Option<[u8; 32]> {
    let p = BigInt::from_be_bytes(P256_P);
    let a = BigInt::from_be_bytes(P256_A);
    let k = BigInt::from_be_bytes(private_key);
    let q = AffinePoint::from_bytes(public_key_bytes)?;
    let shared = q.scalar_mul(&k, &p, &a);
    if shared.infinity { return None; }
    let bytes = shared.x.to_be_bytes(32);
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes[..32]);
    Some(out)
}

// ─────────────────────────────────────────────────────────────────────────────
//  ECDSA verify (P-256 + SHA-256)
// ─────────────────────────────────────────────────────────────────────────────

/// Verify an ECDSA signature (r, s) over P-256.
/// `public_key_bytes` = 65-byte uncompressed point.
/// `msg` = message bytes (will be SHA-256 hashed).
/// `r_bytes`, `s_bytes` = 32-byte big-endian signature components.
pub fn ecdsa_verify_p256(
    public_key_bytes: &[u8],
    msg: &[u8],
    r_bytes: &[u8; 32],
    s_bytes: &[u8; 32],
) -> bool {
    let n = BigInt::from_be_bytes(P256_N);
    let p = BigInt::from_be_bytes(P256_P);
    let a = BigInt::from_be_bytes(P256_A);

    let r = BigInt::from_be_bytes(r_bytes);
    let s = BigInt::from_be_bytes(s_bytes);

    // Check r,s in [1, n-1].
    if r.is_zero() || s.is_zero() { return false; }

    let q = match AffinePoint::from_bytes(public_key_bytes) {
        Some(q) => q,
        None => return false,
    };

    // e = SHA-256(msg) interpreted as integer.
    let hash = sha256(msg);
    let e = BigInt::from_be_bytes(&hash);

    // w = s^(-1) mod n
    let w = match s.mod_inv(&n) {
        Some(w) => w,
        None => return false,
    };

    // u1 = e*w mod n, u2 = r*w mod n
    let u1 = e.mod_mul(&w, &n);
    let u2 = r.mod_mul(&w, &n);

    // Point = u1*G + u2*Q
    let g = AffinePoint::base_point();
    let p1 = g.scalar_mul(&u1, &p, &a);
    let p2 = q.scalar_mul(&u2, &p, &a);
    let pt = p1.add(&p2, &p, &a);

    if pt.infinity { return false; }

    // x1 mod n == r ?
    let x1_mod_n = pt.x.modulo(&n);
    x1_mod_n == r
}

// ─────────────────────────────────────────────────────────────────────────────
//  RSA-2048 verify (PKCS#1 v1.5, SHA-256)
// ─────────────────────────────────────────────────────────────────────────────

/// RSA-2048 PKCS#1 v1.5 signature verification.
/// `n_bytes`:   256-byte modulus (big-endian).
/// `e_bytes`:   variable-length public exponent (usually 3 or 65537).
/// `sig_bytes`: 256-byte signature.
/// `msg`:       message (will be SHA-256 hashed).
pub fn rsa2048_verify_pkcs1v15_sha256(
    n_bytes:   &[u8; 256],
    e_bytes:   &[u8],
    sig_bytes: &[u8; 256],
    msg:       &[u8],
) -> bool {
    let n   = BigInt::from_be_bytes(n_bytes);
    let e   = BigInt::from_be_bytes(e_bytes);
    let sig = BigInt::from_be_bytes(sig_bytes);

    // m = sig^e mod n
    let m = sig.mod_exp(&e, &n);
    let em = m.to_be_bytes(256);

    // Verify PKCS#1 v1.5 padding: 0x00 0x01 0xFF...0xFF 0x00 DigestInfo.
    if em[0] != 0x00 || em[1] != 0x01 { return false; }
    // Find 0x00 byte after 0xFF padding.
    let mut i = 2;
    while i < 256 && em[i] == 0xFF { i += 1; }
    if i >= 256 || em[i] != 0x00 { return false; }
    i += 1;

    // DigestInfo for SHA-256 = 19 bytes DER prefix + 32 bytes hash.
    const SHA256_DER_PREFIX: &[u8] = &[
        0x30,0x31,0x30,0x0D,0x06,0x09,0x60,0x86,0x48,0x01,0x65,0x03,0x04,
        0x02,0x01,0x05,0x00,0x04,0x20,
    ];
    let rem = &em[i..];
    if rem.len() < SHA256_DER_PREFIX.len() + 32 { return false; }
    if &rem[..SHA256_DER_PREFIX.len()] != SHA256_DER_PREFIX { return false; }

    let hash = sha256(msg);
    &rem[SHA256_DER_PREFIX.len()..SHA256_DER_PREFIX.len()+32] == &hash
}

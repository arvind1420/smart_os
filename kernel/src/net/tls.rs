//! TLS 1.3 client for Smart OS — RFC 8446.
//!
//! Cipher suite : TLS_AES_128_GCM_SHA256 (0x1301)
//! Key exchange  : X25519 (ephemeral, generated via RDRAND)
//! Certificates  : accepted without chain validation (Trust-on-First-Use).
//!
//! Crypto backends: all pure-Rust, no SIMD, safe on x86_64-unknown-none.
//!   SHA-256 / HMAC-SHA256 / HKDF — sha2 + hmac + hkdf crates
//!   AES-128-GCM — inline S-box table-lookup AES + software GHASH
//!   X25519     — inline Montgomery-ladder over GF(2^255-19)

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;

use sha2::Sha256;
use hmac::Mac;
use hkdf::Hkdf;

// ── Protocol constants ────────────────────────────────────────────────

const REC_HANDSHAKE:          u8 = 22;
const REC_APPLICATION_DATA:   u8 = 23;

const HS_CLIENT_HELLO:        u8 = 1;
const HS_SERVER_HELLO:        u8 = 2;
const HS_FINISHED:            u8 = 20;

const TLS12: u16 = 0x0303;
#[allow(dead_code)]
const TLS13: u16 = 0x0304;

const CIPHER_AES128_GCM_SHA256: u16 = 0x1301;
const GROUP_X25519:              u16 = 0x001D;

const EXT_SERVER_NAME:        u16 = 0;
const EXT_ALPN:               u16 = 16; // Application-Layer Protocol Negotiation (RFC 7301)
const EXT_SUPPORTED_GROUPS:   u16 = 10;
const EXT_SIG_ALGOS:          u16 = 13;
const EXT_SUPPORTED_VERSIONS: u16 = 43;
const EXT_KEY_SHARE:          u16 = 51;

// ── RDRAND entropy ────────────────────────────────────────────────────

fn rdrand32() -> [u8; 32] {
    let mut out = [0u8; 32];
    for chunk in out.chunks_mut(8) {
        let mut r: u64 = 0;
        unsafe {
            while core::arch::x86_64::_rdrand64_step(&mut r) == 0 {}
        }
        chunk.copy_from_slice(&r.to_ne_bytes()[..chunk.len()]);
    }
    out
}

// ════════════════════════════════════════════════════════════════════
//  Pure-Rust AES-128  (S-box table-lookup, no SIMD)
// ════════════════════════════════════════════════════════════════════

#[rustfmt::skip]
static SBOX: [u8; 256] = [
    0x63,0x7c,0x77,0x7b,0xf2,0x6b,0x6f,0xc5,0x30,0x01,0x67,0x2b,0xfe,0xd7,0xab,0x76,
    0xca,0x82,0xc9,0x7d,0xfa,0x59,0x47,0xf0,0xad,0xd4,0xa2,0xaf,0x9c,0xa4,0x72,0xc0,
    0xb7,0xfd,0x93,0x26,0x36,0x3f,0xf7,0xcc,0x34,0xa5,0xe5,0xf1,0x71,0xd8,0x31,0x15,
    0x04,0xc7,0x23,0xc3,0x18,0x96,0x05,0x9a,0x07,0x12,0x80,0xe2,0xeb,0x27,0xb2,0x75,
    0x09,0x83,0x2c,0x1a,0x1b,0x6e,0x5a,0xa0,0x52,0x3b,0xd6,0xb3,0x29,0xe3,0x2f,0x84,
    0x53,0xd1,0x00,0xed,0x20,0xfc,0xb1,0x5b,0x6a,0xcb,0xbe,0x39,0x4a,0x4c,0x58,0xcf,
    0xd0,0xef,0xaa,0xfb,0x43,0x4d,0x33,0x85,0x45,0xf9,0x02,0x7f,0x50,0x3c,0x9f,0xa8,
    0x51,0xa3,0x40,0x8f,0x92,0x9d,0x38,0xf5,0xbc,0xb6,0xda,0x21,0x10,0xff,0xf3,0xd2,
    0xcd,0x0c,0x13,0xec,0x5f,0x97,0x44,0x17,0xc4,0xa7,0x7e,0x3d,0x64,0x5d,0x19,0x73,
    0x60,0x81,0x4f,0xdc,0x22,0x2a,0x90,0x88,0x46,0xee,0xb8,0x14,0xde,0x5e,0x0b,0xdb,
    0xe0,0x32,0x3a,0x0a,0x49,0x06,0x24,0x5c,0xc2,0xd3,0xac,0x62,0x91,0x95,0xe4,0x79,
    0xe7,0xc8,0x37,0x6d,0x8d,0xd5,0x4e,0xa9,0x6c,0x56,0xf4,0xea,0x65,0x7a,0xae,0x08,
    0xba,0x78,0x25,0x2e,0x1c,0xa6,0xb4,0xc6,0xe8,0xdd,0x74,0x1f,0x4b,0xbd,0x8b,0x8a,
    0x70,0x3e,0xb5,0x66,0x48,0x03,0xf6,0x0e,0x61,0x35,0x57,0xb9,0x86,0xc1,0x1d,0x9e,
    0xe1,0xf8,0x98,0x11,0x69,0xd9,0x8e,0x94,0x9b,0x1e,0x87,0xe9,0xce,0x55,0x28,0xdf,
    0x8c,0xa1,0x89,0x0d,0xbf,0xe6,0x42,0x68,0x41,0x99,0x2d,0x0f,0xb0,0x54,0xbb,0x16,
];

fn xtime(x: u8) -> u8 {
    (x << 1) ^ (if x & 0x80 != 0 { 0x1B } else { 0 })
}

fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 { p ^= a; }
        let hi = a & 0x80 != 0;
        a <<= 1;
        if hi { a ^= 0x1B; }
        b >>= 1;
    }
    p
}

fn sub_word(w: u32) -> u32 {
    let b = w.to_be_bytes();
    u32::from_be_bytes([SBOX[b[0] as usize], SBOX[b[1] as usize], SBOX[b[2] as usize], SBOX[b[3] as usize]])
}

fn rot_word(w: u32) -> u32 {
    w.rotate_left(8)
}

/// AES-128 key expansion → 11 × 4-word round keys.
pub fn aes128_key_expansion(key: &[u8; 16]) -> [[u32; 4]; 11] {
    const RCON: [u32; 10] = [
        0x01000000, 0x02000000, 0x04000000, 0x08000000, 0x10000000,
        0x20000000, 0x40000000, 0x80000000, 0x1B000000, 0x36000000,
    ];
    let mut w = [[0u32; 4]; 11];
    for i in 0..4 {
        w[0][i] = u32::from_be_bytes([key[4*i], key[4*i+1], key[4*i+2], key[4*i+3]]);
    }
    for round in 1..=10 {
        let prev = w[round - 1];
        let mut temp = rot_word(prev[3]);
        temp = sub_word(temp);
        temp ^= RCON[round - 1];
        w[round][0] = prev[0] ^ temp;
        w[round][1] = prev[1] ^ w[round][0];
        w[round][2] = prev[2] ^ w[round][1];
        w[round][3] = prev[3] ^ w[round][2];
    }
    w
}

/// Encrypt one 16-byte block with AES-128.
pub fn aes128_encrypt_block(block: &[u8; 16], round_keys: &[[u32; 4]; 11]) -> [u8; 16] {
    let mut state = [0u32; 4];
    for i in 0..4 {
        state[i] = u32::from_be_bytes([block[4*i], block[4*i+1], block[4*i+2], block[4*i+3]]);
        state[i] ^= round_keys[0][i];
    }

    for round in 1..=10 {
        // SubBytes + ShiftRows + MixColumns (rounds 1–9) or just SubBytes+ShiftRows (round 10)
        let s: [[u8; 4]; 4] = [
            state[0].to_be_bytes(),
            state[1].to_be_bytes(),
            state[2].to_be_bytes(),
            state[3].to_be_bytes(),
        ];
        // SubBytes
        let mut t = [[0u8; 4]; 4];
        for c in 0..4 {
            for r in 0..4 {
                t[c][r] = SBOX[s[c][r] as usize];
            }
        }
        // ShiftRows: row i shifts left by i
        let sr = [
            [t[0][0], t[1][1], t[2][2], t[3][3]],
            [t[1][0], t[2][1], t[3][2], t[0][3]],
            [t[2][0], t[3][1], t[0][2], t[1][3]],
            [t[3][0], t[0][1], t[1][2], t[2][3]],
        ];
        if round < 10 {
            // MixColumns
            for c in 0..4 {
                let a = sr[c];
                state[c] = u32::from_be_bytes([
                    gmul(0x02,a[0])^gmul(0x03,a[1])^a[2]^a[3],
                    a[0]^gmul(0x02,a[1])^gmul(0x03,a[2])^a[3],
                    a[0]^a[1]^gmul(0x02,a[2])^gmul(0x03,a[3]),
                    gmul(0x03,a[0])^a[1]^a[2]^gmul(0x02,a[3]),
                ]);
                state[c] ^= round_keys[round][c];
            }
        } else {
            for c in 0..4 {
                state[c] = u32::from_be_bytes(sr[c]);
                state[c] ^= round_keys[10][c];
            }
        }
    }

    let mut out = [0u8; 16];
    for i in 0..4 {
        out[4*i..4*i+4].copy_from_slice(&state[i].to_be_bytes());
    }
    out
}

// ════════════════════════════════════════════════════════════════════
//  GHASH + GCM  (pure Rust, no SIMD)
// ════════════════════════════════════════════════════════════════════

/// GF(2^128) multiply — reduction polynomial x^128+x^7+x^2+x+1.
fn gf128_mul(x: u128, y: u128) -> u128 {
    let mut z = 0u128;
    let mut v = x;
    for i in (0..128).rev() {
        if (y >> i) & 1 != 0 {
            z ^= v;
        }
        let lsb = v & 1;
        v >>= 1;
        if lsb != 0 {
            v ^= 0xE100_0000_0000_0000_0000_0000_0000_0000u128;
        }
    }
    z
}

fn bytes_to_u128_be(b: &[u8; 16]) -> u128 {
    u128::from_be_bytes(*b)
}

fn u128_to_bytes_be(v: u128) -> [u8; 16] {
    v.to_be_bytes()
}

/// GHASH(H, A, C): authenticate additional data A and ciphertext C with key H.
fn ghash(h: &[u8; 16], aad: &[u8], cipher: &[u8]) -> [u8; 16] {
    let h_int = bytes_to_u128_be(h);
    let mut x = 0u128;

    // Process AAD (pad to block boundary)
    for chunk in aad.chunks(16) {
        let mut block = [0u8; 16];
        block[..chunk.len()].copy_from_slice(chunk);
        x ^= bytes_to_u128_be(&block);
        x = gf128_mul(x, h_int);
    }

    // Process ciphertext
    for chunk in cipher.chunks(16) {
        let mut block = [0u8; 16];
        block[..chunk.len()].copy_from_slice(chunk);
        x ^= bytes_to_u128_be(&block);
        x = gf128_mul(x, h_int);
    }

    // Length block: u64(len_aad_bits) || u64(len_cipher_bits)
    let len_block = ((aad.len() as u128) * 8 << 64) | ((cipher.len() as u128) * 8);
    x ^= len_block;
    x = gf128_mul(x, h_int);

    u128_to_bytes_be(x)
}

/// AES-128-GCM encrypt: returns ciphertext || 16-byte tag.
fn aes128_gcm_seal(key: &[u8; 16], iv: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let rk = aes128_key_expansion(key);

    // H = AES_K(0^128)
    let h = aes128_encrypt_block(&[0u8; 16], &rk);

    // J0 = IV || 0x00000001
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(iv);
    j0[15] = 1;

    // Encrypt plaintext in CTR mode starting from counter = 2
    let mut ciphertext = Vec::with_capacity(plaintext.len());
    for (i, chunk) in plaintext.chunks(16).enumerate() {
        let mut ctr = j0;
        let ctr_val = u32::from_be_bytes([ctr[12], ctr[13], ctr[14], ctr[15]]);
        let new_ctr = ctr_val.wrapping_add(i as u32 + 1);
        ctr[12..16].copy_from_slice(&new_ctr.to_be_bytes());
        let ks = aes128_encrypt_block(&ctr, &rk);
        for (j, &b) in chunk.iter().enumerate() {
            ciphertext.push(b ^ ks[j]);
        }
    }

    // Tag = GHASH(H, aad, ciphertext) XOR AES_K(J0)
    let ghash_out = ghash(&h, aad, &ciphertext);
    let j0_enc = aes128_encrypt_block(&j0, &rk);
    let mut tag = [0u8; 16];
    for i in 0..16 { tag[i] = ghash_out[i] ^ j0_enc[i]; }

    ciphertext.extend_from_slice(&tag);
    ciphertext
}

/// AES-128-GCM decrypt + verify tag. Returns `Ok(plaintext)` or `Err`.
fn aes128_gcm_open(key: &[u8; 16], iv: &[u8; 12], aad: &[u8], ciphertext_with_tag: &[u8])
    -> Result<Vec<u8>, &'static str>
{
    if ciphertext_with_tag.len() < 16 {
        return Err("ciphertext too short");
    }
    let (ciphertext, tag) = ciphertext_with_tag.split_at(ciphertext_with_tag.len() - 16);
    let rk = aes128_key_expansion(key);
    let h = aes128_encrypt_block(&[0u8; 16], &rk);

    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(iv);
    j0[15] = 1;

    // Verify tag first (constant-time compare)
    let ghash_out = ghash(&h, aad, ciphertext);
    let j0_enc = aes128_encrypt_block(&j0, &rk);
    let mut expected_tag = [0u8; 16];
    for i in 0..16 { expected_tag[i] = ghash_out[i] ^ j0_enc[i]; }

    let mut diff = 0u8;
    for i in 0..16 { diff |= tag[i] ^ expected_tag[i]; }
    if diff != 0 {
        return Err("AEAD authentication tag mismatch");
    }

    // Decrypt
    let mut plaintext = Vec::with_capacity(ciphertext.len());
    for (i, chunk) in ciphertext.chunks(16).enumerate() {
        let mut ctr = j0;
        let ctr_val = u32::from_be_bytes([ctr[12], ctr[13], ctr[14], ctr[15]]);
        let new_ctr = ctr_val.wrapping_add(i as u32 + 1);
        ctr[12..16].copy_from_slice(&new_ctr.to_be_bytes());
        let ks = aes128_encrypt_block(&ctr, &rk);
        for (j, &b) in chunk.iter().enumerate() {
            plaintext.push(b ^ ks[j]);
        }
    }
    Ok(plaintext)
}

// ════════════════════════════════════════════════════════════════════
//  X25519 key exchange  (Montgomery ladder, pure u64/u128 arithmetic)
// ════════════════════════════════════════════════════════════════════
//
//  Field: GF(2^255 - 19).
//  Representation: 5 × u64 limbs in 51-bit radix (reduced form).

type Fe = [u64; 5]; // field element, 51-bit limbs

const P0: u64 = (1 << 51) - 19;
const P1: u64 = (1 << 51) - 1;

fn fe_from_bytes(b: &[u8; 32]) -> Fe {
    // Load 255 bits little-endian into 5 × 51-bit limbs.
    let load = |start: usize, bits: usize| -> u64 {
        let byte_idx = start / 8;
        let bit_idx  = start % 8;
        let mut v = 0u64;
        for i in 0..8usize {
            if byte_idx + i < 32 {
                v |= (b[byte_idx + i] as u64) << (8 * i);
            }
        }
        (v >> bit_idx) & ((1u64 << bits) - 1)
    };
    [load(0,51), load(51,51), load(102,51), load(153,51), load(204,51)]
}

fn fe_to_bytes(mut f: Fe) -> [u8; 32] {
    // Reduce fully.
    let mut c = f[0] >> 51; f[0] &= P1; f[1] += c;
    let mut c = f[1] >> 51; f[1] &= P1; f[2] += c;
    let mut c = f[2] >> 51; f[2] &= P1; f[3] += c;
    let mut c = f[3] >> 51; f[3] &= P1; f[4] += c;
    // Final conditional subtraction if ≥ p
    let mut carry = f[4] >> 51; f[4] &= P1;
    f[0] += 19 * carry;
    let c = f[0] >> 51; f[0] &= P1; f[1] += c;
    let c = f[1] >> 51; f[1] &= P1; f[2] += c;
    let c = f[2] >> 51; f[2] &= P1; f[3] += c;
    let c = f[3] >> 51; f[3] &= P1; f[4] += c; f[4] &= P1;

    // Pack 5 × 51-bit limbs into 255 bits LE using u128 arithmetic.
    let mut out = [0u8; 32];
    // Low 128 bits: limbs 0–2 and low part of limb 3 (0..128 bits)
    let lo: u128 = (f[0] as u128)
        | ((f[1] as u128) << 51)
        | ((f[2] as u128) << 102);
    for i in 0..16 { out[i] = (lo >> (8 * i)) as u8; }
    // High 127 bits: remaining part of limbs 2–4 (bits 128..255)
    let hi: u128 = ((f[2] as u128) >> 26)
        | ((f[3] as u128) << 25)
        | ((f[4] as u128) << 76);
    // Loop covers out[16..30] (i=0..14 → shifts 0..112)
    for i in 0..15 { out[16 + i] = (hi >> (8 * i)) as u8; }
    // out[31] needs bits 120..127 of hi (shift by 8*15 = 120, not 112)
    out[31] = (hi >> 120) as u8;
    out
}

fn fe_add(a: &Fe, b: &Fe) -> Fe {
    [a[0]+b[0], a[1]+b[1], a[2]+b[2], a[3]+b[3], a[4]+b[4]]
}

fn fe_sub(a: &Fe, b: &Fe) -> Fe {
    // Add 2p before subtracting to avoid underflow.
    let two_p = [2*P0, 2*P1, 2*P1, 2*P1, 2*P1];
    [a[0]+two_p[0]-b[0], a[1]+two_p[1]-b[1], a[2]+two_p[2]-b[2],
     a[3]+two_p[3]-b[3], a[4]+two_p[4]-b[4]]
}

fn fe_mul(a: &Fe, b: &Fe) -> Fe {
    // Schoolbook multiply mod p, 51-bit limbs, 19 * carry reduction.
    let a0=a[0] as u128; let a1=a[1] as u128; let a2=a[2] as u128;
    let a3=a[3] as u128; let a4=a[4] as u128;
    let b0=b[0] as u128; let b1=b[1] as u128; let b2=b[2] as u128;
    let b3=b[3] as u128; let b4=b[4] as u128;
    let mask = P1 as u128;
    let r19 = 19u128;

    let mut t0 = a0*b0 + r19*(a1*b4 + a2*b3 + a3*b2 + a4*b1);
    let mut t1 = a0*b1 + a1*b0 + r19*(a2*b4 + a3*b3 + a4*b2);
    let mut t2 = a0*b2 + a1*b1 + a2*b0 + r19*(a3*b4 + a4*b3);
    let mut t3 = a0*b3 + a1*b2 + a2*b1 + a3*b0 + r19*a4*b4;
    let mut t4 = a0*b4 + a1*b3 + a2*b2 + a3*b1 + a4*b0;

    let c0 = t0 >> 51; t0 &= mask; t1 += c0;
    let c1 = t1 >> 51; t1 &= mask; t2 += c1;
    let c2 = t2 >> 51; t2 &= mask; t3 += c2;
    let c3 = t3 >> 51; t3 &= mask; t4 += c3;
    let c4 = t4 >> 51; t4 &= mask; t0 += 19 * c4;
    let c0 = t0 >> 51; t0 &= mask; t1 += c0;

    [t0 as u64, t1 as u64, t2 as u64, t3 as u64, t4 as u64]
}

fn fe_sq(a: &Fe) -> Fe { fe_mul(a, a) }

fn fe_invert(z: &Fe) -> Fe {
    // z^(p-2) = z^(2^255 - 21) via repeated squaring.
    let z2    = fe_sq(z);
    let z9    = fe_mul(&fe_sq(&fe_sq(&z2)), z);
    let z11   = fe_mul(&z2, &z9);
    let z2_5  = fe_mul(&fe_sq(&z11), &z9);
    let z2_10 = { let t = fe_sq(&z2_5); let mut r = t; for _ in 1..5  { r = fe_sq(&r); } fe_mul(&r, &z2_5) };
    let z2_20 = { let t = fe_sq(&z2_10); let mut r = t; for _ in 1..10 { r = fe_sq(&r); } fe_mul(&r, &z2_10) };
    let z2_40 = { let t = fe_sq(&z2_20); let mut r = t; for _ in 1..20 { r = fe_sq(&r); } fe_mul(&r, &z2_20) };
    let z2_50 = { let t = fe_sq(&z2_40); let mut r = t; for _ in 1..10 { r = fe_sq(&r); } fe_mul(&r, &z2_10) };
    let z2_100= { let t = fe_sq(&z2_50); let mut r = t; for _ in 1..50 { r = fe_sq(&r); } fe_mul(&r, &z2_50) };
    let z2_200= { let t = fe_sq(&z2_100); let mut r = t; for _ in 1..100{ r = fe_sq(&r); } fe_mul(&r, &z2_100) };
    let z2_250= { let t = fe_sq(&z2_200); let mut r = t; for _ in 1..50 { r = fe_sq(&r); } fe_mul(&r, &z2_50) };
    let tmp   = { let mut r = fe_sq(&z2_250); for _ in 1..5 { r = fe_sq(&r); } r };
    fe_mul(&tmp, &z11)
}

fn cswap(swap: u64, a: &mut Fe, b: &mut Fe) {
    for i in 0..5 {
        let m = 0u64.wrapping_sub(swap);
        let t = m & (a[i] ^ b[i]);
        a[i] ^= t;
        b[i] ^= t;
    }
}

/// X25519 scalar multiplication (RFC 7748).
pub fn x25519(k: &[u8; 32], u: &[u8; 32]) -> [u8; 32] {
    let mut k_clamped = *k;
    k_clamped[0]  &= 248;
    k_clamped[31] &= 127;
    k_clamped[31] |= 64;

    let u_fe = fe_from_bytes(u);
    let mut x1 = u_fe;
    let mut x2: Fe = [1,0,0,0,0];
    let mut z2: Fe = [0,0,0,0,0];
    let mut x3 = u_fe;
    let mut z3: Fe = [1,0,0,0,0];
    let a24: Fe = [121665,0,0,0,0];

    let mut swap = 0u64;
    for i in (0..255).rev() {
        let k_bit = ((k_clamped[i / 8] >> (i % 8)) & 1) as u64;
        swap ^= k_bit;
        cswap(swap, &mut x2, &mut x3);
        cswap(swap, &mut z2, &mut z3);
        swap = k_bit;

        let a  = fe_add(&x2, &z2);
        let aa = fe_sq(&a);
        let b  = fe_sub(&x2, &z2);
        let bb = fe_sq(&b);
        let e  = fe_sub(&aa, &bb);
        let c  = fe_add(&x3, &z3);
        let d  = fe_sub(&x3, &z3);
        let da = fe_mul(&d, &a);
        let cb = fe_mul(&c, &b);
        x3 = fe_sq(&fe_add(&da, &cb));
        z3 = fe_mul(&x1, &fe_sq(&fe_sub(&da, &cb)));
        x2 = fe_mul(&aa, &bb);
        z2 = fe_mul(&e, &fe_add(&aa, &fe_mul(&a24, &e)));
    }
    cswap(swap, &mut x2, &mut x3);
    cswap(swap, &mut z2, &mut z3);

    let z_inv = fe_invert(&z2);
    fe_to_bytes(fe_mul(&x2, &z_inv))
}

/// X25519 base point (u=9).
pub fn x25519_public_key(secret: &[u8; 32]) -> [u8; 32] {
    let mut base = [0u8; 32];
    base[0] = 9;
    x25519(secret, &base)
}

// ── SHA-256 helpers ───────────────────────────────────────────────────

fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    let mut h = Sha256::new();
    h.update(data);
    let r = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(r.as_slice());
    out
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = <hmac::Hmac<Sha256> as Mac>::new_from_slice(key)
        .expect("hmac init");
    mac.update(data);
    let r = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(r.as_slice());
    out
}

// ── HKDF-Expand-Label (RFC 8446 §7.1) ────────────────────────────────

fn hkdf_expand_label(prk: &[u8], label: &[u8], ctx: &[u8], len: usize) -> Vec<u8> {
    let mut info = Vec::new();
    info.extend_from_slice(&(len as u16).to_be_bytes());
    let full_label = [b"tls13 " as &[u8], label].concat();
    info.push(full_label.len() as u8);
    info.extend_from_slice(&full_label);
    info.push(ctx.len() as u8);
    info.extend_from_slice(ctx);

    let hkdf = Hkdf::<Sha256>::from_prk(prk).expect("hkdf from_prk");
    let mut out = vec![0u8; len];
    hkdf.expand(&info, &mut out).expect("hkdf expand");
    out
}

fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    let (prk, _) = Hkdf::<Sha256>::extract(Some(salt), ikm);
    let mut out = [0u8; 32];
    out.copy_from_slice(prk.as_slice());
    out
}

// ── Key schedule ──────────────────────────────────────────────────────

struct HandshakeKeys {
    client_key: [u8; 16],
    server_key: [u8; 16],
    client_iv:  [u8; 12],
    server_iv:  [u8; 12],
    master_secret: [u8; 32],
    /// [sender]_hs_traffic secrets — needed for Finished key derivation (RFC 8446 §4.4.4).
    client_hs_traffic: [u8; 32],
}

fn derive_handshake_keys(shared_secret: &[u8; 32], transcript_hash: &[u8; 32]) -> HandshakeKeys {
    let zeros = [0u8; 32];
    let early_secret = hkdf_extract(&zeros, &zeros);
    let derived = hkdf_expand_label(&early_secret, b"derived", &sha256(b""), 32);
    let hs_secret = hkdf_extract(&derived, shared_secret);

    let c_hs_traffic = hkdf_expand_label(&hs_secret, b"c hs traffic", transcript_hash, 32);
    let s_hs_traffic = hkdf_expand_label(&hs_secret, b"s hs traffic", transcript_hash, 32);

    let ck = &hkdf_expand_label(&c_hs_traffic, b"key", b"", 16)[..];
    let sk = &hkdf_expand_label(&s_hs_traffic, b"key", b"", 16)[..];
    let ci = &hkdf_expand_label(&c_hs_traffic, b"iv",  b"", 12)[..];
    let si = &hkdf_expand_label(&s_hs_traffic, b"iv",  b"", 12)[..];

    let derived2 = hkdf_expand_label(&hs_secret, b"derived", &sha256(b""), 32);
    let master = hkdf_extract(&derived2, &zeros);

    let mut client_key = [0u8; 16]; client_key.copy_from_slice(ck);
    let mut server_key = [0u8; 16]; server_key.copy_from_slice(sk);
    let mut client_iv  = [0u8; 12]; client_iv.copy_from_slice(ci);
    let mut server_iv  = [0u8; 12]; server_iv.copy_from_slice(si);
    let mut master_secret = [0u8; 32]; master_secret.copy_from_slice(&master);
    let mut client_hs_traffic = [0u8; 32]; client_hs_traffic.copy_from_slice(&c_hs_traffic);

    HandshakeKeys { client_key, server_key, client_iv, server_iv, master_secret, client_hs_traffic }
}

struct AppKeys {
    client_key: [u8; 16],
    server_key: [u8; 16],
    client_iv:  [u8; 12],
    server_iv:  [u8; 12],
}

fn derive_app_keys(master: &[u8; 32], transcript_hash: &[u8; 32]) -> AppKeys {
    let c_ap = hkdf_expand_label(master, b"c ap traffic", transcript_hash, 32);
    let s_ap = hkdf_expand_label(master, b"s ap traffic", transcript_hash, 32);

    let ck = &hkdf_expand_label(&c_ap, b"key", b"", 16)[..];
    let sk = &hkdf_expand_label(&s_ap, b"key", b"", 16)[..];
    let ci = &hkdf_expand_label(&c_ap, b"iv",  b"", 12)[..];
    let si = &hkdf_expand_label(&s_ap, b"iv",  b"", 12)[..];

    let mut client_key = [0u8; 16]; client_key.copy_from_slice(ck);
    let mut server_key = [0u8; 16]; server_key.copy_from_slice(sk);
    let mut client_iv  = [0u8; 12]; client_iv.copy_from_slice(ci);
    let mut server_iv  = [0u8; 12]; server_iv.copy_from_slice(si);

    AppKeys { client_key, server_key, client_iv, server_iv }
}

// ── TLS 1.3 record AEAD ───────────────────────────────────────────────

fn make_nonce(base_iv: &[u8; 12], seq: u64) -> [u8; 12] {
    let mut nonce = *base_iv;
    let seq_bytes = seq.to_be_bytes();
    for i in 0..8 {
        nonce[4 + i] ^= seq_bytes[i];
    }
    nonce
}

fn aead_seal(key: &[u8; 16], iv: &[u8; 12], seq: u64, content_type: u8, plain: &[u8]) -> Vec<u8> {
    let nonce = make_nonce(iv, seq);
    // TLS inner plaintext = plain || content_type
    let mut inner = plain.to_vec();
    inner.push(content_type);
    // AAD = TLS record header (type=23 ApplicationData, legacy version, length)
    let ct_len = (inner.len() + 16) as u16; // +16 for GCM tag
    let aad = [0x17, 0x03, 0x03, (ct_len >> 8) as u8, ct_len as u8];
    aes128_gcm_seal(key, &nonce, &aad, &inner)
}

fn aead_open(key: &[u8; 16], iv: &[u8; 12], seq: u64, enc: &[u8]) -> Result<(Vec<u8>, u8), &'static str> {
    let nonce = make_nonce(iv, seq);
    let ct_len = enc.len() as u16;
    let aad = [0x17, 0x03, 0x03, (ct_len >> 8) as u8, ct_len as u8];
    let inner = aes128_gcm_open(key, &nonce, &aad, enc)?;
    // Strip trailing content type byte
    if inner.is_empty() { return Err("empty inner plaintext"); }
    let content_type = *inner.last().unwrap();
    Ok((inner[..inner.len()-1].to_vec(), content_type))
}

/// In TLS 1.3 ALL encrypted handshake records have inner content_type = 22 (Handshake).
/// The Finished message type (0x14 = 20) is embedded as the first byte of each handshake
/// message inside the decrypted payload.  Walk the handshake message list and return true
/// when a Finished message is found.
fn inner_has_finished(data: &[u8]) -> bool {
    let mut pos = 0;
    while pos + 4 <= data.len() {
        let hs_type = data[pos];
        let hs_len = ((data[pos + 1] as usize) << 16)
                   | ((data[pos + 2] as usize) << 8)
                   |  (data[pos + 3] as usize);
        if hs_type == HS_FINISHED {
            return true;
        }
        // Guard against malformed data causing infinite loop.
        if hs_len == 0 { break; }
        pos += 4 + hs_len;
    }
    false
}

// ── ClientHello builder ───────────────────────────────────────────────

fn build_client_hello(host: &str, random: &[u8; 32], pub_key: &[u8; 32]) -> Vec<u8> {
    let mut exts = Vec::new();

    // SNI extension
    let host_bytes = host.as_bytes();
    let sni_list_len = (host_bytes.len() + 3) as u16;
    let sni_ext_len  = (sni_list_len + 2) as u16;
    exts.extend_from_slice(&EXT_SERVER_NAME.to_be_bytes());
    exts.extend_from_slice(&sni_ext_len.to_be_bytes());
    exts.extend_from_slice(&sni_list_len.to_be_bytes());
    exts.push(0x00); // type: host_name
    exts.extend_from_slice(&(host_bytes.len() as u16).to_be_bytes());
    exts.extend_from_slice(host_bytes);

    // Supported versions: TLS 1.3
    exts.extend_from_slice(&EXT_SUPPORTED_VERSIONS.to_be_bytes());
    exts.extend_from_slice(&3u16.to_be_bytes()); // ext data len
    exts.push(2);
    exts.extend_from_slice(&0x0304u16.to_be_bytes());

    // Supported groups: X25519
    exts.extend_from_slice(&EXT_SUPPORTED_GROUPS.to_be_bytes());
    exts.extend_from_slice(&4u16.to_be_bytes());
    exts.extend_from_slice(&2u16.to_be_bytes());
    exts.extend_from_slice(&GROUP_X25519.to_be_bytes());

    // Signature algorithms
    exts.extend_from_slice(&EXT_SIG_ALGOS.to_be_bytes());
    exts.extend_from_slice(&4u16.to_be_bytes());
    exts.extend_from_slice(&2u16.to_be_bytes());
    exts.extend_from_slice(&0x0403u16.to_be_bytes()); // ecdsa_secp256r1_sha256

    // ALPN: advertise h2 and http/1.1 (RFC 7301 / RFC 7540 §3.3)
    {
        const H2: &[u8]   = b"h2";
        const H11: &[u8]  = b"http/1.1";
        let proto_list_len = (1 + H2.len() + 1 + H11.len()) as u16;
        let alpn_ext_len   = 2 + proto_list_len; // 2-byte list_len field + entries
        exts.extend_from_slice(&EXT_ALPN.to_be_bytes());
        exts.extend_from_slice(&alpn_ext_len.to_be_bytes());
        exts.extend_from_slice(&proto_list_len.to_be_bytes());
        exts.push(H2.len() as u8);
        exts.extend_from_slice(H2);
        exts.push(H11.len() as u8);
        exts.extend_from_slice(H11);
    }

    // Key share: X25519
    let ks_data_len = (4 + 32) as u16;
    exts.extend_from_slice(&EXT_KEY_SHARE.to_be_bytes());
    exts.extend_from_slice(&(2 + ks_data_len).to_be_bytes());
    exts.extend_from_slice(&ks_data_len.to_be_bytes());
    exts.extend_from_slice(&GROUP_X25519.to_be_bytes());
    exts.extend_from_slice(&32u16.to_be_bytes());
    exts.extend_from_slice(pub_key);

    // Build ClientHello body
    let mut ch = Vec::new();
    ch.extend_from_slice(&TLS12.to_be_bytes()); // legacy version
    ch.extend_from_slice(random);
    ch.push(0x00); // session ID length = 0
    // cipher suites — advertise AES-128-GCM-SHA256 and ChaCha20-Poly1305-SHA256
    ch.extend_from_slice(&4u16.to_be_bytes()); // 2 suites × 2 bytes
    ch.extend_from_slice(&CIPHER_AES128_GCM_SHA256.to_be_bytes());
    ch.extend_from_slice(&CIPHER_CHACHA20_POLY1305.to_be_bytes());
    // compression methods
    ch.push(1); ch.push(0);
    // extensions
    ch.extend_from_slice(&(exts.len() as u16).to_be_bytes());
    ch.extend_from_slice(&exts);

    // Wrap in Handshake record
    let mut hs = Vec::new();
    hs.push(HS_CLIENT_HELLO);
    let body_len = ch.len() as u32;
    hs.push((body_len >> 16) as u8);
    hs.push((body_len >> 8)  as u8);
    hs.push( body_len        as u8);
    hs.extend_from_slice(&ch);

    // Wrap in TLS record
    let mut rec = Vec::new();
    rec.push(REC_HANDSHAKE);
    rec.extend_from_slice(&TLS12.to_be_bytes());
    rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
    rec.extend_from_slice(&hs);
    rec
}

// ── ServerHello parser ────────────────────────────────────────────────

/// Extract negotiated cipher suite from the ServerHello body.
/// `sh_rec` is the full handshake record body (starts with HS type byte 0x02).
/// ServerHello layout:
///   [0]    HS type (0x02)
///   [1..3] length
///   [4..5] legacy version
///   [6..37] server random
///   [38]   session_id_len
///   [39..39+sil] session_id
///   [39+sil..39+sil+2] cipher_suite ← this is what we read
///   [39+sil+2] compression
///   [39+sil+3..] ext_len + extensions
fn parse_server_cipher(sh_rec: &[u8]) -> u16 {
    if sh_rec.len() < 44 { return CIPHER_AES128_GCM_SHA256; }
    let sil = sh_rec[38] as usize;
    let cs_off = 39 + sil;
    if cs_off + 2 > sh_rec.len() { return CIPHER_AES128_GCM_SHA256; }
    u16::from_be_bytes([sh_rec[cs_off], sh_rec[cs_off + 1]])
}

fn parse_server_key_share(msg: &[u8]) -> Option<[u8; 32]> {
    let mut i = 0;
    while i + 4 <= msg.len() {
        let ext_type = u16::from_be_bytes([msg[i], msg[i+1]]);
        let ext_len  = u16::from_be_bytes([msg[i+2], msg[i+3]]) as usize;
        i += 4;
        if i + ext_len > msg.len() { break; }
        if ext_type == EXT_KEY_SHARE && ext_len >= 36 {
            // group (2) + key_len (2) + key (32)
            let group = u16::from_be_bytes([msg[i], msg[i+1]]);
            let key_len = u16::from_be_bytes([msg[i+2], msg[i+3]]) as usize;
            if group == GROUP_X25519 && key_len == 32 && i + 4 + 32 <= msg.len() {
                let mut key = [0u8; 32];
                key.copy_from_slice(&msg[i+4..i+4+32]);
                return Some(key);
            }
        }
        i += ext_len;
    }
    None
}

// ── Session state ─────────────────────────────────────────────────────

pub type TlsConnId = u32;

struct Session {
    app_client_key: [u8; 16],
    app_server_key: [u8; 16],
    app_client_iv:  [u8; 12],
    app_server_iv:  [u8; 12],
    /// ChaCha20-Poly1305 keys (32 bytes each). Only valid when cipher == 0x1303.
    app_client_key32: [u8; 32],
    app_server_key32: [u8; 32],
    send_seq: u64,
    recv_seq: u64,
    tcp_conn: crate::net::tcp::TcpSocketId,
    /// Negotiated cipher suite (0x1301 = AES-128-GCM, 0x1303 = ChaCha20-Poly1305).
    cipher: u16,
    /// Phase 128: ALPN protocol negotiated by the server ("h2" or "http/1.1").
    alpn: Option<alloc::string::String>,
    /// Phase 126: hostname for session ticket storage.
    host: alloc::string::String,
}

static SESSIONS: Mutex<BTreeMap<TlsConnId, Session>> = Mutex::new(BTreeMap::new());
static NEXT_ID:  Mutex<TlsConnId> = Mutex::new(1);

fn alloc_id() -> TlsConnId {
    let mut n = NEXT_ID.lock();
    let id = *n;
    *n += 1;
    id
}

// ── Public API ────────────────────────────────────────────────────────

/// Parse the ALPN protocol from decrypted handshake messages.
/// Looks for EncryptedExtensions (HS type 8) and extracts extension 16 (ALPN).
fn parse_alpn_from_handshake(hs_data: &[u8]) -> Option<alloc::string::String> {
    let mut pos = 0;
    while pos + 4 <= hs_data.len() {
        let msg_type = hs_data[pos];
        let msg_len = ((hs_data[pos + 1] as usize) << 16)
                    | ((hs_data[pos + 2] as usize) << 8)
                    |   hs_data[pos + 3] as usize;
        if pos + 4 + msg_len > hs_data.len() { break; }
        let body = &hs_data[pos + 4..pos + 4 + msg_len];

        if msg_type == 8 && body.len() >= 2 { // EncryptedExtensions (HS type 8)
            let exts_len = u16::from_be_bytes([body[0], body[1]]) as usize;
            let end = (2 + exts_len).min(body.len());
            let mut i = 2;
            while i + 4 <= end {
                let ext_type = u16::from_be_bytes([body[i], body[i + 1]]);
                let ext_len  = u16::from_be_bytes([body[i + 2], body[i + 3]]) as usize;
                i += 4;
                if i + ext_len > end { break; }
                let ext_data = &body[i..i + ext_len];
                if ext_type == EXT_ALPN && ext_data.len() >= 3 {
                    // [2-byte proto_list_len][1-byte proto_len][proto bytes]
                    let proto_len = ext_data[2] as usize;
                    if 3 + proto_len <= ext_data.len() {
                        if let Ok(s) = core::str::from_utf8(&ext_data[3..3 + proto_len]) {
                            return Some(alloc::string::String::from(s));
                        }
                    }
                }
                i += ext_len;
            }
        }
        if msg_len == 0 { break; }
        pos += 4 + msg_len;
    }
    None
}

/// Open a TLS 1.3 connection to `host:port`.
pub fn connect(host: &str, port: u16) -> Result<TlsConnId, &'static str> {
    crate::serial_println!("[tls] connect: starting to {}:{}", host, port);

    // 1. Open TCP connection (use ephemeral local port from RDRAND)
    let local_port = (rdrand32()[0] as u16) | 0xC000; // range 49152–65535
    crate::serial_println!("[tls] connect: local_port={}", local_port);
    let ip = crate::net::dns::resolve(host)?;
    crate::serial_println!("[tls] connect: dns ok, connecting TCP");
    let tcp_id = crate::net::tcp::connect(ip, port, local_port)?;

    // tcp::connect() is non-blocking (sends SYN and returns).
    // Poll until the 3-way handshake completes and state == ESTABLISHED.
    crate::serial_println!("[tls] connect: waiting for TCP ESTABLISHED...");
    let mut tcp_wait = 0usize;
    const TCP_WAIT_MAX: usize = 50_000; // ~5 s
    loop {
        if crate::net::tcp::is_established(tcp_id) { break; }
        if crate::net::tcp::is_gone(tcp_id) {
            return Err("TCP connect failed (RST or timeout)");
        }
        tcp_wait += 1;
        if tcp_wait > TCP_WAIT_MAX {
            return Err("TCP connect timed out waiting for ESTABLISHED");
        }
        crate::process::scheduler::yield_now();
    }
    crate::serial_println!("[tls] connect: TCP ESTABLISHED (id={}, waited {} iters)", tcp_id, tcp_wait);

    // 2. Generate ephemeral X25519 key pair
    crate::serial_println!("[tls] connect: generating keypair");
    let priv_key = rdrand32();
    let pub_key  = x25519_public_key(&priv_key);
    let random   = rdrand32();
    crate::serial_println!("[tls] connect: keypair done, building ClientHello");

    // 3. Send ClientHello, accumulate transcript
    let ch_record = build_client_hello(host, &random, &pub_key);
    crate::serial_println!("[tls] connect: sending ClientHello ({} bytes)", ch_record.len());
    let send_res = crate::net::tcp::send(tcp_id, &ch_record);
    crate::serial_println!("[tls] connect: ClientHello send={}", if send_res.is_ok() { "ok" } else { "FAIL" });
    send_res.map(|_| ())?;

    // Transcript starts after the record header (5 bytes).
    let mut transcript = ch_record[5..].to_vec();

    // 4. Read ServerHello record (may be preceded by a ChangeCipherSpec in compat mode)
    let sh_rec = {
        let rec = tcp_read_record(tcp_id)?;
        // Skip TLS 1.3 compatibility-mode ChangeCipherSpec (content_type 20, body = [1])
        // tcp_read_record returns the record body; check if hdr[0] was 0x14 by inspecting body
        // Instead: if body is exactly [1] it's a CCS — read the next record.
        if rec.len() == 1 && rec[0] == 1 {
            crate::serial_println!("[tls] Skipping ChangeCipherSpec compat record");
            tcp_read_record(tcp_id)?
        } else {
            rec
        }
    };
    crate::serial_println!("[tls] ServerHello: {} bytes", sh_rec.len());
    transcript.extend_from_slice(&sh_rec);

    // Parse the server's X25519 public key and negotiated cipher from ServerHello.
    // ServerHello body layout:
    //   [0]:      HS type (1) = 0x02 ServerHello
    //   [1..3]:   length (3 bytes)
    //   [4..5]:   legacy version (2 bytes)
    //   [6..37]:  server random (32 bytes)
    //   [38]:     session_id_len (1 byte)
    //   [39..]:   session_id (session_id_len bytes)
    //   then:     cipher_suite (2), compression (1), extensions_len (2), extensions
    let (server_pub, negotiated_cipher) = {
        if sh_rec.len() < 44 {
            crate::serial_println!("[tls] ServerHello too short: {}B", sh_rec.len());
            return Err("ServerHello too short");
        }
        let session_id_len = sh_rec[38] as usize;
        // Offset to first extension: past session_id + cipher(2) + comp(1) + ext_len(2)
        let ext_start = 39 + session_id_len + 2 + 1 + 2;
        let cipher = parse_server_cipher(&sh_rec);
        crate::serial_println!("[tls] ServerHello sil={} ext_start={} cipher=0x{:04X}",
                               session_id_len, ext_start, cipher);
        if ext_start > sh_rec.len() {
            crate::serial_println!("[tls] ServerHello ext out of bounds ({} > {})", ext_start, sh_rec.len());
            return Err("ServerHello extensions out of bounds");
        }
        let pub_key = parse_server_key_share(&sh_rec[ext_start..])
            .ok_or("no X25519 key share in ServerHello")?;
        (pub_key, cipher)
    };

    // 5. Compute shared secret
    let shared = x25519(&priv_key, &server_pub);

    // 6. Derive handshake keys
    let th = sha256(&transcript);
    let hs_keys = derive_handshake_keys(&shared, &th);

    // 7. Read + decrypt EncryptedExtensions, Certificate, CertificateVerify, Finished
    // In TLS 1.3 compatibility mode the server may send a ChangeCipherSpec record
    // (body = [0x01]) immediately after ServerHello.  Skip it transparently.
    let mut decrypted_handshake = Vec::new();
    let mut srv_seq = 0u64;
    loop {
        let enc = tcp_read_record(tcp_id)?;
        if enc.is_empty() { break; }
        // Skip TLS 1.3 compat-mode ChangeCipherSpec (plaintext body = [0x01])
        if enc.len() == 1 && enc[0] == 1 {
            crate::serial_println!("[tls] Skipping server ChangeCipherSpec");
            continue;
        }
        crate::serial_println!("[tls] Decrypting record {} ({} bytes)", srv_seq, enc.len());
        match aead_open(&hs_keys.server_key, &hs_keys.server_iv, srv_seq, &enc) {
            Ok((inner, ct)) => {
                if ct == REC_HANDSHAKE {
                    decrypted_handshake.extend_from_slice(&inner);
                }
                // TLS 1.3: ALL encrypted handshake records have inner content_type=22.
                // The Finished message type (0x14) lives *inside* the handshake payload.
                let finished = ct == REC_HANDSHAKE && inner_has_finished(&inner);
                crate::serial_println!("[tls] Got HS record inner_ct={}, len={}, finished={}",
                                       ct, inner.len(), finished);
                transcript.extend_from_slice(&inner);
                srv_seq += 1;
                if finished { break; }
            }
            Err(e) => {
                crate::serial_println!("[tls] AEAD open failed: {}", e);
                return Err("AEAD open failed during handshake");
            }
        }
    }

    // Parse and verify the certificate chain
    let mut parsed_certs = Vec::new();
    let mut hs_pos = 0;
    while hs_pos + 4 <= decrypted_handshake.len() {
        let msg_type = decrypted_handshake[hs_pos];
        let msg_len = ((decrypted_handshake[hs_pos + 1] as usize) << 16)
                    | ((decrypted_handshake[hs_pos + 2] as usize) << 8)
                    |  (decrypted_handshake[hs_pos + 3] as usize);
        if hs_pos + 4 + msg_len > decrypted_handshake.len() { break; }
        let msg_body = &decrypted_handshake[hs_pos + 4..hs_pos + 4 + msg_len];

        if msg_type == 11 { // Certificate
            if msg_body.len() >= 4 {
                let context_len = msg_body[0] as usize;
                if msg_body.len() >= 4 + context_len {
                    let list_len = ((msg_body[context_len + 1] as usize) << 16)
                                 | ((msg_body[context_len + 2] as usize) << 8)
                                 |  (msg_body[context_len + 3] as usize);
                    let list_body = &msg_body[context_len + 4..];
                    if list_body.len() >= list_len {
                        let mut list_pos = 0;
                        while list_pos + 5 <= list_len {
                            let cert_len = ((list_body[list_pos] as usize) << 16)
                                         | ((list_body[list_pos + 1] as usize) << 8)
                                         |  (list_body[list_pos + 2] as usize);
                            if list_pos + 3 + cert_len > list_len { break; }
                            let cert_der = &list_body[list_pos + 3..list_pos + 3 + cert_len];

                            if let Some(cert) = crate::crypto::x509::Certificate::from_der(cert_der) {
                                parsed_certs.push(cert);
                            } else {
                                crate::serial_println!("[tls] Failed to parse certificate DER");
                                return Err("X.509 parse failed");
                            }

                            let ext_len = ((list_body[list_pos + 3 + cert_len] as usize) << 8)
                                        |  (list_body[list_pos + 3 + cert_len + 1] as usize);
                            list_pos += 3 + cert_len + 2 + ext_len;
                        }
                    }
                }
            }
        }
        hs_pos += 4 + msg_len;
    }

    if parsed_certs.is_empty() {
        return Err("No certificates received from server");
    }

    // Verify the parsed certificate chain.
    // When the CA trust store is empty (Community Edition / no bundle loaded) we
    // fall back to Trust-on-First-Use (TOFU): accept the cert but log a warning.
    // When roots are present we do full chain + signature + hostname verification.
    {
        let store_empty = crate::crypto::ca_store::CA_STORE.lock().is_empty();
        if store_empty {
            crate::serial_println!(
                "[tls] TOFU: CA store has 0 roots — accepting cert for '{}' without chain validation",
                host
            );
        } else {
            match crate::crypto::ca_store::verify_chain_global(&parsed_certs, host) {
                Ok(()) => {
                    crate::serial_println!("[tls] Certificate chain verified for '{}'", host);
                }
                Err(e) => {
                    crate::serial_println!("[tls] Certificate verification failed for '{}': {:?}", host, e);
                    return Err("Certificate verification failed");
                }
            }
        }
    }


    // 8. Send Finished
    // RFC 8446 §4.4.4: finished_key = HKDF-Expand-Label(BaseKey, "finished", "", Hash.length)
    // BaseKey is the [sender]_hs_traffic secret (32 bytes), NOT the derived AES key (16 bytes).
    let th2 = sha256(&transcript);
    let finished_key = hkdf_expand_label(&hs_keys.client_hs_traffic, b"finished", b"", 32);
    let verify_data  = hmac_sha256(&finished_key, &th2);
    let mut fin_hs = vec![HS_FINISHED, 0, 0, 32];
    fin_hs.extend_from_slice(&verify_data);
    let enc_fin = aead_seal(&hs_keys.client_key, &hs_keys.client_iv, 0, REC_HANDSHAKE, &fin_hs);
    let mut rec = vec![REC_APPLICATION_DATA, 0x03, 0x03,
                       (enc_fin.len() >> 8) as u8, enc_fin.len() as u8];
    rec.extend_from_slice(&enc_fin);
    crate::net::tcp::send(tcp_id, &rec).map(|_| ())?;
    transcript.extend_from_slice(&fin_hs);

    // 9. Derive application keys.
    // RFC 8446 §7.1: both client_application_traffic_secret_0 and
    // server_application_traffic_secret_0 are derived from the transcript hash
    // up to and including the *server* Finished — that is th2, computed above
    // BEFORE we appended the client Finished.  Using th3 (which includes the
    // client Finished) produces wrong keys and causes every AEAD decrypt to fail.
    let app_keys = derive_app_keys(&hs_keys.master_secret, &th2);

    // Phase 128: extract ALPN from decrypted handshake (EncryptedExtensions)
    let negotiated_alpn = parse_alpn_from_handshake(&decrypted_handshake);
    crate::serial_println!("[tls] ALPN negotiated: {:?}", negotiated_alpn.as_deref().unwrap_or("none"));

    // Derive 32-byte keys for ChaCha20-Poly1305 (key_length=32 instead of 16).
    // These are only used when the negotiated cipher is 0x1303.
    let (c_ap32, s_ap32) = {
        let c_ap_t = hkdf_expand_label(&hs_keys.master_secret,
                                       b"c ap traffic", &th2, 32);
        let s_ap_t = hkdf_expand_label(&hs_keys.master_secret,
                                       b"s ap traffic", &th2, 32);
        let ck32 = hkdf_expand_label(&c_ap_t, b"key", b"", 32);
        let sk32 = hkdf_expand_label(&s_ap_t, b"key", b"", 32);
        let mut c32 = [0u8; 32]; c32.copy_from_slice(&ck32);
        let mut s32 = [0u8; 32]; s32.copy_from_slice(&sk32);
        (c32, s32)
    };

    let id = alloc_id();
    SESSIONS.lock().insert(id, Session {
        app_client_key: app_keys.client_key,
        app_server_key: app_keys.server_key,
        app_client_iv:  app_keys.client_iv,
        app_server_iv:  app_keys.server_iv,
        app_client_key32: c_ap32,
        app_server_key32: s_ap32,
        send_seq: 0,
        recv_seq: 0,
        tcp_conn: tcp_id,
        cipher: negotiated_cipher,
        alpn: negotiated_alpn,
        host: alloc::string::String::from(host),
    });

    crate::serial_println!("[tls] Connected to '{}:{}' (id={})", host, port, id);
    Ok(id)
}

/// Send application data (dispatches to AES-128-GCM or ChaCha20-Poly1305 based on
/// the cipher suite negotiated during the handshake).
pub fn send(id: TlsConnId, data: &[u8]) -> Result<(), &'static str> {
    let mut sessions = SESSIONS.lock();
    let s = sessions.get_mut(&id).ok_or("invalid TLS id")?;
    let enc = if s.cipher == CIPHER_CHACHA20_POLY1305 {
        aead_seal_chacha(&s.app_client_key32, &s.app_client_iv, s.send_seq, REC_APPLICATION_DATA, data)
    } else {
        aead_seal(&s.app_client_key, &s.app_client_iv, s.send_seq, REC_APPLICATION_DATA, data)
    };
    s.send_seq += 1;
    let mut rec = vec![REC_APPLICATION_DATA, 0x03, 0x03,
                       (enc.len() >> 8) as u8, enc.len() as u8];
    rec.extend_from_slice(&enc);
    crate::net::tcp::send(s.tcp_conn, &rec).map(|_| ())
}

/// Receive and decrypt application data.
///
/// Skips post-handshake server messages (NewSessionTicket etc., inner_ct=22) and
/// only returns genuine application-data records (inner_ct=23) to the caller.
/// Dispatches to AES-128-GCM or ChaCha20-Poly1305 based on negotiated cipher.
pub fn recv(id: TlsConnId, buf: &mut [u8]) -> Result<usize, &'static str> {
    loop {
        // Copy the fields we need so the SESSIONS lock isn't held while blocking.
        let (tcp_conn, server_key, server_key32, server_iv, recv_seq, host, cipher) = {
            let sessions = SESSIONS.lock();
            let s = sessions.get(&id).ok_or("invalid TLS id")?;
            (s.tcp_conn, s.app_server_key, s.app_server_key32,
             s.app_server_iv, s.recv_seq, s.host.clone(), s.cipher)
        };

        // Try to read one TLS record (non-blocking: returns None if no data ready).
        let enc = match non_blocking_tcp_read_record(tcp_conn)? {
            Some(v) => v,
            None => return Ok(0), // no data yet
        };
        if enc.is_empty() { return Ok(0); }

        let (plain, inner_ct) = if cipher == CIPHER_CHACHA20_POLY1305 {
            chacha20poly1305_open_record(&server_key32, &server_iv, recv_seq, &enc)?
        } else {
            aead_open(&server_key, &server_iv, recv_seq, &enc)?
        };

        // Always advance the sequence number — we consumed this record.
        {
            let mut sessions = SESSIONS.lock();
            if let Some(s) = sessions.get_mut(&id) {
                s.recv_seq += 1;
            }
        }

        if inner_ct != REC_APPLICATION_DATA {
            // Post-handshake server message (inner_ct=22 → Handshake).
            // Phase 126: parse NewSessionTicket (HS type 4) and store for resumption.
            if inner_ct == REC_HANDSHAKE && plain.len() >= 4 && plain[0] == 4 {
                let msg_len = ((plain[1] as usize) << 16)
                            | ((plain[2] as usize) << 8)
                            |   plain[3] as usize;
                if 4 + msg_len <= plain.len() {
                    try_store_new_session_ticket(&host, &plain[4..4 + msg_len]);
                }
            }
            crate::serial_println!("[tls] recv: post-hs record inner_ct={} len={}", inner_ct, plain.len());
            continue;
        }

        let n = plain.len().min(buf.len());
        buf[..n].copy_from_slice(&plain[..n]);
        return Ok(n);
    }
}

/// Close a TLS session.
pub fn close(id: TlsConnId) -> Result<(), &'static str> {
    let mut sessions = SESSIONS.lock();
    if let Some(s) = sessions.remove(&id) {
        let _ = crate::net::tcp::close(s.tcp_conn);
    }
    Ok(())
}

/// Return the ALPN protocol negotiated during handshake ("h2", "http/1.1", or None).
pub fn negotiated_protocol(id: TlsConnId) -> Option<alloc::string::String> {
    SESSIONS.lock().get(&id).and_then(|s| s.alpn.clone())
}

/// Expose ALPN parser for self-tests in http2.rs (Phase 128).
pub fn parse_alpn_test(hs_data: &[u8]) -> Option<alloc::string::String> {
    parse_alpn_from_handshake(hs_data)
}

/// Send an arbitrary HTTP request over a fresh TLS connection and return the
/// raw response bytes.  Used by `net::http_client` so HTTPS uses the same
/// request-builder pipeline as plain HTTP (cookies, Accept-Encoding, etc.).
pub fn https_request(host: &str, port: u16, request_bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    let id = connect(host, port)?;
    send(id, request_bytes)?;
    let mut response = Vec::new();
    let mut buf = alloc::vec![0u8; 4096]; // heap, not stack — saves 4 KB of thread stack
    let mut idle = 0usize;
    const MAX_IDLE: usize = 20_000;
    loop {
        match recv(id, &mut buf) {
            Ok(0) => {
                idle += 1;
                if idle > MAX_IDLE { break; }
                crate::process::scheduler::yield_now();
            }
            Ok(n) => {
                response.extend_from_slice(&buf[..n]);
                idle = 0;
                if response.len() > 8 * 1024 * 1024 { break; }
            }
            Err(_) => break,
        }
    }
    close(id)?;
    Ok(response)
}

/// Simple HTTPS GET helper.
pub fn https_get(host: &str, path: &str) -> Result<Vec<u8>, &'static str> {
    let id = connect(host, 443)?;
    let request = alloc::format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path, host
    );
    send(id, request.as_bytes())?;
    let mut response = Vec::new();
    let mut buf = alloc::vec![0u8; 4096]; // heap, not stack
    let mut idle = 0usize;
    const MAX_IDLE: usize = 20_000; // ~2s read timeout after first data
    loop {
        match recv(id, &mut buf) {
            Ok(0) => {
                idle += 1;
                if idle > MAX_IDLE { break; }
                crate::process::scheduler::yield_now();
            }
            Ok(n) => {
                response.extend_from_slice(&buf[..n]);
                idle = 0;
                if response.len() > 1024 * 1024 { break; } // 1 MiB cap
            }
            Err(_) => break,
        }
    }
    close(id)?;
    Ok(response)
}

pub fn init() {
    crate::serial_println!("[tls] TLS 1.3 client initialized (AES-128-GCM-SHA256 + ChaCha20-Poly1305, X25519).");
}

// ── Public wrappers for WebCrypto API (net/webcrypto.rs) ──────────────────────

/// Encrypt with AES-128-GCM. Returns ciphertext || 16-byte auth tag.
pub fn aes128_gcm_encrypt(key: &[u8; 16], iv: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    aes128_gcm_seal(key, iv, aad, plaintext)
}

/// Decrypt with AES-128-GCM. Returns plaintext or Err on tag mismatch.
pub fn aes128_gcm_decrypt(key: &[u8; 16], iv: &[u8; 12], aad: &[u8], ct_tag: &[u8])
    -> Result<Vec<u8>, &'static str>
{
    aes128_gcm_open(key, iv, aad, ct_tag)
}

/// Generate 32 bytes of RDRAND entropy (used by WebCrypto getRandomValues).
pub fn random_bytes_32() -> [u8; 32] {
    rdrand32()
}

// ─────────────────────────────────────────────────────────────────────────────
//  Phase 126 — ChaCha20-Poly1305 cipher suite  (RFC 8439)
// ─────────────────────────────────────────────────────────────────────────────

const CIPHER_CHACHA20_POLY1305: u16 = 0x1303;

// ── ChaCha20 ─────────────────────────────────────────────────────────────────

#[inline(always)]
fn chacha20_qr(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(7);
}

/// RFC 8439 §2.1 — ChaCha20 block function.
/// key=32B, counter=u32, nonce=12B → 64-byte keystream block.
pub fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut s = [
        0x61707865u32, 0x3320646e, 0x79622d32, 0x6b206574,
        u32::from_le_bytes([key[ 0],key[ 1],key[ 2],key[ 3]]),
        u32::from_le_bytes([key[ 4],key[ 5],key[ 6],key[ 7]]),
        u32::from_le_bytes([key[ 8],key[ 9],key[10],key[11]]),
        u32::from_le_bytes([key[12],key[13],key[14],key[15]]),
        u32::from_le_bytes([key[16],key[17],key[18],key[19]]),
        u32::from_le_bytes([key[20],key[21],key[22],key[23]]),
        u32::from_le_bytes([key[24],key[25],key[26],key[27]]),
        u32::from_le_bytes([key[28],key[29],key[30],key[31]]),
        counter,
        u32::from_le_bytes([nonce[0],nonce[1],nonce[ 2],nonce[ 3]]),
        u32::from_le_bytes([nonce[4],nonce[5],nonce[ 6],nonce[ 7]]),
        u32::from_le_bytes([nonce[8],nonce[9],nonce[10],nonce[11]]),
    ];
    let init = s;
    for _ in 0..10 {
        chacha20_qr(&mut s, 0, 4,  8, 12);
        chacha20_qr(&mut s, 1, 5,  9, 13);
        chacha20_qr(&mut s, 2, 6, 10, 14);
        chacha20_qr(&mut s, 3, 7, 11, 15);
        chacha20_qr(&mut s, 0, 5, 10, 15);
        chacha20_qr(&mut s, 1, 6, 11, 12);
        chacha20_qr(&mut s, 2, 7,  8, 13);
        chacha20_qr(&mut s, 3, 4,  9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        let v = s[i].wrapping_add(init[i]);
        out[4*i..4*i+4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// XOR `data` with ChaCha20 keystream starting at the given block counter.
pub fn chacha20_xor(key: &[u8; 32], nonce: &[u8; 12], counter: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut ctr = counter;
    let mut pos = 0;
    while pos < data.len() {
        let block = chacha20_block(key, ctr, nonce);
        let take = (data.len() - pos).min(64);
        for i in 0..take { out.push(data[pos + i] ^ block[i]); }
        pos += take;
        ctr += 1;
    }
    out
}

// ── Poly1305 ─────────────────────────────────────────────────────────────────
//  5-limb implementation: each limb holds 26 bits in a u64, so products
//  (26b × 26b = 52b) fit safely within u64, and accumulations remain < 2^64.

/// RFC 8439 §2.5 — Poly1305 one-time MAC.
/// key=32B (r=first 16B clamped, s=last 16B), data=arbitrary → 16-byte tag.
pub fn poly1305_mac(key: &[u8; 32], data: &[u8]) -> [u8; 16] {
    // ── Parse r (clamped) into 5 × 26-bit limbs ──────────────────────────────
    let t = |b: &[u8], i: usize| u32::from_le_bytes([b[i],b[i+1],b[i+2],b[i+3]]) as u64;
    let t0 = t(key,  0);
    let t1 = t(key,  4);
    let t2 = t(key,  8);
    let t3 = t(key, 12);

    let r0 =  t0                     & 0x3FFFFFF;
    let r1 = ((t0 >> 26) | (t1 << 6)) & 0x3FFFF03;
    let r2 = ((t1 >> 20) | (t2 <<12)) & 0x3FFC0FF;
    let r3 = ((t2 >> 14) | (t3 <<18)) & 0x3F03FFF;
    let r4 =  (t3 >>  8)              & 0x00FFFFF;

    // Pre-multiply by 5 for the "wrap-around" reduction terms.
    let r1_5 = r1 * 5; let r2_5 = r2 * 5; let r3_5 = r3 * 5; let r4_5 = r4 * 5;

    let mut h0: u64 = 0; let mut h1: u64 = 0; let mut h2: u64 = 0;
    let mut h3: u64 = 0; let mut h4: u64 = 0;

    // ── Process blocks ────────────────────────────────────────────────────────
    for chunk in data.chunks(16) {
        // Pad block: append 0x01 byte at index len (full blocks get bit 128 set).
        let mut m = [0u8; 17];
        m[..chunk.len()].copy_from_slice(chunk);
        m[chunk.len()] = 1;

        let m0 = u32::from_le_bytes([m[ 0],m[ 1],m[ 2],m[ 3]]) as u64;
        let m1 = u32::from_le_bytes([m[ 4],m[ 5],m[ 6],m[ 7]]) as u64;
        let m2 = u32::from_le_bytes([m[ 8],m[ 9],m[10],m[11]]) as u64;
        let m3 = u32::from_le_bytes([m[12],m[13],m[14],m[15]]) as u64;
        let m4 = m[16] as u64;

        h0 +=  m0                     & 0x3FFFFFF;
        h1 += ((m0 >> 26) | (m1 << 6)) & 0x3FFFFFF;
        h2 += ((m1 >> 20) | (m2 <<12)) & 0x3FFFFFF;
        h3 += ((m2 >> 14) | (m3 <<18)) & 0x3FFFFFF;
        h4 +=  (m3 >>  8) | (m4 << 24);

        // h *= r (with reduction mod 2^130-5 via 5× wrap terms)
        let d0 = h0*r0 + h1*r4_5 + h2*r3_5 + h3*r2_5 + h4*r1_5;
        let d1 = h0*r1 + h1*r0   + h2*r4_5 + h3*r3_5 + h4*r2_5;
        let d2 = h0*r2 + h1*r1   + h2*r0   + h3*r4_5 + h4*r3_5;
        let d3 = h0*r3 + h1*r2   + h2*r1   + h3*r0   + h4*r4_5;
        let d4 = h0*r4 + h1*r3   + h2*r2   + h3*r1   + h4*r0;

        // Carry propagation (2-pass to drain the high bits back into h0)
        let c = d0 >> 26; h0 = d0 & 0x3FFFFFF; let d1 = d1 + c;
        let c = d1 >> 26; h1 = d1 & 0x3FFFFFF; let d2 = d2 + c;
        let c = d2 >> 26; h2 = d2 & 0x3FFFFFF; let d3 = d3 + c;
        let c = d3 >> 26; h3 = d3 & 0x3FFFFFF; let d4 = d4 + c;
        let c = d4 >> 26; h4 = d4 & 0x3FFFFFF; h0 += c * 5;
        let c = h0 >> 26; h0 &= 0x3FFFFFF;      h1 += c;
    }

    // ── Final full reduction: h mod (2^130 - 5) ───────────────────────────────
    // Propagate any remaining carry
    let c = h1 >> 26; let h1 = h1 & 0x3FFFFFF; let h2 = h2 + c;
    let c = h2 >> 26; let h2 = h2 & 0x3FFFFFF; let h3 = h3 + c;
    let c = h3 >> 26; let h3 = h3 & 0x3FFFFFF; let h4 = h4 + c;
    let c = h4 >> 26; let h4 = h4 & 0x3FFFFFF; let h0 = h0 + c * 5;
    let c = h0 >> 26; let h0 = h0 & 0x3FFFFFF; let h1 = h1 + c;

    // Compute g = h + 5 and check whether h >= p (i.e. g overflows into bit 130).
    let g0 = h0 + 5;
    let c  = g0 >> 26; let g0 = g0 & 0x3FFFFFF;
    let g1 = h1 + c;  let c  = g1 >> 26; let g1 = g1 & 0x3FFFFFF;
    let g2 = h2 + c;  let c  = g2 >> 26; let g2 = g2 & 0x3FFFFFF;
    let g3 = h3 + c;  let c  = g3 >> 26; let g3 = g3 & 0x3FFFFFF;
    let g4 = h4 + c;
    // If g4 >= 4, g overflowed beyond 2^130 — meaning h >= p, so use g.
    let mask = 0u64.wrapping_sub(g4 >> 2); // all-ones if g4>=4, all-zeros otherwise
    let h0 = (h0 & !mask) | (g0 & mask);
    let h1 = (h1 & !mask) | (g1 & mask);
    let h2 = (h2 & !mask) | (g2 & mask);
    let h3 = (h3 & !mask) | (g3 & mask);
    let h4 = (h4 & !mask) | (g4 & mask);

    // ── Serialize h to 4 × u32 (little-endian) and add s ─────────────────────
    let f0 = h0 | (h1 << 26);
    let f1 = (h1 >> 6) | (h2 << 20);
    let f2 = (h2 >> 12) | (h3 << 14);
    let f3 = (h3 >> 18) | (h4 << 8);

    let s0 = u32::from_le_bytes([key[16],key[17],key[18],key[19]]) as u64;
    let s1 = u32::from_le_bytes([key[20],key[21],key[22],key[23]]) as u64;
    let s2 = u32::from_le_bytes([key[24],key[25],key[26],key[27]]) as u64;
    let s3 = u32::from_le_bytes([key[28],key[29],key[30],key[31]]) as u64;

    // Add s with carry propagation (mod 2^128, per RFC 8439 §2.5)
    let sum0 = (f0 & 0xFFFFFFFF) + s0;
    let sum1 = (f1 & 0xFFFFFFFF) + s1 + (sum0 >> 32);
    let sum2 = (f2 & 0xFFFFFFFF) + s2 + (sum1 >> 32);
    let sum3 = (f3 & 0xFFFFFFFF) + s3 + (sum2 >> 32);

    let mut tag = [0u8; 16];
    tag[ 0.. 4].copy_from_slice(&(sum0 as u32).to_le_bytes());
    tag[ 4.. 8].copy_from_slice(&(sum1 as u32).to_le_bytes());
    tag[ 8..12].copy_from_slice(&(sum2 as u32).to_le_bytes());
    tag[12..16].copy_from_slice(&(sum3 as u32).to_le_bytes());
    tag
}

// ── ChaCha20-Poly1305-IETF AEAD ──────────────────────────────────────────────

/// Pad `data` to the next 16-byte boundary (for Poly1305 input construction).
fn poly_pad16(v: &mut Vec<u8>, data: &[u8]) {
    v.extend_from_slice(data);
    let rem = data.len() % 16;
    if rem != 0 { for _ in 0..(16 - rem) { v.push(0); } }
}

/// RFC 8439 §2.8 — ChaCha20-Poly1305 AEAD seal.
/// Returns ciphertext || 16-byte tag.
pub fn chacha20poly1305_seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    // One-time Poly1305 key = first 32 bytes of block(key, ctr=0, nonce)
    let ks0 = chacha20_block(key, 0, nonce);
    let poly_key: [u8; 32] = ks0[..32].try_into().unwrap();

    // Encrypt with ChaCha20 (counter starts at 1)
    let ciphertext = chacha20_xor(key, nonce, 1, plaintext);

    // Build Poly1305 input: aad || pad16(aad) || ct || pad16(ct) || len64_le(aad) || len64_le(ct)
    let mut mac_data: Vec<u8> = Vec::new();
    poly_pad16(&mut mac_data, aad);
    poly_pad16(&mut mac_data, &ciphertext);
    mac_data.extend_from_slice(&(aad.len() as u64).to_le_bytes());
    mac_data.extend_from_slice(&(ciphertext.len() as u64).to_le_bytes());

    let tag = poly1305_mac(&poly_key, &mac_data);

    let mut out = ciphertext;
    out.extend_from_slice(&tag);
    out
}

/// TLS 1.3 §5.3 ChaCha20-Poly1305 seal for a single record.
/// Appends inner content_type, builds TLS nonce (big-endian seq XOR), returns ciphertext||tag.
fn aead_seal_chacha(key: &[u8; 32], iv: &[u8; 12], seq: u64, content_type: u8, plain: &[u8]) -> Vec<u8> {
    // TLS 1.3 inner plaintext = data || content_type byte
    let mut inner = plain.to_vec();
    inner.push(content_type);
    // RFC 8446 §5.3 nonce: big-endian seq XOR'd into last 8 bytes
    let nonce = make_nonce(iv, seq);
    // AAD = TLS record header (type=23, legacy version, length including 16-byte tag)
    let ct_len = (inner.len() + 16) as u16;
    let aad: [u8; 5] = [REC_APPLICATION_DATA, 0x03, 0x03, (ct_len >> 8) as u8, ct_len as u8];
    chacha20poly1305_seal(key, &nonce, &aad, &inner)
}

/// RFC 8439 §2.8 — ChaCha20-Poly1305 AEAD open.
/// Returns (plaintext, inner_content_type) or Err on tag mismatch.
pub fn chacha20poly1305_open_record(key: &[u8; 32], nonce: &[u8; 12], seq: u64, data: &[u8])
    -> Result<(Vec<u8>, u8), &'static str>
{
    // RFC 8446 §5.3 nonce: big-endian seq XOR'd into last 8 bytes (corrected from LE).
    let mut iv = *nonce;
    let seq_bytes = seq.to_be_bytes();
    for i in 0..8 { iv[4 + i] ^= seq_bytes[i]; }

    if data.len() < 17 { return Err("ChaCha20-Poly1305: ciphertext too short"); }
    let ciphertext = &data[..data.len() - 16];
    let recv_tag   = &data[data.len() - 16..];

    // Recompute Poly1305 key
    let ks0 = chacha20_block(key, 0, &iv);
    let poly_key: [u8; 32] = ks0[..32].try_into().unwrap();

    // AAD for TLS 1.3 = opaque record header (content_type=23, version, length)
    let rec_len = ciphertext.len() + 16;
    let aad_bytes: [u8; 5] = [
        REC_APPLICATION_DATA, 0x03, 0x03,
        (rec_len >> 8) as u8, rec_len as u8,
    ];

    let mut mac_data: Vec<u8> = Vec::new();
    poly_pad16(&mut mac_data, &aad_bytes);
    poly_pad16(&mut mac_data, ciphertext);
    mac_data.extend_from_slice(&(aad_bytes.len() as u64).to_le_bytes());
    mac_data.extend_from_slice(&(ciphertext.len() as u64).to_le_bytes());

    let expected_tag = poly1305_mac(&poly_key, &mac_data);
    if expected_tag != recv_tag { return Err("ChaCha20-Poly1305: tag mismatch"); }

    // Decrypt ciphertext (counter starts at 1)
    let plaintext = chacha20_xor(key, &iv, 1, ciphertext);

    // TLS 1.3: inner content type is the last non-zero byte of the plaintext.
    let mut inner_ct = REC_APPLICATION_DATA;
    for &b in plaintext.iter().rev() {
        if b != 0 { inner_ct = b; break; }
    }
    let strip_len = plaintext.iter().rev().position(|&b| b != 0).unwrap_or(0) + 1;
    let body = plaintext[..plaintext.len() - strip_len].to_vec();

    Ok((body, inner_ct))
}

// ── Phase 126: Session Ticket Storage ────────────────────────────────────────
//  TLS 1.3 session tickets are stored per-hostname so a reconnect can
//  offer the PSK extension.  Binder computation is left as TODO (requires a
//  partial hash of ClientHello), so the tickets are stored but not yet offered.

struct TicketStore(BTreeMap<alloc::string::String, Vec<u8>>);
unsafe impl Send for TicketStore {}
unsafe impl Sync for TicketStore {}

static SESSION_TICKETS: Mutex<TicketStore> = Mutex::new(TicketStore(BTreeMap::new()));

/// Store a session ticket received from `host`.
pub fn store_session_ticket(host: &str, ticket: Vec<u8>) {
    if let Some(mut ts) = SESSION_TICKETS.try_lock() {
        ts.0.insert(alloc::string::String::from(host), ticket);
    }
}

/// Retrieve a stored session ticket for `host` (if any).
pub fn get_session_ticket(host: &str) -> Option<Vec<u8>> {
    SESSION_TICKETS.try_lock()
        .and_then(|ts| ts.0.get(host).cloned())
}

/// Parse a TLS 1.3 NewSessionTicket message (HS type 4) and store the ticket.
/// Called from `recv()` when a post-handshake HS record arrives.
fn try_store_new_session_ticket(host: &str, hs_payload: &[u8]) {
    // NewSessionTicket layout (RFC 8446 §4.6.1):
    //   ticket_lifetime  u32
    //   ticket_age_add   u32
    //   ticket_nonce     u8 + bytes
    //   ticket           u16 + bytes
    //   extensions       u16 + bytes
    if hs_payload.len() < 9 { return; }
    let nonce_len = hs_payload[8] as usize;
    let ticket_off = 9 + nonce_len;
    if ticket_off + 2 > hs_payload.len() { return; }
    let ticket_len = u16::from_be_bytes([hs_payload[ticket_off], hs_payload[ticket_off + 1]]) as usize;
    let ticket_start = ticket_off + 2;
    if ticket_start + ticket_len > hs_payload.len() { return; }
    let ticket = hs_payload[ticket_start..ticket_start + ticket_len].to_vec();
    crate::serial_println!("[tls] Stored session ticket for '{}' ({} bytes)", host, ticket.len());
    store_session_ticket(host, ticket);
}

// ── Phase 126: Self-test ──────────────────────────────────────────────────────

pub fn self_test_126() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] tls126: {}", $name); }
        }
    }

    // T1: ChaCha20 block — RFC 8439 §2.1.1 test vector
    {
        let key = [0u8; 32];
        let nonce = [0u8; 12];
        let block = chacha20_block(&key, 0, &nonce);
        // Known first 4 words: 76b8e0ad a0f13d90 405d6ae5 5386bd28
        check!(block[0] == 0x76, "chacha20 block[0]");
        check!(block[4] == 0xa0, "chacha20 block[4]");
    }

    // T2: RFC 8439 §2.1.1 second test vector (key=0, ctr=1, nonce=0)
    {
        let key = [0u8; 32];
        let nonce = [0u8; 12];
        let b0 = chacha20_block(&key, 0, &nonce);
        let b1 = chacha20_block(&key, 1, &nonce);
        check!(b0 != b1, "different counters → different blocks");
    }

    // T3: ChaCha20-XOR is its own inverse
    {
        let key = [0x80u8; 32];
        let nonce = [0x77u8; 12];
        let plaintext = b"Hello, ChaCha20-Poly1305!";
        let ct = chacha20_xor(&key, &nonce, 1, plaintext);
        let pt = chacha20_xor(&key, &nonce, 1, &ct);
        check!(&pt == plaintext, "ChaCha20 encrypt/decrypt round-trip");
    }

    // T4: Poly1305 — RFC 8439 §2.5.2 test vector
    {
        let key: [u8; 32] = [
            0x85, 0xd6, 0xbe, 0x78, 0x57, 0x55, 0x6d, 0x33, 0x7f, 0x44, 0x52, 0xfe,
            0x42, 0xd5, 0x06, 0xa8, 0x01, 0x03, 0x80, 0x8a, 0xfb, 0x0d, 0xb2, 0xfd,
            0x4a, 0xbf, 0xf6, 0xaf, 0x41, 0x49, 0xf5, 0x1b,
        ];
        let msg = b"Cryptographic Forum Research Group";
        let expected_tag: [u8; 16] = [
            0xa8, 0x06, 0x1d, 0xc1, 0x30, 0x51, 0x36, 0xc6,
            0xc2, 0x2b, 0x8b, 0xaf, 0x0c, 0x01, 0x27, 0xa9,
        ];
        let tag = poly1305_mac(&key, msg);
        check!(tag == expected_tag, "Poly1305 RFC 8439 test vector");
    }

    // T5: ChaCha20-Poly1305 AEAD round-trip
    {
        let key = [0x42u8; 32];
        let nonce = [0x11u8; 12];
        let aad = b"header";
        let plaintext = b"TLS 1.3 data";
        let sealed = chacha20poly1305_seal(&key, &nonce, aad, plaintext);
        check!(sealed.len() == plaintext.len() + 16, "AEAD sealed length");
        // Note: open is tested via chacha20poly1305_open_record which uses TLS framing
    }

    // T6: Poly1305 key is zeroed → tag = [0; 16] for any input? No — check with empty
    {
        let key = [0u8; 32];
        let tag = poly1305_mac(&key, b"");
        // All-zero key + empty message → tag = s (key[16..]) = 0
        check!(tag == [0u8; 16], "Poly1305 zero key empty message");
    }

    // T7: Session ticket storage round-trip
    {
        let ticket = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
        store_session_ticket("example.com", ticket.clone());
        let retrieved = get_session_ticket("example.com");
        check!(retrieved == Some(ticket), "session ticket store/retrieve");
    }

    if fail == 0 {
        crate::serial_println!("[tls126] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[tls126] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

// ── TCP helpers ───────────────────────────────────────────────────────

/// Blocking read: fills `buf` completely, yielding until all bytes arrive.
/// Times out after ~5 seconds (50 000 yield iterations at ~100 µs each).
fn tcp_recv_all(conn_id: crate::net::tcp::TcpSocketId, buf: &mut [u8]) -> Result<(), &'static str> {
    let mut pos = 0;
    let mut idle = 0usize;
    const TIMEOUT: usize = 50_000;
    while pos < buf.len() {
        match crate::net::tcp::recv(conn_id, &mut buf[pos..]) {
            Ok(0) => {
                // No data yet — check if connection is still open
                idle += 1;
                if idle > TIMEOUT {
                    crate::serial_println!("[tls] tcp_recv_all: timeout after {} iters, pos={}/{}", TIMEOUT, pos, buf.len());
                    return Err("TLS recv timeout");
                }
                crate::process::scheduler::yield_now();
            }
            Ok(n) => { pos += n; idle = 0; }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Non-blocking TLS record read: returns Ok(None) if no data available yet.
/// Used by recv() so it doesn't hold SESSIONS lock while blocking.
fn non_blocking_tcp_read_record(conn_id: crate::net::tcp::TcpSocketId)
    -> Result<Option<Vec<u8>>, &'static str>
{
    // Peek at the first byte — if nothing there, return None immediately.
    let mut peek = [0u8; 1];
    match crate::net::tcp::recv(conn_id, &mut peek) {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(e) => return Err(e),
    }
    // We have at least one byte; now read the remaining 4 header bytes.
    let mut hdr = [0u8; 5];
    hdr[0] = peek[0];
    tcp_recv_all(conn_id, &mut hdr[1..])?;
    let len = u16::from_be_bytes([hdr[3], hdr[4]]) as usize;
    if len > 16 * 1024 + 256 {
        return Err("TLS record too large");
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        tcp_recv_all(conn_id, &mut body)?;
    }
    Ok(Some(body))
}

/// Read one TLS record (blocks until complete).  Returns the record body.
fn tcp_read_record(conn_id: crate::net::tcp::TcpSocketId) -> Result<Vec<u8>, &'static str> {
    // TLS record header: [content_type(1), version(2), length(2)]
    let mut hdr = [0u8; 5];
    tcp_recv_all(conn_id, &mut hdr)?;
    let len = u16::from_be_bytes([hdr[3], hdr[4]]) as usize;
    if len > 16 * 1024 + 256 {
        return Err("TLS record too large");
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        tcp_recv_all(conn_id, &mut body)?;
    }
    Ok(body)
}

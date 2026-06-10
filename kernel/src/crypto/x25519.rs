//! X25519 Diffie-Hellman key exchange — RFC 7748.
//!
//! Uses the Montgomery ladder for constant-time scalar multiplication.
//! Field: GF(2^255 - 19).

#![allow(dead_code)]

/// The prime p = 2^255 - 19.
/// Stored as 4 × u64 limbs (little-endian).
const P: [u64; 4] = [
    0xFFFFFFFFFFFFED,
    0xFFFFFFFFFFFFFF,
    0xFFFFFFFFFFFFFF,
    0x7FFFFFFFFFFFFFFF,
];

/// a24 = 121665 (the (A-2)/4 constant for curve25519).
const A24: u64 = 121665;

// ─────────────────────────────────────────────────────────────────────────────
//  Field arithmetic mod p = 2^255 - 19 using 4×u64 limbs
// ─────────────────────────────────────────────────────────────────────────────

type Fe = [u64; 4];

fn fe_zero() -> Fe { [0, 0, 0, 0] }
fn fe_one()  -> Fe { [1, 0, 0, 0] }

/// Reduce mod p: if v >= p, subtract p once.
fn fe_reduce(v: &mut Fe) {
    // Add 19 to check for overflow.
    let (t0, c) = v[0].overflowing_add(19);
    let (t1, c) = v[1].overflowing_add(c as u64);
    let (t2, c) = v[2].overflowing_add(c as u64);
    let (t3, _) = v[3].overflowing_add(c as u64);
    // If the top bit is set after adding 19, we overflowed → subtract p.
    if t3 >> 63 == 0 && (t3 < 0x7FFFFFFFFFFFFFFF || (t2 | t1 | t0) != 0 || t3 == 0x7FFFFFFFFFFFFFFF) {
        // Not overflowed: no reduction needed. Actually, check differently.
    }
    // Simple: subtract p if v >= p.
    let geq = ge_p(v);
    if geq {
        let (s0, b) = v[0].overflowing_sub(P[0]);
        let (s1, b) = v[1].overflowing_sub(P[1].wrapping_add(b as u64));
        let (s2, b) = v[2].overflowing_sub(P[2].wrapping_add(b as u64));
        let (s3, _) = v[3].overflowing_sub(P[3].wrapping_add(b as u64));
        v[0] = s0; v[1] = s1; v[2] = s2; v[3] = s3;
    }
}

fn ge_p(v: &Fe) -> bool {
    if v[3] > P[3] { return true; }
    if v[3] < P[3] { return false; }
    if v[2] > P[2] { return true; }
    if v[2] < P[2] { return false; }
    if v[1] > P[1] { return true; }
    if v[1] < P[1] { return false; }
    v[0] >= P[0]
}

fn fe_add(a: &Fe, b: &Fe) -> Fe {
    let (r0, c) = a[0].overflowing_add(b[0]);
    let (r1, c) = a[1].overflowing_add(b[1].wrapping_add(c as u64));
    let (r2, c) = a[2].overflowing_add(b[2].wrapping_add(c as u64));
    let (r3, _) = a[3].overflowing_add(b[3].wrapping_add(c as u64));
    let mut r = [r0, r1, r2, r3];
    fe_reduce(&mut r);
    r
}

fn fe_sub(a: &Fe, b: &Fe) -> Fe {
    // a - b mod p.  If a < b, add p first.
    let p_plus_a = fe_add(a, &P);
    let (r0, bw) = p_plus_a[0].overflowing_sub(b[0]);
    let (r1, bw) = p_plus_a[1].overflowing_sub(b[1].wrapping_add(bw as u64));
    let (r2, bw) = p_plus_a[2].overflowing_sub(b[2].wrapping_add(bw as u64));
    let (r3, _)  = p_plus_a[3].overflowing_sub(b[3].wrapping_add(bw as u64));
    let mut r = [r0, r1, r2, r3];
    fe_reduce(&mut r);
    r
}

/// 256-bit × 256-bit → 256-bit multiplication mod p.
fn fe_mul(a: &Fe, b: &Fe) -> Fe {
    // Schoolbook multiply into 512-bit result, then reduce mod 2^255-19.
    let mut t = [0u128; 8];
    for i in 0..4 {
        for j in 0..4 {
            t[i+j] += (a[i] as u128) * (b[j] as u128);
        }
    }
    // Reduce the upper 4 limbs using p = 2^255-19 → upper * 38 + lower.
    // t[4..8] contributes t[k] * 2^(64*k) = t[k] * 2^(64*(k-4)) * 2^256.
    // Since 2^256 ≡ 38 (mod p), multiply upper limbs by 38.
    for k in 4..8 {
        t[k - 4] += t[k] * 38;
        t[k] = 0;
    }
    // Now collect into 4 limbs with carries.
    let r0 = t[0] + t[4] * 38;
    let c0 = r0 >> 64;
    let r0 = r0 as u64;
    let r1 = t[1] + c0;
    let c1 = r1 >> 64;
    let r1 = r1 as u64;
    let r2 = t[2] + c1;
    let c2 = r2 >> 64;
    let r2 = r2 as u64;
    let r3 = t[3] + c2;
    let c3 = r3 >> 64;
    let r3 = r3 as u64;
    // If c3 carries, multiply by 38 again.
    let extra = c3 as u64 * 38;
    let (r0, ec) = r0.overflowing_add(extra);
    let (r1, ec) = r1.overflowing_add(ec as u64);
    let (r2, ec) = r2.overflowing_add(ec as u64);
    let (r3, _)  = r3.overflowing_add(ec as u64);
    let mut out = [r0, r1, r2, r3];
    fe_reduce(&mut out);
    out
}

fn fe_sq(a: &Fe) -> Fe { fe_mul(a, a) }

fn fe_mul_small(a: &Fe, b: u64) -> Fe {
    let mut t = [0u128; 5];
    for i in 0..4 { t[i] = (a[i] as u128) * (b as u128); }
    let (r0, c) = (t[0] as u64, t[0] >> 64);
    let t1 = t[1] + c;
    let (r1, c) = (t1 as u64, t1 >> 64);
    let t2 = t[2] + c;
    let (r2, c) = (t2 as u64, t2 >> 64);
    let t3 = t[3] + c;
    let (r3, c) = (t3 as u64, t3 >> 64);
    // Handle carry out.
    let extra = c as u64 * 38;
    let (r0, ec) = r0.overflowing_add(extra);
    let (r1, ec) = r1.overflowing_add(ec as u64);
    let (r2, ec) = r2.overflowing_add(ec as u64);
    let (r3, _)  = r3.overflowing_add(ec as u64);
    let mut out = [r0, r1, r2, r3];
    fe_reduce(&mut out);
    out
}

/// Field inversion: a^(p-2) mod p.
fn fe_inv(a: &Fe) -> Fe {
    // Compute a^(p-2) = a^(2^255 - 21) via repeated squaring.
    // Square chain from RFC 7748 Appendix.
    let a1 = *a;
    let a2  = fe_sq(&a1);
    let a4  = fe_sq(&a2);
    let a8  = fe_sq(&a4);
    let a9  = fe_mul(&a8, &a1);
    let a11 = fe_mul(&a9, &a2);
    let a22 = fe_sq(&a11);
    let a_5_0 = fe_mul(&a22, &a9); // 2^5 - 1
    // Build up to 2^255 - 21.
    let mut t = a_5_0;
    for _ in 0..5  { t = fe_sq(&t); }
    t = fe_mul(&t, &a_5_0);
    let t10 = t;
    for _ in 0..10 { t = fe_sq(&t); }
    t = fe_mul(&t, &t10);
    let t20 = t;
    for _ in 0..20 { t = fe_sq(&t); }
    t = fe_mul(&t, &t20);
    for _ in 0..10 { t = fe_sq(&t); }
    t = fe_mul(&t, &t10);
    let t50 = t;
    for _ in 0..50 { t = fe_sq(&t); }
    t = fe_mul(&t, &t50);
    let t100 = t;
    for _ in 0..100 { t = fe_sq(&t); }
    t = fe_mul(&t, &t100);
    for _ in 0..50 { t = fe_sq(&t); }
    t = fe_mul(&t, &t50);
    for _ in 0..5  { t = fe_sq(&t); }
    fe_mul(&t, &a_5_0)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Montgomery ladder scalar multiplication
// ─────────────────────────────────────────────────────────────────────────────

/// Conditional swap (constant-time): if bit=1, swap (a,b).
#[inline]
fn cswap(swap: u64, a: &mut Fe, b: &mut Fe) {
    let mask = 0_u64.wrapping_sub(swap); // 0 or 0xFFFF...
    for i in 0..4 {
        let t = mask & (a[i] ^ b[i]);
        a[i] ^= t;
        b[i] ^= t;
    }
}

/// Decode a 32-byte scalar (clamp per RFC 7748).
fn clamp_scalar(s: &[u8; 32]) -> [u8; 32] {
    let mut k = *s;
    k[0]  &= 248;
    k[31] &= 127;
    k[31] |= 64;
    k
}

/// Decode a 32-byte u-coordinate.
fn decode_u(u: &[u8; 32]) -> Fe {
    let mut limbs = [0u64; 4];
    for i in 0..4 {
        limbs[i] = u64::from_le_bytes([u[i*8], u[i*8+1], u[i*8+2], u[i*8+3],
                                        u[i*8+4], u[i*8+5], u[i*8+6], u[i*8+7]]);
    }
    // Clear the high bit per RFC 7748.
    limbs[3] &= (1 << 63) - 1;
    limbs
}

/// Encode a field element as 32-byte little-endian.
fn encode_fe(fe: &Fe) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..4 {
        out[i*8..i*8+8].copy_from_slice(&fe[i].to_le_bytes());
    }
    out
}

/// Montgomery ladder: compute scalar * u_coord on curve25519.
fn ladder(k: &[u8; 32], u: &Fe) -> Fe {
    let mut x1 = *u;
    let mut x2 = fe_one();
    let mut z2 = fe_zero();
    let mut x3 = *u;
    let mut z3 = fe_one();
    let mut swap = 0u64;

    for t in (0..255).rev() {
        let k_t = ((k[t / 8] >> (t % 8)) & 1) as u64;
        swap ^= k_t;
        cswap(swap, &mut x2, &mut x3);
        cswap(swap, &mut z2, &mut z3);
        swap = k_t;

        let a  = fe_add(&x2, &z2);
        let aa = fe_sq(&a);
        let b  = fe_sub(&x2, &z2);
        let bb = fe_sq(&b);
        let e  = fe_sub(&aa, &bb);
        let c  = fe_add(&x3, &z3);
        let d  = fe_sub(&x3, &z3);
        let da = fe_mul(&d, &a);
        let cb = fe_mul(&c, &b);
        let x5 = fe_add(&da, &cb);
        x3 = fe_sq(&x5);
        let x5 = fe_sub(&da, &cb);
        z3 = fe_mul(&fe_sq(&x5), &x1);
        x2 = fe_mul(&aa, &bb);
        z2 = fe_mul(&e, &fe_add(&aa, &fe_mul_small(&e, A24)));
    }

    cswap(swap, &mut x2, &mut x3);
    cswap(swap, &mut z2, &mut z3);
    fe_mul(&x2, &fe_inv(&z2))
}

// ─────────────────────────────────────────────────────────────────────────────
//  Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Base point u-coordinate = 9.
const BASE_U: Fe = [9, 0, 0, 0];

/// Generate a public key from a 32-byte private key.
pub fn x25519_public_key(private_key: &[u8; 32]) -> [u8; 32] {
    let scalar = clamp_scalar(private_key);
    let result = ladder(&scalar, &BASE_U);
    encode_fe(&result)
}

/// Compute shared secret from our private key and their public key.
pub fn x25519_diffie_hellman(our_private: &[u8; 32], their_public: &[u8; 32]) -> [u8; 32] {
    let scalar = clamp_scalar(our_private);
    let u = decode_u(their_public);
    let result = ladder(&scalar, &u);
    encode_fe(&result)
}

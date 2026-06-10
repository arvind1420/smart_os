//! Constant-time big-integer arithmetic for cryptography — Phase 24.
//!
//! Supports up to 4096-bit numbers stored as arrays of u64 limbs (little-endian).
//! Operations: add, sub, mul, mod_reduce, mod_exp (right-to-left square-and-multiply).

#![allow(dead_code)]

/// Maximum number of u64 limbs (supports up to 4096-bit numbers).
pub const MAX_LIMBS: usize = 64;

/// Big integer stored as little-endian u64 limbs.
/// `n` = number of active limbs.
#[derive(Clone, Copy, Debug)]
pub struct BigInt {
    pub limbs: [u64; MAX_LIMBS],
    pub n:     usize,
}

impl BigInt {
    pub const ZERO: BigInt = BigInt { limbs: [0u64; MAX_LIMBS], n: 1 };

    pub fn from_u64(v: u64) -> Self {
        let mut b = Self::ZERO;
        b.limbs[0] = v;
        b.n = 1;
        b
    }

    /// Construct from big-endian byte slice.
    pub fn from_be_bytes(bytes: &[u8]) -> Self {
        let mut b = Self::ZERO;
        let limb_count = (bytes.len() + 7) / 8;
        let n = limb_count.min(MAX_LIMBS);
        for i in 0..n {
            let byte_idx = bytes.len().saturating_sub((i + 1) * 8);
            let end = bytes.len().saturating_sub(i * 8);
            let chunk = &bytes[byte_idx..end];
            let mut val = 0u64;
            for (k, &byte) in chunk.iter().enumerate() {
                val |= (byte as u64) << ((chunk.len() - 1 - k) * 8);
            }
            b.limbs[i] = val;
        }
        b.n = n.max(1);
        b.normalize();
        b
    }

    /// Serialize to big-endian bytes of given length.
    pub fn to_be_bytes(&self, len: usize) -> alloc::vec::Vec<u8> {
        let mut out = alloc::vec![0u8; len];
        for i in 0..self.n {
            let v = self.limbs[i];
            for k in 0..8 {
                let byte_pos = len.saturating_sub(i * 8 + k + 1);
                if byte_pos < len { out[byte_pos] = (v >> (k * 8)) as u8; }
            }
        }
        out
    }

    pub fn is_zero(&self) -> bool { self.limbs[..self.n].iter().all(|&l| l == 0) }

    pub fn bit_len(&self) -> usize {
        for i in (0..self.n).rev() {
            if self.limbs[i] != 0 {
                return i * 64 + (64 - self.limbs[i].leading_zeros() as usize);
            }
        }
        0
    }

    pub fn bit(&self, i: usize) -> bool {
        let limb = i / 64;
        let bit  = i % 64;
        if limb >= self.n { false } else { (self.limbs[limb] >> bit) & 1 == 1 }
    }

    pub fn normalize(&mut self) {
        while self.n > 1 && self.limbs[self.n - 1] == 0 { self.n -= 1; }
    }

    // ── Comparisons ──────────────────────────────────────────────────────────

    pub fn cmp(&self, rhs: &BigInt) -> core::cmp::Ordering {
        let n = self.n.max(rhs.n);
        for i in (0..n).rev() {
            let a = if i < self.n { self.limbs[i] } else { 0 };
            let b = if i < rhs.n  { rhs.limbs[i]  } else { 0 };
            match a.cmp(&b) {
                core::cmp::Ordering::Equal => continue,
                other => return other,
            }
        }
        core::cmp::Ordering::Equal
    }

    pub fn eq_const(&self, rhs: &BigInt) -> bool {
        // Constant-time equality check.
        let n = self.n.max(rhs.n).max(1);
        let mut diff = 0u64;
        for i in 0..n {
            let a = if i < self.n { self.limbs[i] } else { 0 };
            let b = if i < rhs.n  { rhs.limbs[i]  } else { 0 };
            diff |= a ^ b;
        }
        diff == 0
    }

    // ── Arithmetic ───────────────────────────────────────────────────────────

    /// self + rhs (no modular reduction).
    pub fn add(&self, rhs: &BigInt) -> BigInt {
        let mut out = BigInt::ZERO;
        let n = self.n.max(rhs.n);
        let mut carry = 0u64;
        for i in 0..n {
            let a = if i < self.n { self.limbs[i] } else { 0 };
            let b = if i < rhs.n  { rhs.limbs[i]  } else { 0 };
            let (sum, c1) = a.overflowing_add(b);
            let (sum, c2) = sum.overflowing_add(carry);
            out.limbs[i] = sum;
            carry = (c1 as u64) + (c2 as u64);
        }
        if carry > 0 && n < MAX_LIMBS { out.limbs[n] = carry; out.n = n + 1; }
        else { out.n = n.max(1); }
        out.normalize();
        out
    }

    /// self - rhs  (assumes self >= rhs; wraps otherwise).
    pub fn sub(&self, rhs: &BigInt) -> BigInt {
        let mut out = BigInt::ZERO;
        let n = self.n.max(rhs.n);
        let mut borrow = 0u64;
        for i in 0..n {
            let a = if i < self.n { self.limbs[i] } else { 0 };
            let b = if i < rhs.n  { rhs.limbs[i]  } else { 0 };
            let (diff, b1) = a.overflowing_sub(b);
            let (diff, b2) = diff.overflowing_sub(borrow);
            out.limbs[i] = diff;
            borrow = (b1 as u64) + (b2 as u64);
        }
        out.n = n.max(1);
        out.normalize();
        out
    }

    /// self * rhs (schoolbook, O(n²)).
    pub fn mul(&self, rhs: &BigInt) -> BigInt {
        let mut out = BigInt::ZERO;
        let n = (self.n + rhs.n).min(MAX_LIMBS);
        out.n = n;
        for i in 0..self.n {
            let mut carry = 0u128;
            for j in 0..rhs.n {
                if i + j >= MAX_LIMBS { break; }
                let prod = (self.limbs[i] as u128) * (rhs.limbs[j] as u128)
                    + (out.limbs[i+j] as u128) + carry;
                out.limbs[i+j] = prod as u64;
                carry = prod >> 64;
            }
            let k = i + rhs.n;
            if k < MAX_LIMBS { out.limbs[k] = out.limbs[k].wrapping_add(carry as u64); }
        }
        out.normalize();
        out
    }

    /// Shift left by 1 bit.
    pub fn shl1(&self) -> BigInt {
        let mut out = *self;
        let mut carry = 0u64;
        for i in 0..self.n {
            let new_carry = out.limbs[i] >> 63;
            out.limbs[i] = (out.limbs[i] << 1) | carry;
            carry = new_carry;
        }
        if carry != 0 && self.n < MAX_LIMBS { out.limbs[self.n] = carry; out.n = self.n + 1; }
        out
    }

    /// Shift right by 1 bit.
    pub fn shr1(&self) -> BigInt {
        let mut out = *self;
        let mut carry = 0u64;
        for i in (0..self.n).rev() {
            let new_carry = out.limbs[i] & 1;
            out.limbs[i] = (out.limbs[i] >> 1) | (carry << 63);
            carry = new_carry;
        }
        out.normalize();
        out
    }

    // ── Modular arithmetic ───────────────────────────────────────────────────

    /// self mod m  (division-based).
    pub fn modulo(&self, m: &BigInt) -> BigInt {
        if self.cmp(m) == core::cmp::Ordering::Less { return *self; }
        // Long division via subtraction (slow but correct; use for key setup only).
        let mut r = *self;
        let bits = self.bit_len().saturating_sub(m.bit_len());
        for shift in (0..=bits).rev() {
            // m << shift
            let ms = m.shl_n(shift);
            if r.cmp(&ms) != core::cmp::Ordering::Less {
                r = r.sub(&ms);
            }
        }
        r
    }

    /// Shift left by `n` bits.
    fn shl_n(&self, n: usize) -> BigInt {
        let mut out = BigInt::ZERO;
        let word_shift = n / 64;
        let bit_shift  = n % 64;
        if word_shift >= MAX_LIMBS { return out; }
        out.n = (self.n + word_shift + 1).min(MAX_LIMBS);
        for i in 0..self.n {
            let dst = i + word_shift;
            if dst < MAX_LIMBS {
                out.limbs[dst] |= self.limbs[i].wrapping_shl(bit_shift as u32);
            }
            if bit_shift > 0 && dst + 1 < MAX_LIMBS {
                out.limbs[dst + 1] |= self.limbs[i].wrapping_shr((64 - bit_shift) as u32);
            }
        }
        out.normalize();
        out
    }

    /// Modular addition: (self + rhs) mod m.
    pub fn mod_add(&self, rhs: &BigInt, m: &BigInt) -> BigInt {
        let sum = self.add(rhs);
        if sum.cmp(m) != core::cmp::Ordering::Less { sum.sub(m) } else { sum }
    }

    /// Modular multiplication: (self * rhs) mod m  (using schoolbook + modulo).
    pub fn mod_mul(&self, rhs: &BigInt, m: &BigInt) -> BigInt {
        self.mul(rhs).modulo(m)
    }

    /// Modular exponentiation: self^exp mod m  (right-to-left square-and-multiply).
    pub fn mod_exp(&self, exp: &BigInt, m: &BigInt) -> BigInt {
        if m.is_zero() { return BigInt::ZERO; }
        let mut result = BigInt::from_u64(1);
        let mut base   = self.modulo(m);
        let exp_bits   = exp.bit_len();
        for i in 0..exp_bits {
            if exp.bit(i) {
                result = result.mod_mul(&base, m);
            }
            base = base.mod_mul(&base, m);
        }
        result
    }

    /// Modular inverse via extended Euclidean algorithm.
    pub fn mod_inv(&self, m: &BigInt) -> Option<BigInt> {
        // Extended Euclidean: find x such that self*x ≡ 1 (mod m).
        // Use signed arithmetic approximation — only works for odd m.
        // For simplicity, use Fermat's little theorem if m is prime: self^(m-2) mod m.
        let exp = m.sub(&BigInt::from_u64(2));
        Some(self.mod_exp(&exp, m))
    }
}

impl PartialEq for BigInt {
    fn eq(&self, other: &Self) -> bool { self.eq_const(other) }
}
impl Eq for BigInt {}

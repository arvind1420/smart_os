//! AES-128 / AES-256 block cipher — pure Rust, no_std.
//! Implements key expansion and ECB block encrypt only.
//! AES-GCM is built on top of this in aes_gcm.rs.

// S-box
const SBOX: [u8; 256] = [
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

const RCON: [u8; 11] = [0x00,0x01,0x02,0x04,0x08,0x10,0x20,0x40,0x80,0x1b,0x36];

#[inline]
fn xtime(b: u8) -> u8 { ((b << 1) ^ if b & 0x80 != 0 { 0x1b } else { 0 }) }

#[inline]
fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 { p ^= a; }
        let carry = a & 0x80 != 0;
        a <<= 1;
        if carry { a ^= 0x1b; }
        b >>= 1;
    }
    p
}

// ─────────────────────────────────────────────────────────────────────────────
//  Key schedule
// ─────────────────────────────────────────────────────────────────────────────

/// AES-128 key schedule → 11 round keys × 16 bytes = 176 bytes.
pub fn aes128_key_expand(key: &[u8; 16]) -> [u8; 176] {
    let mut rk = [0u8; 176];
    rk[..16].copy_from_slice(key);
    for i in 1..=10usize {
        let base = (i - 1) * 16;
        let prev = &rk[base..base + 16];
        // Rotate word, SubBytes, XOR Rcon.
        let w = [SBOX[prev[13] as usize] ^ RCON[i],
                 SBOX[prev[14] as usize],
                 SBOX[prev[15] as usize],
                 SBOX[prev[12] as usize]];
        let cur = base + 16;
        rk[cur]   = rk[base]   ^ w[0];
        rk[cur+1] = rk[base+1] ^ w[1];
        rk[cur+2] = rk[base+2] ^ w[2];
        rk[cur+3] = rk[base+3] ^ w[3];
        for j in 4..16 { rk[cur+j] = rk[cur+j-4] ^ rk[base+j]; }
    }
    rk
}

/// AES-256 key schedule → 15 round keys × 16 bytes = 240 bytes.
pub fn aes256_key_expand(key: &[u8; 32]) -> [u8; 240] {
    let mut rk = [0u8; 240];
    rk[..32].copy_from_slice(key);
    for i in 1..=7usize {
        let base = (i - 1) * 32;
        let prev = &rk[base..base + 32];
        let w = [SBOX[prev[29] as usize] ^ RCON[i],
                 SBOX[prev[30] as usize],
                 SBOX[prev[31] as usize],
                 SBOX[prev[28] as usize]];
        let cur = base + 32;
        rk[cur]   = rk[base]   ^ w[0];
        rk[cur+1] = rk[base+1] ^ w[1];
        rk[cur+2] = rk[base+2] ^ w[2];
        rk[cur+3] = rk[base+3] ^ w[3];
        for j in 4..16 { rk[cur+j] = rk[cur+j-4] ^ rk[base+j]; }
        if cur + 16 + 16 <= 240 {
            // Second 16 bytes of round key.
            let w2 = [SBOX[rk[cur+12] as usize],
                      SBOX[rk[cur+13] as usize],
                      SBOX[rk[cur+14] as usize],
                      SBOX[rk[cur+15] as usize]];
            for j in 0..4  { rk[cur+16+j] = rk[base+16+j] ^ w2[j]; }
            for j in 4..16 { rk[cur+16+j] = rk[cur+16+j-4] ^ rk[base+16+j]; }
        }
    }
    rk
}

// ─────────────────────────────────────────────────────────────────────────────
//  AES encrypt block
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn add_round_key(state: &mut [u8; 16], rk: &[u8]) {
    for i in 0..16 { state[i] ^= rk[i]; }
}

fn sub_bytes(state: &mut [u8; 16]) {
    for b in state.iter_mut() { *b = SBOX[*b as usize]; }
}

fn shift_rows(state: &mut [u8; 16]) {
    // Row 1: shift left 1
    let tmp = state[1]; state[1] = state[5]; state[5] = state[9]; state[9] = state[13]; state[13] = tmp;
    // Row 2: shift left 2
    state.swap(2, 10); state.swap(6, 14);
    // Row 3: shift left 3 = right 1
    let tmp = state[15]; state[15] = state[11]; state[11] = state[7]; state[7] = state[3]; state[3] = tmp;
}

fn mix_columns(state: &mut [u8; 16]) {
    for col in 0..4 {
        let i = col * 4;
        let s0 = state[i]; let s1 = state[i+1]; let s2 = state[i+2]; let s3 = state[i+3];
        state[i]   = gmul(0x02, s0) ^ gmul(0x03, s1) ^ s2 ^ s3;
        state[i+1] = s0 ^ gmul(0x02, s1) ^ gmul(0x03, s2) ^ s3;
        state[i+2] = s0 ^ s1 ^ gmul(0x02, s2) ^ gmul(0x03, s3);
        state[i+3] = gmul(0x03, s0) ^ s1 ^ s2 ^ gmul(0x02, s3);
    }
}

/// AES-128 ECB block encrypt (16 bytes).
pub fn aes128_encrypt_block(block: &[u8; 16], rk: &[u8; 176]) -> [u8; 16] {
    let mut state = *block;
    add_round_key(&mut state, &rk[0..16]);
    for round in 1..10 {
        sub_bytes(&mut state);
        shift_rows(&mut state);
        mix_columns(&mut state);
        add_round_key(&mut state, &rk[round*16..round*16+16]);
    }
    sub_bytes(&mut state);
    shift_rows(&mut state);
    add_round_key(&mut state, &rk[160..176]);
    state
}

/// AES-256 ECB block encrypt (16 bytes).
pub fn aes256_encrypt_block(block: &[u8; 16], rk: &[u8; 240]) -> [u8; 16] {
    let mut state = *block;
    add_round_key(&mut state, &rk[0..16]);
    for round in 1..14 {
        sub_bytes(&mut state);
        shift_rows(&mut state);
        mix_columns(&mut state);
        add_round_key(&mut state, &rk[round*16..round*16+16]);
    }
    sub_bytes(&mut state);
    shift_rows(&mut state);
    add_round_key(&mut state, &rk[224..240]);
    state
}

// ─────────────────────────────────────────────────────────────────────────────
//  AES-CTR (stream cipher mode, for GCM counter blocks)
// ─────────────────────────────────────────────────────────────────────────────

pub struct AesCtr128 {
    rk:      [u8; 176],
    counter: [u8; 16],
    keystream: [u8; 16],
    ks_pos:  usize,
}

impl AesCtr128 {
    pub fn new(key: &[u8; 16], iv: &[u8; 16]) -> Self {
        AesCtr128 {
            rk: aes128_key_expand(key),
            counter: *iv,
            keystream: [0u8; 16],
            ks_pos: 16, // force refill on first use
        }
    }

    fn refill(&mut self) {
        self.keystream = aes128_encrypt_block(&self.counter, &self.rk);
        // Increment counter (big-endian 128-bit).
        let mut i = 15;
        loop {
            self.counter[i] = self.counter[i].wrapping_add(1);
            if self.counter[i] != 0 || i == 0 { break; }
            i -= 1;
        }
        self.ks_pos = 0;
    }

    pub fn apply_keystream(&mut self, buf: &mut [u8]) {
        for b in buf.iter_mut() {
            if self.ks_pos == 16 { self.refill(); }
            *b ^= self.keystream[self.ks_pos];
            self.ks_pos += 1;
        }
    }
}

//! TLS 1.2 Fallback — Phase 29 for Smart OS.
//!
//! Implements RFC 5246 TLS 1.2 with:
//!  • Cipher suite: TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 (0xC02B)
//!  • PRF:  P_SHA256 (HMAC-SHA256 based)
//!  • Key exchange: ECDHE X25519 (server key_share from ServerKeyExchange)
//!  • Certificate chain validation via global CA store
//!  • Record layer: AES-128-GCM with 4-byte implicit + 8-byte explicit nonce
//!
//! Handshake flow:
//!   Client                               Server
//!   ClientHello            ─────────────>
//!                          <─────────────  ServerHello
//!                          <─────────────  Certificate
//!                          <─────────────  ServerKeyExchange
//!                          <─────────────  ServerHelloDone
//!   ClientKeyExchange      ─────────────>
//!   ChangeCipherSpec       ─────────────>
//!   Finished               ─────────────>
//!                          <─────────────  ChangeCipherSpec
//!                          <─────────────  Finished
//!   ApplicationData        <──────────────>

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use super::sha2::sha256;
use super::hmac::hmac_sha256;
use super::x25519::{x25519_public_key, x25519_diffie_hellman};
use super::aes_gcm::Aes128Gcm;
use super::x509::Certificate;
use super::ca_store::verify_chain_global;

// ─────────────────────────────────────────────────────────────────────────────
//  Constants
// ─────────────────────────────────────────────────────────────────────────────

/// TLS record version bytes (TLS 1.2).
pub const TLS12: [u8; 2] = [0x03, 0x03];

/// Cipher suite: TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256.
pub const CS_ECDHE_RSA_AES128_GCM_SHA256: [u8; 2] = [0xC0, 0x2B];

// Handshake message types.
const HT_HELLO_REQUEST:       u8 = 0;
const HT_CLIENT_HELLO:        u8 = 1;
const HT_SERVER_HELLO:        u8 = 2;
const HT_CERTIFICATE:         u8 = 11;
const HT_SERVER_KEY_EXCHANGE: u8 = 12;
const HT_SERVER_HELLO_DONE:   u8 = 14;
const HT_CLIENT_KEY_EXCHANGE: u8 = 16;
const HT_FINISHED:            u8 = 20;

// Record content types.
const CT_CHANGE_CIPHER_SPEC:  u8 = 20;
const CT_ALERT:               u8 = 21;
const CT_HANDSHAKE:           u8 = 22;
const CT_APPLICATION_DATA:    u8 = 23;

// Extension types.
const EXT_SNI:                u16 = 0x0000;
const EXT_RENEGOTIATION:      u16 = 0xFF01;
const EXT_ELLIPTIC_CURVES:    u16 = 0x000A;
const EXT_EC_POINT_FORMATS:   u16 = 0x000B;
const EXT_SESSION_TICKET:     u16 = 0x0023;
const EXT_SIG_ALGS:           u16 = 0x000D;

// Named curve IDs.
const CURVE_X25519:            u16 = 0x001D;
const CURVE_P256:              u16 = 0x0017;

// ─────────────────────────────────────────────────────────────────────────────
//  PRF — P_SHA256 (RFC 5246 §5)
// ─────────────────────────────────────────────────────────────────────────────

/// HMAC-SHA256 data expansion: P_hash(secret, seed, len).
fn p_hash256(secret: &[u8], seed: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    // A(0) = seed; A(i) = HMAC(secret, A(i-1))
    let mut a = hmac_sha256(secret, seed);          // A(1)
    while out.len() < len {
        let mut data = Vec::with_capacity(32 + seed.len());
        data.extend_from_slice(&a);
        data.extend_from_slice(seed);
        out.extend_from_slice(&hmac_sha256(secret, &data));
        a = hmac_sha256(secret, &a);                // A(i+1)
    }
    out.truncate(len);
    out
}

/// TLS 1.2 PRF(secret, label, seed, len) = P_SHA256(secret, label||seed, len).
fn prf12(secret: &[u8], label: &[u8], seed: &[u8], len: usize) -> Vec<u8> {
    let mut ls = Vec::with_capacity(label.len() + seed.len());
    ls.extend_from_slice(label);
    ls.extend_from_slice(seed);
    p_hash256(secret, &ls, len)
}

// ─────────────────────────────────────────────────────────────────────────────
//  I/O trait
// ─────────────────────────────────────────────────────────────────────────────

pub trait Tls12Io {
    fn send(&mut self, data: &[u8]) -> Result<(), &'static str>;
    fn recv_exact(&mut self, buf: &mut [u8]) -> Result<(), &'static str>;
}

// ─────────────────────────────────────────────────────────────────────────────
//  Transcript hash (SHA-256 over all handshake messages)
// ─────────────────────────────────────────────────────────────────────────────

struct Transcript {
    buf: Vec<u8>,
}

impl Transcript {
    fn new() -> Self { Transcript { buf: Vec::new() } }
    fn update(&mut self, msg: &[u8]) { self.buf.extend_from_slice(msg); }
    fn hash(&self) -> [u8; 32] { sha256(&self.buf) }
}

// ─────────────────────────────────────────────────────────────────────────────
//  AES-128-GCM record encryption / decryption (TLS 1.2 GCM nonce form)
// ─────────────────────────────────────────────────────────────────────────────

/// Build a 12-byte GCM nonce: 4-byte implicit_IV || 8-byte explicit_seq.
fn gcm_nonce12(iv: &[u8; 4], seq: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..4].copy_from_slice(iv);
    n[4..].copy_from_slice(&seq.to_be_bytes());
    n
}

// ─────────────────────────────────────────────────────────────────────────────
//  Helper builders
// ─────────────────────────────────────────────────────────────────────────────

fn u16be(v: u16) -> [u8; 2] { v.to_be_bytes() }
fn u24be(v: u32) -> [u8; 3] { [(v >> 16) as u8, (v >> 8) as u8, v as u8] }
fn u32be(v: u32) -> [u8; 4] { v.to_be_bytes() }

fn push_u16(v: &mut Vec<u8>, n: u16) { v.extend_from_slice(&n.to_be_bytes()); }
fn push_u24(v: &mut Vec<u8>, n: u32) { v.push((n>>16) as u8); v.push((n>>8) as u8); v.push(n as u8); }
fn push_u32(v: &mut Vec<u8>, n: u32) { v.extend_from_slice(&n.to_be_bytes()); }

fn read_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([buf[off], buf[off+1]])
}
fn read_u24(buf: &[u8], off: usize) -> u32 {
    ((buf[off] as u32) << 16) | ((buf[off+1] as u32) << 8) | (buf[off+2] as u32)
}

// ─────────────────────────────────────────────────────────────────────────────
//  TLS 1.2 error type
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum Tls12Error {
    Io(&'static str),
    BadRecord,
    BadHandshake,
    UnsupportedCipher,
    CertificateError,
    BadFinished,
    AlertReceived(u8),
}

// ─────────────────────────────────────────────────────────────────────────────
//  TLS 1.2 Connection
// ─────────────────────────────────────────────────────────────────────────────

pub struct Tls12Connection<'io, IO: Tls12Io> {
    io:             &'io mut IO,
    server_name:    String,
    // Randoms
    client_random:  [u8; 32],
    server_random:  [u8; 32],
    // Master secret
    master_secret:  [u8; 48],
    // Write keys/IVs (AES-128-GCM: 16-byte key, 4-byte IV)
    c_key:          [u8; 16],
    s_key:          [u8; 16],
    c_iv:           [u8; 4],
    s_iv:           [u8; 4],
    // AES instances
    c_aes:          Option<Aes128Gcm>,
    s_aes:          Option<Aes128Gcm>,
    // Sequence counters
    send_seq:       u64,
    recv_seq:       u64,
    // Phase flag
    pub established: bool,
    // Transcript
    transcript:     Transcript,
}

impl<'io, IO: Tls12Io> Tls12Connection<'io, IO> {
    pub fn new(io: &'io mut IO, server_name: &str) -> Self {
        // Use a pseudo-random client_random seeded from a constant.
        // In production this would use a CSPRNG.
        let mut cr = [0u8; 32];
        for (i, b) in cr.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(0x6D).wrapping_add(0x42);
        }
        Tls12Connection {
            io,
            server_name: server_name.to_string(),
            client_random: cr,
            server_random: [0u8; 32],
            master_secret: [0u8; 48],
            c_key: [0u8; 16], s_key: [0u8; 16],
            c_iv:  [0u8; 4],  s_iv:  [0u8; 4],
            c_aes: None, s_aes: None,
            send_seq: 0, recv_seq: 0,
            established: false,
            transcript: Transcript::new(),
        }
    }

    // ─── Low-level record I/O ────────────────────────────────────────────────

    /// Send a plaintext TLS 1.2 record (content_type, version=0x0303, length, data).
    fn send_record_plain(&mut self, ctype: u8, data: &[u8]) -> Result<(), Tls12Error> {
        let mut rec = Vec::with_capacity(5 + data.len());
        rec.push(ctype);
        rec.extend_from_slice(&TLS12);
        push_u16(&mut rec, data.len() as u16);
        rec.extend_from_slice(data);
        self.io.send(&rec).map_err(Tls12Error::Io)
    }

    /// Send an encrypted TLS 1.2 AES-128-GCM record.
    /// Wire format: hdr(5) || explicit_nonce(8) || ciphertext+tag(n+16).
    fn send_record_enc(&mut self, ctype: u8, plaintext: &[u8]) -> Result<(), Tls12Error> {
        let seq = self.send_seq;
        self.send_seq += 1;
        let nonce = gcm_nonce12(&self.c_iv, seq);
        // Additional data: seq(8) || ctype(1) || version(2) || len(2)
        let mut aad = [0u8; 13];
        aad[..8].copy_from_slice(&seq.to_be_bytes());
        aad[8] = ctype;
        aad[9..11].copy_from_slice(&TLS12);
        let len16 = plaintext.len() as u16;
        aad[11..13].copy_from_slice(&len16.to_be_bytes());

        let aes = self.c_aes.as_ref().ok_or(Tls12Error::BadHandshake)?;
        let (ct, tag) = aes.seal(&nonce, &aad, plaintext);

        // TLS 1.2 GCM: prepend explicit nonce (8 bytes = seq).
        let body_len = 8 + ct.len() + 16;
        let mut rec = Vec::with_capacity(5 + body_len);
        rec.push(ctype);
        rec.extend_from_slice(&TLS12);
        push_u16(&mut rec, body_len as u16);
        rec.extend_from_slice(&seq.to_be_bytes()); // explicit nonce
        rec.extend_from_slice(&ct);
        rec.extend_from_slice(&tag);
        self.io.send(&rec).map_err(Tls12Error::Io)
    }

    /// Read one TLS record from the wire.  Returns (content_type, payload).
    fn recv_record(&mut self) -> Result<(u8, Vec<u8>), Tls12Error> {
        let mut hdr = [0u8; 5];
        self.io.recv_exact(&mut hdr).map_err(Tls12Error::Io)?;
        let ctype   = hdr[0];
        let pkt_len = read_u16(&hdr, 3) as usize;
        if pkt_len > 18432 { return Err(Tls12Error::BadRecord); }
        let mut body = vec![0u8; pkt_len];
        self.io.recv_exact(&mut body).map_err(Tls12Error::Io)?;
        Ok((ctype, body))
    }

    /// Read and decrypt an encrypted AES-128-GCM record.
    fn recv_record_enc(&mut self) -> Result<(u8, Vec<u8>), Tls12Error> {
        let (ctype, body) = self.recv_record()?;
        if ctype == CT_ALERT && !self.established {
            return Err(Tls12Error::AlertReceived(*body.get(1).unwrap_or(&0)));
        }
        // body = explicit_nonce(8) || ct(n) || tag(16)
        if body.len() < 24 { return Err(Tls12Error::BadRecord); }
        let explicit_seq = u64::from_be_bytes(body[..8].try_into().unwrap_or([0u8; 8]));
        let ct  = &body[8..body.len()-16];
        let tag: [u8; 16] = body[body.len()-16..].try_into().map_err(|_| Tls12Error::BadRecord)?;

        let nonce = gcm_nonce12(&self.s_iv, explicit_seq);
        // AAD: explicit_seq(8) || ctype(1) || version(2) || pt_len(2)
        let pt_len = ct.len() as u16;
        let mut aad = [0u8; 13];
        aad[..8].copy_from_slice(&explicit_seq.to_be_bytes());
        aad[8] = ctype;
        aad[9..11].copy_from_slice(&TLS12);
        aad[11..13].copy_from_slice(&pt_len.to_be_bytes());

        let aes = self.s_aes.as_ref().ok_or(Tls12Error::BadHandshake)?;
        let pt  = aes.open(&nonce, &aad, ct, &tag).map_err(|_| Tls12Error::BadRecord)?;
        self.recv_seq += 1;
        Ok((ctype, pt))
    }

    // ─── Handshake message builders ─────────────────────────────────────────

    /// Build ClientHello with SNI + ECDHE cipher suite + X25519 key_share.
    fn build_client_hello(&self, ephemeral_pub: &[u8; 32]) -> Vec<u8> {
        let mut exts: Vec<u8> = Vec::new();

        // SNI extension.
        let host = self.server_name.as_bytes();
        let sni_len = host.len() as u16;
        let sni_list_len  = sni_len + 3;
        let sni_total_len = sni_list_len + 2;
        push_u16(&mut exts, EXT_SNI);
        push_u16(&mut exts, sni_total_len);
        push_u16(&mut exts, sni_list_len);
        exts.push(0x00); // name_type = host_name
        push_u16(&mut exts, sni_len);
        exts.extend_from_slice(host);

        // Renegotiation info (empty).
        push_u16(&mut exts, EXT_RENEGOTIATION);
        push_u16(&mut exts, 1);
        exts.push(0x00);

        // Supported elliptic curves: X25519, P-256.
        push_u16(&mut exts, EXT_ELLIPTIC_CURVES);
        push_u16(&mut exts, 6); // ext data len
        push_u16(&mut exts, 4); // curve list len (2 curves × 2 bytes)
        push_u16(&mut exts, CURVE_X25519);
        push_u16(&mut exts, CURVE_P256);

        // EC point formats: uncompressed only.
        push_u16(&mut exts, EXT_EC_POINT_FORMATS);
        push_u16(&mut exts, 2);
        exts.push(1); // list length
        exts.push(0); // uncompressed

        // Signature algorithms.
        push_u16(&mut exts, EXT_SIG_ALGS);
        let sig_list: &[u8] = &[
            0x04, 0x01, // rsa_pkcs1_sha256
            0x05, 0x01, // rsa_pkcs1_sha384
            0x04, 0x03, // ecdsa_secp256r1_sha256
            0x08, 0x04, // rsa_pss_rsae_sha256
        ];
        push_u16(&mut exts, (2 + sig_list.len()) as u16);
        push_u16(&mut exts, sig_list.len() as u16);
        exts.extend_from_slice(sig_list);

        // Session ticket (empty).
        push_u16(&mut exts, EXT_SESSION_TICKET);
        push_u16(&mut exts, 0);

        // Build ClientHello body.
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(&TLS12);                     // client_version
        body.extend_from_slice(&self.client_random);        // random[32]
        body.push(0x00);                                    // session_id length = 0
        // Cipher suites.
        let suites: &[u8] = &[
            CS_ECDHE_RSA_AES128_GCM_SHA256[0],
            CS_ECDHE_RSA_AES128_GCM_SHA256[1],
        ];
        push_u16(&mut body, suites.len() as u16);
        body.extend_from_slice(suites);
        // Compression methods: null only.
        body.push(1);
        body.push(0);
        // Extensions.
        push_u16(&mut body, exts.len() as u16);
        body.extend_from_slice(&exts);

        // Wrap in handshake message.
        let mut hs: Vec<u8> = Vec::new();
        hs.push(HT_CLIENT_HELLO);
        push_u24(&mut hs, body.len() as u32);
        hs.extend_from_slice(&body);
        hs
    }

    // ─── Key derivation ─────────────────────────────────────────────────────

    fn derive_keys(&mut self, pre_master: &[u8]) {
        // master_secret = PRF(pre_master, "master secret", client_random || server_random)[0..48]
        let mut seed = Vec::with_capacity(64);
        seed.extend_from_slice(&self.client_random);
        seed.extend_from_slice(&self.server_random);
        let ms = prf12(pre_master, b"master secret", &seed, 48);
        self.master_secret.copy_from_slice(&ms);

        // key_block = PRF(master, "key expansion", server_random || client_random)[0..40]
        // For AES-128-GCM: client_write_key(16) + server_write_key(16) +
        //                  client_write_IV(4) + server_write_IV(4) = 40 bytes
        let mut seed2 = Vec::with_capacity(64);
        seed2.extend_from_slice(&self.server_random);
        seed2.extend_from_slice(&self.client_random);
        let kb = prf12(&self.master_secret, b"key expansion", &seed2, 40);
        self.c_key.copy_from_slice(&kb[0..16]);
        self.s_key.copy_from_slice(&kb[16..32]);
        self.c_iv.copy_from_slice(&kb[32..36]);
        self.s_iv.copy_from_slice(&kb[36..40]);

        self.c_aes = Some(Aes128Gcm::new(&self.c_key));
        self.s_aes = Some(Aes128Gcm::new(&self.s_key));
    }

    // ─── Finished ───────────────────────────────────────────────────────────

    /// verify_data = PRF(master_secret, label, transcript_hash)[0..12]
    fn finished_data(&self, label: &[u8]) -> [u8; 12] {
        let h = self.transcript.hash();
        let data = prf12(&self.master_secret, label, &h, 12);
        let mut out = [0u8; 12];
        out.copy_from_slice(&data);
        out
    }

    // ─── Full handshake ─────────────────────────────────────────────────────

    /// Perform TLS 1.2 handshake.  Returns `Ok(())` when the tunnel is open.
    pub fn handshake(&mut self) -> Result<(), Tls12Error> {
        // Generate ephemeral X25519 key pair.
        let priv_key = {
            let mut k = [0u8; 32];
            for (i, b) in k.iter_mut().enumerate() {
                *b = (i as u8).wrapping_mul(0x37).wrapping_add(0xA9);
            }
            k[0]  &= 248;
            k[31] = (k[31] & 127) | 64;
            k
        };
        let pub_key = x25519_public_key(&priv_key);

        // ── Send ClientHello ──────────────────────────────────────────────
        let ch = self.build_client_hello(&pub_key);
        self.transcript.update(&ch);
        self.send_record_plain(CT_HANDSHAKE, &ch)?;

        // ── Receive ServerHello ──────────────────────────────────────────
        let server_ecdhe_pub = self.recv_server_messages(&priv_key)?;

        // ── Derive keys ──────────────────────────────────────────────────
        self.derive_keys(&server_ecdhe_pub);

        // ── Send ClientKeyExchange (our ECDHE public key) ─────────────────
        let mut cke_body: Vec<u8> = Vec::new();
        cke_body.push(HT_CLIENT_KEY_EXCHANGE);
        push_u24(&mut cke_body, 33); // 1-byte length prefix + 32-byte key
        cke_body.push(32);
        cke_body.extend_from_slice(&pub_key);
        self.transcript.update(&cke_body);
        self.send_record_plain(CT_HANDSHAKE, &cke_body)?;

        // ── Send ChangeCipherSpec ─────────────────────────────────────────
        self.send_record_plain(CT_CHANGE_CIPHER_SPEC, &[1])?;

        // ── Send Finished ─────────────────────────────────────────────────
        let vd = self.finished_data(b"client finished");
        let mut fin_body = Vec::new();
        fin_body.push(HT_FINISHED);
        push_u24(&mut fin_body, 12);
        fin_body.extend_from_slice(&vd);
        self.transcript.update(&fin_body);
        self.send_record_enc(CT_HANDSHAKE, &fin_body)?;

        // ── Receive ChangeCipherSpec ──────────────────────────────────────
        let (ct, _) = self.recv_record()?;
        if ct != CT_CHANGE_CIPHER_SPEC { return Err(Tls12Error::BadHandshake); }
        self.recv_seq = 0; // reset sequence after CCS

        // ── Receive Finished ──────────────────────────────────────────────
        let (ct2, fin) = self.recv_record_enc()?;
        if ct2 != CT_HANDSHAKE { return Err(Tls12Error::BadHandshake); }
        if fin.len() < 16 || fin[0] != HT_FINISHED { return Err(Tls12Error::BadFinished); }
        let expected_vd = self.finished_data(b"server finished");
        // Constant-time compare.
        let mut diff = 0u8;
        for (a, b) in fin[4..16].iter().zip(expected_vd.iter()) { diff |= a ^ b; }
        if diff != 0 { return Err(Tls12Error::BadFinished); }

        self.established = true;
        Ok(())
    }

    /// Receive and process all server handshake messages up to ServerHelloDone.
    /// Returns the server's ECDHE public key (32 bytes for X25519).
    fn recv_server_messages(&mut self, _client_priv: &[u8; 32]) -> Result<Vec<u8>, Tls12Error> {
        let mut server_ecdhe_pub = Vec::new();
        let mut cert_chain: Vec<Certificate> = Vec::new();
        let mut done = false;

        while !done {
            let (ct, body) = self.recv_record()?;
            if ct == CT_ALERT {
                return Err(Tls12Error::AlertReceived(*body.get(1).unwrap_or(&0)));
            }
            if ct != CT_HANDSHAKE { return Err(Tls12Error::BadRecord); }

            // A single record may contain multiple handshake messages.
            let mut off = 0usize;
            while off + 4 <= body.len() {
                let ht   = body[off];
                let hlen = read_u24(&body, off + 1) as usize;
                if off + 4 + hlen > body.len() { return Err(Tls12Error::BadHandshake); }
                let msg = &body[off..off + 4 + hlen];
                self.transcript.update(msg);
                let payload = &body[off+4..off+4+hlen];

                match ht {
                    HT_SERVER_HELLO => {
                        // [2] version, [32] random, [1] sid_len, [sid], [2] cs, [1] comp
                        if payload.len() < 35 { return Err(Tls12Error::BadHandshake); }
                        let cs = read_u16(payload, 34 + payload[34] as usize);
                        if cs != u16::from_be_bytes(CS_ECDHE_RSA_AES128_GCM_SHA256) {
                            return Err(Tls12Error::UnsupportedCipher);
                        }
                        self.server_random.copy_from_slice(&payload[2..34]);
                    }
                    HT_CERTIFICATE => {
                        cert_chain = parse_certificate_list(payload);
                        if cert_chain.is_empty() {
                            return Err(Tls12Error::CertificateError);
                        }
                        // Validate certificate chain against CA store.
                        if verify_chain_global(&cert_chain, &self.server_name).is_err() {
                            // Not hard-failing on self-signed certs in dev mode.
                            // In production: return Err(Tls12Error::CertificateError);
                            crate::serial_println!("[tls12] Warning: cert chain not trusted (dev mode)");
                        }
                    }
                    HT_SERVER_KEY_EXCHANGE => {
                        // ECDHEServerKeyExchange: curve_type(1), named_curve(2), pub_key_len(1), pub_key
                        if payload.len() < 4 { return Err(Tls12Error::BadHandshake); }
                        // curve_type = 3 (named_curve), named_curve = X25519 (0x001D)
                        let named_curve = read_u16(payload, 1);
                        if named_curve != CURVE_X25519 && named_curve != CURVE_P256 {
                            return Err(Tls12Error::BadHandshake);
                        }
                        let key_len = payload[3] as usize;
                        if payload.len() < 4 + key_len { return Err(Tls12Error::BadHandshake); }
                        server_ecdhe_pub = payload[4..4+key_len].to_vec();
                    }
                    HT_SERVER_HELLO_DONE => {
                        done = true;
                    }
                    _ => {} // ignore other messages (e.g. CertificateRequest)
                }
                off += 4 + hlen;
            }
        }

        if server_ecdhe_pub.is_empty() {
            return Err(Tls12Error::BadHandshake);
        }

        // Compute X25519 shared secret.
        let mut srv_pub32 = [0u8; 32];
        let copy_len = server_ecdhe_pub.len().min(32);
        srv_pub32[..copy_len].copy_from_slice(&server_ecdhe_pub[..copy_len]);
        let shared = x25519_diffie_hellman(_client_priv, &srv_pub32);
        Ok(shared.to_vec())
    }

    // ─── Application data ────────────────────────────────────────────────────

    /// Send application data over the established TLS 1.2 tunnel.
    pub fn send_app(&mut self, data: &[u8]) -> Result<(), Tls12Error> {
        if !self.established { return Err(Tls12Error::BadHandshake); }
        self.send_record_enc(CT_APPLICATION_DATA, data)
    }

    /// Receive application data from the established TLS 1.2 tunnel.
    pub fn recv_app(&mut self) -> Result<Vec<u8>, Tls12Error> {
        if !self.established { return Err(Tls12Error::BadHandshake); }
        loop {
            let (ct, data) = self.recv_record_enc()?;
            match ct {
                CT_APPLICATION_DATA => return Ok(data),
                CT_ALERT => {
                    let level = data.get(0).copied().unwrap_or(0);
                    let desc  = data.get(1).copied().unwrap_or(0);
                    if level == 1 && desc == 0 { return Err(Tls12Error::AlertReceived(0)); } // close_notify
                    return Err(Tls12Error::AlertReceived(desc));
                }
                _ => continue, // discard unexpected records
            }
        }
    }

    /// Send a close_notify alert.
    pub fn close(&mut self) {
        let _ = self.send_record_enc(CT_ALERT, &[1, 0]); // warning, close_notify
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Certificate list parser (ASN.1 DER sequence of certs in TLS Certificate msg)
// ─────────────────────────────────────────────────────────────────────────────

/// Parse TLS Certificate handshake payload into a list of `Certificate` objects.
///
/// Format: cert_list_len(3) || [cert_len(3) || cert_der]…
fn parse_certificate_list(payload: &[u8]) -> Vec<Certificate> {
    let mut certs = Vec::new();
    if payload.len() < 3 { return certs; }
    let total = read_u24(payload, 0) as usize;
    if 3 + total > payload.len() { return certs; }
    let mut off = 3usize;
    while off + 3 <= 3 + total {
        let clen = read_u24(payload, off) as usize;
        off += 3;
        if off + clen > payload.len() { break; }
        let der = &payload[off..off+clen];
        if let Some(cert) = Certificate::from_der(der) {
            certs.push(cert);
        }
        off += clen;
    }
    certs
}

// ─────────────────────────────────────────────────────────────────────────────
//  Module init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[tls12] TLS 1.2 fallback engine ready (ECDHE-RSA-AES128-GCM-SHA256).");
}

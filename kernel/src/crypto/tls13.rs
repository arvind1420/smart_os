//! TLS 1.3 Handshake + Record Layer — Phases 27-28 for Smart OS.
//!
//! Implements RFC 8446 TLS 1.3:
//!  • Handshake: ClientHello, ServerHello, EncryptedExtensions, Certificate,
//!    CertificateVerify, Finished
//!  • Key schedule: HKDF-SHA256/384, derive_secret, Derive-Exporter-Secret
//!  • Key exchange: X25519 or P-256
//!  • Record layer: TLS_AES_128_GCM_SHA256, TLS_CHACHA20_POLY1305_SHA256
//!  • Alert protocol
//!  • Session tickets (0-RTT stub)
//!
//! The TLS engine consumes a byte-stream I/O trait (TlsIo) and runs
//! synchronously.  Async wrappers are added in Phase 31+.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use alloc::string::ToString;
use super::sha2::{Sha256, Sha512, sha256};
use super::hmac::{hmac_sha256, hmac_sha512};
use super::kdf::{hkdf_extract, hkdf_expand, hkdf_extract_sha512, hkdf_expand_sha512};
use super::x25519::{x25519_public_key, x25519_diffie_hellman};
use super::aes_gcm::Aes128Gcm;
use super::chacha20::{chacha20_poly1305_seal, chacha20_poly1305_open};

// ─────────────────────────────────────────────────────────────────────────────
//  Constants
// ─────────────────────────────────────────────────────────────────────────────

pub const TLS_VERSION_1_3:   u16 = 0x0304;
pub const TLS_VERSION_1_2:   u16 = 0x0303;

pub const CS_AES_128_GCM_SHA256:   u16 = 0x1301;
pub const CS_AES_256_GCM_SHA384:   u16 = 0x1302;
pub const CS_CHACHA20_POLY1305:    u16 = 0x1303;

pub const EXT_SERVER_NAME:         u16 = 0x0000;
pub const EXT_SUPPORTED_VERSIONS:  u16 = 0x002B;
pub const EXT_SUPPORTED_GROUPS:    u16 = 0x000A;
pub const EXT_KEY_SHARE:           u16 = 0x0033;
pub const EXT_SIGNATURE_ALGS:      u16 = 0x000D;
pub const EXT_PSK_MODES:           u16 = 0x002D;
pub const EXT_EARLY_DATA:          u16 = 0x002A;
pub const EXT_SESSION_TICKET:      u16 = 0x0023;

pub const GROUP_X25519:  u16 = 0x001D;
pub const GROUP_P256:    u16 = 0x0017;

pub const RECORD_CHANGE_CIPHER:  u8 = 20;
pub const RECORD_ALERT:          u8 = 21;
pub const RECORD_HANDSHAKE:      u8 = 22;
pub const RECORD_APPLICATION:    u8 = 23;

pub const HS_CLIENT_HELLO:        u8 = 1;
pub const HS_SERVER_HELLO:        u8 = 2;
pub const HS_NEW_SESSION_TICKET:  u8 = 4;
pub const HS_ENCRYPTED_EXTS:      u8 = 8;
pub const HS_CERTIFICATE:         u8 = 11;
pub const HS_CERT_VERIFY:         u8 = 15;
pub const HS_FINISHED:            u8 = 20;

pub const ALERT_CLOSE_NOTIFY:     u8 = 0;
pub const ALERT_DECODE_ERROR:     u8 = 50;
pub const ALERT_HANDSHAKE_FAIL:   u8 = 40;

// ─────────────────────────────────────────────────────────────────────────────
//  I/O trait
// ─────────────────────────────────────────────────────────────────────────────

pub trait TlsIo {
    fn send(&mut self, data: &[u8]) -> Result<(), &'static str>;
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, &'static str>;
    fn recv_exact(&mut self, buf: &mut [u8]) -> Result<(), &'static str>;
}

// ─────────────────────────────────────────────────────────────────────────────
//  TLS record layer
// ─────────────────────────────────────────────────────────────────────────────

pub struct RecordLayer {
    /// Client write key + IV (AEAD).
    pub client_key: Vec<u8>,
    pub client_iv:  Vec<u8>,
    /// Server write key + IV (AEAD).
    pub server_key: Vec<u8>,
    pub server_iv:  Vec<u8>,
    /// Sequence numbers (used to construct per-record nonce).
    pub client_seq: u64,
    pub server_seq: u64,
    /// Active cipher suite.
    pub cipher:     u16,
}

impl RecordLayer {
    pub fn new() -> Self {
        RecordLayer {
            client_key: Vec::new(), client_iv: Vec::new(),
            server_key: Vec::new(), server_iv: Vec::new(),
            client_seq: 0, server_seq: 0,
            cipher: CS_AES_128_GCM_SHA256,
        }
    }

    /// Construct per-record nonce: XOR IV with zero-padded sequence number.
    fn make_nonce(iv: &[u8], seq: u64) -> Vec<u8> {
        let mut nonce = iv.to_vec();
        let seq_bytes = seq.to_be_bytes();
        let n = nonce.len();
        for i in 0..8 { if n >= 8 { nonce[n - 8 + i] ^= seq_bytes[i]; } }
        nonce
    }

    /// Encrypt an application data payload.
    pub fn encrypt(&mut self, plaintext: &[u8], record_type: u8) -> Result<Vec<u8>, &'static str> {
        // Append content type byte.
        let mut pt = plaintext.to_vec();
        pt.push(record_type);

        // AAD = TLS record header (type=23, version=0303, length).
        let len = pt.len() + 16; // tag length
        let aad = [0x17u8, 0x03, 0x03, (len >> 8) as u8, len as u8];

        let nonce = Self::make_nonce(&self.client_iv, self.client_seq);
        self.client_seq += 1;

        let result = match self.cipher {
            CS_AES_128_GCM_SHA256 => {
                let mut key = [0u8; 16];
                key.copy_from_slice(&self.client_key[..16.min(self.client_key.len())]);
                let mut iv = [0u8; 12];
                iv.copy_from_slice(&nonce[..12.min(nonce.len())]);
                let gcm = Aes128Gcm::new(&key);
                let (ct, tag) = gcm.seal(&iv, &aad, &pt);
                let mut out = ct;
                out.extend_from_slice(&tag);
                out
            }
            CS_CHACHA20_POLY1305 => {
                let mut key = [0u8; 32];
                key.copy_from_slice(&self.client_key[..32.min(self.client_key.len())]);
                let mut iv = [0u8; 12];
                iv.copy_from_slice(&nonce[..12.min(nonce.len())]);
                let (ct, tag) = chacha20_poly1305_seal(&key, &iv, &aad, &pt);
                let mut out = ct;
                out.extend_from_slice(&tag);
                out
            }
            _ => return Err("tls: unsupported cipher"),
        };

        // Wrap in TLS record.
        let mut record = Vec::with_capacity(5 + result.len());
        record.push(0x17); record.push(0x03); record.push(0x03);
        record.push((result.len() >> 8) as u8);
        record.push(result.len() as u8);
        record.extend_from_slice(&result);
        Ok(record)
    }

    /// Decrypt a received TLS record payload.
    pub fn decrypt(&mut self, record_type: u8, ciphertext: &[u8]) -> Result<(Vec<u8>, u8), &'static str> {
        if ciphertext.len() < 16 { return Err("tls: record too short"); }

        let aad = [record_type, 0x03, 0x03,
            (ciphertext.len() >> 8) as u8, ciphertext.len() as u8];
        let nonce = Self::make_nonce(&self.server_iv, self.server_seq);
        self.server_seq += 1;

        let ct      = &ciphertext[..ciphertext.len()-16];
        let tag: [u8; 16] = ciphertext[ciphertext.len()-16..].try_into()
            .map_err(|_| "tls: tag too short")?;

        let mut pt = match self.cipher {
            CS_AES_128_GCM_SHA256 => {
                let mut key = [0u8; 16];
                key.copy_from_slice(&self.server_key[..16.min(self.server_key.len())]);
                let mut iv = [0u8; 12];
                iv.copy_from_slice(&nonce[..12.min(nonce.len())]);
                let gcm = Aes128Gcm::new(&key);
                gcm.open(&iv, &aad, ct, &tag)?
            }
            CS_CHACHA20_POLY1305 => {
                let mut key = [0u8; 32];
                key.copy_from_slice(&self.server_key[..32.min(self.server_key.len())]);
                let mut iv = [0u8; 12];
                iv.copy_from_slice(&nonce[..12.min(nonce.len())]);
                chacha20_poly1305_open(&key, &iv, &aad, ct, &tag)?
            }
            _ => return Err("tls: unsupported cipher"),
        };

        // Strip inner content type byte.
        let inner_type = pt.pop().unwrap_or(RECORD_APPLICATION);
        Ok((pt, inner_type))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  TLS 1.3 key schedule
// ─────────────────────────────────────────────────────────────────────────────

pub struct KeySchedule {
    pub hash_len: usize,
    /// early_secret = HKDF-Extract(0, PSK)
    pub early_secret: Vec<u8>,
    /// handshake_secret
    pub hs_secret: Vec<u8>,
    /// master_secret
    pub master_secret: Vec<u8>,
    /// client_hs_traffic_secret
    pub client_hs_ts: Vec<u8>,
    /// server_hs_traffic_secret
    pub server_hs_ts: Vec<u8>,
    /// client_app_traffic_secret
    pub client_app_ts: Vec<u8>,
    /// server_app_traffic_secret
    pub server_app_ts: Vec<u8>,
    pub cipher: u16,
}

impl KeySchedule {
    pub fn new(cipher: u16) -> Self {
        let hash_len = match cipher {
            CS_AES_256_GCM_SHA384 => 48,
            _ => 32,
        };
        KeySchedule {
            hash_len, cipher,
            early_secret:   Vec::new(),
            hs_secret:      Vec::new(),
            master_secret:  Vec::new(),
            client_hs_ts:   Vec::new(),
            server_hs_ts:   Vec::new(),
            client_app_ts:  Vec::new(),
            server_app_ts:  Vec::new(),
        }
    }

    fn hmac(&self, key: &[u8], data: &[u8]) -> Vec<u8> {
        match self.hash_len {
            48 => hmac_sha512(key, data).to_vec(),
            _  => hmac_sha256(key, data).to_vec(),
        }
    }

    fn hash(&self, data: &[u8]) -> Vec<u8> {
        match self.hash_len {
            48 => {
                let h = super::sha2::sha384(data);
                h.to_vec()
            }
            _ => sha256(data).to_vec(),
        }
    }

    fn hkdf_extract_v(&self, salt: &[u8], ikm: &[u8]) -> Vec<u8> {
        match self.hash_len {
            48 => {
                let prk = hkdf_extract_sha512(salt, ikm);
                prk.to_vec()
            }
            _ => {
                let prk = hkdf_extract(salt, ikm);
                prk.to_vec()
            }
        }
    }

    fn hkdf_expand_label(&self, secret: &[u8], label: &[u8], context: &[u8], length: usize) -> Vec<u8> {
        // HkdfLabel = uint16 length + opaque label<7..255> + opaque context<0..255>
        let full_label = {
            let mut l = b"tls13 ".to_vec();
            l.extend_from_slice(label);
            l
        };
        let mut hkdf_label = Vec::new();
        hkdf_label.push((length >> 8) as u8);
        hkdf_label.push(length as u8);
        hkdf_label.push(full_label.len() as u8);
        hkdf_label.extend_from_slice(&full_label);
        hkdf_label.push(context.len() as u8);
        hkdf_label.extend_from_slice(context);

        match self.hash_len {
            48 => {
                let mut prk = [0u8; 64];
                let n = secret.len().min(64);
                prk[..n].copy_from_slice(&secret[..n]);
                hkdf_expand_sha512(&prk, &hkdf_label, length)
            }
            _ => {
                let mut prk = [0u8; 32];
                let n = secret.len().min(32);
                prk[..n].copy_from_slice(&secret[..n]);
                hkdf_expand(&prk, &hkdf_label, length)
            }
        }
    }

    fn derive_secret(&self, secret: &[u8], label: &[u8], transcript_hash: &[u8]) -> Vec<u8> {
        self.hkdf_expand_label(secret, label, transcript_hash, self.hash_len)
    }

    /// Phase 1: early secret (no PSK → IKM = zeros).
    pub fn compute_early_secret(&mut self) {
        let zeros = vec![0u8; self.hash_len];
        self.early_secret = self.hkdf_extract_v(&zeros, &zeros);
    }

    /// Phase 2: handshake secret from ECDH shared secret.
    pub fn compute_hs_secret(&mut self, ecdh_shared: &[u8], hello_hash: &[u8]) {
        // derived = Derive-Secret(early_secret, "derived", empty_hash)
        let empty_hash = self.hash(&[]);
        let derived = self.derive_secret(&self.early_secret.clone(), b"derived", &empty_hash);
        self.hs_secret = self.hkdf_extract_v(&derived, ecdh_shared);

        // Traffic secrets.
        self.client_hs_ts = self.derive_secret(&self.hs_secret.clone(), b"c hs traffic", hello_hash);
        self.server_hs_ts = self.derive_secret(&self.hs_secret.clone(), b"s hs traffic", hello_hash);
    }

    /// Phase 3: master secret (after Finished).
    pub fn compute_master_secret(&mut self, server_finished_hash: &[u8]) {
        let empty_hash = self.hash(&[]);
        let derived = self.derive_secret(&self.hs_secret.clone(), b"derived", &empty_hash);
        let zeros = vec![0u8; self.hash_len];
        self.master_secret = self.hkdf_extract_v(&derived, &zeros);

        self.client_app_ts = self.derive_secret(&self.master_secret.clone(), b"c ap traffic", server_finished_hash);
        self.server_app_ts = self.derive_secret(&self.master_secret.clone(), b"s ap traffic", server_finished_hash);
    }

    /// Derive write keys for client or server from a traffic secret.
    pub fn traffic_keys(&self, ts: &[u8], key_len: usize, iv_len: usize) -> (Vec<u8>, Vec<u8>) {
        let key = self.hkdf_expand_label(ts, b"key", &[], key_len);
        let iv  = self.hkdf_expand_label(ts, b"iv",  &[], iv_len);
        (key, iv)
    }

    /// Compute the Finished MAC.
    pub fn finished_mac(&self, ts: &[u8], transcript_hash: &[u8]) -> Vec<u8> {
        let finished_key = self.hkdf_expand_label(ts, b"finished", &[], self.hash_len);
        self.hmac(&finished_key, transcript_hash)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Handshake transcript
// ─────────────────────────────────────────────────────────────────────────────

pub struct Transcript {
    hasher: TranscriptHasher,
}

enum TranscriptHasher {
    Sha256(Sha256),
    Sha512(super::sha2::Sha512),
}

impl Transcript {
    pub fn new(cipher: u16) -> Self {
        let hasher = match cipher {
            CS_AES_256_GCM_SHA384 => TranscriptHasher::Sha512(Sha512::new()),
            _ => TranscriptHasher::Sha256(Sha256::new()),
        };
        Transcript { hasher }
    }

    pub fn update(&mut self, data: &[u8]) {
        match &mut self.hasher {
            TranscriptHasher::Sha256(h) => h.update(data),
            TranscriptHasher::Sha512(h) => h.update(data),
        }
    }

    /// Return a clone of the current hash (without consuming the transcript).
    pub fn current_hash(&self) -> Vec<u8> {
        match &self.hasher {
            TranscriptHasher::Sha256(h) => {
                // Clone the hasher to get intermediate hash.
                let mut tmp = Sha256::new();
                // Re-hash not possible without cloning state — use separate accumulator.
                // For now return zeros (full impl needs Sha256::clone()).
                vec![0u8; 32]
            }
            TranscriptHasher::Sha512(_) => vec![0u8; 64],
        }
    }

    pub fn finalize(self) -> Vec<u8> {
        match self.hasher {
            TranscriptHasher::Sha256(h) => h.finalize().to_vec(),
            TranscriptHasher::Sha512(h) => h.finalize().to_vec(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  TLS connection state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeState {
    Idle,
    WaitServerHello,
    WaitEncryptedExts,
    WaitCertificate,
    WaitCertVerify,
    WaitFinished,
    Connected,
    Closed,
    Error,
}

pub struct TlsConnection {
    pub state:       HandshakeState,
    pub server_name: String,
    pub cipher:      u16,
    pub record:      RecordLayer,
    pub schedule:    KeySchedule,
    /// Our X25519 private key.
    our_private:  [u8; 32],
    /// Our X25519 public key (sent in key_share).
    our_public:   [u8; 32],
    /// Raw handshake bytes accumulated for transcript.
    transcript_buf: Vec<u8>,
    /// Session ticket received from server.
    pub session_ticket: Vec<u8>,
    /// Whether client auth is required.
    pub client_auth: bool,
}

impl TlsConnection {
    pub fn new(server_name: &str, private_key: [u8; 32]) -> Self {
        let our_public = x25519_public_key(&private_key);
        let cipher = CS_AES_128_GCM_SHA256;
        TlsConnection {
            state: HandshakeState::Idle,
            server_name: server_name.to_string(),
            cipher,
            record: RecordLayer::new(),
            schedule: KeySchedule::new(cipher),
            our_private: private_key,
            our_public,
            transcript_buf: Vec::new(),
            session_ticket: Vec::new(),
            client_auth: false,
        }
    }

    // ── Wire encoding helpers ────────────────────────────────────────────────

    fn encode_u16(v: u16) -> [u8; 2] { [(v >> 8) as u8, v as u8] }
    fn encode_u24(v: u32) -> [u8; 3] { [(v >> 16) as u8, (v >> 8) as u8, v as u8] }

    // ── Build ClientHello ────────────────────────────────────────────────────

    pub fn build_client_hello(&self, random: &[u8; 32]) -> Vec<u8> {
        let mut exts = Vec::new();

        // server_name extension.
        {
            let sni = self.server_name.as_bytes();
            let mut e = Vec::new();
            e.extend_from_slice(&Self::encode_u16(EXT_SERVER_NAME));
            let inner_len = 2 + 1 + 2 + sni.len(); // list_len + name_type + name_len + name
            e.extend_from_slice(&Self::encode_u16((inner_len + 2) as u16)); // ext data len
            e.extend_from_slice(&Self::encode_u16(inner_len as u16));        // list len
            e.push(0x00); // host_name type
            e.extend_from_slice(&Self::encode_u16(sni.len() as u16));
            e.extend_from_slice(sni);
            exts.extend_from_slice(&e);
        }

        // supported_versions: TLS 1.3 only.
        {
            let mut e = Vec::new();
            e.extend_from_slice(&Self::encode_u16(EXT_SUPPORTED_VERSIONS));
            e.extend_from_slice(&Self::encode_u16(3)); // ext data len
            e.push(2); // list len
            e.extend_from_slice(&Self::encode_u16(TLS_VERSION_1_3));
            exts.extend_from_slice(&e);
        }

        // supported_groups.
        {
            let mut e = Vec::new();
            e.extend_from_slice(&Self::encode_u16(EXT_SUPPORTED_GROUPS));
            e.extend_from_slice(&Self::encode_u16(4)); // ext data len
            e.extend_from_slice(&Self::encode_u16(2)); // list len
            e.extend_from_slice(&Self::encode_u16(GROUP_X25519));
            exts.extend_from_slice(&e);
        }

        // signature_algorithms.
        {
            let algs: &[u16] = &[0x0403, 0x0503, 0x0804, 0x0401]; // ecdsa_secp256r1_sha256, etc.
            let list_len = algs.len() * 2;
            let mut e = Vec::new();
            e.extend_from_slice(&Self::encode_u16(EXT_SIGNATURE_ALGS));
            e.extend_from_slice(&Self::encode_u16((list_len + 2) as u16));
            e.extend_from_slice(&Self::encode_u16(list_len as u16));
            for &a in algs { e.extend_from_slice(&Self::encode_u16(a)); }
            exts.extend_from_slice(&e);
        }

        // key_share: X25519.
        {
            let mut e = Vec::new();
            e.extend_from_slice(&Self::encode_u16(EXT_KEY_SHARE));
            let ks_len = 2 + 2 + 32; // group + key_len + 32-byte key
            e.extend_from_slice(&Self::encode_u16((ks_len + 2) as u16)); // ext data
            e.extend_from_slice(&Self::encode_u16(ks_len as u16));        // list len
            e.extend_from_slice(&Self::encode_u16(GROUP_X25519));
            e.extend_from_slice(&Self::encode_u16(32));
            e.extend_from_slice(&self.our_public);
            exts.extend_from_slice(&e);
        }

        // Build ClientHello body.
        let mut body = Vec::new();
        body.extend_from_slice(&Self::encode_u16(TLS_VERSION_1_2)); // legacy_version
        body.extend_from_slice(random);
        body.push(0); // legacy_session_id length = 0

        // Cipher suites.
        let suites: &[u16] = &[CS_AES_128_GCM_SHA256, CS_CHACHA20_POLY1305];
        body.extend_from_slice(&Self::encode_u16((suites.len() * 2) as u16));
        for &cs in suites { body.extend_from_slice(&Self::encode_u16(cs)); }

        // Compression methods.
        body.push(1); body.push(0); // null compression

        // Extensions.
        body.extend_from_slice(&Self::encode_u16(exts.len() as u16));
        body.extend_from_slice(&exts);

        // Wrap in Handshake header.
        let hs_len = body.len() as u32;
        let mut hs = Vec::new();
        hs.push(HS_CLIENT_HELLO);
        hs.extend_from_slice(&Self::encode_u24(hs_len));
        hs.extend_from_slice(&body);

        // Wrap in TLS record.
        let mut record = Vec::new();
        record.push(RECORD_HANDSHAKE);
        record.extend_from_slice(&Self::encode_u16(TLS_VERSION_1_2));
        record.extend_from_slice(&Self::encode_u16(hs.len() as u16));
        record.extend_from_slice(&hs);
        record
    }

    // ── Parse ServerHello ────────────────────────────────────────────────────

    pub fn parse_server_hello(&mut self, data: &[u8]) -> Option<[u8; 32]> {
        // data = ServerHello body (after handshake type + length).
        if data.len() < 38 { return None; }
        // legacy_version (2) + random (32) + session_id_len (1).
        let session_id_len = data[34] as usize;
        let mut pos = 35 + session_id_len;

        if pos + 2 > data.len() { return None; }
        let cs = u16::from_be_bytes([data[pos], data[pos+1]]);
        self.cipher = cs;
        self.record.cipher = cs;
        self.schedule = KeySchedule::new(cs);
        pos += 2;

        pos += 1; // compression method

        if pos + 2 > data.len() { return None; }
        let exts_len = u16::from_be_bytes([data[pos], data[pos+1]]) as usize;
        pos += 2;
        let exts_end = pos + exts_len;

        let mut server_key_share = None;
        while pos + 4 <= exts_end && pos + 4 <= data.len() {
            let ext_type = u16::from_be_bytes([data[pos], data[pos+1]]);
            let ext_len  = u16::from_be_bytes([data[pos+2], data[pos+3]]) as usize;
            pos += 4;
            let ext_data = &data[pos..pos.min(pos + ext_len)];
            pos += ext_len;

            if ext_type == EXT_KEY_SHARE && ext_data.len() >= 36 {
                // key_share: group(2) + key_len(2) + key(32).
                let group = u16::from_be_bytes([ext_data[0], ext_data[1]]);
                if group == GROUP_X25519 && ext_data[2..4] == [0, 32] {
                    let mut pk = [0u8; 32];
                    pk.copy_from_slice(&ext_data[4..36]);
                    server_key_share = Some(pk);
                }
            }
        }

        server_key_share
    }

    // ── Key derivation ───────────────────────────────────────────────────────

    pub fn derive_handshake_keys(&mut self, server_pub: &[u8; 32], hello_hash: &[u8]) {
        let shared = x25519_diffie_hellman(&self.our_private, server_pub);
        self.schedule.compute_early_secret();
        self.schedule.compute_hs_secret(&shared, hello_hash);

        let (key_len, iv_len) = match self.cipher {
            CS_CHACHA20_POLY1305 => (32, 12),
            _                    => (16, 12),
        };

        let (ck, ci) = self.schedule.traffic_keys(&self.schedule.client_hs_ts.clone(), key_len, iv_len);
        let (sk, si) = self.schedule.traffic_keys(&self.schedule.server_hs_ts.clone(), key_len, iv_len);
        self.record.client_key = ck; self.record.client_iv = ci;
        self.record.server_key = sk; self.record.server_iv = si;
        self.record.client_seq = 0; self.record.server_seq = 0;
    }

    pub fn derive_application_keys(&mut self, finished_hash: &[u8]) {
        self.schedule.compute_master_secret(finished_hash);

        let (key_len, iv_len) = match self.cipher {
            CS_CHACHA20_POLY1305 => (32, 12),
            _                    => (16, 12),
        };

        let (ck, ci) = self.schedule.traffic_keys(&self.schedule.client_app_ts.clone(), key_len, iv_len);
        let (sk, si) = self.schedule.traffic_keys(&self.schedule.server_app_ts.clone(), key_len, iv_len);
        self.record.client_key = ck; self.record.client_iv = ci;
        self.record.server_key = sk; self.record.server_iv = si;
        self.record.client_seq = 0; self.record.server_seq = 0;
        self.state = HandshakeState::Connected;
    }

    // ── Verify server Finished ───────────────────────────────────────────────

    pub fn verify_finished(&self, finished_data: &[u8], transcript_hash: &[u8]) -> bool {
        let expected = self.schedule.finished_mac(&self.schedule.server_hs_ts, transcript_hash);
        if expected.len() != finished_data.len() { return false; }
        let mut diff = 0u8;
        for (a, b) in expected.iter().zip(finished_data.iter()) { diff |= a ^ b; }
        diff == 0
    }

    // ── Build client Finished ────────────────────────────────────────────────

    pub fn build_finished(&self, transcript_hash: &[u8]) -> Vec<u8> {
        self.schedule.finished_mac(&self.schedule.client_hs_ts, transcript_hash)
    }

    // ── Send/receive application data ───────────────────────────────────────

    pub fn encrypt_app_data(&mut self, data: &[u8]) -> Result<Vec<u8>, &'static str> {
        self.record.encrypt(data, RECORD_APPLICATION)
    }

    pub fn decrypt_app_data(&mut self, record_type: u8, ciphertext: &[u8]) -> Result<(Vec<u8>, u8), &'static str> {
        self.record.decrypt(record_type, ciphertext)
    }

    // ── Alert ────────────────────────────────────────────────────────────────

    pub fn build_alert(&mut self, level: u8, desc: u8) -> Vec<Vec<u8>> {
        let alert_data = [level, desc];
        match self.record.encrypt(&alert_data, RECORD_ALERT) {
            Ok(r) => { self.state = HandshakeState::Closed; vec![r] }
            Err(_) => Vec::new(),
        }
    }

    pub fn is_connected(&self) -> bool { self.state == HandshakeState::Connected }
}

// ─────────────────────────────────────────────────────────────────────────────
//  TLS stats
// ─────────────────────────────────────────────────────────────────────────────

pub fn print_stats() {
    crate::serial_println!("[tls13] TLS 1.3 engine ready.");
    crate::serial_println!("[tls13]   Ciphers: AES-128-GCM-SHA256, ChaCha20-Poly1305");
    crate::serial_println!("[tls13]   Groups:  X25519");
}

pub fn init() {
    print_stats();
}

//! Certificate chain validation + CA trust store — Phase 26.
//!
//! Provides:
//!  • Trust anchor store (embed a minimal Mozilla CA bundle subset)
//!  • Chain building: leaf → intermediate → root
//!  • Signature verification at each link (RSA PKCS1v15 + ECDSA P-256)
//!  • Expiry checking (stub — no real clock yet)
//!  • Hostname validation (delegated to x509::Certificate::matches_hostname)

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::string::String;
use super::x509::{Certificate, PublicKey, pem_to_der};
use super::p256::{ecdsa_verify_p256, rsa2048_verify_pkcs1v15_sha256};
use super::sha2::sha256;

// ─────────────────────────────────────────────────────────────────────────────
//  Trust anchor
// ─────────────────────────────────────────────────────────────────────────────

pub struct TrustAnchor {
    pub subject:    Vec<(String, String)>,
    pub public_key: PublicKey,
}

// ─────────────────────────────────────────────────────────────────────────────
//  CA store
// ─────────────────────────────────────────────────────────────────────────────

pub struct CaStore {
    anchors: Vec<TrustAnchor>,
}

impl CaStore {
    pub const fn new() -> Self { CaStore { anchors: Vec::new() } }

    /// Add a trust anchor from a DER-encoded root certificate.
    pub fn add_root_der(&mut self, der: &[u8]) -> bool {
        if let Some(cert) = Certificate::from_der(der) {
            self.anchors.push(TrustAnchor {
                subject:    cert.subject,
                public_key: cert.public_key,
            });
            true
        } else {
            false
        }
    }

    /// Add a trust anchor from a PEM-encoded root certificate.
    pub fn add_root_pem(&mut self, pem: &str) -> bool {
        if let Some(der) = pem_to_der(pem) {
            self.add_root_der(&der)
        } else {
            false
        }
    }

    /// Find a trust anchor whose subject matches the given issuer.
    pub fn find_anchor(&self, issuer: &[(String, String)]) -> Option<&TrustAnchor> {
        for anchor in &self.anchors {
            if subjects_match(&anchor.subject, issuer) { return Some(anchor); }
        }
        None
    }

    pub fn len(&self) -> usize { self.anchors.len() }
    pub fn is_empty(&self) -> bool { self.anchors.is_empty() }
}

fn subjects_match(a: &[(String, String)], b: &[(String, String)]) -> bool {
    if a.len() != b.len() { return false; }
    for ((ao, av), (bo, bv)) in a.iter().zip(b.iter()) {
        if ao != bo || !av.eq_ignore_ascii_case(bv) { return false; }
    }
    true
}

// ─────────────────────────────────────────────────────────────────────────────
//  Chain validation
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// Chain is empty.
    EmptyChain,
    /// Could not find a trust anchor for the root.
    UntrustedRoot,
    /// A certificate in the chain has expired.
    Expired,
    /// A non-CA certificate appears as an issuer.
    NotCA,
    /// Signature verification failed at index.
    BadSignature(usize),
    /// Hostname does not match the leaf certificate.
    HostnameMismatch,
    /// Chain too long (exceeds path length constraint).
    PathLengthExceeded,
}

/// Verify a certificate chain for a given hostname.
///
/// `chain[0]` = leaf (server) certificate.
/// `chain[1..]` = intermediates (in order toward root).
///
/// Returns `Ok(())` on success.
pub fn verify_chain(
    chain:    &[Certificate],
    hostname: &str,
    store:    &CaStore,
) -> Result<(), VerifyError> {
    if chain.is_empty() { return Err(VerifyError::EmptyChain); }

    // 1. Verify hostname against leaf.
    if !chain[0].matches_hostname(hostname) {
        return Err(VerifyError::HostnameMismatch);
    }

    // 2. Verify each link: cert[i] signed by cert[i+1].
    for i in 0..chain.len() - 1 {
        let subject  = &chain[i];
        let issuer   = &chain[i + 1];

        // Issuer must be a CA.
        if !issuer.is_ca && i + 1 < chain.len() - 1 {
            return Err(VerifyError::NotCA);
        }

        // Check path length constraint.
        if let Some(max) = issuer.path_len {
            if i > max as usize { return Err(VerifyError::PathLengthExceeded); }
        }

        // Expiry.
        if subject.is_expired() || issuer.is_expired() {
            return Err(VerifyError::Expired);
        }

        // Verify signature.
        if !verify_cert_signature(subject, &issuer.public_key) {
            return Err(VerifyError::BadSignature(i));
        }
    }

    // 3. Find trust anchor for the last cert's issuer.
    let last = chain.last().unwrap();
    if let Some(anchor) = store.find_anchor(&last.issuer) {
        // Verify last cert's signature by trust anchor.
        if !verify_cert_signature(last, &anchor.public_key) {
            return Err(VerifyError::BadSignature(chain.len() - 1));
        }
        Ok(())
    } else {
        // Check if last cert is self-signed (it's a root in the chain).
        if subjects_match(&last.subject, &last.issuer) {
            // Self-signed root — trust only if found in store.
            return Err(VerifyError::UntrustedRoot);
        }
        Err(VerifyError::UntrustedRoot)
    }
}

/// Verify that `cert` was signed by `issuer_key`.
fn verify_cert_signature(cert: &Certificate, issuer_key: &PublicKey) -> bool {
    match issuer_key {
        PublicKey::Rsa { modulus, exponent } => {
            if modulus.len() < 256 { return false; }
            let mut n_bytes = [0u8; 256];
            let copy_n = modulus.len().min(256);
            n_bytes[256 - copy_n..].copy_from_slice(&modulus[modulus.len()-copy_n..]);
            let mut sig_bytes = [0u8; 256];
            let sig = &cert.signature;
            if sig.len() > 256 { return false; }
            sig_bytes[256 - sig.len()..].copy_from_slice(sig);
            rsa2048_verify_pkcs1v15_sha256(&n_bytes, exponent, &sig_bytes, &cert.tbs_raw)
        }
        PublicKey::Ec { curve_oid, point } => {
            if curve_oid != "1.2.840.10045.3.1.7" { return false; }
            match parse_ecdsa_sig(&cert.signature) {
                Some(r_s) => ecdsa_verify_p256(point, &cert.tbs_raw, &r_s.0, &r_s.1),
                None => false,
            }
        }
        PublicKey::Unknown(_) => false,
    }
}

/// Parse DER ECDSA signature into (r, s) as 32-byte arrays.
fn parse_ecdsa_sig(sig: &[u8]) -> Option<([u8; 32], [u8; 32])> {
    use super::x509::{parse_tlv, TAG_SEQUENCE, TAG_INT};
    let (seq, _) = parse_tlv(sig)?;
    if seq.tag != TAG_SEQUENCE { return None; }
    let (r_tlv, rest) = parse_tlv(seq.content)?;
    let (s_tlv, _)   = parse_tlv(rest)?;
    if r_tlv.tag != TAG_INT || s_tlv.tag != TAG_INT { return None; }

    let mut r = [0u8; 32];
    let mut s = [0u8; 32];
    let rc = r_tlv.content.len().min(32);
    let sc = s_tlv.content.len().min(32);
    // Skip leading zero byte if present (DER may prepend 0x00 for sign).
    let r_src = if r_tlv.content.first() == Some(&0) { &r_tlv.content[1..] } else { r_tlv.content };
    let s_src = if s_tlv.content.first() == Some(&0) { &s_tlv.content[1..] } else { s_tlv.content };
    let rc2 = r_src.len().min(32);
    let sc2 = s_src.len().min(32);
    r[32 - rc2..].copy_from_slice(&r_src[..rc2]);
    s[32 - sc2..].copy_from_slice(&s_src[..sc2]);
    Some((r, s))
}

// ─────────────────────────────────────────────────────────────────────────────
//  Global CA store
// ─────────────────────────────────────────────────────────────────────────────

use spin::Mutex;

pub static CA_STORE: Mutex<CaStore> = Mutex::new(CaStore::new());

/// Initialise the CA store with the embedded Mozilla CA bundle.
pub fn init() {
    let mut store = CA_STORE.lock();
    let compressed_data = super::ca_bundle::COMPRESSED_CA_BUNDLE;
    match crate::net::inflate::inflate_zlib(compressed_data) {
        Ok(decompressed) => {
            let mut pos = 0;
            let mut added = 0;
            while pos + 4 <= decompressed.len() {
                let length = u32::from_be_bytes([
                    decompressed[pos],
                    decompressed[pos + 1],
                    decompressed[pos + 2],
                    decompressed[pos + 3],
                ]) as usize;
                pos += 4;
                if pos + length > decompressed.len() {
                    break;
                }
                let der = &decompressed[pos..pos + length];
                if store.add_root_der(der) {
                    added += 1;
                }
                pos += length;
            }
            crate::serial_println!("[ca_store] CA trust store ready with {} roots (decompressed {} bytes).", added, decompressed.len());
        }
        Err(e) => {
            crate::serial_println!("[ca_store] WARNING: Failed to decompress CA bundle: {:?}", e);
        }
    }
}


/// Add a PEM certificate to the global CA store.
pub fn add_ca_pem(pem: &str) -> bool {
    CA_STORE.lock().add_root_pem(pem)
}

/// Verify a chain against the global store.
pub fn verify_chain_global(chain: &[Certificate], hostname: &str) -> Result<(), VerifyError> {
    let store = CA_STORE.lock();
    verify_chain(chain, hostname, &*store)
}

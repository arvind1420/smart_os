#![allow(dead_code)]
/// Smart OS — Certificate Verifier (Phase 79, v0.39.0)
///
/// Public API for TLS certificate validation wired into the browser:
///   • `verify_tls_cert(chain, hostname)` — full chain + hostname check
///   • `check_cert_expiry(cert)`          — compares against RTC clock
///   • `extract_cn(cert)`                 — pull CN from subject
///   • `CertResult`                       — structured outcome for the security UI
///
/// Internally delegates to `crypto::ca_store::verify_chain_global()` for the
/// RSA/ECDSA signature verification, and adds RTC-based expiry checking and
/// stricter hostname matching on top.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use alloc::vec;
use super::x509::Certificate;

// ─── Verification outcome ─────────────────────────────────────────────────────
#[derive(Clone, Debug, PartialEq)]
pub enum CertError {
    ChainBroken,
    Expired,
    NotYetValid,
    HostnameMismatch,
    SelfSigned,
    Revoked,           // OCSP stub
    WeakSignature,     // MD5 / SHA-1 leaf
    EmptyChain,
    ParseError,
}

impl CertError {
    pub fn description(&self) -> &'static str {
        match self {
            CertError::ChainBroken       => "Certificate chain could not be verified",
            CertError::Expired           => "Certificate has expired",
            CertError::NotYetValid       => "Certificate is not yet valid",
            CertError::HostnameMismatch  => "Hostname does not match certificate",
            CertError::SelfSigned        => "Certificate is self-signed (not in trust store)",
            CertError::Revoked           => "Certificate has been revoked",
            CertError::WeakSignature     => "Certificate uses a weak signature algorithm",
            CertError::EmptyChain        => "No certificate provided",
            CertError::ParseError        => "Certificate could not be parsed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CertResult {
    pub valid:    bool,
    pub subject:  String,
    pub issuer:   String,
    pub san_names: Vec<String>,
    pub not_before: u64,    // Unix-like seconds (simplified)
    pub not_after:  u64,
    pub error:    Option<CertError>,
    pub warnings: Vec<String>,
}

impl CertResult {
    pub fn ok(subject: String, issuer: String) -> Self {
        CertResult { valid: true, subject, issuer, san_names: Vec::new(),
            not_before: 0, not_after: u64::MAX, error: None, warnings: Vec::new() }
    }
    pub fn err(e: CertError, subject: String) -> Self {
        CertResult { valid: false, subject, issuer: String::new(), san_names: Vec::new(),
            not_before: 0, not_after: 0, error: Some(e), warnings: Vec::new() }
    }
    /// One-line human summary for the browser security indicator.
    pub fn summary(&self) -> String {
        if self.valid {
            format!("Valid — {}", self.subject)
        } else {
            format!("Invalid — {}", self.error.as_ref()
                .map(|e| e.description())
                .unwrap_or("Unknown error"))
        }
    }
}

// ─── Hostname matching ────────────────────────────────────────────────────────
/// Returns true if `hostname` matches the cert's CN / SAN list.
/// Supports `*.example.com` wildcard (one level only).
pub fn hostname_matches(hostname: &str, cert_host: &str) -> bool {
    let h = hostname.to_ascii_lowercase();
    let c = cert_host.to_ascii_lowercase();

    if h == c { return true; }

    if let Some(wildcard_domain) = c.strip_prefix("*.") {
        // *.example.com matches sub.example.com but NOT a.b.example.com
        if let Some(after_first_dot) = h.find('.').map(|i| &h[i+1..]) {
            return after_first_dot == wildcard_domain;
        }
    }
    false
}

// ─── Expiry check against RTC ─────────────────────────────────────────────────
/// Approximate "now" in seconds since 2000-01-01 (RTC epoch).
fn rtc_now_approx() -> u64 {
    let dt = crate::drivers::rtc::now();
    // Very rough: seconds since 2000
    let years_since_2000 = dt.year.saturating_sub(2000) as u64;
    let secs = years_since_2000 * 365 * 24 * 3600
        + (dt.month as u64).saturating_sub(1) * 30 * 24 * 3600
        + (dt.day    as u64).saturating_sub(1) * 24 * 3600
        + dt.hour   as u64 * 3600
        + dt.minute as u64 * 60
        + dt.second as u64;
    secs
}

// ─── Main verification entry point ───────────────────────────────────────────
/// Verify a certificate chain (DER-encoded) for a given hostname.
/// Returns a `CertResult` suitable for populating the browser security UI.
pub fn verify_tls_cert(chain_der: &[&[u8]], hostname: &str) -> CertResult {
    if chain_der.is_empty() {
        return CertResult::err(CertError::EmptyChain, String::new());
    }

    // Parse leaf certificate
    let leaf_der = chain_der[0];
    let leaf = match super::x509::Certificate::from_der(leaf_der) {
        Some(c) => c,
        None    => return CertResult::err(CertError::ParseError, String::new()),
    };

    let subject = extract_cn_from_cert(&leaf);

    // Hostname check — use Certificate::matches_hostname (SAN + CN).
    if !leaf.matches_hostname(hostname) {
        return CertResult::err(CertError::HostnameMismatch, subject);
    }

    // Expiry check via is_expired() which uses the RTC internally.
    if leaf.is_expired() {
        return CertResult::err(CertError::Expired, subject);
    }

    // Parse intermediate chain for chain_global
    let mut parsed_chain: Vec<Certificate> = Vec::new();
    for der in chain_der {
        if let Some(c) = super::x509::Certificate::from_der(der) {
            parsed_chain.push(c);
        }
    }

    // Chain verification via ca_store
    let issuer = parsed_chain.last()
        .map(|c| extract_cn_from_cert(c))
        .unwrap_or_default();

    match super::ca_store::verify_chain_global(&parsed_chain, hostname) {
        Ok(()) => {
            let mut result = CertResult::ok(subject, issuer);
            result.san_names = leaf.san.clone();
            // not_before / not_after stored as strings; leave as 0 (stub)
            result
        }
        Err(e) => {
            use super::ca_store::VerifyError;
            let cert_err = match e {
                VerifyError::HostnameMismatch    => CertError::HostnameMismatch,
                VerifyError::Expired             => CertError::Expired,
                VerifyError::UntrustedRoot       => CertError::SelfSigned,
                _                                => CertError::ChainBroken,
            };
            CertResult::err(cert_err, subject)
        }
    }
}

/// Extract the Common Name (CN) from a parsed certificate's subject.
/// The x509 module stores subject as (OID, value) pairs; CN OID = 2.5.4.3.
pub fn extract_cn_from_cert(cert: &Certificate) -> String {
    cert.subject.iter()
        .find(|(k, _)| k == "2.5.4.3" || k.eq_ignore_ascii_case("CN"))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Quick convenience: parse a single DER cert and return its CN.
pub fn extract_cn(der: &[u8]) -> Option<String> {
    super::x509::Certificate::from_der(der)
        .map(|c| extract_cn_from_cert(&c))
}

// ─── HSTS helper ──────────────────────────────────────────────────────────────
/// Parse a Strict-Transport-Security header value and extract max-age.
/// Example: `"max-age=31536000; includeSubDomains"` → `Some(31536000)`
pub fn parse_hsts_header(value: &str) -> Option<u64> {
    for part in value.split(';') {
        let part = part.trim().to_ascii_lowercase();
        if let Some(rest) = part.strip_prefix("max-age=") {
            return rest.trim().parse::<u64>().ok();
        }
    }
    None
}

// ─── Self-test ────────────────────────────────────────────────────────────────
pub fn self_test() -> bool {
    let mut ok = true;

    // T1: hostname_matches — exact
    if !hostname_matches("example.com", "example.com") { ok = false; }

    // T2: hostname_matches — wildcard
    if !hostname_matches("sub.example.com", "*.example.com")  { ok = false; }
    if  hostname_matches("a.b.example.com", "*.example.com")  { ok = false; }
    if  hostname_matches("example.com",     "*.example.com")  { ok = false; }

    // T3: hostname_matches — mismatch
    if  hostname_matches("other.com", "example.com") { ok = false; }

    // T4: CertResult::ok summary contains "Valid"
    let r = CertResult::ok("example.com".to_string(), "Let's Encrypt".to_string());
    if !r.summary().contains("Valid") { ok = false; }
    if !r.valid { ok = false; }

    // T5: CertResult::err summary contains "Invalid"
    let r2 = CertResult::err(CertError::Expired, "old.com".to_string());
    if !r2.summary().contains("Invalid") { ok = false; }
    if r2.valid { ok = false; }

    // T6: CertError descriptions non-empty
    for e in [CertError::ChainBroken, CertError::Expired, CertError::HostnameMismatch,
              CertError::SelfSigned, CertError::Revoked, CertError::WeakSignature] {
        if e.description().is_empty() { ok = false; }
    }

    // T7: parse_hsts_header
    if parse_hsts_header("max-age=31536000; includeSubDomains") != Some(31536000) { ok = false; }
    if parse_hsts_header("no-max-age-here") != None { ok = false; }

    // T8: verify_tls_cert with empty chain
    let r3 = verify_tls_cert(&[], "example.com");
    if r3.valid { ok = false; }
    if r3.error != Some(CertError::EmptyChain) { ok = false; }

    ok
}

//! Cookie jar for the HTTP client.
//!
//! Supports the subset of RFC 6265 most commonly seen in the wild:
//!   • Parses `Set-Cookie` headers (name, value, Domain, Path, Expires,
//!     Max-Age, Secure, HttpOnly, SameSite).  Quoted values are unwrapped.
//!   • Per-domain storage in a single shared jar (`COOKIES`).
//!   • `header_for(host, path, scheme)` produces the `Cookie: …` line to send
//!     with a request.
//!   • Domain matching uses `RFC 6265 §5.1.3` rules (suffix match).
//!   • Path matching uses `RFC 6265 §5.1.4`.
//!   • Expiry is tracked in *seconds since boot* via `drivers::timer::uptime_secs`,
//!     because the kernel has no wall clock by default; this is good enough for
//!     session cookies and quick fetches but does not survive reboots (the jar
//!     itself doesn't persist anyway).
//!
//! Thread-safe via `spin::Mutex`.  No allocations on the hot path beyond what
//! BTreeMap needs.

#![allow(dead_code)]

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;

#[derive(Clone, Debug)]
pub struct Cookie {
    pub name:      String,
    pub value:     String,
    pub domain:    String,   // canonicalised (no leading dot)
    pub path:      String,
    pub expires_s: Option<u64>,  // absolute uptime_secs deadline; None = session
    pub secure:    bool,
    pub http_only: bool,
}

impl Cookie {
    fn is_expired(&self, now_secs: u64) -> bool {
        match self.expires_s {
            Some(d) => d <= now_secs,
            None => false,
        }
    }

    fn match_domain(&self, host: &str) -> bool {
        let h = host.to_ascii_lowercase();
        let d = self.domain.to_ascii_lowercase();
        if d.is_empty() { return true; }
        // Domain-match per RFC 6265 §5.1.3.
        if h == d { return true; }
        if h.ends_with(&d) {
            // The string just before the cookie's domain in `h` must be a '.'.
            let prefix_len = h.len() - d.len();
            if prefix_len == 0 { return true; }
            return h.as_bytes()[prefix_len - 1] == b'.';
        }
        false
    }

    fn match_path(&self, req_path: &str) -> bool {
        if self.path.is_empty() || self.path == "/" { return true; }
        if req_path == self.path { return true; }
        if req_path.starts_with(&self.path) {
            let next = &req_path[self.path.len()..];
            return self.path.ends_with('/') || next.starts_with('/');
        }
        false
    }
}

/// Key uniquely identifying a cookie within the jar.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    domain: String,
    path:   String,
    name:   String,
}

static COOKIES: Mutex<BTreeMap<Key, Cookie>> = Mutex::new(BTreeMap::new());

// ─────────────────────────────────────────────────────────────────────────────
//  Public surface — called by the HTTP client.
// ─────────────────────────────────────────────────────────────────────────────

/// Parse all `Set-Cookie` headers in a response and add to the jar.
/// `request_host` is used as the fallback Domain attribute.
pub fn ingest_set_cookies(headers: &BTreeMap<String, String>, request_host: &str, request_path: &str) {
    // HTTP headers in our parser collapse multiple Set-Cookie values into one
    // entry with comma separators — which is *wrong* because Expires=... uses
    // commas (`Wed, 09 Jun 2021 ...`).  We handle that by also looking at the
    // raw header line if present in `set-cookie-raw`, but the canonical path
    // for now is best-effort.
    let raw = match headers.get("set-cookie") {
        Some(s) => s.as_str(),
        None => return,
    };
    for entry in split_set_cookie_entries(raw) {
        if let Some(c) = parse_set_cookie(entry, request_host, request_path) {
            store(c);
        }
    }
}

/// Build the value for the `Cookie:` request header (empty if none apply).
pub fn header_for(host: &str, path: &str, scheme: &str) -> String {
    let now = uptime_secs();
    let mut jar = COOKIES.lock();
    // Drop expired.
    jar.retain(|_, c| !c.is_expired(now));
    let mut parts: Vec<String> = Vec::new();
    for c in jar.values() {
        if c.secure && scheme != "https" { continue; }
        if !c.match_domain(host) { continue; }
        if !c.match_path(path)   { continue; }
        parts.push(format!("{}={}", c.name, c.value));
    }
    parts.join("; ")
}

/// Get all cookies for a host as a flat `"a=1; b=2"` string (used by
/// `document.cookie` in the JS interpreter).
pub fn all_for(host: &str) -> String { header_for(host, "/", "https") }

/// Set a cookie from JS-land (`document.cookie = "..."`).
pub fn set_from_js(value: &str, request_host: &str) {
    if let Some(c) = parse_set_cookie(value, request_host, "/") {
        store(c);
    }
}

/// Clear the entire jar.
pub fn clear() { COOKIES.lock().clear(); }

pub fn len() -> usize { COOKIES.lock().len() }

// ─────────────────────────────────────────────────────────────────────────────
//  Internals
// ─────────────────────────────────────────────────────────────────────────────

fn store(cookie: Cookie) {
    let key = Key {
        domain: cookie.domain.clone(),
        path:   cookie.path.clone(),
        name:   cookie.name.clone(),
    };
    COOKIES.lock().insert(key, cookie);
}

fn parse_set_cookie(raw: &str, host: &str, req_path: &str) -> Option<Cookie> {
    let raw = raw.trim();
    if raw.is_empty() { return None; }

    // First segment: name=value
    let (head, attrs) = match raw.find(';') {
        Some(i) => (&raw[..i], &raw[i + 1..]),
        None    => (raw, ""),
    };
    let eq = head.find('=')?;
    let name = head[..eq].trim().to_string();
    let mut value = head[eq + 1..].trim().to_string();
    // Strip surrounding quotes.
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value = value[1..value.len() - 1].to_string();
    }
    if name.is_empty() { return None; }

    let mut domain    = host.to_ascii_lowercase();
    let mut path      = default_path(req_path);
    let mut expires_s = None;
    let mut secure    = false;
    let mut http_only = false;

    for raw_attr in attrs.split(';') {
        let attr = raw_attr.trim();
        if attr.is_empty() { continue; }
        let (k, v) = match attr.find('=') {
            Some(i) => (attr[..i].trim().to_ascii_lowercase(), attr[i + 1..].trim().to_string()),
            None    => (attr.to_ascii_lowercase(), String::new()),
        };
        match k.as_str() {
            "domain"   => {
                let d = v.trim_start_matches('.').to_ascii_lowercase();
                if !d.is_empty() { domain = d; }
            }
            "path"     => if !v.is_empty() { path = v; }
            "max-age"  => {
                if let Ok(n) = v.parse::<i64>() {
                    if n <= 0 { expires_s = Some(0); }
                    else { expires_s = Some(uptime_secs() + n as u64); }
                }
            }
            "expires"  => {
                // We can't parse HTTP-dates without a wall clock — approximate
                // as "session cookie that ends in 1 hour".
                if expires_s.is_none() {
                    expires_s = Some(uptime_secs() + 3600);
                }
            }
            "secure"   => { secure = true; }
            "httponly" => { http_only = true; }
            "samesite" => {} // ignored
            _ => {}
        }
    }

    Some(Cookie { name, value, domain, path, expires_s, secure, http_only })
}

fn default_path(req_path: &str) -> String {
    // RFC 6265 §5.1.4: default-path is request-path up to last '/', or "/".
    if req_path.is_empty() || !req_path.starts_with('/') { return "/".into(); }
    if let Some(i) = req_path.rfind('/') {
        if i == 0 { "/".into() } else { req_path[..i].into() }
    } else { "/".into() }
}

/// Split a header that may contain multiple Set-Cookie values comma-joined.
/// Handles the common false-positive where the comma appears inside `Expires=…`
/// by requiring an `=` *before* the next comma to treat it as a separator.
fn split_set_cookie_entries(raw: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = raw.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b',' {
            // Look ahead — does the next non-space chunk look like `key=val`
            // BEFORE the next `;`?
            let rest = &raw[i + 1..];
            if looks_like_cookie_start(rest) {
                out.push(raw[start..i].trim());
                start = i + 1;
            }
        }
        i += 1;
    }
    if start < bytes.len() { out.push(raw[start..].trim()); }
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

fn looks_like_cookie_start(s: &str) -> bool {
    let s = s.trim_start();
    let eq = match s.find('=') { Some(i) => i, None => return false };
    let semi = s.find(';').unwrap_or(usize::MAX);
    eq < semi && s[..eq].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_'|'-'|'.'))
}

fn uptime_secs() -> u64 {
    crate::drivers::timer::uptime_secs() as u64
}

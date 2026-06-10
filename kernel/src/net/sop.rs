#![allow(dead_code)]
/// Smart OS — Same-Origin Policy (Phase 80, v0.40.0)
///
/// Enforces the Same-Origin Policy for browser Fetch / XHR:
///   • `Origin`       — (scheme, host, port) triple
///   • `same_origin`  — strict equality check
///   • `cors_allowed` — parse CORS response headers, decide if access is granted
///   • `should_block` — top-level decision for Fetch/XHR requests
///
/// The SOP prevents a page at origin A from reading responses from origin B
/// unless B explicitly opts in via CORS headers.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;

// ─── Origin ───────────────────────────────────────────────────────────────────
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub scheme: String,   // "https" / "http"
    pub host:   String,   // lower-cased
    pub port:   u16,      // 0 = default for scheme
}

impl Origin {
    pub fn new(scheme: &str, host: &str, port: u16) -> Self {
        Origin {
            scheme: scheme.to_ascii_lowercase(),
            host:   host.to_ascii_lowercase(),
            port,
        }
    }

    /// Opaque origin — used for data: / file: / null origins.
    pub fn opaque() -> Self {
        Origin { scheme: "null".to_string(), host: String::new(), port: 0 }
    }

    pub fn is_opaque(&self) -> bool { self.scheme == "null" }

    /// Default port for a scheme (0 = "no explicit port").
    pub fn default_port(scheme: &str) -> u16 {
        match scheme {
            "https" => 443,
            "http"  => 80,
            "ws"    => 80,
            "wss"   => 443,
            _       => 0,
        }
    }

    /// Effective port — scheme default if port == 0.
    pub fn effective_port(&self) -> u16 {
        if self.port == 0 { Self::default_port(&self.scheme) } else { self.port }
    }

    pub fn to_string_repr(&self) -> String {
        if self.port == 0 {
            format!("{}://{}", self.scheme, self.host)
        } else {
            format!("{}://{}:{}", self.scheme, self.host, self.port)
        }
    }
}

/// Parse an origin from a full URL.
pub fn parse_origin(url: &str) -> Origin {
    let url_lc = url.to_ascii_lowercase();

    let (scheme, rest) = if let Some(r) = url_lc.strip_prefix("https://") {
        ("https", r)
    } else if let Some(r) = url_lc.strip_prefix("http://") {
        ("http", r)
    } else if let Some(r) = url_lc.strip_prefix("wss://") {
        ("wss", r)
    } else if let Some(r) = url_lc.strip_prefix("ws://") {
        ("ws", r)
    } else {
        return Origin::opaque();
    };

    let authority = rest.split('/').next().unwrap_or(rest);
    let (host_part, port_part) = if let Some(idx) = authority.rfind(':') {
        (&authority[..idx], Some(&authority[idx+1..]))
    } else {
        (authority, None)
    };

    let port = port_part.and_then(|p| p.parse::<u16>().ok()).unwrap_or(0);
    Origin::new(scheme, host_part, port)
}

// ─── SOP check ────────────────────────────────────────────────────────────────
/// Two origins are same-origin if scheme, host, and effective port all match.
pub fn same_origin(a: &Origin, b: &Origin) -> bool {
    !a.is_opaque()
        && !b.is_opaque()
        && a.scheme           == b.scheme
        && a.host             == b.host
        && a.effective_port() == b.effective_port()
}

// ─── CORS ─────────────────────────────────────────────────────────────────────
/// Parsed CORS response headers.
#[derive(Clone, Debug)]
pub struct CorsHeaders {
    pub allow_origin:      Option<String>,  // "*" or specific origin
    pub allow_credentials: bool,
    pub allow_methods:     Vec<String>,
    pub allow_headers:     Vec<String>,
    pub expose_headers:    Vec<String>,
    pub max_age:           Option<u32>,
}

impl CorsHeaders {
    pub fn empty() -> Self {
        CorsHeaders {
            allow_origin:      None,
            allow_credentials: false,
            allow_methods:     Vec::new(),
            allow_headers:     Vec::new(),
            expose_headers:    Vec::new(),
            max_age:           None,
        }
    }
}

/// Parse CORS headers from a slice of (name, value) header pairs.
pub fn parse_cors_headers(headers: &[(&str, &str)]) -> CorsHeaders {
    let mut c = CorsHeaders::empty();
    for (name, value) in headers {
        let n = name.to_ascii_lowercase();
        match n.as_str() {
            "access-control-allow-origin" => {
                c.allow_origin = Some(value.trim().to_string());
            }
            "access-control-allow-credentials" => {
                c.allow_credentials = value.trim().eq_ignore_ascii_case("true");
            }
            "access-control-allow-methods" => {
                c.allow_methods = value.split(',').map(|s| s.trim().to_ascii_uppercase()).collect();
            }
            "access-control-allow-headers" => {
                c.allow_headers = value.split(',').map(|s| s.trim().to_ascii_lowercase()).collect();
            }
            "access-control-expose-headers" => {
                c.expose_headers = value.split(',').map(|s| s.trim().to_ascii_lowercase()).collect();
            }
            "access-control-max-age" => {
                c.max_age = value.trim().parse::<u32>().ok();
            }
            _ => {}
        }
    }
    c
}

/// Returns true if the CORS headers allow the given initiating origin to access
/// the response.  `with_credentials` = true if the request carried cookies.
pub fn cors_allows(cors: &CorsHeaders, initiator: &Origin, with_credentials: bool) -> bool {
    let ao = match &cors.allow_origin {
        None    => return false,
        Some(v) => v.trim(),
    };

    if ao == "*" {
        // Wildcard is disallowed when credentials are involved.
        return !with_credentials;
    }

    let allowed_origin = parse_origin(ao);
    if !same_origin(&allowed_origin, initiator) { return false; }

    // Credentials only work when the server explicitly echoes the exact origin.
    if with_credentials && !cors.allow_credentials { return false; }

    true
}

// ─── Top-level fetch decision ─────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FetchMode {
    SameOrigin,   // same-origin requests only
    NoCors,       // simple cross-origin (no reading response body)
    Cors,         // cross-origin with CORS preflight
    Navigate,     // top-level navigations always allowed
}

#[derive(Clone, Debug, PartialEq)]
pub enum SopDecision {
    Allow,
    BlockNoSameOrigin,
    BlockNoCors,
    BlockCorsDenied,
}

/// Decide whether a fetch from `initiator` to `target` should proceed.
/// `cors` = parsed CORS response headers (None if not yet received / preflight).
pub fn should_block(
    initiator: &Origin,
    target:    &Origin,
    mode:      FetchMode,
    cors:      Option<&CorsHeaders>,
) -> SopDecision {
    // Navigation always goes through
    if mode == FetchMode::Navigate { return SopDecision::Allow; }

    // Same-origin: always allow
    if same_origin(initiator, target) { return SopDecision::Allow; }

    // Cross-origin from opaque initiator (data:, about:blank opened programmatically)
    if initiator.is_opaque() { return SopDecision::BlockNoSameOrigin; }

    match mode {
        FetchMode::SameOrigin => SopDecision::BlockNoSameOrigin,
        FetchMode::NoCors     => SopDecision::Allow, // allowed but body is opaque
        FetchMode::Cors => {
            match cors {
                None    => SopDecision::Allow,   // preflight not yet done; allow request to go
                Some(c) => {
                    if cors_allows(c, initiator, false) {
                        SopDecision::Allow
                    } else {
                        SopDecision::BlockCorsDenied
                    }
                }
            }
        }
        FetchMode::Navigate => SopDecision::Allow,
    }
}

// ─── Self-test ────────────────────────────────────────────────────────────────
pub fn self_test() -> bool {
    let mut ok = true;

    // T1: parse_origin https
    let o = parse_origin("https://example.com/path");
    if o.scheme != "https" || o.host != "example.com" || o.effective_port() != 443 { ok = false; }

    // T2: parse_origin with port
    let o2 = parse_origin("http://localhost:8080/");
    if o2.port != 8080 || o2.host != "localhost" { ok = false; }

    // T3: parse_origin opaque
    let o3 = parse_origin("data:text/html,<h1>Hi</h1>");
    if !o3.is_opaque() { ok = false; }

    // T4: same_origin — match
    let a = parse_origin("https://example.com/a");
    let b = parse_origin("https://example.com/b");
    if !same_origin(&a, &b) { ok = false; }

    // T5: same_origin — scheme mismatch
    let c = parse_origin("http://example.com/");
    if same_origin(&a, &c) { ok = false; }

    // T6: same_origin — port mismatch
    let d = parse_origin("https://example.com:8443/");
    if same_origin(&a, &d) { ok = false; }

    // T7: CORS wildcard allows non-credentialed requests
    let cors = parse_cors_headers(&[("Access-Control-Allow-Origin", "*")]);
    let initiator = parse_origin("https://other.com/");
    let target    = parse_origin("https://api.com/");
    if !cors_allows(&cors, &initiator, false) { ok = false; }

    // T8: CORS wildcard blocks credentialed requests
    if cors_allows(&cors, &initiator, true) { ok = false; }

    // T9: CORS specific origin allows matching origin
    let cors2 = parse_cors_headers(&[
        ("Access-Control-Allow-Origin",      "https://other.com"),
        ("Access-Control-Allow-Credentials", "true"),
    ]);
    if !cors_allows(&cors2, &initiator, true) { ok = false; }

    // T10: should_block — same origin → Allow
    let init = parse_origin("https://example.com/page");
    let tgt  = parse_origin("https://example.com/api");
    if should_block(&init, &tgt, FetchMode::Cors, None) != SopDecision::Allow { ok = false; }

    // T11: should_block — cross origin SameOrigin mode → block
    let cross = parse_origin("https://other.com/api");
    if should_block(&init, &cross, FetchMode::SameOrigin, None) != SopDecision::BlockNoSameOrigin { ok = false; }

    ok
}

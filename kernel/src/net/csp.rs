#![allow(dead_code)]
/// Smart OS — Content Security Policy (Phase 80, v0.40.0)
///
/// Parses and enforces `Content-Security-Policy` response headers:
///   • `CspPolicy`         — parsed set of directives
///   • `parse_csp_header`  — turn raw header string into `CspPolicy`
///   • `check_resource`    — decide if a resource load is allowed
///   • `check_inline_script` — enforce `script-src 'unsafe-inline'`
///   • `check_eval`          — enforce `script-src 'unsafe-eval'`
///
/// Directive subset implemented (covers ~90 % of real-world CSP usage):
///   default-src, script-src, style-src, img-src, font-src, connect-src,
///   media-src, object-src, frame-src, frame-ancestors, base-uri,
///   form-action, upgrade-insecure-requests, block-all-mixed-content

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use alloc::collections::BTreeMap;

// ─── Resource type ────────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceType {
    Script,
    Style,
    Image,
    Font,
    Connect,    // Fetch / XHR / WebSocket
    Media,
    Object,
    Frame,
    Worker,
    Manifest,
    Default,
}

impl ResourceType {
    fn directive_name(self) -> &'static str {
        match self {
            ResourceType::Script   => "script-src",
            ResourceType::Style    => "style-src",
            ResourceType::Image    => "img-src",
            ResourceType::Font     => "font-src",
            ResourceType::Connect  => "connect-src",
            ResourceType::Media    => "media-src",
            ResourceType::Object   => "object-src",
            ResourceType::Frame    => "frame-src",
            ResourceType::Worker   => "worker-src",
            ResourceType::Manifest => "manifest-src",
            ResourceType::Default  => "default-src",
        }
    }
}

// ─── Source expression ────────────────────────────────────────────────────────
#[derive(Clone, Debug, PartialEq)]
pub enum SourceExpr {
    None,                    // 'none'
    Self_,                   // 'self'
    UnsafeInline,            // 'unsafe-inline'
    UnsafeEval,              // 'unsafe-eval'
    StrictDynamic,           // 'strict-dynamic'
    Https,                   // https:
    Http,                    // http:
    Data,                    // data:
    Blob,                    // blob:
    Host(String),            // example.com  /  *.example.com
    Nonce(String),           // 'nonce-<base64>'
    Hash(String),            // 'sha256-<base64>'
}

impl SourceExpr {
    pub fn parse(s: &str) -> Self {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "'none'"            => SourceExpr::None,
            "'self'"            => SourceExpr::Self_,
            "'unsafe-inline'"   => SourceExpr::UnsafeInline,
            "'unsafe-eval'"     => SourceExpr::UnsafeEval,
            "'strict-dynamic'"  => SourceExpr::StrictDynamic,
            "https:"            => SourceExpr::Https,
            "http:"             => SourceExpr::Http,
            "data:"             => SourceExpr::Data,
            "blob:"             => SourceExpr::Blob,
            _ if s.starts_with("'nonce-") && s.ends_with('\'') => {
                SourceExpr::Nonce(s[7..s.len()-1].to_string())
            }
            _ if s.starts_with("'sha256-") || s.starts_with("'sha384-") || s.starts_with("'sha512-") => {
                SourceExpr::Hash(s[1..s.len()-1].to_string())
            }
            _ => SourceExpr::Host(s),
        }
    }
}

// ─── Policy ───────────────────────────────────────────────────────────────────
#[derive(Clone, Debug, Default)]
pub struct CspPolicy {
    /// Directive → source list mapping.
    pub directives: BTreeMap<String, Vec<SourceExpr>>,
    pub upgrade_insecure: bool,
    pub block_all_mixed:  bool,
    pub report_uri:       Option<String>,
}

impl CspPolicy {
    pub fn empty() -> Self { CspPolicy::default() }

    /// Resolve the effective source list for a resource type, falling back to
    /// `default-src` when the specific directive is absent.
    pub fn effective_sources(&self, rt: ResourceType) -> Option<&Vec<SourceExpr>> {
        self.directives.get(rt.directive_name())
            .or_else(|| self.directives.get("default-src"))
    }
}

// ─── Parser ───────────────────────────────────────────────────────────────────
/// Parse a raw `Content-Security-Policy` header value into a `CspPolicy`.
///
/// ```text
/// Content-Security-Policy: default-src 'self'; script-src 'self' cdn.example.com; img-src *
/// ```
pub fn parse_csp_header(header: &str) -> CspPolicy {
    let mut policy = CspPolicy::empty();

    for directive in header.split(';') {
        let directive = directive.trim();
        if directive.is_empty() { continue; }

        let mut parts = directive.splitn(2, |c: char| c.is_ascii_whitespace());
        let name  = match parts.next() { Some(n) => n.trim().to_ascii_lowercase(), None => continue };
        let value = parts.next().unwrap_or("").trim();

        match name.as_str() {
            "upgrade-insecure-requests" => { policy.upgrade_insecure = true; }
            "block-all-mixed-content"   => { policy.block_all_mixed  = true; }
            "report-uri" | "report-to"  => {
                policy.report_uri = Some(value.to_string());
            }
            _ => {
                let sources: Vec<SourceExpr> = value.split_ascii_whitespace()
                    .map(SourceExpr::parse)
                    .collect();
                if !sources.is_empty() {
                    policy.directives.insert(name, sources);
                }
            }
        }
    }

    policy
}

// ─── Enforcement ─────────────────────────────────────────────────────────────
#[derive(Clone, Debug, PartialEq)]
pub enum CspResult {
    Allow,
    Block { reason: String },
}

/// Check whether loading `url` as `resource_type` from `page_origin` is allowed.
pub fn check_resource(
    policy:    &CspPolicy,
    page_origin: &str,
    url:       &str,
    rt:        ResourceType,
) -> CspResult {
    let sources = match policy.effective_sources(rt) {
        None => return CspResult::Allow, // no policy for this type → allow
        Some(s) => s,
    };

    // 'none' blocks everything
    if sources.iter().any(|s| *s == SourceExpr::None) {
        return CspResult::Block { reason: format!("'none' in {:?}", rt.directive_name()) };
    }

    let url_lc   = url.to_ascii_lowercase();
    let is_https = url_lc.starts_with("https://");
    let is_http  = url_lc.starts_with("http://");
    let is_data  = url_lc.starts_with("data:");
    let is_blob  = url_lc.starts_with("blob:");
    let same_origin_url = is_same_origin_url(page_origin, url);

    for src in sources {
        match src {
            SourceExpr::Self_       => if same_origin_url   { return CspResult::Allow; }
            SourceExpr::UnsafeInline => {}   // not relevant for URL resources
            SourceExpr::UnsafeEval  => {}
            SourceExpr::StrictDynamic => {}
            SourceExpr::Https       => if is_https            { return CspResult::Allow; }
            SourceExpr::Http        => if is_http || is_https  { return CspResult::Allow; }
            SourceExpr::Data        => if is_data              { return CspResult::Allow; }
            SourceExpr::Blob        => if is_blob              { return CspResult::Allow; }
            SourceExpr::Host(pat)   => if host_matches(url, pat) { return CspResult::Allow; }
            SourceExpr::Nonce(_)    => {}   // nonces only apply to inline scripts
            SourceExpr::Hash(_)     => {}   // hashes only apply to inline content
            SourceExpr::None        => {}   // handled above
        }
    }

    CspResult::Block { reason: format!("No matching source for {:?}", url) }
}

/// Check whether inline script execution is allowed.
/// Pass `nonce` if the script tag has a `nonce=` attribute; `hash` if caller
/// computed the script body hash.
pub fn check_inline_script(
    policy: &CspPolicy,
    nonce:  Option<&str>,
    hash:   Option<&str>,
) -> CspResult {
    let sources = match policy.effective_sources(ResourceType::Script) {
        None => return CspResult::Allow,
        Some(s) => s,
    };

    if sources.iter().any(|s| *s == SourceExpr::None) {
        return CspResult::Block { reason: "script-src 'none'".to_string() };
    }

    // Nonce match
    if let Some(n) = nonce {
        if sources.iter().any(|s| matches!(s, SourceExpr::Nonce(x) if x == n)) {
            return CspResult::Allow;
        }
    }
    // Hash match
    if let Some(h) = hash {
        if sources.iter().any(|s| matches!(s, SourceExpr::Hash(x) if x.ends_with(h))) {
            return CspResult::Allow;
        }
    }
    // 'unsafe-inline'
    if sources.iter().any(|s| *s == SourceExpr::UnsafeInline) {
        return CspResult::Allow;
    }
    // 'strict-dynamic' suppresses 'unsafe-inline' but allows hashes/nonces (covered above)
    CspResult::Block { reason: "Inline script blocked by CSP".to_string() }
}

/// Check whether `eval()` / `new Function()` is allowed.
pub fn check_eval(policy: &CspPolicy) -> CspResult {
    let sources = match policy.effective_sources(ResourceType::Script) {
        None => return CspResult::Allow,
        Some(s) => s,
    };
    if sources.iter().any(|s| *s == SourceExpr::UnsafeEval) {
        CspResult::Allow
    } else {
        CspResult::Block { reason: "eval() blocked by CSP (no 'unsafe-eval')".to_string() }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────
fn is_same_origin_url(page_origin: &str, url: &str) -> bool {
    let po = page_origin.to_ascii_lowercase();
    let u  = url.to_ascii_lowercase();
    // Simple prefix check: same scheme+host
    if let Some(rest) = u.strip_prefix(&po) {
        return rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('#');
    }
    false
}

fn host_matches(url: &str, pattern: &str) -> bool {
    // Extract host from URL
    let url_lc = url.to_ascii_lowercase();
    let authority = if let Some(r) = url_lc.strip_prefix("https://").or_else(|| url_lc.strip_prefix("http://")) {
        r.split('/').next().unwrap_or(r)
    } else {
        return false;
    };
    let host = authority.split(':').next().unwrap_or(authority);

    let pat = pattern.to_ascii_lowercase();
    if let Some(domain) = pat.strip_prefix("*.") {
        if let Some(after_dot) = host.find('.').map(|i| &host[i+1..]) {
            return after_dot == domain;
        }
        return false;
    }
    host == pat || pat == "*"
}

// ─── Self-test ────────────────────────────────────────────────────────────────
pub fn self_test() -> bool {
    let mut ok = true;

    // T1: parse simple policy
    let policy = parse_csp_header("default-src 'self'; script-src 'self' cdn.example.com; img-src *");
    if !policy.directives.contains_key("default-src") { ok = false; }
    if !policy.directives.contains_key("script-src")  { ok = false; }
    if !policy.directives.contains_key("img-src")     { ok = false; }

    // T2: upgrade-insecure-requests flag
    let p2 = parse_csp_header("default-src 'self'; upgrade-insecure-requests");
    if !p2.upgrade_insecure { ok = false; }

    // T3: check_resource — same origin allowed by 'self'
    let p3 = parse_csp_header("default-src 'self'");
    let res = check_resource(&p3, "https://example.com", "https://example.com/app.js", ResourceType::Script);
    if res != CspResult::Allow { ok = false; }

    // T4: check_resource — cross origin blocked
    let res2 = check_resource(&p3, "https://example.com", "https://cdn.other.com/lib.js", ResourceType::Script);
    if res2 == CspResult::Allow { ok = false; }

    // T5: check_resource — 'none' blocks everything
    let p5 = parse_csp_header("default-src 'none'");
    let res5 = check_resource(&p5, "https://example.com", "https://example.com/img.png", ResourceType::Image);
    if res5 == CspResult::Allow { ok = false; }

    // T6: check_resource — img-src *
    let res6 = check_resource(&policy, "https://example.com", "https://any.cdn.net/img.png", ResourceType::Image);
    if res6 != CspResult::Allow { ok = false; }

    // T7: check_inline_script — blocked without unsafe-inline
    let p7 = parse_csp_header("script-src 'self'");
    if check_inline_script(&p7, None, None) == CspResult::Allow { ok = false; }

    // T8: check_inline_script — allowed with unsafe-inline
    let p8 = parse_csp_header("script-src 'unsafe-inline'");
    if check_inline_script(&p8, None, None) != CspResult::Allow { ok = false; }

    // T9: check_eval — blocked without unsafe-eval
    let p9 = parse_csp_header("script-src 'self'");
    if check_eval(&p9) == CspResult::Allow { ok = false; }

    // T10: nonce match
    let p10 = parse_csp_header("script-src 'nonce-abc123'");
    if check_inline_script(&p10, Some("abc123"), None) != CspResult::Allow { ok = false; }
    if check_inline_script(&p10, Some("wrong"),  None) == CspResult::Allow { ok = false; }

    ok
}

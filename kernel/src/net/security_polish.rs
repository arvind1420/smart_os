//! Phase 137 — Security Polish
//!
//! • HSTS Store — record and enforce HTTP→HTTPS upgrades
//! • Mixed Content Blocking — block HTTP sub-resources on HTTPS pages
//! • Permissions API — camera/mic/geolocation/notifications/clipboard/midi
//! • Secure Context — window.isSecureContext, navigator.userActivation
//! • Referrer Policy — parse + apply
//! • Trusted Types stub (DOM XSS prevention)
//! • Feature Policy / Permissions-Policy header parse
//! • navigator.sendBeacon stub

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use core::cell::RefCell;
use spin::Mutex;

use super::js_interp::{Interpreter, JsValue, JsObject};

// ── HSTS Store ────────────────────────────────────────────────────────────────

struct HstsEntry {
    max_age:             u64,   // seconds remaining
    include_subdomains:  bool,
}

struct HstsStore {
    entries: BTreeMap<String, HstsEntry>,
}
unsafe impl Send for HstsStore {}
unsafe impl Sync for HstsStore {}

static HSTS: Mutex<HstsStore> = Mutex::new(HstsStore { entries: BTreeMap::new() });

/// Record an HSTS directive for a domain.
pub fn hsts_record(domain: &str, max_age_secs: u64, include_subdomains: bool) {
    let key = domain.to_lowercase();
    if max_age_secs == 0 {
        HSTS.lock().entries.remove(&key);
    } else {
        HSTS.lock().entries.insert(key, HstsEntry { max_age: max_age_secs, include_subdomains });
    }
}

/// Returns true if `url` should be upgraded to HTTPS due to HSTS.
pub fn hsts_should_upgrade(url: &str) -> bool {
    if !url.starts_with("http://") { return false; }
    let host = extract_host(url).to_lowercase();
    let store = HSTS.lock();
    if store.entries.contains_key(&host) { return true; }
    // Check subdomain inclusion
    let mut parts = host.split('.').collect::<Vec<_>>();
    while parts.len() > 2 {
        parts.remove(0);
        let parent = parts.join(".");
        if let Some(e) = store.entries.get(&parent) {
            if e.include_subdomains { return true; }
        }
    }
    false
}

/// Upgrade an HTTP URL to HTTPS if covered by HSTS.
pub fn hsts_upgrade_url(url: &str) -> String {
    if hsts_should_upgrade(url) {
        format!("https://{}", &url[7..]) // replace "http://" → "https://"
    } else {
        url.to_string()
    }
}

fn extract_host(url: &str) -> &str {
    let start = if let Some(i) = url.find("://") { i + 3 } else { 0 };
    let rest = &url[start..];
    let end = rest.find(['/', '?', '#', ':'].as_ref()).unwrap_or(rest.len());
    &rest[..end]
}

// ── Mixed Content ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MixedContentResult {
    /// Sub-resource is safe (same scheme or upgrading is OK)
    Allow,
    /// Passive mixed content (image, video, audio) — warn in console, still load
    Warn,
    /// Active mixed content (script, style, XHR, iframe) — hard block
    Block,
}

/// Check whether loading `sub_url` from a page at `page_url` is allowed.
pub fn check_mixed_content(page_url: &str, sub_url: &str, resource_type: &str) -> MixedContentResult {
    // If page is not HTTPS, no mixed content concern
    if !page_url.starts_with("https://") { return MixedContentResult::Allow; }
    // If sub-resource is also HTTPS or data: or blob: → OK
    if sub_url.starts_with("https://") || sub_url.starts_with("data:")
        || sub_url.starts_with("blob:") || sub_url.starts_with("//")
        || !sub_url.contains("://")
    { return MixedContentResult::Allow; }

    // Sub-resource is HTTP on an HTTPS page
    let is_active = matches!(resource_type,
        "script" | "style" | "stylesheet" | "xhr" | "fetch" | "websocket"
        | "iframe" | "frame" | "object" | "embed" | "manifest" | "serviceworker"
        | "font" | "import"
    );
    if is_active { MixedContentResult::Block } else { MixedContentResult::Warn }
}

/// Returns a URL to use for a sub-resource, potentially upgrading to HTTPS.
pub fn upgrade_if_possible(sub_url: &str) -> String {
    if sub_url.starts_with("http://") {
        format!("https://{}", &sub_url[7..])
    } else {
        sub_url.to_string()
    }
}

// ── Referrer Policy ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReferrerPolicy {
    NoReferrer,
    NoReferrerWhenDowngrade,
    Origin,
    OriginWhenCrossOrigin,
    SameOrigin,
    StrictOrigin,
    StrictOriginWhenCrossOrigin,
    UnsafeUrl,
}

impl ReferrerPolicy {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "no-referrer"                        => Self::NoReferrer,
            "no-referrer-when-downgrade"         => Self::NoReferrerWhenDowngrade,
            "origin"                             => Self::Origin,
            "origin-when-cross-origin"           => Self::OriginWhenCrossOrigin,
            "same-origin"                        => Self::SameOrigin,
            "strict-origin"                      => Self::StrictOrigin,
            "strict-origin-when-cross-origin"    => Self::StrictOriginWhenCrossOrigin,
            "unsafe-url"                         => Self::UnsafeUrl,
            _                                    => Self::StrictOriginWhenCrossOrigin,
        }
    }

    /// Compute the Referer header value for navigating from `from_url` to `to_url`.
    pub fn compute_referrer(&self, from_url: &str, to_url: &str) -> Option<String> {
        let from_origin = url_origin(from_url);
        let to_origin   = url_origin(to_url);
        let downgrade   = from_url.starts_with("https://") && to_url.starts_with("http://");
        let cross_origin = from_origin != to_origin;

        match self {
            Self::NoReferrer => None,
            Self::NoReferrerWhenDowngrade => {
                if downgrade { None } else { Some(from_url.to_string()) }
            }
            Self::Origin => Some(from_origin),
            Self::OriginWhenCrossOrigin => {
                if cross_origin { Some(from_origin) } else { Some(from_url.to_string()) }
            }
            Self::SameOrigin => {
                if cross_origin { None } else { Some(from_url.to_string()) }
            }
            Self::StrictOrigin => {
                if downgrade { None } else { Some(from_origin) }
            }
            Self::StrictOriginWhenCrossOrigin => {
                if downgrade { None }
                else if cross_origin { Some(from_origin) }
                else { Some(from_url.to_string()) }
            }
            Self::UnsafeUrl => Some(from_url.to_string()),
        }
    }
}

fn url_origin(url: &str) -> String {
    if let Some(i) = url.find("://") {
        let rest = &url[i + 3..];
        let end  = rest.find('/').unwrap_or(rest.len());
        format!("{}://{}", &url[..i], &rest[..end])
    } else {
        url.to_string()
    }
}

/// Convenience: check if an (already-stripped) hostname has an HSTS entry.
pub fn should_upgrade_to_https(host: &str) -> bool {
    // Build a fake https URL to test against hsts_should_upgrade
    hsts_should_upgrade(&format!("http://{}/", host))
}

/// Convenience: alias for hsts_upgrade_url.
pub fn upgrade_url(url: &str) -> String { hsts_upgrade_url(url) }

/// Standalone referrer computation for tests (mirrors ReferrerPolicy::compute_referrer).
pub fn compute_referrer(policy: ReferrerPolicy, from: &str, to: &str) -> String {
    policy.compute_referrer(from, to).unwrap_or_default()
}

// ── Permissions API ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PermState { Granted, Denied, Prompt }

impl PermState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Denied  => "denied",
            Self::Prompt  => "prompt",
        }
    }
}

struct PermStore { map: BTreeMap<String, PermState> }
unsafe impl Send for PermStore {}
unsafe impl Sync for PermStore {}

static PERMS: Mutex<PermStore> = Mutex::new(PermStore { map: BTreeMap::new() });

pub fn perm_get(name: &str) -> PermState {
    // Default states
    let default = match name {
        "notifications" | "push" | "background-sync" | "persistent-storage" => PermState::Prompt,
        "camera" | "microphone" | "geolocation" | "clipboard-read"          => PermState::Prompt,
        "clipboard-write"                                                    => PermState::Granted,
        "midi"                                                               => PermState::Prompt,
        _                                                                    => PermState::Prompt,
    };
    PERMS.lock().map.get(name).copied().unwrap_or(default)
}

pub fn perm_set(name: &str, state: PermState) {
    PERMS.lock().map.insert(name.to_string(), state);
}

fn make_permission_status(name: &str) -> JsValue {
    let state = perm_get(name);
    let obj   = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("name".to_string(),     JsValue::Str(name.to_string()));
    obj.borrow_mut().set("state".to_string(),    JsValue::Str(state.as_str().to_string()));
    obj.borrow_mut().set("onchange".to_string(), JsValue::Null);
    // addEventListener stub
    obj.borrow_mut().set("addEventListener".to_string(),
        JsValue::NativeFunction("addEventListener", |_,_| JsValue::Undefined));
    JsValue::Object(obj)
}

fn native_perms_query(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let name = if let Some(JsValue::Object(o)) = args.get(0) {
        o.borrow().get("name").to_string_val()
    } else { String::new() };

    let status = make_permission_status(&name);

    // Return a pseudo-Promise that resolves synchronously
    let p = Rc::new(RefCell::new(JsObject::new()));
    p.borrow_mut().set("__result__".to_string(), status);
    p.borrow_mut().set("then".to_string(), JsValue::NativeFunction("then", |args, i| {
        let cb  = args.get(0).cloned().unwrap_or(JsValue::Undefined);
        let this = i.env.get("this");
        let val  = if let JsValue::Object(p) = &this { p.borrow().get("__result__") } else { JsValue::Undefined };
        i.call_value(cb, JsValue::Undefined, &[val]);
        JsValue::Undefined
    }));
    JsValue::Object(p)
}

fn make_permissions_obj() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("query".to_string(), JsValue::NativeFunction("query", native_perms_query));
    obj.borrow_mut().set("revoke".to_string(),
        JsValue::NativeFunction("revoke", |args, i| native_perms_query(args, i)));
    JsValue::Object(obj)
}

// ── Secure context ────────────────────────────────────────────────────────────

/// Returns true if the given URL is considered a "secure context".
pub fn is_secure_context(url: &str) -> bool {
    url.starts_with("https://")
        || url.starts_with("wss://")
        || url == "about:blank"
        || url.starts_with("file://")
        || {
            let host = extract_host(url);
            host == "localhost" || host == "127.0.0.1" || host == "[::1]"
        }
}

// ── Feature/Permissions-Policy ────────────────────────────────────────────────

/// Parse a `Permissions-Policy` header value into a map of feature → allowed-origins.
pub fn parse_permissions_policy(header: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for token in header.split(',') {
        let token = token.trim();
        if let Some(eq) = token.find('=') {
            let feature = token[..eq].trim().to_string();
            let origins = token[eq+1..].trim().to_string();
            map.insert(feature, origins);
        } else if !token.is_empty() {
            // bare feature → allow
            map.insert(token.to_string(), "*".to_string());
        }
    }
    map
}

// ── Install ───────────────────────────────────────────────────────────────────

pub fn install_security_api(interp: &mut Interpreter, page_url: &str) {
    let secure = is_secure_context(page_url);

    // window.isSecureContext
    interp.env.define("isSecureContext".to_string(), JsValue::Bool(secure));

    // navigator.permissions
    let perms = make_permissions_obj();
    // Patch existing navigator object
    let nav = interp.env.get("navigator");
    if let JsValue::Object(n) = &nav {
        n.borrow_mut().set("permissions".to_string(), perms.clone());
        // navigator.userActivation
        let ua = Rc::new(RefCell::new(JsObject::new()));
        ua.borrow_mut().set("isActive".to_string(),      JsValue::Bool(false));
        ua.borrow_mut().set("hasBeenActive".to_string(), JsValue::Bool(false));
        n.borrow_mut().set("userActivation".to_string(), JsValue::Object(ua));
        // navigator.sendBeacon
        n.borrow_mut().set("sendBeacon".to_string(),
            JsValue::NativeFunction("sendBeacon", |_,_| JsValue::Bool(true)));
    } else {
        // Create navigator from scratch
        let nav = Rc::new(RefCell::new(JsObject::new()));
        nav.borrow_mut().set("permissions".to_string(), perms);
        let ua = Rc::new(RefCell::new(JsObject::new()));
        ua.borrow_mut().set("isActive".to_string(),      JsValue::Bool(false));
        ua.borrow_mut().set("hasBeenActive".to_string(), JsValue::Bool(false));
        nav.borrow_mut().set("userActivation".to_string(), JsValue::Object(ua));
        nav.borrow_mut().set("sendBeacon".to_string(),
            JsValue::NativeFunction("sendBeacon", |_,_| JsValue::Bool(true)));
        interp.env.define("navigator".to_string(), JsValue::Object(nav));
    }

    // Trusted Types stub
    interp.run(r#"
        if (typeof trustedTypes === 'undefined') {
            var trustedTypes = {
                createPolicy: function(name, rules) {
                    return {
                        name: name,
                        createHTML: rules && rules.createHTML ? rules.createHTML : function(s){ return s; },
                        createScript: rules && rules.createScript ? rules.createScript : function(s){ return s; },
                        createScriptURL: rules && rules.createScriptURL ? rules.createScriptURL : function(s){ return s; }
                    };
                },
                getAttributeType: function(){ return null; },
                getPropertyType: function(){ return null; },
                isHTML: function(v){ return typeof v === 'string'; },
                isScript: function(v){ return typeof v === 'string'; },
                isScriptURL: function(v){ return typeof v === 'string'; }
            };
        }
    "#);

    // ReportingObserver stub
    interp.env.define("ReportingObserver".to_string(),
        JsValue::NativeFunction("ReportingObserver", |_,_| {
            let o = Rc::new(RefCell::new(JsObject::new()));
            o.borrow_mut().set("observe".to_string(),     JsValue::NativeFunction("observe",     |_,_| JsValue::Undefined));
            o.borrow_mut().set("disconnect".to_string(),  JsValue::NativeFunction("disconnect",  |_,_| JsValue::Undefined));
            o.borrow_mut().set("takeRecords".to_string(), JsValue::NativeFunction("takeRecords", |_,_|
                JsValue::Array(Rc::new(RefCell::new(vec![])))));
            JsValue::Object(o)
        }));
}

// ── Self-test ─────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] security_polish: {}", $name); }
        }
    }

    // T1: HSTS record + upgrade
    hsts_record("secure.example.com", 31536000, false);
    check!( hsts_should_upgrade("http://secure.example.com/page"), "HSTS upgrade plain domain");
    check!(!hsts_should_upgrade("http://other.example.com/page"),  "HSTS no upgrade other domain");
    check!(hsts_upgrade_url("http://secure.example.com/x") == "https://secure.example.com/x", "HSTS upgraded URL");

    // T2: HSTS subdomains
    hsts_record("example.com", 3600, true);
    check!(hsts_should_upgrade("http://api.example.com/"), "HSTS subdomain upgrade");

    // T3: Mixed content blocking
    check!(check_mixed_content("https://a.com/", "http://b.com/script.js", "script")
        == MixedContentResult::Block, "active mixed content blocked");
    check!(check_mixed_content("https://a.com/", "http://b.com/image.jpg", "image")
        == MixedContentResult::Warn, "passive mixed content warn");
    check!(check_mixed_content("https://a.com/", "https://b.com/x", "script")
        == MixedContentResult::Allow, "https→https allowed");
    check!(check_mixed_content("http://a.com/", "http://b.com/x", "script")
        == MixedContentResult::Allow, "http page → http allowed");

    // T4: Referrer policy
    let p = ReferrerPolicy::NoReferrer;
    check!(p.compute_referrer("https://a.com/page", "https://b.com/").is_none(), "no-referrer = None");
    let p = ReferrerPolicy::Origin;
    check!(p.compute_referrer("https://a.com/page?q=1", "https://b.com/") == Some("https://a.com".to_string()), "origin policy");
    let p = ReferrerPolicy::StrictOriginWhenCrossOrigin;
    // Same origin → full URL
    check!(p.compute_referrer("https://a.com/page", "https://a.com/other") == Some("https://a.com/page".to_string()), "SOWCO same-origin");
    // Cross origin → origin only
    check!(p.compute_referrer("https://a.com/page", "https://b.com/x") == Some("https://a.com".to_string()), "SOWCO cross-origin");
    // Downgrade → None
    check!(p.compute_referrer("https://a.com/page", "http://b.com/x").is_none(), "SOWCO downgrade");

    // T5: Secure context detection
    check!( is_secure_context("https://example.com/"), "https = secure");
    check!(!is_secure_context("http://example.com/"),  "http = not secure");
    check!( is_secure_context("http://localhost/"),    "localhost = secure");
    check!( is_secure_context("http://127.0.0.1/"),   "127.0.0.1 = secure");

    // T6: Permissions API JS
    {
        let mut interp = Interpreter::new();
        install_security_api(&mut interp, "https://example.com/");
        interp.run(r#"
            var state = "";
            navigator.permissions.query({name: 'clipboard-write'}).then(function(ps){
                state = ps.state;
            });
        "#);
        check!(interp.env.get("state").to_string_val() == "granted", "clipboard-write = granted");
    }

    // T7: isSecureContext in JS
    {
        let mut interp = Interpreter::new();
        install_security_api(&mut interp, "https://example.com/");
        interp.run("var sc = isSecureContext;");
        check!(interp.env.get("sc").is_truthy(), "isSecureContext = true for https");
    }

    // T8: Permissions-Policy parse
    let pp = parse_permissions_policy("camera=(), microphone=(), geolocation=(self)");
    check!(pp.get("camera") == Some(&"()".to_string()),       "PP camera blocked");
    check!(pp.get("geolocation") == Some(&"(self)".to_string()), "PP geolocation self");

    if fail == 0 {
        crate::serial_println!("[security_polish] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[security_polish] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

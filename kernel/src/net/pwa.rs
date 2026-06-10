//! PWA Support — Phase 130 for Smart OS.
//!
//! Implements the key primitives for Progressive Web Apps:
//!   • Web App Manifest (manifest.json) — JSON parse, field extraction
//!   • `<link rel="manifest">` detection in HTML (via browser.rs)
//!   • `window.matchMedia(query)` — display-mode, prefers-color-scheme, etc.
//!   • `navigator.standalone` property
//!   • `BeforeInstallPromptEvent` — prompt(), userChoice
//!   • Installed PWA registry (origin → manifest)
//!   • Standalone display mode flag per tab (set when "installed")
//!
//! Architecture notes:
//!  - Manifest parsing is pure Rust (no external JSON crate needed — we
//!    walk the JSON via our existing js_interp eval or a tiny inline parser).
//!  - matchMedia returns a pre-resolved MediaQueryList object.
//!  - The "install" flow pushes an ACTION to the GUI action queue.

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

// ── Web App Manifest types ────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum DisplayMode {
    Browser,
    MinimalUi,
    Standalone,
    Fullscreen,
}

impl DisplayMode {
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "fullscreen"  => DisplayMode::Fullscreen,
            "standalone"  => DisplayMode::Standalone,
            "minimal-ui"  => DisplayMode::MinimalUi,
            _             => DisplayMode::Browser,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            DisplayMode::Browser   => "browser",
            DisplayMode::MinimalUi => "minimal-ui",
            DisplayMode::Standalone=> "standalone",
            DisplayMode::Fullscreen=> "fullscreen",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ManifestIcon {
    pub src:   String,
    pub sizes: String,          // "192x192", "512x512"
    pub kind:  String,          // "image/png"
    pub purpose: String,        // "any maskable"
}

#[derive(Clone, Debug)]
pub struct WebAppManifest {
    pub name:             String,
    pub short_name:       String,
    pub description:      String,
    pub start_url:        String,
    pub scope:            String,
    pub display:          DisplayMode,
    pub orientation:      String,
    pub theme_color:      String,
    pub background_color: String,
    pub icons:            Vec<ManifestIcon>,
    pub categories:       Vec<String>,
    pub lang:             String,
}

impl Default for WebAppManifest {
    fn default() -> Self {
        WebAppManifest {
            name: String::new(),
            short_name: String::new(),
            description: String::new(),
            start_url: "/".to_string(),
            scope: "/".to_string(),
            display: DisplayMode::Browser,
            orientation: "any".to_string(),
            theme_color: "#000000".to_string(),
            background_color: "#ffffff".to_string(),
            icons: Vec::new(),
            categories: Vec::new(),
            lang: "en".to_string(),
        }
    }
}

// ── Installed PWA registry ────────────────────────────────────────────────────

static PWA_REGISTRY: Mutex<BTreeMap<String, WebAppManifest>> = Mutex::new(BTreeMap::new());
unsafe impl Send for WebAppManifest {}
unsafe impl Sync for WebAppManifest {}

/// Store a parsed manifest keyed by origin.
pub fn register_pwa(origin: String, manifest: WebAppManifest) {
    PWA_REGISTRY.lock().insert(origin, manifest);
}

/// Check if an origin has an installed PWA.
pub fn is_installed(origin: &str) -> bool {
    PWA_REGISTRY.lock().contains_key(origin)
}

/// Get the manifest for an origin.
pub fn get_manifest(origin: &str) -> Option<WebAppManifest> {
    PWA_REGISTRY.lock().get(origin).cloned()
}

// ── Per-tab standalone state ──────────────────────────────────────────────────
// When a PWA is "launched" in standalone mode the tab stores this flag.

static STANDALONE_TABS: Mutex<BTreeMap<u32, bool>> = Mutex::new(BTreeMap::new());

pub fn set_standalone(tab_id: u32, value: bool) {
    STANDALONE_TABS.lock().insert(tab_id, value);
}

pub fn is_standalone(tab_id: u32) -> bool {
    STANDALONE_TABS.lock().get(&tab_id).copied().unwrap_or(false)
}

// ── Minimal JSON parser for manifest.json ────────────────────────────────────
//
// We parse a small subset of JSON sufficient for manifests:
//   - String values: "..."
//   - Boolean values: true / false
//   - Arrays of strings or objects: [...]
//   - Top-level object: {...}
//
// We delegate to the JS interpreter's JSON.parse for robustness.

pub fn parse_manifest(json: &str) -> WebAppManifest {
    let mut m = WebAppManifest::default();
    // Use inline mini-parser: find key-value pairs at the top level.
    // For each recognised field, extract string or nested array.
    m.name             = json_str_field(json, "name").unwrap_or_default();
    m.short_name       = json_str_field(json, "short_name").unwrap_or_else(|| m.name.clone());
    m.description      = json_str_field(json, "description").unwrap_or_default();
    m.start_url        = json_str_field(json, "start_url").unwrap_or_else(|| "/".to_string());
    m.scope            = json_str_field(json, "scope").unwrap_or_else(|| "/".to_string());
    m.theme_color      = json_str_field(json, "theme_color").unwrap_or_else(|| "#000000".to_string());
    m.background_color = json_str_field(json, "background_color").unwrap_or_else(|| "#ffffff".to_string());
    m.lang             = json_str_field(json, "lang").unwrap_or_else(|| "en".to_string());
    m.orientation      = json_str_field(json, "orientation").unwrap_or_else(|| "any".to_string());

    if let Some(d) = json_str_field(json, "display") {
        m.display = DisplayMode::from_str(&d);
    }

    // Parse icons array
    m.icons = parse_icons_array(json);

    // Parse categories array (array of strings)
    m.categories = parse_string_array(json, "categories");

    m
}

/// Extract a string value for `"key": "value"` from JSON.
fn json_str_field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let pos = json.find(needle.as_str())?;
    let after_key = &json[pos + needle.len()..];
    // Skip whitespace and ':'
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    if after_colon.starts_with('"') {
        // String value
        let inner = &after_colon[1..];
        let end = find_unescaped_quote(inner)?;
        Some(unescape_json(&inner[..end]))
    } else {
        None
    }
}

/// Find the position of the next unescaped `"` in a string slice.
fn find_unescaped_quote(s: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped { escaped = false; continue; }
        if c == '\\' { escaped = true; continue; }
        if c == '"' { return Some(i); }
    }
    None
}

/// Basic JSON string unescape (\\n, \\t, \\\", \\\\).
fn unescape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n')  => out.push('\n'),
                Some('t')  => out.push('\t'),
                Some('r')  => out.push('\r'),
                Some('"')  => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some(c)  => { out.push('\\'); out.push(c); }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse the `"icons": [...]` array.
fn parse_icons_array(json: &str) -> Vec<ManifestIcon> {
    let mut icons = Vec::new();
    let key = "\"icons\"";
    let pos = match json.find(key) {
        Some(p) => p,
        None => return icons,
    };
    let after = &json[pos + key.len()..];
    let bracket = match after.find('[') {
        Some(b) => b,
        None => return icons,
    };
    let array_start = &after[bracket + 1..];
    // Walk through objects in the array
    let mut depth = 0i32;
    let mut obj_start: Option<usize> = None;
    for (i, c) in array_start.char_indices() {
        match c {
            '{' => {
                depth += 1;
                if depth == 1 { obj_start = Some(i); }
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = obj_start {
                        let obj_json = &array_start[start..=i];
                        let src     = json_str_field(obj_json, "src").unwrap_or_default();
                        let sizes   = json_str_field(obj_json, "sizes").unwrap_or_default();
                        let kind    = json_str_field(obj_json, "type").unwrap_or_default();
                        let purpose = json_str_field(obj_json, "purpose").unwrap_or_else(|| "any".to_string());
                        if !src.is_empty() {
                            icons.push(ManifestIcon { src, sizes, kind, purpose });
                        }
                        obj_start = None;
                    }
                }
            }
            ']' if depth == 0 => break,
            _ => {}
        }
    }
    icons
}

/// Parse a `"key": ["str1", "str2"]` string array.
fn parse_string_array(json: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let needle = format!("\"{}\"", key);
    let pos = match json.find(needle.as_str()) {
        Some(p) => p,
        None => return out,
    };
    let after = &json[pos + needle.len()..];
    let colon = match after.find(':') { Some(c) => c, None => return out };
    let after_colon = after[colon + 1..].trim_start();
    if !after_colon.starts_with('[') { return out; }
    let inner = &after_colon[1..];
    let end = match inner.find(']') { Some(e) => e, None => return out };
    let items = &inner[..end];
    // Extract each "..." item
    let mut remaining = items;
    while let Some(start) = remaining.find('"') {
        remaining = &remaining[start + 1..];
        if let Some(end) = find_unescaped_quote(remaining) {
            out.push(unescape_json(&remaining[..end]));
            remaining = &remaining[end + 1..];
        } else {
            break;
        }
    }
    out
}

// ── JS API ────────────────────────────────────────────────────────────────────

/// Build a MediaQueryList object.
fn make_mql(matches: bool, media: &str) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("matches".to_string(), JsValue::Bool(matches));
    obj.borrow_mut().set("media".to_string(),   JsValue::Str(media.to_string()));
    obj.borrow_mut().set("addListener".to_string(),    JsValue::NativeFunction("addListener",    |_,_| JsValue::Undefined));
    obj.borrow_mut().set("removeListener".to_string(), JsValue::NativeFunction("removeListener", |_,_| JsValue::Undefined));
    obj.borrow_mut().set("addEventListener".to_string(),    JsValue::NativeFunction("addEventListener",    |_,_| JsValue::Undefined));
    obj.borrow_mut().set("removeEventListener".to_string(), JsValue::NativeFunction("removeEventListener", |_,_| JsValue::Undefined));
    JsValue::Object(obj)
}

// ── Public test helpers ───────────────────────────────────────────────────────

/// Install-prompt queue for testing.
static INSTALL_PROMPT_PENDING: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Queue a pending install prompt for a tab.
pub fn queue_install_prompt(_tab_id: u32) {
    INSTALL_PROMPT_PENDING.store(true, core::sync::atomic::Ordering::Relaxed);
}

/// Returns true if there is a pending install prompt.
pub fn has_install_prompt() -> bool {
    INSTALL_PROMPT_PENDING.load(core::sync::atomic::Ordering::Relaxed)
}

static REGISTRY_NEXT_ID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(1);
static REGISTRY_IDS: Mutex<alloc::collections::BTreeMap<u32, String>> =
    Mutex::new(alloc::collections::BTreeMap::new());

/// Parse and register a manifest for an origin; returns a stable ID.
pub fn registry_register(origin: &str, json_str: &str) -> u32 {
    let manifest = parse_manifest(json_str);
    register_pwa(origin.to_string(), manifest);
    let id = REGISTRY_NEXT_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    REGISTRY_IDS.lock().insert(id, origin.to_string());
    id
}

/// Look up a registered manifest by ID (returned from `registry_register`).
pub fn registry_lookup(id: u32) -> Option<WebAppManifest> {
    let origin = REGISTRY_IDS.lock().get(&id).cloned()?;
    get_manifest(&origin)
}

/// Convenience wrapper for `evaluate_media_query` with tab_id=0.
pub fn match_media(query: &str) -> bool { evaluate_media_query(query, 0) }

/// Build a `BeforeInstallPromptEvent`.
pub fn make_before_install_prompt(origin: &str) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("type".to_string(),       JsValue::Str("beforeinstallprompt".to_string()));
    obj.borrow_mut().set("platforms".to_string(),  {
        let arr = Rc::new(RefCell::new(vec![JsValue::Str("web".to_string())]));
        JsValue::Array(arr)
    });
    // Store origin for prompt() to access
    obj.borrow_mut().set("__origin__".to_string(), JsValue::Str(origin.to_string()));
    // userChoice: pre-resolved {outcome: "accepted"|"dismissed"}
    let uc_obj = Rc::new(RefCell::new(JsObject::new()));
    uc_obj.borrow_mut().set("outcome".to_string(), JsValue::Str("accepted".to_string()));
    obj.borrow_mut().set("userChoice".to_string(), JsValue::Object(uc_obj));
    // prompt() — triggers install (reads __origin__ from this via interp.env.get("this"))
    obj.borrow_mut().set("prompt".to_string(), JsValue::NativeFunction("prompt", |_args, interp| {
        let this = interp.env.get("this");
        let origin = if let JsValue::Object(o) = &this {
            o.borrow().get("__origin__").to_string_val()
        } else { String::new() };
        crate::serial_println!("[pwa] Install prompt shown for origin: {}", origin);
        JsValue::Undefined
    }));
    // preventDefault — noop
    obj.borrow_mut().set("preventDefault".to_string(), JsValue::NativeFunction("preventDefault", |_,_| JsValue::Undefined));
    JsValue::Object(obj)
}

// window.matchMedia(query) → MediaQueryList
fn native_match_media(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let query = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    // Look up current tab's standalone state via __tab_id__ env variable
    let tab_id = {
        let v = interp.env.get("__tab_id__");
        if matches!(v, JsValue::Undefined) { 0u32 } else { v.to_number() as u32 }
    };
    let matches = evaluate_media_query(&query, tab_id);
    make_mql(matches, &query)
}

/// Evaluate a CSS media query string.
pub fn evaluate_media_query(query: &str, tab_id: u32) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if q.contains("display-mode") {
        // (display-mode: standalone) or (display-mode: fullscreen)
        let standalone = is_standalone(tab_id);
        if q.contains("standalone") || q.contains("fullscreen") {
            return standalone;
        }
        return !standalone; // display-mode: browser
    }
    if q.contains("prefers-color-scheme") {
        // Default to light (no dark mode in kernel OS)
        if q.contains("dark")  { return false; }
        if q.contains("light") { return true; }
    }
    if q.contains("prefers-reduced-motion") {
        return q.contains("no-preference") || q.contains("no preference");
    }
    if q.contains("max-width") {
        // e.g. (max-width: 768px) — assume 1024px viewport
        if let Some(px) = extract_px_value(&q) { return px >= 1024; }
    }
    if q.contains("min-width") {
        if let Some(px) = extract_px_value(&q) { return px <= 1024; }
    }
    // screen, print, all
    !q.contains("print")
}

fn extract_px_value(query: &str) -> Option<u32> {
    // Find a number followed by "px"
    let px_pos = query.find("px")?;
    let before = &query[..px_pos];
    let start = before.rfind(|c: char| !c.is_ascii_digit())
        .map(|p| p + 1)
        .unwrap_or(0);
    before[start..].parse::<u32>().ok()
}

/// Install PWA-related APIs on the interpreter.
/// Also requires `__tab_id__` to be defined in env for per-tab mode awareness.
pub fn install_pwa_api(interp: &mut Interpreter, tab_id: u32) {
    // Store tab id for matchMedia
    interp.env.define("__tab_id__".to_string(), JsValue::Number(tab_id as f64));

    // window.matchMedia
    interp.env.define("matchMedia".to_string(),
        JsValue::NativeFunction("matchMedia", native_match_media));

    // navigator.standalone
    let standalone = is_standalone(tab_id);
    let nav = interp.env.get("navigator");
    if let JsValue::Object(nav_obj) = &nav {
        nav_obj.borrow_mut().set("standalone".to_string(), JsValue::Bool(standalone));
        // navigator.appInstalled (legacy)
        nav_obj.borrow_mut().set("appInstalled".to_string(), JsValue::Bool(false));
    }

    // BeforeInstallPromptEvent constructor (mainly for addEventListener pattern)
    interp.env.define("BeforeInstallPromptEvent".to_string(),
        JsValue::NativeFunction("BeforeInstallPromptEvent", |args, _interp| {
            let origin = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
            make_before_install_prompt(&origin)
        }));

    // window.addEventListener for beforeinstallprompt — store handler
    // (simplified: we expose __pwa_prompt_event__ in env when ready to install)
    interp.env.define("__pwa_dispatch_install_prompt__".to_string(),
        JsValue::NativeFunction("__pwa_dispatch_install_prompt__", |args, interp| {
            let origin = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
            let event = make_before_install_prompt(&origin);
            // Look for a registered beforeinstallprompt handler in env
            let handler = interp.env.get("__beforeinstallprompt_handler__");
            if !matches!(handler, JsValue::Undefined | JsValue::Null) {
                interp.call_value(handler, JsValue::Undefined, &[event]);
            }
            JsValue::Undefined
        }));
}

// ── HTML manifest link extraction ─────────────────────────────────────────────

/// Extract `<link rel="manifest" href="...">` href from raw HTML.
pub fn extract_manifest_url(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(link_pos) = lower[pos..].find("<link") {
        let link_pos = pos + link_pos;
        let tag_end = lower[link_pos..].find('>').map(|e| link_pos + e).unwrap_or(html.len());
        let tag = &html[link_pos..=tag_end];
        let tag_lower = tag.to_ascii_lowercase();
        if tag_lower.contains("rel=\"manifest\"") || tag_lower.contains("rel='manifest'") {
            // Extract href
            if let Some(href) = extract_attr(tag, "href") {
                return Some(href);
            }
        }
        pos = tag_end + 1;
    }
    None
}

/// Extract an attribute value from an HTML tag string.
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let needle_dq = format!("{}=\"", attr);
    let needle_sq = format!("{}='",  attr);
    let tag_lower = tag.to_ascii_lowercase();
    if let Some(p) = tag_lower.find(needle_dq.as_str()) {
        let inner = &tag[p + needle_dq.len()..];
        let end = inner.find('"')?;
        return Some(inner[..end].to_string());
    }
    if let Some(p) = tag_lower.find(needle_sq.as_str()) {
        let inner = &tag[p + needle_sq.len()..];
        let end = inner.find('\'')?;
        return Some(inner[..end].to_string());
    }
    None
}

// ── Self-test ─────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] pwa: {}", $name); }
        }
    }

    // T1: Manifest JSON parsing — name / short_name
    {
        // Note: use r##"..."## so '#' in color values doesn't terminate the raw string
        let json = r##"{"name":"Smart Notes","short_name":"Notes","start_url":"/app","display":"standalone","theme_color":"#3f51b5","background_color":"#fafafa"}"##;
        let m = parse_manifest(json);
        check!(m.name == "Smart Notes", "manifest name");
        check!(m.short_name == "Notes", "manifest short_name");
        check!(m.start_url == "/app", "manifest start_url");
        check!(m.display == DisplayMode::Standalone, "manifest display standalone");
        check!(m.theme_color == "#3f51b5", "manifest theme_color");
    }

    // T2: DisplayMode from_str
    {
        check!(DisplayMode::from_str("standalone") == DisplayMode::Standalone, "display from_str standalone");
        check!(DisplayMode::from_str("fullscreen")  == DisplayMode::Fullscreen, "display from_str fullscreen");
        check!(DisplayMode::from_str("minimal-ui")  == DisplayMode::MinimalUi,  "display from_str minimal-ui");
        check!(DisplayMode::from_str("browser")     == DisplayMode::Browser,    "display from_str browser");
        check!(DisplayMode::from_str("STANDALONE")  == DisplayMode::Standalone, "display from_str case-insensitive");
    }

    // T3: Icons array parsing
    {
        let json = r#"{"name":"App","icons":[{"src":"/icon-192.png","sizes":"192x192","type":"image/png"},{"src":"/icon-512.png","sizes":"512x512","type":"image/png","purpose":"any maskable"}]}"#;
        let m = parse_manifest(json);
        check!(m.icons.len() == 2, "manifest icons count");
        check!(m.icons[0].src == "/icon-192.png", "manifest icon 0 src");
        check!(m.icons[0].sizes == "192x192",      "manifest icon 0 sizes");
        check!(m.icons[1].purpose == "any maskable", "manifest icon 1 purpose");
    }

    // T4: Categories array parsing
    {
        let json = r#"{"name":"App","categories":["productivity","utilities","education"]}"#;
        let m = parse_manifest(json);
        check!(m.categories.len() == 3, "manifest categories count");
        check!(m.categories[0] == "productivity", "manifest categories[0]");
        check!(m.categories[2] == "education",    "manifest categories[2]");
    }

    // T5: extract_manifest_url from HTML
    {
        let html1 = r#"<html><head><link rel="manifest" href="/manifest.json"><title>App</title></head></html>"#;
        let html2 = r#"<link rel="stylesheet" href="app.css"><link rel="manifest" href="/app.webmanifest">"#;
        let html3 = r#"<link rel="icon" href="/favicon.ico">"#;
        check!(extract_manifest_url(html1) == Some("/manifest.json".to_string()), "extract manifest url 1");
        check!(extract_manifest_url(html2) == Some("/app.webmanifest".to_string()), "extract manifest url 2");
        check!(extract_manifest_url(html3).is_none(), "no manifest url");
    }

    // T6: matchMedia display-mode evaluation
    {
        // Non-standalone tab
        set_standalone(9001, false);
        check!(!evaluate_media_query("(display-mode: standalone)", 9001), "matchMedia standalone=false");
        check!(evaluate_media_query("(display-mode: browser)", 9001),    "matchMedia browser mode");
        // Standalone tab
        set_standalone(9002, true);
        check!(evaluate_media_query("(display-mode: standalone)", 9002), "matchMedia standalone=true");
    }

    // T7: matchMedia prefers-color-scheme
    {
        check!(!evaluate_media_query("(prefers-color-scheme: dark)",  0), "matchMedia dark=false");
        check!(evaluate_media_query("(prefers-color-scheme: light)", 0),  "matchMedia light=true");
    }

    // T8: PWA registry — register and lookup
    {
        let m = parse_manifest(r#"{"name":"My PWA","start_url":"/","display":"standalone"}"#);
        register_pwa("https://example.com".to_string(), m.clone());
        check!(is_installed("https://example.com"), "pwa registry: installed");
        check!(!is_installed("https://other.com"),  "pwa registry: not installed");
        let retrieved = get_manifest("https://example.com").unwrap();
        check!(retrieved.name == "My PWA", "pwa registry: retrieve name");
        check!(retrieved.display == DisplayMode::Standalone, "pwa registry: retrieve display");
    }

    // T9: matchMedia JS API via interpreter
    {
        let mut interp = Interpreter::new();
        install_pwa_api(&mut interp, 9003);
        set_standalone(9003, true);
        interp.run(r#"
            var mql = matchMedia('(display-mode: standalone)');
            var isStandalone = mql.matches;
            var mqlDark = matchMedia('(prefers-color-scheme: dark)');
            var isDark = mqlDark.matches;
        "#);
        let is_sa = interp.env.get("isStandalone");
        let is_dark = interp.env.get("isDark");
        check!(matches!(is_sa,   JsValue::Bool(true)),  "matchMedia JS standalone");
        check!(matches!(is_dark, JsValue::Bool(false)), "matchMedia JS dark=false");
    }

    if fail == 0 {
        crate::serial_println!("[pwa] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[pwa] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

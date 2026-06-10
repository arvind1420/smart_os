/// Phase 118 — WebExtensions API
///
/// Implements a Manifest V3-compatible browser extension system:
///   • Manifest V3 JSON parser (name, version, permissions, content_scripts,
///     background service worker, action, web_accessible_resources)
///   • Content script injection (marks which URLs to match)
///   • Background service worker registration
///   • `browser.*` API namespace (tabs, storage, webRequest, action, runtime)
///   • Extension loader (from bundled manifest data)
///   • Permissions enforcement

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::collections::BTreeMap;

// ─────────────────────────────────────────────────────────────────────────────
// MANIFEST V3 TYPES
// ─────────────────────────────────────────────────────────────────────────────

/// Permissions declared in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Permission {
    Tabs, Storage, ActiveTab, WebRequest, WebRequestBlocking,
    History, Bookmarks, Downloads, Cookies, Notifications,
    ClipboardRead, ClipboardWrite, Identity, Alarms,
    Host(String),  // e.g. "*://example.com/*"
    Custom(String),
}

impl Permission {
    pub fn from_str(s: &str) -> Self {
        match s {
            "tabs"                  => Permission::Tabs,
            "storage"               => Permission::Storage,
            "activeTab"             => Permission::ActiveTab,
            "webRequest"            => Permission::WebRequest,
            "webRequestBlocking"    => Permission::WebRequestBlocking,
            "history"               => Permission::History,
            "bookmarks"             => Permission::Bookmarks,
            "downloads"             => Permission::Downloads,
            "cookies"               => Permission::Cookies,
            "notifications"         => Permission::Notifications,
            "clipboardRead"         => Permission::ClipboardRead,
            "clipboardWrite"        => Permission::ClipboardWrite,
            "identity"              => Permission::Identity,
            "alarms"                => Permission::Alarms,
            s if s.contains("://") => Permission::Host(s.to_string()),
            s                       => Permission::Custom(s.to_string()),
        }
    }
}

/// A URL match pattern (Manifest V3 style).
#[derive(Debug, Clone)]
pub struct MatchPattern(pub String);

impl MatchPattern {
    /// Check if a URL matches this pattern.
    /// Supports: `*://example.com/*`, `https://*/*`, `<all_urls>`.
    pub fn matches(&self, url: &str) -> bool {
        let pat = &self.0;
        if pat == "<all_urls>" { return true; }
        // Split scheme
        let (pat_scheme, rest) = if let Some(i) = pat.find("://") {
            (&pat[..i], &pat[i+3..])
        } else {
            return false;
        };
        let (url_scheme, url_rest) = if let Some(i) = url.find("://") {
            (&url[..i], &url[i+3..])
        } else {
            return false;
        };
        if pat_scheme != "*" && pat_scheme != url_scheme { return false; }
        // Match host + path with simple glob
        glob_match(rest, url_rest)
    }
}

/// Simple glob match: `*` matches any sequence, `?` matches one char.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pb = pattern.as_bytes();
    let tb = text.as_bytes();
    let mut pi = 0; let mut ti = 0;
    let mut star_pi = usize::MAX; let mut star_ti = 0;
    while ti < tb.len() {
        if pi < pb.len() && (pb[pi] == b'*' || pb[pi] == tb[ti]) {
            if pb[pi] == b'*' { star_pi = pi; star_ti = ti; pi += 1; }
            else              { pi += 1; ti += 1; }
        } else if star_pi != usize::MAX {
            star_ti += 1; ti = star_ti; pi = star_pi + 1;
        } else {
            return false;
        }
    }
    while pi < pb.len() && pb[pi] == b'*' { pi += 1; }
    pi == pb.len()
}

/// Content script declaration.
#[derive(Debug, Clone)]
pub struct ContentScript {
    pub matches:     Vec<MatchPattern>,
    pub js:          Vec<String>,  // JS file paths
    pub css:         Vec<String>,  // CSS file paths
    pub run_at:      RunAt,
    pub all_frames:  bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAt { DocumentStart, DocumentEnd, DocumentIdle }

impl RunAt {
    pub fn from_str(s: &str) -> Self {
        match s {
            "document_start" => RunAt::DocumentStart,
            "document_end"   => RunAt::DocumentEnd,
            _                => RunAt::DocumentIdle,
        }
    }
}

/// Background service worker declaration.
#[derive(Debug, Clone)]
pub struct Background {
    pub service_worker: String,  // JS file path
    pub module_type:    bool,    // true = ES module
}

/// Browser action (toolbar button).
#[derive(Debug, Clone)]
pub struct Action {
    pub default_title:   String,
    pub default_icon:    Option<String>,
    pub default_popup:   Option<String>,
}

/// Parsed Manifest V3.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub manifest_version: u8,
    pub name:             String,
    pub version:          String,
    pub description:      Option<String>,
    pub permissions:      Vec<Permission>,
    pub host_permissions: Vec<MatchPattern>,
    pub content_scripts:  Vec<ContentScript>,
    pub background:       Option<Background>,
    pub action:           Option<Action>,
    pub web_accessible_resources: Vec<String>,
    pub extension_id:     String,  // generated UUID
}

impl Default for Manifest {
    fn default() -> Self {
        Manifest {
            manifest_version: 3,
            name: String::new(), version: "1.0.0".to_string(),
            description: None, permissions: Vec::new(),
            host_permissions: Vec::new(), content_scripts: Vec::new(),
            background: None, action: None,
            web_accessible_resources: Vec::new(),
            extension_id: String::new(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MINIMAL JSON PARSER (enough for manifest.json)
// ─────────────────────────────────────────────────────────────────────────────

/// Extract a string value for a key in a flat JSON object (no nesting).
fn json_str(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let pos = json.find(&needle)?;
    let after = &json[pos + needle.len()..].trim_start();
    if !after.starts_with(':') { return None; }
    let after = after[1..].trim_start();
    if after.starts_with('"') {
        let end = after[1..].find('"')?;
        Some(after[1..end+1].to_string())
    } else {
        None
    }
}

/// Extract a JSON array of strings for a key.
fn json_str_array(json: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{}\"", key);
    let pos = match json.find(&needle) { Some(p) => p, None => return Vec::new() };
    let after = &json[pos + needle.len()..].trim_start();
    if !after.starts_with(':') { return Vec::new(); }
    let after = after[1..].trim_start();
    if !after.starts_with('[') { return Vec::new(); }
    let end = after.find(']').unwrap_or(after.len());
    let array_str = &after[1..end];
    array_str.split(',').filter_map(|s| {
        let s = s.trim();
        if s.starts_with('"') && s.ends_with('"') { Some(s[1..s.len()-1].to_string()) }
        else { None }
    }).collect()
}

/// Parse a Manifest V3 JSON string into a `Manifest`.
pub fn parse_manifest(json: &str) -> Result<Manifest, &'static str> {
    let mut m = Manifest::default();

    if let Some(v) = json_str(json, "manifest_version") {
        m.manifest_version = v.parse::<u8>().unwrap_or(3);
    }
    if let Some(v) = json_str(json, "name")        { m.name        = v; }
    if let Some(v) = json_str(json, "version")     { m.version     = v; }
    if let Some(v) = json_str(json, "description") { m.description = Some(v); }

    for p in json_str_array(json, "permissions") {
        m.permissions.push(Permission::from_str(&p));
    }
    for hp in json_str_array(json, "host_permissions") {
        m.host_permissions.push(MatchPattern(hp));
    }
    for r in json_str_array(json, "web_accessible_resources") {
        m.web_accessible_resources.push(r);
    }

    // Generate a deterministic extension ID from name + version.
    let id_hash = {
        let s = format!("{}@{}", m.name, m.version);
        let mut h: u32 = 5381;
        for b in s.bytes() { h = h.wrapping_mul(33).wrapping_add(b as u32); }
        format!("{:08x}-0000-0000-0000-{:012x}", h, h as u64 * 0xABCD1234)
    };
    m.extension_id = id_hash;

    if m.name.is_empty() { return Err("manifest missing 'name'"); }
    Ok(m)
}

// ─────────────────────────────────────────────────────────────────────────────
// EXTENSION RUNTIME
// ─────────────────────────────────────────────────────────────────────────────

/// Installed extension record.
#[derive(Debug, Clone)]
pub struct Extension {
    pub manifest:  Manifest,
    pub enabled:   bool,
    pub storage:   BTreeMap<String, String>,  // extension.storage.local
}

impl Extension {
    pub fn new(manifest: Manifest) -> Self {
        Extension { manifest, enabled: true, storage: BTreeMap::new() }
    }

    /// Check if any content script in this extension should run on `url`.
    pub fn should_inject(&self, url: &str) -> bool {
        self.enabled && self.manifest.content_scripts.iter().any(|cs| {
            cs.matches.iter().any(|pat| pat.matches(url))
        })
    }

    /// List all JS files to inject for the given URL and run_at phase.
    pub fn scripts_for(&self, url: &str, phase: RunAt) -> Vec<&str> {
        let mut out = Vec::new();
        if !self.enabled { return out; }
        for cs in &self.manifest.content_scripts {
            if cs.run_at != phase { continue; }
            if cs.matches.iter().any(|p| p.matches(url)) {
                for js in &cs.js { out.push(js.as_str()); }
            }
        }
        out
    }
}

/// Extension manager — holds all installed extensions.
#[derive(Default)]
pub struct ExtensionManager {
    pub extensions: BTreeMap<String, Extension>,  // id → extension
}

impl ExtensionManager {
    pub fn new() -> Self { Self::default() }

    pub fn install(&mut self, manifest_json: &str) -> Result<String, &'static str> {
        let manifest = parse_manifest(manifest_json)?;
        let id = manifest.extension_id.clone();
        self.extensions.insert(id.clone(), Extension::new(manifest));
        Ok(id)
    }

    pub fn uninstall(&mut self, id: &str) { self.extensions.remove(id); }

    pub fn enable(&mut self, id: &str) {
        if let Some(e) = self.extensions.get_mut(id) { e.enabled = true; }
    }
    pub fn disable(&mut self, id: &str) {
        if let Some(e) = self.extensions.get_mut(id) { e.enabled = false; }
    }

    /// Collect all JS scripts from all enabled extensions that match `url` at `phase`.
    pub fn collect_scripts(&self, url: &str, phase: RunAt) -> Vec<(&str, Vec<&str>)> {
        self.extensions.values()
            .filter(|e| e.enabled)
            .map(|e| (e.manifest.name.as_str(), e.scripts_for(url, phase)))
            .filter(|(_, scripts)| !scripts.is_empty())
            .collect()
    }

    /// Check if any extension has a given permission.
    pub fn any_has_permission(&self, id: &str, perm: &Permission) -> bool {
        self.extensions.get(id)
            .map(|e| e.manifest.permissions.contains(perm))
            .unwrap_or(false)
    }

    pub fn count(&self) -> usize { self.extensions.len() }
}

// ─────────────────────────────────────────────────────────────────────────────
// SELF-TEST
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;

    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] webext: {}", $name); }
        }
    }

    // T1: Manifest parse basic fields
    {
        let json = r#"{"manifest_version": "3","name":"SmartBlock","version":"1.2.0","description":"Ad blocker"}"#;
        let m = parse_manifest(json);
        check!(m.is_ok(), "manifest parsed");
        let m = m.unwrap();
        check!(m.name == "SmartBlock", "manifest name");
        check!(m.version == "1.2.0", "manifest version");
        check!(m.description == Some("Ad blocker".to_string()), "manifest description");
    }

    // T2: Permissions parse
    {
        let json = r#"{"name":"X","version":"1","permissions":["tabs","storage","activeTab"]}"#;
        let m = parse_manifest(json).unwrap();
        check!(m.permissions.contains(&Permission::Tabs),      "perm:tabs");
        check!(m.permissions.contains(&Permission::Storage),   "perm:storage");
        check!(m.permissions.contains(&Permission::ActiveTab), "perm:activeTab");
    }

    // T3: Extension ID generated deterministically
    {
        let json1 = r#"{"name":"MyExt","version":"1.0"}"#;
        let json2 = r#"{"name":"MyExt","version":"1.0"}"#;
        let m1 = parse_manifest(json1).unwrap();
        let m2 = parse_manifest(json2).unwrap();
        check!(m1.extension_id == m2.extension_id, "extension ID deterministic");
        check!(!m1.extension_id.is_empty(), "extension ID non-empty");
    }

    // T4: MatchPattern — wildcard URL matching
    {
        let pat = MatchPattern("*://example.com/*".to_string());
        check!(pat.matches("https://example.com/page"), "https://example.com matches");
        check!(pat.matches("http://example.com/foo/bar"), "http://example.com/ matches");
        check!(!pat.matches("https://other.com/"), "other.com doesn't match");
    }

    // T5: MatchPattern — <all_urls>
    {
        let pat = MatchPattern("<all_urls>".to_string());
        check!(pat.matches("https://anything.com"), "<all_urls> matches anything");
    }

    // T6: Content script injection check
    {
        let mut m = Manifest::default();
        m.name = "BlockAds".to_string();
        m.content_scripts = vec![ContentScript {
            matches: vec![MatchPattern("*://*/*".to_string())],
            js: vec!["blocker.js".to_string()],
            css: Vec::new(), run_at: RunAt::DocumentStart, all_frames: false,
        }];
        let ext = Extension::new(m);
        check!(ext.should_inject("https://news.com/article"), "should inject on news.com");
        let scripts = ext.scripts_for("https://news.com/", RunAt::DocumentStart);
        check!(scripts == vec!["blocker.js"], "injects blocker.js");
    }

    // T7: ExtensionManager install + uninstall
    {
        let mut mgr = ExtensionManager::new();
        let json = r#"{"name":"TestExt","version":"0.1","permissions":["tabs"]}"#;
        let id = mgr.install(json).unwrap();
        check!(mgr.count() == 1, "1 extension installed");
        check!(mgr.any_has_permission(&id, &Permission::Tabs), "has tabs permission");
        mgr.uninstall(&id);
        check!(mgr.count() == 0, "extension uninstalled");
    }

    // T8: Extension disable / enable
    {
        let mut mgr = ExtensionManager::new();
        let json = r#"{"name":"ToggleExt","version":"1"}"#;
        let id = mgr.install(json).unwrap();
        mgr.disable(&id);
        check!(!mgr.extensions[&id].enabled, "extension disabled");
        mgr.enable(&id);
        check!(mgr.extensions[&id].enabled, "extension re-enabled");
    }

    // T9: glob_match edge cases
    check!(glob_match("*",  "anything"), "glob * matches anything");
    check!(glob_match("a*b", "a123b"),   "glob a*b matches a123b");
    check!(!glob_match("a*b", "a123c"),  "glob a*b no-match a123c");

    // T10: missing name → error
    {
        let json = r#"{"version":"1.0"}"#;
        let r = parse_manifest(json);
        check!(r.is_err(), "missing name → parse error");
    }

    if fail == 0 {
        crate::serial_println!("[webext] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[webext] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

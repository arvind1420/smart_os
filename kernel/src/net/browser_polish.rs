//! Browser Beta Polish — Phase 98 for Smart OS.
//!
//! Everything a first-time user expects to just work:
//!
//! ## Find-in-page  (Ctrl+F)
//! `FindBar` — query string, match list, highlight ranges, Ctrl+G / Ctrl+Shift+G
//! navigation, match count display.
//!
//! ## Keyboard shortcuts
//! `BrowserShortcut` — maps (Ctrl/Shift/Alt, key) → `BrowserAction`.
//! Includes Ctrl+T, Ctrl+W, Ctrl+L, Ctrl+R, Ctrl+Shift+R, F5, F11, F12,
//! Alt+Left, Alt+Right, Ctrl+F, Ctrl+U (view source), Ctrl+P (print).
//!
//! ## Context menu
//! `ContextMenu` — items vary by the element under the pointer
//! (link / image / text / background).  Includes copy, open-in-new-tab,
//! save-image-as, view-source, inspect.
//!
//! ## Favicon
//! `FaviconStore` — caches 16×16 RGBA pixels keyed by origin.
//! `favicon_url_for_page(html)` extracts `<link rel=icon>` or falls back to
//! `/favicon.ico`.
//!
//! ## Settings page (`about:settings`)
//! `BrowserSettings` — all preferences; `render_settings_page()` generates
//! the HTML string served for `about:settings`.
//!
//! ## Page info
//! `PageInfo` — title, URL, security level, favicon origin, loading state.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::borrow::ToOwned;

// ─── Find-in-page ─────────────────────────────────────────────────────────────

/// A single text match: byte offset + length within the page text buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FindMatch {
    /// Byte offset in the flattened page text.
    pub start: usize,
    /// Byte length of the matching substring.
    pub len:   usize,
}

/// Find-in-page bar state.
#[derive(Debug, Default)]
pub struct FindBar {
    /// Current search query.
    pub query:       String,
    /// All matches found in the current page.
    pub matches:     Vec<FindMatch>,
    /// Index of the currently-highlighted match (0-based).
    pub current:     usize,
    /// Whether the bar is visible.
    pub visible:     bool,
    /// Whether search is case-sensitive.
    pub case_sensitive: bool,
}

impl FindBar {
    pub fn new() -> Self { Self::default() }

    /// Show the find bar and optionally pre-fill the query.
    pub fn show(&mut self, initial: Option<&str>) {
        self.visible = true;
        if let Some(q) = initial { self.query = q.to_string(); }
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.matches.clear();
    }

    /// Run the search over `text` (flattened page text).
    /// Returns the number of matches found.
    pub fn search(&mut self, text: &str) -> usize {
        self.matches.clear();
        if self.query.is_empty() { return 0; }

        let (haystack, needle_owned);
        let needle: &str;
        if self.case_sensitive {
            haystack = text;
            needle = &self.query;
        } else {
            needle_owned = self.query.to_ascii_lowercase();
            needle = &needle_owned;
            haystack = text; // we'll compare lowercased slices below
        };

        let mut offset = 0usize;
        let bytes = haystack.as_bytes();
        let nb = needle.as_bytes();
        while offset + nb.len() <= bytes.len() {
            let window = &bytes[offset..offset + nb.len()];
            let matches_here = if self.case_sensitive {
                window == nb
            } else {
                window.iter().zip(nb.iter()).all(|(&a, &b)| a.to_ascii_lowercase() == b)
            };
            if matches_here {
                self.matches.push(FindMatch { start: offset, len: nb.len() });
                offset += nb.len().max(1);
            } else {
                offset += 1;
            }
        }

        if self.current >= self.matches.len() {
            self.current = 0;
        }
        self.matches.len()
    }

    /// Advance to the next match (wraps around).
    pub fn next_match(&mut self) {
        if self.matches.is_empty() { return; }
        self.current = (self.current + 1) % self.matches.len();
    }

    /// Go to the previous match (wraps around).
    pub fn prev_match(&mut self) {
        if self.matches.is_empty() { return; }
        if self.current == 0 {
            self.current = self.matches.len() - 1;
        } else {
            self.current -= 1;
        }
    }

    pub fn current_match(&self) -> Option<FindMatch> {
        self.matches.get(self.current).copied()
    }

    pub fn status_text(&self) -> String {
        if self.matches.is_empty() {
            if self.query.is_empty() { String::new() }
            else { "No results".to_string() }
        } else {
            format!("{} of {}", self.current + 1, self.matches.len())
        }
    }
}

// ─── Keyboard Shortcuts ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl:  bool,
    pub shift: bool,
    pub alt:   bool,
}

impl Modifiers {
    pub const NONE:       Modifiers = Modifiers { ctrl: false, shift: false, alt: false };
    pub const CTRL:       Modifiers = Modifiers { ctrl: true,  shift: false, alt: false };
    pub const CTRL_SHIFT: Modifiers = Modifiers { ctrl: true,  shift: true,  alt: false };
    pub const ALT:        Modifiers = Modifiers { ctrl: false, shift: false, alt: true  };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserAction {
    // Tab management
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    // Navigation
    FocusAddressBar,
    Reload,
    HardReload,      // bypass cache
    GoBack,
    GoForward,
    Stop,
    // UI
    ToggleFullscreen,
    OpenDevTools,
    // Find
    OpenFindBar,
    CloseFindBar,
    FindNext,
    FindPrev,
    // Page
    ViewSource,
    Print,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    // Misc
    OpenSettings,
    OpenBookmarks,
    OpenHistory,
    OpenDownloads,
    SavePage,
    SelectAll,
    Copy,
    Paste,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: Modifiers,
    /// ASCII key character OR special key code:
    /// 0x25=ArrowLeft, 0x26=ArrowUp, 0x27=ArrowRight, 0x28=ArrowDown,
    /// 0x74=F5, 0x7A=F11, 0x73=F4, 0x79=F10.
    pub key:       u8,
    pub action:    BrowserAction,
}

/// Build the default SmartBrowser keyboard shortcut table.
pub fn default_shortcuts() -> Vec<Shortcut> {
    use BrowserAction::*;
    use Modifiers as M;
    vec![
        Shortcut { modifiers: M::CTRL,       key: b't',  action: NewTab },
        Shortcut { modifiers: M::CTRL,       key: b'w',  action: CloseTab },
        Shortcut { modifiers: M::CTRL,       key: b'l',  action: FocusAddressBar },
        Shortcut { modifiers: M::CTRL,       key: b'r',  action: Reload },
        Shortcut { modifiers: M::CTRL_SHIFT, key: b'r',  action: HardReload },
        Shortcut { modifiers: M::CTRL,       key: 0x74,  action: Reload },      // F5
        Shortcut { modifiers: M::NONE,       key: 0x7A,  action: ToggleFullscreen }, // F11
        Shortcut { modifiers: M::NONE,       key: 0x79,  action: OpenDevTools }, // F10 stub
        Shortcut { modifiers: M::CTRL_SHIFT, key: b'i',  action: OpenDevTools },
        Shortcut { modifiers: M::ALT,        key: 0x25,  action: GoBack },      // Alt+Left
        Shortcut { modifiers: M::ALT,        key: 0x27,  action: GoForward },   // Alt+Right
        Shortcut { modifiers: M::CTRL,       key: b'f',  action: OpenFindBar },
        Shortcut { modifiers: M::NONE,       key: 0x1B,  action: CloseFindBar }, // Esc
        Shortcut { modifiers: M::CTRL,       key: b'g',  action: FindNext },
        Shortcut { modifiers: M::CTRL_SHIFT, key: b'g',  action: FindPrev },
        Shortcut { modifiers: M::CTRL,       key: b'u',  action: ViewSource },
        Shortcut { modifiers: M::CTRL,       key: b'p',  action: Print },
        Shortcut { modifiers: M::CTRL,       key: b'=',  action: ZoomIn },
        Shortcut { modifiers: M::CTRL,       key: b'+',  action: ZoomIn },
        Shortcut { modifiers: M::CTRL,       key: b'-',  action: ZoomOut },
        Shortcut { modifiers: M::CTRL,       key: b'0',  action: ZoomReset },
        Shortcut { modifiers: M::CTRL,       key: b',',  action: OpenSettings },
        Shortcut { modifiers: M::CTRL,       key: b'a',  action: SelectAll },
        Shortcut { modifiers: M::CTRL,       key: b'c',  action: Copy },
        Shortcut { modifiers: M::CTRL,       key: b'v',  action: Paste },
        Shortcut { modifiers: M::CTRL_SHIFT, key: b'b',  action: OpenBookmarks },
        Shortcut { modifiers: M::CTRL,       key: b'h',  action: OpenHistory },
        Shortcut { modifiers: M::CTRL,       key: b'j',  action: OpenDownloads },
        Shortcut { modifiers: M::CTRL,       key: b's',  action: SavePage },
        Shortcut { modifiers: M::CTRL,       key: b'\t', action: NextTab },
        Shortcut { modifiers: M::CTRL_SHIFT, key: b'\t', action: PrevTab },
    ]
}

/// Look up a keyboard event in the shortcut table.
pub fn resolve_shortcut(
    shortcuts: &[Shortcut],
    mods: Modifiers,
    key: u8,
) -> Option<BrowserAction> {
    shortcuts.iter()
        .find(|s| s.modifiers == mods && s.key == key)
        .map(|s| s.action)
}

// ─── Context Menu ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum ContextTarget {
    Background,
    Link    { href: String },
    Image   { src: String, alt: String },
    Text    { selected: String },
    Input,
    Video   { src: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContextMenuAction {
    Back,
    Forward,
    Reload,
    SavePage,
    Print,
    ViewSource,
    Inspect,
    // Link
    OpenInNewTab   { href: String },
    OpenInNewWindow{ href: String },
    CopyLink,
    // Image
    OpenImageInTab { src: String },
    SaveImageAs    { src: String },
    CopyImageUrl,
    // Text
    CopyText       { text: String },
    SearchFor      { text: String },
    // Input
    Undo,
    Paste,
    SelectAll,
}

/// Build the context menu for the given target and selected text.
pub fn build_context_menu(
    target:   &ContextTarget,
    selected: &str,
) -> Vec<ContextMenuAction> {
    let mut items = Vec::new();

    match target {
        ContextTarget::Background => {
            items.push(ContextMenuAction::Back);
            items.push(ContextMenuAction::Forward);
            items.push(ContextMenuAction::Reload);
            items.push(ContextMenuAction::SavePage);
            items.push(ContextMenuAction::Print);
            items.push(ContextMenuAction::ViewSource);
            items.push(ContextMenuAction::Inspect);
        }
        ContextTarget::Link { href } => {
            items.push(ContextMenuAction::OpenInNewTab { href: href.clone() });
            items.push(ContextMenuAction::OpenInNewWindow { href: href.clone() });
            items.push(ContextMenuAction::CopyLink);
            items.push(ContextMenuAction::Inspect);
        }
        ContextTarget::Image { src, .. } => {
            items.push(ContextMenuAction::OpenImageInTab { src: src.clone() });
            items.push(ContextMenuAction::SaveImageAs   { src: src.clone() });
            items.push(ContextMenuAction::CopyImageUrl);
            items.push(ContextMenuAction::Inspect);
        }
        ContextTarget::Text { selected: sel } => {
            items.push(ContextMenuAction::CopyText { text: sel.clone() });
            if !sel.is_empty() {
                items.push(ContextMenuAction::SearchFor { text: sel.clone() });
            }
        }
        ContextTarget::Input => {
            items.push(ContextMenuAction::Undo);
            items.push(ContextMenuAction::Paste);
            items.push(ContextMenuAction::SelectAll);
        }
        ContextTarget::Video { src } => {
            items.push(ContextMenuAction::OpenImageInTab { src: src.clone() });
            items.push(ContextMenuAction::Inspect);
        }
    }

    if !selected.is_empty() && !matches!(target, ContextTarget::Text { .. }) {
        items.push(ContextMenuAction::CopyText { text: selected.to_string() });
        items.push(ContextMenuAction::SearchFor { text: selected.to_string() });
    }

    items
}

/// Label for a context menu action (shown in the menu UI).
pub fn context_menu_label(action: &ContextMenuAction) -> &'static str {
    match action {
        ContextMenuAction::Back                => "Back",
        ContextMenuAction::Forward             => "Forward",
        ContextMenuAction::Reload              => "Reload",
        ContextMenuAction::SavePage            => "Save Page As…",
        ContextMenuAction::Print               => "Print…",
        ContextMenuAction::ViewSource          => "View Page Source",
        ContextMenuAction::Inspect             => "Inspect Element",
        ContextMenuAction::OpenInNewTab { .. } => "Open Link in New Tab",
        ContextMenuAction::OpenInNewWindow{..} => "Open Link in New Window",
        ContextMenuAction::CopyLink            => "Copy Link Address",
        ContextMenuAction::OpenImageInTab{..}  => "Open Image in New Tab",
        ContextMenuAction::SaveImageAs { .. }  => "Save Image As…",
        ContextMenuAction::CopyImageUrl        => "Copy Image Address",
        ContextMenuAction::CopyText { .. }     => "Copy",
        ContextMenuAction::SearchFor { .. }    => "Search for…",
        ContextMenuAction::Undo                => "Undo",
        ContextMenuAction::Paste               => "Paste",
        ContextMenuAction::SelectAll           => "Select All",
    }
}

// ─── Favicon ──────────────────────────────────────────────────────────────────

/// Cached 16×16 RGBA favicon for an origin.
#[derive(Clone, Debug)]
pub struct FaviconEntry {
    pub origin: String,
    /// 16 × 16 × 4 RGBA bytes, or empty if still loading / not available.
    pub pixels: Vec<u8>,
    pub loaded: bool,
}

/// In-memory favicon cache (keyed by origin string, e.g. `"https://example.com"`).
#[derive(Default)]
pub struct FaviconStore {
    entries: BTreeMap<String, FaviconEntry>,
}

impl FaviconStore {
    pub fn new() -> Self { Self::default() }

    pub fn get(&self, origin: &str) -> Option<&FaviconEntry> {
        self.entries.get(origin)
    }

    /// Store 16×16 RGBA pixels for `origin`.
    pub fn store(&mut self, origin: String, pixels: Vec<u8>) {
        let loaded = !pixels.is_empty();
        self.entries.insert(origin.clone(), FaviconEntry { origin, pixels, loaded });
    }

    /// Mark an origin as having no favicon (e.g., 404 on /favicon.ico).
    pub fn mark_missing(&mut self, origin: &str) {
        self.entries.insert(origin.to_string(), FaviconEntry {
            origin: origin.to_string(), pixels: Vec::new(), loaded: false,
        });
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
}

/// Extract the favicon URL from an HTML document.
///
/// Looks for `<link rel="icon" href="...">` or `<link rel="shortcut icon" ...>`.
/// Falls back to `{origin}/favicon.ico`.
pub fn favicon_url_for_page(html: &str, origin: &str) -> String {
    let html_lc = html.to_ascii_lowercase();
    // Find <link … rel="icon" … href="…">
    let mut pos = 0;
    while let Some(link_pos) = html_lc[pos..].find("<link") {
        let abs = pos + link_pos;
        let end = html_lc[abs..].find('>').map(|e| abs + e + 1).unwrap_or(html.len());
        let tag = &html_lc[abs..end];
        if (tag.contains("rel=\"icon\"") || tag.contains("rel=\"shortcut icon\"")
            || tag.contains("rel='icon'") || tag.contains("rel='shortcut icon'"))
        {
            // Extract href from the original (non-lowercased) tag
            let orig_tag = &html[abs..end];
            if let Some(href) = extract_attr(orig_tag, "href") {
                if href.starts_with("http://") || href.starts_with("https://") {
                    return href.to_string();
                } else if href.starts_with("//") {
                    return format!("https:{}", href);
                } else if href.starts_with('/') {
                    return format!("{}{}", origin, href);
                } else {
                    return format!("{}/{}", origin, href);
                }
            }
        }
        pos = end;
        if pos >= html.len() { break; }
    }
    format!("{}/favicon.ico", origin)
}

/// Extract an attribute value from a tag string (naive, ASCII-only).
fn extract_attr<'a>(tag: &'a str, attr: &str) -> Option<&'a str> {
    let mut p = 0;
    while p < tag.len() {
        let rest = &tag[p..];
        let attr_lc = attr;
        // look for attr= (case-insensitive)
        let found = rest.to_ascii_lowercase().find(&format!("{}=", attr_lc));
        if let Some(off) = found {
            let val_start = p + off + attr.len() + 1;
            if val_start >= tag.len() { break; }
            let bytes = tag.as_bytes();
            if bytes[val_start] == b'"' || bytes[val_start] == b'\'' {
                let delim = bytes[val_start] as char;
                let inner = &tag[val_start + 1..];
                if let Some(end) = inner.find(delim) {
                    return Some(&inner[..end]);
                }
            } else {
                // unquoted value — ends at whitespace or >
                let inner = &tag[val_start..];
                let end = inner.find(|c: char| c.is_ascii_whitespace() || c == '>')
                    .unwrap_or(inner.len());
                return Some(&inner[..end]);
            }
        }
        break;
    }
    None
}

// ─── Browser Settings ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum SearchEngine { DuckDuckGo, SmartSearch, Custom(String) }

impl SearchEngine {
    pub fn search_url(&self, query: &str) -> String {
        let q = url_encode(query);
        match self {
            SearchEngine::DuckDuckGo      => format!("https://duckduckgo.com/?q={}", q),
            SearchEngine::SmartSearch     => format!("https://search.smartos.local/?q={}", q),
            SearchEngine::Custom(tpl)     => tpl.replace("{q}", &q),
        }
    }
    pub fn name(&self) -> &str {
        match self { Self::DuckDuckGo => "DuckDuckGo",
                     Self::SmartSearch => "SmartSearch",
                     Self::Custom(_) => "Custom" }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrackingProtectionLevel { Off, Standard, Strict }

/// All user-configurable browser preferences.
#[derive(Debug, Clone)]
pub struct BrowserSettings {
    pub search_engine:       SearchEngine,
    pub homepage:            String,
    pub download_path:       String,
    pub language:            String,
    /// Enable / disable JavaScript entirely.
    pub js_enabled:          bool,
    /// Accept / block cookies.
    pub cookies_enabled:     bool,
    pub tracking_protection: TrackingProtectionLevel,
    /// Dark / light theme for browser chrome.
    pub dark_theme:          bool,
    /// Font size scaling factor (1.0 = default).
    pub font_scale:          f32,
    /// HTTPS-only mode.
    pub https_only:          bool,
    /// Ask before downloading (vs auto-save to download_path).
    pub ask_before_download: bool,
    /// Restore previous session on launch.
    pub restore_session:     bool,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        BrowserSettings {
            search_engine:       SearchEngine::DuckDuckGo,
            homepage:            "about:home".to_string(),
            download_path:       "/home/user/Downloads".to_string(),
            language:            "en-US".to_string(),
            js_enabled:          true,
            cookies_enabled:     true,
            tracking_protection: TrackingProtectionLevel::Standard,
            dark_theme:          true,
            font_scale:          1.0,
            https_only:          false,
            ask_before_download: true,
            restore_session:     true,
        }
    }
}

impl BrowserSettings {
    pub fn new() -> Self { Self::default() }

    /// Serialise to a simple key=value string for VFS persistence.
    pub fn to_kv(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("search_engine={}\n", self.search_engine.name()));
        out.push_str(&format!("homepage={}\n", self.homepage));
        out.push_str(&format!("download_path={}\n", self.download_path));
        out.push_str(&format!("language={}\n", self.language));
        out.push_str(&format!("js_enabled={}\n", self.js_enabled));
        out.push_str(&format!("cookies_enabled={}\n", self.cookies_enabled));
        out.push_str(&format!("tracking_protection={}\n",
            match self.tracking_protection { TrackingProtectionLevel::Off => "off",
                TrackingProtectionLevel::Standard => "standard", _ => "strict" }));
        out.push_str(&format!("dark_theme={}\n", self.dark_theme));
        out.push_str(&format!("font_scale={:.2}\n", self.font_scale));
        out.push_str(&format!("https_only={}\n", self.https_only));
        out.push_str(&format!("ask_before_download={}\n", self.ask_before_download));
        out.push_str(&format!("restore_session={}\n", self.restore_session));
        out
    }

    /// Parse from key=value lines.
    pub fn from_kv(s: &str) -> Self {
        let mut cfg = Self::default();
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            if let Some(eq) = line.find('=') {
                let k = &line[..eq];
                let v = &line[eq+1..];
                match k {
                    "homepage"            => cfg.homepage = v.to_string(),
                    "download_path"       => cfg.download_path = v.to_string(),
                    "language"            => cfg.language = v.to_string(),
                    "js_enabled"          => cfg.js_enabled = v == "true",
                    "cookies_enabled"     => cfg.cookies_enabled = v == "true",
                    "dark_theme"          => cfg.dark_theme = v == "true",
                    "https_only"          => cfg.https_only = v == "true",
                    "ask_before_download" => cfg.ask_before_download = v == "true",
                    "restore_session"     => cfg.restore_session = v == "true",
                    "font_scale"          => {
                        if let Ok(f) = v.parse::<f32>() { cfg.font_scale = f; }
                    }
                    "tracking_protection" => {
                        cfg.tracking_protection = match v {
                            "off"    => TrackingProtectionLevel::Off,
                            "strict" => TrackingProtectionLevel::Strict,
                            _        => TrackingProtectionLevel::Standard,
                        };
                    }
                    "search_engine" => {
                        cfg.search_engine = match v {
                            "SmartSearch" => SearchEngine::SmartSearch,
                            _             => SearchEngine::DuckDuckGo,
                        };
                    }
                    _ => {}
                }
            }
        }
        cfg
    }
}

// ─── about:settings HTML generator ───────────────────────────────────────────

/// Generate the `about:settings` HTML page from `BrowserSettings`.
pub fn render_settings_page(cfg: &BrowserSettings) -> String {
    let js_checked    = if cfg.js_enabled          { "checked" } else { "" };
    let ck_checked    = if cfg.cookies_enabled      { "checked" } else { "" };
    let dark_checked  = if cfg.dark_theme            { "checked" } else { "" };
    let https_checked = if cfg.https_only            { "checked" } else { "" };
    let ask_checked   = if cfg.ask_before_download   { "checked" } else { "" };
    let sess_checked  = if cfg.restore_session       { "checked" } else { "" };

    let ddg_sel  = if cfg.search_engine == SearchEngine::DuckDuckGo  { "selected" } else { "" };
    let ss_sel   = if cfg.search_engine == SearchEngine::SmartSearch  { "selected" } else { "" };

    let tp_off = if cfg.tracking_protection == TrackingProtectionLevel::Off      { "selected" } else { "" };
    let tp_std = if cfg.tracking_protection == TrackingProtectionLevel::Standard { "selected" } else { "" };
    let tp_str = if cfg.tracking_protection == TrackingProtectionLevel::Strict   { "selected" } else { "" };

    format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<title>Settings — SmartBrowser</title>
<style>
  body {{ font-family: system-ui, sans-serif; background: #1a1a2e; color: #e0e0e0;
         max-width: 680px; margin: 0 auto; padding: 32px 16px; }}
  h1 {{ color: #4fc3f7; margin-bottom: 24px; }}
  h2 {{ color: #90caf9; font-size: 1rem; border-bottom: 1px solid #333; padding-bottom: 6px; margin-top: 24px; }}
  label {{ display: flex; justify-content: space-between; align-items: center;
           padding: 8px 0; border-bottom: 1px solid #222; }}
  input[type=text], select {{ background: #2a2a3e; color: #e0e0e0; border: 1px solid #444;
                              padding: 4px 8px; border-radius: 4px; min-width: 200px; }}
  input[type=checkbox] {{ width: 16px; height: 16px; cursor: pointer; }}
  button {{ background: #1565c0; color: white; border: none; padding: 8px 20px;
             border-radius: 4px; cursor: pointer; margin-top: 16px; }}
  button:hover {{ background: #1976d2; }}
  .danger {{ background: #c62828; }}
  .danger:hover {{ background: #d32f2f; }}
  .section {{ margin-bottom: 8px; }}
</style>
</head>
<body>
<h1>⚙ SmartBrowser Settings</h1>

<h2>Search & Home</h2>
<label>Search engine
  <select name="search_engine">
    <option value="DuckDuckGo" {ddg_sel}>DuckDuckGo</option>
    <option value="SmartSearch" {ss_sel}>SmartSearch</option>
  </select>
</label>
<label>Homepage
  <input type="text" name="homepage" value="{homepage}">
</label>

<h2>Downloads</h2>
<label>Download location
  <input type="text" name="download_path" value="{download_path}">
</label>
<label>Ask where to save each file
  <input type="checkbox" name="ask_before_download" {ask_checked}>
</label>

<h2>Privacy & Security</h2>
<label>Tracking protection
  <select name="tracking_protection">
    <option value="off" {tp_off}>Off</option>
    <option value="standard" {tp_std}>Standard</option>
    <option value="strict" {tp_str}>Strict</option>
  </select>
</label>
<label>HTTPS-only mode
  <input type="checkbox" name="https_only" {https_checked}>
</label>
<label>Enable cookies
  <input type="checkbox" name="cookies_enabled" {ck_checked}>
</label>

<h2>JavaScript</h2>
<label>Enable JavaScript
  <input type="checkbox" name="js_enabled" {js_checked}>
</label>

<h2>Appearance</h2>
<label>Dark theme
  <input type="checkbox" name="dark_theme" {dark_checked}>
</label>
<label>Font scale ({font_scale_pct}%)
  <input type="range" name="font_scale" min="50" max="200" value="{font_scale_pct}">
</label>
<label>Language
  <input type="text" name="language" value="{language}">
</label>

<h2>On Start-up</h2>
<label>Restore previous session
  <input type="checkbox" name="restore_session" {sess_checked}>
</label>

<h2>Data</h2>
<button onclick="clearBrowsingData()">Clear Browsing Data…</button>
<button class="danger" onclick="resetAllSettings()">Reset All Settings</button>

<script>
function clearBrowsingData() {{
  if (confirm('Clear cookies, cache, and history?')) {{
    window.location.href = 'about:cleardata';
  }}
}}
function resetAllSettings() {{
  if (confirm('Reset all settings to defaults?')) {{
    window.location.href = 'about:resetsettings';
  }}
}}
</script>
</body>
</html>"#,
        homepage        = cfg.homepage,
        download_path   = cfg.download_path,
        font_scale_pct  = (cfg.font_scale * 100.0) as u32,
        language        = cfg.language,
        ddg_sel         = ddg_sel,
        ss_sel          = ss_sel,
        tp_off          = tp_off,
        tp_std          = tp_std,
        tp_str          = tp_str,
        js_checked      = js_checked,
        ck_checked      = ck_checked,
        dark_checked    = dark_checked,
        https_checked   = https_checked,
        ask_checked     = ask_checked,
        sess_checked    = sess_checked,
    )
}

// ─── Page Info ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum LoadingState { Idle, Loading, Done, Error(String) }

#[derive(Debug, Clone, PartialEq)]
pub enum SecurityLevel { Secure, Warning, Insecure, LocalPage }

impl SecurityLevel {
    pub fn badge(&self) -> &'static str {
        match self { Self::Secure    => "🔒", Self::Warning   => "⚠",
                     Self::Insecure  => "🔓", Self::LocalPage => "" }
    }
}

#[derive(Debug, Clone)]
pub struct PageInfo {
    pub url:            String,
    pub title:          String,
    pub security:       SecurityLevel,
    pub favicon_origin: String,
    pub state:          LoadingState,
    /// Zoom level as a percentage (100 = normal).
    pub zoom_pct:       u32,
    /// Whether the page is in full-screen mode.
    pub fullscreen:     bool,
}

impl PageInfo {
    pub fn blank() -> Self {
        PageInfo {
            url:            "about:blank".to_string(),
            title:          "New Tab".to_string(),
            security:       SecurityLevel::LocalPage,
            favicon_origin: String::new(),
            state:          LoadingState::Idle,
            zoom_pct:       100,
            fullscreen:     false,
        }
    }

    pub fn is_loading(&self) -> bool { self.state == LoadingState::Loading }

    /// Title to display in the tab bar (truncated to 20 chars).
    pub fn tab_title(&self) -> String {
        if self.title.is_empty() { return self.url.clone(); }
        if self.title.len() > 20 {
            format!("{}…", &self.title[..19])
        } else {
            self.title.clone()
        }
    }
}

// ─── URL encoding helper (minimal, ASCII-only) ────────────────────────────────

pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                let hi = (b >> 4) & 0x0F;
                let lo = b & 0x0F;
                let hex = b"0123456789ABCDEF";
                out.push(hex[hi as usize] as char);
                out.push(hex[lo as usize] as char);
            }
        }
    }
    out
}

// ─── Self-test ───────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // ── T1: Find-bar search ───────────────────────────────────────────────────
    let mut fb = FindBar::new();
    fb.show(None);
    fb.query = "hello".to_string();
    let page_text = "Hello world, hello Rust, say hello!";
    let count = fb.search(page_text);
    ok &= count == 3; // case-insensitive by default? No — case sensitive
    // Let's do case-insensitive
    fb.case_sensitive = false;
    let count2 = fb.search(page_text);
    ok &= count2 == 3;
    ok &= fb.current_match().is_some();
    ok &= !fb.status_text().is_empty();

    // next/prev wrapping
    fb.next_match();
    ok &= fb.current == 1;
    fb.prev_match();
    ok &= fb.current == 0;
    // prev from 0 wraps to last
    fb.prev_match();
    ok &= fb.current == count2 - 1;

    // empty query
    fb.query = String::new();
    ok &= fb.search(page_text) == 0;
    ok &= fb.status_text().is_empty();

    fb.hide();
    ok &= !fb.visible;
    ok &= fb.matches.is_empty();

    // ── T2: Keyboard shortcuts ────────────────────────────────────────────────
    let shortcuts = default_shortcuts();
    ok &= shortcuts.len() >= 20;

    // Ctrl+T → NewTab
    let action = resolve_shortcut(&shortcuts, Modifiers::CTRL, b't');
    ok &= action == Some(BrowserAction::NewTab);

    // Alt+Left → GoBack
    let action2 = resolve_shortcut(&shortcuts, Modifiers::ALT, 0x25);
    ok &= action2 == Some(BrowserAction::GoBack);

    // Unknown combo → None
    let action3 = resolve_shortcut(&shortcuts, Modifiers::NONE, b'x');
    ok &= action3.is_none();

    // ── T3: Context menu ─────────────────────────────────────────────────────
    let link_menu = build_context_menu(
        &ContextTarget::Link { href: "https://example.com".to_string() },
        "",
    );
    ok &= link_menu.iter().any(|a| matches!(a, ContextMenuAction::OpenInNewTab { .. }));
    ok &= link_menu.iter().any(|a| matches!(a, ContextMenuAction::CopyLink));

    let bg_menu = build_context_menu(&ContextTarget::Background, "selected text");
    ok &= bg_menu.iter().any(|a| matches!(a, ContextMenuAction::ViewSource));
    ok &= bg_menu.iter().any(|a| matches!(a, ContextMenuAction::CopyText { .. }));

    let img_menu = build_context_menu(
        &ContextTarget::Image { src: "https://cdn.test/img.png".to_string(), alt: String::new() },
        "",
    );
    ok &= img_menu.iter().any(|a| matches!(a, ContextMenuAction::SaveImageAs { .. }));

    // Labels are non-empty
    for item in &bg_menu {
        ok &= !context_menu_label(item).is_empty();
    }

    // ── T4: Favicon store ─────────────────────────────────────────────────────
    let mut fstore = FaviconStore::new();
    fstore.store("https://example.com".to_string(), vec![0u8; 16 * 16 * 4]);
    ok &= fstore.len() == 1;
    ok &= fstore.get("https://example.com").map(|e| e.loaded).unwrap_or(false);
    ok &= fstore.get("https://other.com").is_none();
    fstore.mark_missing("https://no-icon.com");
    ok &= fstore.get("https://no-icon.com").map(|e| !e.loaded).unwrap_or(false);

    // favicon URL extraction
    let html_with_icon = r#"<html><head><link rel="icon" href="/assets/favicon.png"></head></html>"#;
    let furl = favicon_url_for_page(html_with_icon, "https://site.com");
    ok &= furl.contains("favicon.png");
    ok &= furl.starts_with("https://site.com");

    let html_no_icon = "<html><head><title>Test</title></head></html>";
    let furl2 = favicon_url_for_page(html_no_icon, "https://other.com");
    ok &= furl2 == "https://other.com/favicon.ico";

    // ── T5: BrowserSettings serialise/parse round-trip ────────────────────────
    let cfg = BrowserSettings {
        homepage: "https://example.com".to_string(),
        js_enabled: false,
        https_only: true,
        font_scale: 1.25,
        ..BrowserSettings::default()
    };
    let kv = cfg.to_kv();
    ok &= kv.contains("homepage=https://example.com");
    ok &= kv.contains("js_enabled=false");
    ok &= kv.contains("https_only=true");

    let cfg2 = BrowserSettings::from_kv(&kv);
    ok &= cfg2.homepage == "https://example.com";
    ok &= !cfg2.js_enabled;
    ok &= cfg2.https_only;
    ok &= (cfg2.font_scale - 1.25).abs() < 0.01;

    // ── T6: Settings page HTML ────────────────────────────────────────────────
    let cfg3 = BrowserSettings::default();
    let html = render_settings_page(&cfg3);
    ok &= html.contains("<!DOCTYPE html>");
    ok &= html.contains("SmartBrowser Settings");
    ok &= html.contains("DuckDuckGo");
    ok &= html.contains("javascript");
    ok &= html.contains("HTTPS-only");

    // ── T7: SearchEngine URL building ─────────────────────────────────────────
    let ddg = SearchEngine::DuckDuckGo;
    let url = ddg.search_url("hello world");
    ok &= url.contains("duckduckgo.com");
    ok &= url.contains("hello");

    let custom = SearchEngine::Custom("https://search.example.com/?q={q}".to_string());
    let curl = custom.search_url("rust lang");
    ok &= curl.contains("search.example.com");
    ok &= curl.contains("rust");

    // ── T8: PageInfo ─────────────────────────────────────────────────────────
    let mut pi = PageInfo::blank();
    ok &= pi.title == "New Tab";
    ok &= pi.tab_title() == "New Tab";
    ok &= !pi.is_loading();

    pi.title = "A very long page title that exceeds twenty characters".to_string();
    ok &= pi.tab_title().ends_with('…');
    ok &= pi.tab_title().len() <= 21; // 19 + "…" (3 bytes as UTF-8 but 1 char)

    pi.state = LoadingState::Loading;
    ok &= pi.is_loading();

    // ── T9: url_encode ────────────────────────────────────────────────────────
    ok &= url_encode("hello world") == "hello+world";
    ok &= url_encode("a&b=c") == "a%26b%3Dc";
    ok &= url_encode("safe-chars_~.") == "safe-chars_~.";

    if ok {
        crate::serial_println!("[browser_polish] Phase 98: all 9 beta-polish tests PASSED");
    } else {
        crate::serial_println!("[browser_polish] Phase 98: FAILED");
    }
    ok
}

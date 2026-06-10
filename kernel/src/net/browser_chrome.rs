//! Browser Chrome — Phase 39 / Phase 103 polish for Smart OS.
//!
//! Implements the non-content browser UI:
//!  • Tab bar: open/close/switch tabs, tab title + favicon colour
//!  • Address bar: URL input, progress bar, security indicator
//!  • Navigation: back/forward history stacks, reload
//!  • Find-in-page: highlight match count, next/prev
//!  • Zoom: percentage scale applied before layout
//!  • Downloads: in-progress item list + download bar at bottom of viewport
//!  • Status bar: link hover text, load status
//!  • Keyboard shortcuts (Ctrl+T, Ctrl+W, Ctrl+L, Alt+←/→, F5, Ctrl+F)
//!
//! Phase 103 additions:
//!  • Security badge: 🔒 "Secure" / ⚠️ "Not Secure" / 🚫 "Error" text in address bar
//!  • Download bar strip at bottom of viewport (one row per active download)
//!
//! The chrome is rendered into a `ChromeFrame` that the browser compositor
//! stacks above the page content area.

#![allow(dead_code)]

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::collections::BTreeMap;

// ─────────────────────────────────────────────────────────────────────────────
//  Security indicator
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum SecurityLevel { Secure, Insecure, Mixed, Error }

impl SecurityLevel {
    pub fn from_url(url: &str) -> Self {
        if url.starts_with("https://") { Self::Secure }
        else if url.starts_with("http://") { Self::Insecure }
        else { Self::Secure } // file://, about:, etc.
    }
    /// Emoji icon for the badge.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Secure   => "🔒",
            Self::Insecure => "⚠️",
            Self::Mixed    => "⚠️",
            Self::Error    => "🚫",
        }
    }
    /// Short text shown next to the icon.
    pub fn badge_text(&self) -> &'static str {
        match self {
            Self::Secure   => "Secure",
            Self::Insecure => "Not Secure",
            Self::Mixed    => "Mixed Content",
            Self::Error    => "Error",
        }
    }
    /// ARGB colour for the badge background chip.
    pub fn badge_color(&self) -> u32 {
        match self {
            Self::Secure   => 0xFF_1A6B2A, // dark green
            Self::Insecure => 0xFF_8A5A00, // amber
            Self::Mixed    => 0xFF_7A4500, // orange-amber
            Self::Error    => 0xFF_8A0000, // dark red
        }
    }
    /// ARGB text/icon colour on the badge.
    pub fn badge_fg(&self) -> u32 {
        match self {
            Self::Secure   => 0xFF_6EE296,
            Self::Insecure => 0xFF_FFD166,
            Self::Mixed    => 0xFF_FFAA44,
            Self::Error    => 0xFF_FF7070,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Navigation history
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct NavHistory {
    pub back:    Vec<String>,   // oldest → newest
    pub current: String,
    pub forward: Vec<String>,   // oldest = next → newest = furthest ahead
}

impl NavHistory {
    pub fn new(url: &str) -> Self {
        NavHistory { back: Vec::new(), current: url.to_string(), forward: Vec::new() }
    }

    /// Navigate to a new URL, clearing the forward stack.
    pub fn navigate(&mut self, url: &str) {
        if !self.current.is_empty() {
            self.back.push(self.current.clone());
        }
        self.current = url.to_string();
        self.forward.clear();
    }

    pub fn can_go_back(&self)    -> bool { !self.back.is_empty() }
    pub fn can_go_forward(&self) -> bool { !self.forward.is_empty() }

    pub fn go_back(&mut self) -> Option<&str> {
        if let Some(prev) = self.back.pop() {
            self.forward.insert(0, self.current.clone());
            self.current = prev;
            Some(&self.current)
        } else { None }
    }

    pub fn go_forward(&mut self) -> Option<&str> {
        if !self.forward.is_empty() {
            self.back.push(self.current.clone());
            self.current = self.forward.remove(0);
            Some(&self.current)
        } else { None }
    }

    pub fn reload_url(&self) -> &str { &self.current }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Tab
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabLoadState { Idle, Loading, Complete, Error(String) }

#[derive(Debug, Clone)]
pub struct Tab {
    pub id:           u32,
    pub title:        String,
    pub favicon_color: u32,   // ARGB — colour approximation of favicon
    pub history:      NavHistory,
    pub load_state:   TabLoadState,
    pub load_progress: u8,   // 0–100
    pub security:     SecurityLevel,
    pub zoom_pct:     u32,   // 100 = no zoom
    pub scroll_x:     f32,
    pub scroll_y:     f32,
    pub find_query:   String,
    pub find_matches: u32,
    pub find_current: u32,
    pub is_pinned:    bool,
    pub is_muted:     bool,
}

impl Tab {
    pub fn new(id: u32, url: &str) -> Self {
        Tab {
            id,
            title: url.to_string(),
            favicon_color: 0xFF_4444FF,  // default blue
            history: NavHistory::new(url),
            load_state: TabLoadState::Idle,
            load_progress: 0,
            security: SecurityLevel::from_url(url),
            zoom_pct: 100,
            scroll_x: 0.0,
            scroll_y: 0.0,
            find_query: String::new(),
            find_matches: 0,
            find_current: 0,
            is_pinned: false,
            is_muted: false,
        }
    }

    pub fn navigate(&mut self, url: &str) {
        self.history.navigate(url);
        self.security     = SecurityLevel::from_url(url);
        self.load_state   = TabLoadState::Loading;
        self.load_progress = 0;
        self.scroll_x     = 0.0;
        self.scroll_y     = 0.0;
        self.find_query   = String::new();
        self.find_matches = 0;
        self.find_current = 0;
    }

    pub fn set_loaded(&mut self, title: &str) {
        self.title        = title.to_string();
        self.load_state   = TabLoadState::Complete;
        self.load_progress = 100;
    }

    pub fn set_error(&mut self, msg: &str) {
        self.load_state = TabLoadState::Error(msg.to_string());
        self.load_progress = 0;
    }

    pub fn current_url(&self) -> &str { &self.history.current }

    pub fn go_back(&mut self) -> Option<String> {
        self.history.go_back().map(|u| {
            let url = u.to_string();
            self.security     = SecurityLevel::from_url(&url);
            self.load_state   = TabLoadState::Loading;
            self.load_progress = 0;
            url
        })
    }

    pub fn go_forward(&mut self) -> Option<String> {
        self.history.go_forward().map(|u| {
            let url = u.to_string();
            self.security     = SecurityLevel::from_url(&url);
            self.load_state   = TabLoadState::Loading;
            self.load_progress = 0;
            url
        })
    }

    /// Short display title (≤30 chars).
    pub fn display_title(&self) -> String {
        let t = if self.title.is_empty() { self.current_url() } else { &self.title };
        if t.len() > 30 { format!("{}…", &t[..29]) } else { t.to_string() }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Download item
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum DownloadState { Pending, InProgress(u64, u64), Complete, Failed(String), Cancelled }

#[derive(Debug, Clone)]
pub struct Download {
    pub id:       u32,
    pub url:      String,
    pub filename: String,
    pub state:    DownloadState,
    pub mime:     String,
}

impl Download {
    pub fn new(id: u32, url: &str, filename: &str, mime: &str) -> Self {
        Download { id, url: url.to_string(), filename: filename.to_string(),
                   state: DownloadState::Pending, mime: mime.to_string() }
    }
    pub fn progress_pct(&self) -> u8 {
        match &self.state {
            DownloadState::InProgress(done, total) => {
                if *total == 0 { 0 } else { (*done * 100 / *total) as u8 }
            }
            DownloadState::Complete => 100,
            _ => 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Address bar state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AddressBar {
    pub text:     String,
    pub cursor:   usize,   // byte offset
    pub focused:  bool,
    pub selected: Option<(usize, usize)>, // selection range
}

impl AddressBar {
    pub fn new() -> Self {
        AddressBar { text: String::new(), cursor: 0, focused: false, selected: None }
    }

    pub fn set_url(&mut self, url: &str) {
        self.text     = url.to_string();
        self.cursor   = url.len();
        self.selected = None;
    }

    pub fn select_all(&mut self) {
        self.selected = Some((0, self.text.len()));
        self.cursor   = self.text.len();
    }

    pub fn type_char(&mut self, c: char) {
        if let Some((s, e)) = self.selected.take() {
            self.text.drain(s..e);
            self.cursor = s;
        }
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    pub fn backspace(&mut self) {
        if let Some((s, e)) = self.selected.take() {
            self.text.drain(s..e);
            self.cursor = s;
            return;
        }
        if self.cursor > 0 {
            let mut i = self.cursor - 1;
            while i > 0 && !self.text.is_char_boundary(i) { i -= 1; }
            self.text.drain(i..self.cursor);
            self.cursor = i;
        }
    }

    pub fn move_cursor_left(&mut self) {
        self.selected = None;
        if self.cursor > 0 {
            let mut i = self.cursor - 1;
            while i > 0 && !self.text.is_char_boundary(i) { i -= 1; }
            self.cursor = i;
        }
    }

    pub fn move_cursor_right(&mut self) {
        self.selected = None;
        if self.cursor < self.text.len() {
            let mut i = self.cursor + 1;
            while i < self.text.len() && !self.text.is_char_boundary(i) { i += 1; }
            self.cursor = i;
        }
    }

    /// Return the committed URL (called on Enter).
    pub fn commit(&mut self) -> String {
        self.focused  = false;
        self.selected = None;
        let url = normalise_url(&self.text);
        self.text = url.clone();
        url
    }
}

/// Add scheme if missing; treat bare words without dots as search queries.
fn normalise_url(input: &str) -> String {
    let s = input.trim();
    if s.starts_with("http://") || s.starts_with("https://")
        || s.starts_with("file://") || s.starts_with("about:")
        || s.starts_with("data:") {
        return s.to_string();
    }
    // Contains a dot → likely a domain
    if s.contains('.') && !s.contains(' ') {
        return format!("https://{}", s);
    }
    // Otherwise → search
    let encoded = s.replace(' ', "+");
    format!("https://search.smartos.local/?q={}", encoded)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Browser Chrome
// ─────────────────────────────────────────────────────────────────────────────

pub const CHROME_HEIGHT:       u32 = 72;  // px tall (tab bar + toolbar)
pub const TAB_HEIGHT:          u32 = 32;
pub const TOOLBAR_HEIGHT:      u32 = 40;
pub const DOWNLOAD_BAR_HEIGHT: u32 = 36;  // per-item row in download shelf

/// Keyboard action decoded from raw key events.
#[derive(Debug, Clone, PartialEq)]
pub enum ChromeAction {
    /// Open URL in active tab.
    Navigate(String),
    /// Open URL in new tab.
    NewTab(Option<String>),
    CloseTab(u32),
    SwitchTab(u32),
    GoBack,
    GoForward,
    Reload,
    HardReload,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleFind,
    FindNext,
    FindPrev,
    /// Scroll the active page.
    Scroll { dx: f32, dy: f32 },
    /// Click at viewport-relative coordinates (below chrome).
    Click { x: f32, y: f32, button: u8 },
    AddressBarFocus,
    AddressBarType(char),
    AddressBarBackspace,
    AddressBarLeft,
    AddressBarRight,
    None,
}

#[derive(Debug)]
pub struct BrowserChrome {
    tabs:          Vec<Tab>,
    active_tab:    u32,
    next_tab_id:   u32,
    pub address_bar: AddressBar,
    downloads:     Vec<Download>,
    next_dl_id:    u32,
    status_text:   String,
    find_open:     bool,
}

impl BrowserChrome {
    pub fn new() -> Self {
        let mut chrome = BrowserChrome {
            tabs:        Vec::new(),
            active_tab:  0,
            next_tab_id: 1,
            address_bar: AddressBar::new(),
            downloads:   Vec::new(),
            next_dl_id:  1,
            status_text: String::new(),
            find_open:   false,
        };
        chrome.new_tab(Some("about:newtab"));
        chrome
    }

    // ── Tab management ───────────────────────────────────────────────────────

    pub fn new_tab(&mut self, url: Option<&str>) -> u32 {
        let id  = self.next_tab_id;
        self.next_tab_id += 1;
        let tab = Tab::new(id, url.unwrap_or("about:newtab"));
        self.tabs.push(tab);
        self.active_tab = id;
        self.sync_address_bar();
        id
    }

    pub fn close_tab(&mut self, id: u32) {
        self.tabs.retain(|t| t.id != id);
        if self.active_tab == id {
            self.active_tab = self.tabs.last().map(|t| t.id).unwrap_or(0);
            if self.tabs.is_empty() {
                self.new_tab(Some("about:newtab"));
            } else {
                self.sync_address_bar();
            }
        }
    }

    pub fn switch_tab(&mut self, id: u32) {
        if self.tabs.iter().any(|t| t.id == id) {
            self.active_tab = id;
            self.sync_address_bar();
        }
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == self.active_tab)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        let id = self.active_tab;
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn tabs(&self) -> &[Tab] { &self.tabs }

    // ── Navigation ──────────────────────────────────────────────────────────

    pub fn navigate(&mut self, url: &str) -> String {
        let url = normalise_url(url);
        if let Some(tab) = self.active_tab_mut() {
            tab.navigate(&url);
        }
        self.address_bar.set_url(&url);
        url
    }

    pub fn go_back(&mut self) -> Option<String> {
        let id = self.active_tab;
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            let url = tab.go_back()?;
            self.address_bar.set_url(&url);
            Some(url)
        } else { None }
    }

    pub fn go_forward(&mut self) -> Option<String> {
        let id = self.active_tab;
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            let url = tab.go_forward()?;
            self.address_bar.set_url(&url);
            Some(url)
        } else { None }
    }

    pub fn reload(&mut self) -> Option<String> {
        self.active_tab().map(|t| t.current_url().to_string())
    }

    // ── Load events (called by browser engine) ───────────────────────────────

    pub fn on_load_start(&mut self, url: &str) {
        if let Some(tab) = self.active_tab_mut() {
            tab.load_state    = TabLoadState::Loading;
            tab.load_progress = 0;
            tab.security      = SecurityLevel::from_url(url);
        }
    }

    pub fn on_load_progress(&mut self, pct: u8) {
        if let Some(tab) = self.active_tab_mut() {
            tab.load_progress = pct;
        }
    }

    pub fn on_load_complete(&mut self, title: &str) {
        if let Some(tab) = self.active_tab_mut() { tab.set_loaded(title); }
    }

    pub fn on_load_error(&mut self, msg: &str) {
        if let Some(tab) = self.active_tab_mut() { tab.set_error(msg); }
    }

    pub fn set_status(&mut self, text: &str) { self.status_text = text.to_string(); }

    // ── Zoom ────────────────────────────────────────────────────────────────

    pub fn zoom_in(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.zoom_pct = (tab.zoom_pct + 10).min(500);
        }
    }
    pub fn zoom_out(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.zoom_pct = tab.zoom_pct.saturating_sub(10).max(10);
        }
    }
    pub fn zoom_reset(&mut self) {
        if let Some(tab) = self.active_tab_mut() { tab.zoom_pct = 100; }
    }
    pub fn zoom_factor(&self) -> f32 {
        self.active_tab().map(|t| t.zoom_pct as f32 / 100.0).unwrap_or(1.0)
    }

    // ── Find ────────────────────────────────────────────────────────────────

    pub fn toggle_find(&mut self) { self.find_open = !self.find_open; }
    pub fn find_open(&self) -> bool { self.find_open }

    pub fn set_find_results(&mut self, matches: u32) {
        if let Some(tab) = self.active_tab_mut() {
            tab.find_matches = matches;
            tab.find_current = if matches > 0 { 1 } else { 0 };
        }
    }

    pub fn find_next(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            if tab.find_matches > 0 {
                tab.find_current = tab.find_current % tab.find_matches + 1;
            }
        }
    }
    pub fn find_prev(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            if tab.find_matches > 0 {
                tab.find_current = if tab.find_current <= 1 { tab.find_matches } else { tab.find_current - 1 };
            }
        }
    }

    // ── Downloads ───────────────────────────────────────────────────────────

    pub fn start_download(&mut self, url: &str, filename: &str, mime: &str) -> u32 {
        let id = self.next_dl_id;
        self.next_dl_id += 1;
        self.downloads.push(Download::new(id, url, filename, mime));
        id
    }

    pub fn update_download(&mut self, id: u32, done: u64, total: u64) {
        if let Some(dl) = self.downloads.iter_mut().find(|d| d.id == id) {
            dl.state = DownloadState::InProgress(done, total);
        }
    }

    pub fn complete_download(&mut self, id: u32) {
        if let Some(dl) = self.downloads.iter_mut().find(|d| d.id == id) {
            dl.state = DownloadState::Complete;
        }
    }

    pub fn downloads(&self) -> &[Download] { &self.downloads }

    // ── Input dispatch ───────────────────────────────────────────────────────

    /// Translate a keyboard scancode + modifiers into a `ChromeAction`.
    /// `key` is a Unicode codepoint or special key code below.
    pub fn handle_key(&mut self, key: u32, ctrl: bool, alt: bool, shift: bool) -> ChromeAction {
        // Special keys (non-printable)
        match key {
            // F5 = Reload; Ctrl+Shift+R = Hard reload
            0xF005 => return if shift { ChromeAction::HardReload } else { ChromeAction::Reload },
            // Backspace in address bar
            0x0008 if self.address_bar.focused => {
                self.address_bar.backspace();
                return ChromeAction::AddressBarBackspace;
            }
            // Enter in address bar
            0x000D if self.address_bar.focused => {
                let url = self.address_bar.commit();
                return ChromeAction::Navigate(url);
            }
            // Arrow keys in address bar
            0xF100 if self.address_bar.focused => { self.address_bar.move_cursor_left(); return ChromeAction::AddressBarLeft; }
            0xF101 if self.address_bar.focused => { self.address_bar.move_cursor_right(); return ChromeAction::AddressBarRight; }
            // Escape: blur address bar / close find
            0x001B => {
                self.address_bar.focused = false;
                if self.find_open { self.find_open = false; }
                return ChromeAction::None;
            }
            _ => {}
        }

        // Ctrl+Letter combos — normalise to lowercase for comparison
        if ctrl {
            let lower = if key >= 0x41 && key <= 0x5A { key + 0x20 } else { key };
            match lower {
                0x74 => return ChromeAction::NewTab(None),  // 't'
                0x77 => { let id = self.active_tab; return ChromeAction::CloseTab(id); }  // 'w'
                0x6C => {                                   // 'l'
                    self.address_bar.focused = true;
                    self.address_bar.select_all();
                    return ChromeAction::AddressBarFocus;
                }
                0x72 => return ChromeAction::Reload,        // 'r'
                0x66 => { self.toggle_find(); return ChromeAction::ToggleFind; } // 'f'
                0x3D | 0x2B => return ChromeAction::ZoomIn,  // '=' | '+'
                0x2D         => return ChromeAction::ZoomOut, // '-'
                0x30         => return ChromeAction::ZoomReset,// '0'
                // Ctrl+1..9 = switch tab (0x31='1'..0x39='9')
                0x31..=0x39 => {
                    let idx = (lower - 0x31) as usize;
                    if let Some(tab) = self.tabs.get(idx) {
                        let id = tab.id;
                        self.switch_tab(id);
                        return ChromeAction::SwitchTab(id);
                    }
                }
                _ => {}
            }
        }

        // Alt+Left / Alt+Right = back/forward
        if alt {
            match key {
                0xF100 => return ChromeAction::GoBack,
                0xF101 => return ChromeAction::GoForward,
                _ => {}
            }
        }

        // Printable character in address bar
        if self.address_bar.focused {
            if let Some(c) = char::from_u32(key) {
                if !c.is_control() {
                    self.address_bar.type_char(c);
                    return ChromeAction::AddressBarType(c);
                }
            }
        }

        ChromeAction::None
    }

    /// Handle a mouse click in the chrome area.
    /// Returns a `ChromeAction` if the click was consumed.
    pub fn handle_mouse(&mut self, x: u32, y: u32, viewport_w: u32, button: u8) -> ChromeAction {
        if y < TAB_HEIGHT {
            return self.handle_tab_click(x, viewport_w, button);
        }
        if y < CHROME_HEIGHT {
            return self.handle_toolbar_click(x, y - TAB_HEIGHT, viewport_w, button);
        }
        ChromeAction::None
    }

    fn handle_tab_click(&mut self, x: u32, viewport_w: u32, _button: u8) -> ChromeAction {
        // Tab widths: pinned=48px, others = evenly share remaining width, max 200px
        let (pinned_count, normal_count) = {
            let p = self.tabs.iter().filter(|t| t.is_pinned).count() as u32;
            (p, self.tabs.len() as u32 - p)
        };
        let pinned_w: u32 = 48;
        let normal_w: u32 = if normal_count == 0 { 0 }
            else { ((viewport_w - pinned_count * pinned_w).min(normal_count * 200)) / normal_count };

        let mut cx: u32 = 0;
        for tab in &self.tabs.clone() {
            let w = if tab.is_pinned { pinned_w } else { normal_w };
            // Close button at right edge of tab (16px wide)
            if x >= cx + w.saturating_sub(16) && x < cx + w {
                let id = tab.id;
                self.close_tab(id);
                return ChromeAction::CloseTab(id);
            }
            if x >= cx && x < cx + w {
                let id = tab.id;
                self.switch_tab(id);
                return ChromeAction::SwitchTab(id);
            }
            cx += w;
        }
        // Click after all tabs → new tab
        ChromeAction::NewTab(None)
    }

    fn handle_toolbar_click(&mut self, x: u32, _y: u32, viewport_w: u32, _button: u8) -> ChromeAction {
        // Layout:  [←][→][↺]  [===address bar===]  [☆]  [≡]
        if x < 40 {
            if let Some(url) = self.go_back() { return ChromeAction::Navigate(url); }
            return ChromeAction::GoBack;
        }
        if x < 80 {
            if let Some(url) = self.go_forward() { return ChromeAction::Navigate(url); }
            return ChromeAction::GoForward;
        }
        if x < 120 { return ChromeAction::Reload; }
        // Address bar region
        if x < viewport_w.saturating_sub(80) {
            self.address_bar.focused = true;
            self.address_bar.select_all();
            return ChromeAction::AddressBarFocus;
        }
        ChromeAction::None
    }

    // ── Download bar ─────────────────────────────────────────────────────────

    /// Paint rects for the download shelf (rendered at the bottom of viewport).
    /// `viewport_y_bottom` is the y-coordinate of the bottom of the content area.
    pub fn download_bar_rects(&self, viewport_w: u32, viewport_y_bottom: u32)
        -> Vec<ChromePaintRect>
    {
        let active: Vec<&Download> = self.downloads.iter()
            .filter(|d| !matches!(d.state, DownloadState::Complete | DownloadState::Cancelled))
            .collect();
        if active.is_empty() { return Vec::new(); }

        let mut rects = Vec::new();
        // Shelf background
        let shelf_h = DOWNLOAD_BAR_HEIGHT * active.len() as u32;
        let shelf_y = viewport_y_bottom.saturating_sub(shelf_h);
        rects.push(ChromePaintRect::fill(0, shelf_y, viewport_w, shelf_h, 0xFF_1E1E2E));
        // Top border
        rects.push(ChromePaintRect::fill(0, shelf_y, viewport_w, 1, 0xFF_4D9FFF));

        for (i, dl) in active.iter().enumerate() {
            let row_y = shelf_y + i as u32 * DOWNLOAD_BAR_HEIGHT;
            let pct = dl.progress_pct() as u32;

            // Filename label area background
            rects.push(ChromePaintRect::fill(0, row_y, viewport_w, DOWNLOAD_BAR_HEIGHT, 0xFF_252535));

            // Progress fill
            let pw = viewport_w * pct / 100;
            rects.push(ChromePaintRect::fill(0, row_y + DOWNLOAD_BAR_HEIGHT - 4, pw, 4, 0xFF_4D9FFF));

            // State indicator dot
            let dot_color = match &dl.state {
                DownloadState::Pending        => 0xFF_AAAAAA,
                DownloadState::InProgress(..) => 0xFF_4D9FFF,
                DownloadState::Failed(_)      => 0xFF_FF4444,
                _ => 0xFF_44CC44,
            };
            rects.push(ChromePaintRect::fill(8, row_y + 10, 12, 12, dot_color));

            // Filename text
            rects.push(ChromePaintRect::text(28, row_y + 8, &dl.filename, 0xFF_EEEEEE));

            // Percentage text
            let pct_text = format!("{}%", pct);
            rects.push(ChromePaintRect::text(viewport_w.saturating_sub(60), row_y + 8, &pct_text, 0xFF_AAAAAA));

            // MIME label
            if !dl.mime.is_empty() {
                rects.push(ChromePaintRect::text(
                    viewport_w / 2,
                    row_y + 8,
                    &dl.mime,
                    0xFF_777799,
                ));
            }
        }

        rects
    }

    /// Security badge rects for the address bar area (overlaid inside the bar).
    pub fn security_badge_rects(&self, bar_x: u32, bar_y: u32) -> Vec<ChromePaintRect> {
        let mut rects = Vec::new();
        if let Some(tab) = self.active_tab() {
            let bg  = tab.security.badge_color();
            let fg  = tab.security.badge_fg();
            let txt = tab.security.badge_text();
            // Badge pill: 80px wide, 20px tall, 4px from left inside the bar
            rects.push(ChromePaintRect::fill(bar_x + 4, bar_y + 4, 80, 20, bg));
            rects.push(ChromePaintRect::text(bar_x + 8, bar_y + 6, txt, fg));
        }
        rects
    }

    // ── Sync helpers ─────────────────────────────────────────────────────────

    fn sync_address_bar(&mut self) {
        let url = self.active_tab().map(|t| t.current_url().to_string())
                                   .unwrap_or_default();
        self.address_bar.set_url(&url);
    }

    // ── Render (returns a simple text description for serial debug) ───────────

    pub fn render_debug(&self) -> alloc::string::String {
        let tab = match self.active_tab() { Some(t) => t, None => return "no tabs".to_string() };
        let state = match &tab.load_state {
            TabLoadState::Idle      => "idle",
            TabLoadState::Loading   => "loading",
            TabLoadState::Complete  => "done",
            TabLoadState::Error(_)  => "err",
        };
        format!("[{}] {} | {} | {}% | zoom={}%",
            tab.id, self.address_bar.text,
            state, tab.load_progress, tab.zoom_pct)
    }

    /// Describe the chrome layout as paint rects for the compositor.
    pub fn paint_rects(&self, viewport_w: u32) -> Vec<ChromePaintRect> {
        let mut rects = Vec::new();

        // Tab bar background
        rects.push(ChromePaintRect::fill(0, 0, viewport_w, TAB_HEIGHT, 0xFF_2D2D2D));

        // Tabs
        let (pinned_count, normal_count) = {
            let p = self.tabs.iter().filter(|t| t.is_pinned).count() as u32;
            (p, self.tabs.len() as u32 - p)
        };
        let pinned_w: u32 = 48;
        let normal_w: u32 = if normal_count == 0 { 0 }
            else { ((viewport_w - pinned_count * pinned_w).min(normal_count * 200)) / normal_count };

        let mut cx: u32 = 0;
        for tab in &self.tabs {
            let w = if tab.is_pinned { pinned_w } else { normal_w };
            let is_active = tab.id == self.active_tab;
            let bg = if is_active { 0xFF_3C3C3C } else { 0xFF_252525 };
            rects.push(ChromePaintRect::fill(cx, 0, w.saturating_sub(1), TAB_HEIGHT, bg));
            // Favicon dot
            rects.push(ChromePaintRect::fill(cx + 8, 8, 16, 16, tab.favicon_color));
            // Active tab indicator line
            if is_active {
                rects.push(ChromePaintRect::fill(cx, TAB_HEIGHT - 2, w.saturating_sub(1), 2, 0xFF_4D9FFF));
            }
            cx += w;
        }
        // New-tab button
        rects.push(ChromePaintRect::fill(cx, 4, 24, TAB_HEIGHT - 8, 0xFF_404040));

        // Toolbar background
        rects.push(ChromePaintRect::fill(0, TAB_HEIGHT, viewport_w, TOOLBAR_HEIGHT, 0xFF_3C3C3C));

        // Back / Forward / Reload buttons
        let can_back = self.active_tab().map(|t| t.history.can_go_back()).unwrap_or(false);
        let can_fwd  = self.active_tab().map(|t| t.history.can_go_forward()).unwrap_or(false);
        let btn_back = if can_back { 0xFF_EEEEEE } else { 0xFF_888888 };
        let btn_fwd  = if can_fwd  { 0xFF_EEEEEE } else { 0xFF_888888 };
        rects.push(ChromePaintRect::fill(4,  TAB_HEIGHT + 8, 28, 24, btn_back));
        rects.push(ChromePaintRect::fill(36, TAB_HEIGHT + 8, 28, 24, btn_fwd));
        rects.push(ChromePaintRect::fill(68, TAB_HEIGHT + 8, 28, 24, 0xFF_EEEEEE)); // reload

        // Address bar
        let bar_x = 100u32;
        let bar_w = viewport_w.saturating_sub(160);
        let bar_bg = if self.address_bar.focused { 0xFF_1A1A2E } else { 0xFF_2A2A2A };
        rects.push(ChromePaintRect::fill(bar_x, TAB_HEIGHT + 6, bar_w, 28, bar_bg));

        // Security badge (pill inside address bar)
        let badge_rects = self.security_badge_rects(bar_x, TAB_HEIGHT + 6);
        rects.extend(badge_rects);

        // URL text (after badge, 90px offset)
        let url_text = self.address_bar.text.clone();
        rects.push(ChromePaintRect::text(bar_x + 94, TAB_HEIGHT + 14, &url_text, 0xFF_CCCCCC));

        // Progress bar (if loading)
        if let Some(tab) = self.active_tab() {
            if tab.load_state == TabLoadState::Loading && tab.load_progress < 100 {
                let pw = bar_w * tab.load_progress as u32 / 100;
                rects.push(ChromePaintRect::fill(bar_x, TAB_HEIGHT + TOOLBAR_HEIGHT - 3, pw, 3, 0xFF_4D9FFF));
            }
        }

        rects
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Paint rect (output of chrome rendering)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ChromePaintRect {
    pub x: u32, pub y: u32, pub w: u32, pub h: u32,
    pub color: u32, // ARGB
    pub kind: ChromeRectKind,
}

#[derive(Debug, Clone)]
pub enum ChromeRectKind {
    Fill,
    Text(String),
    Border,
}

impl ChromePaintRect {
    pub fn fill(x: u32, y: u32, w: u32, h: u32, color: u32) -> Self {
        ChromePaintRect { x, y, w, h, color, kind: ChromeRectKind::Fill }
    }
    pub fn text(x: u32, y: u32, text: &str, color: u32) -> Self {
        ChromePaintRect { x, y, w: 0, h: 0, color, kind: ChromeRectKind::Text(text.to_string()) }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[chrome] Browser chrome ready (Phase 103 polish — security badge + download bar).");
}

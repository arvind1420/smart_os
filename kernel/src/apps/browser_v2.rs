//! Browser v2 — Phase 67: Enhanced Browser Features (v0.27.0).
//!
//! Adds four companion panels to the web browsing stack:
//!
//! ┌ Bookmarks ─────────────────────────────────────────────────────────┐
//! │ Save / delete / pin / categorise bookmarks.                        │
//! │ Categories: Work · Personal · Tech · Misc                          │
//! └────────────────────────────────────────────────────────────────────┘
//! ┌ Downloads ─────────────────────────────────────────────────────────┐
//! │ State machine: Queued → InProgress(pct) → Complete / Failed.       │
//! │ Simulated progress ticks; "Clear done" removes finished entries.   │
//! └────────────────────────────────────────────────────────────────────┘
//! ┌ History ───────────────────────────────────────────────────────────┐
//! │ Chronological visit log with title, URL, timestamp.                │
//! │ Incremental substring search; "Clear" wipes the log.              │
//! └────────────────────────────────────────────────────────────────────┘
//! ┌ Reader ────────────────────────────────────────────────────────────┐
//! │ Reading-mode HTML → text extractor (strip tags, decode entities).  │
//! │ Estimates reading time; renders clean article view.                │
//! └────────────────────────────────────────────────────────────────────┘

#![allow(dead_code)]

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::window::{Window, WindowId};
use crate::gui::desktop::DESKTOP;
use crate::gui::theme::*;
use crate::gui::widget::{
    Widget, WidgetKind, Button, StaticLabel, ScrollableText, AppCommand, WidgetAction,
};

// ════════════════════════════════════════════════════════════════════════════
//  BOOKMARKS
// ════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BmCategory { Work, Personal, Tech, Misc }

impl BmCategory {
    fn name(self) -> &'static str {
        match self { Self::Work => "Work", Self::Personal => "Personal",
                     Self::Tech => "Tech", Self::Misc => "Misc" }
    }
    fn color(self) -> Color {
        match self { Self::Work => ACCENT_CYAN, Self::Personal => ACCENT_GREEN,
                     Self::Tech => ACCENT_ORANGE, Self::Misc => TEXT_SECONDARY }
    }
}

#[derive(Clone)]
pub struct Bookmark {
    pub id:       u32,
    pub title:    String,
    pub url:      String,
    pub category: BmCategory,
    pub pinned:   bool,
    pub visits:   u32,
}

impl Bookmark {
    fn new(id: u32, title: &str, url: &str, cat: BmCategory, pinned: bool) -> Self {
        Self { id, title: title.into(), url: url.into(), category: cat, pinned, visits: 0 }
    }
}

pub struct BookmarkStore {
    pub items: Vec<Bookmark>,
    pub next_id: u32,
}

impl BookmarkStore {
    fn new() -> Self {
        let mut s = Self { items: Vec::new(), next_id: 1 };
        s.seed();
        s
    }

    fn seed(&mut self) {
        let demos: &[(&str, &str, BmCategory, bool)] = &[
            ("Smart OS Homepage",  "https://smartos.local/",         BmCategory::Tech,     true),
            ("Kernel Docs",        "https://smartos.local/docs",     BmCategory::Tech,     true),
            ("Rust Reference",     "https://doc.rust-lang.org/",     BmCategory::Tech,     false),
            ("Work Dashboard",     "https://work.example.com/",      BmCategory::Work,     true),
            ("Project Tracker",    "https://tracker.example.com/",   BmCategory::Work,     false),
            ("Personal Blog",      "https://blog.example.com/",      BmCategory::Personal, false),
            ("News Feed",          "https://news.example.com/",      BmCategory::Misc,     false),
        ];
        for (t, u, c, p) in demos {
            let id = self.next_id; self.next_id += 1;
            self.items.push(Bookmark::new(id, t, u, *c, *p));
        }
    }

    pub fn add(&mut self, title: &str, url: &str, cat: BmCategory) -> u32 {
        let id = self.next_id; self.next_id += 1;
        self.items.push(Bookmark::new(id, title, url, cat, false));
        id
    }

    pub fn remove(&mut self, id: u32) { self.items.retain(|b| b.id != id); }

    pub fn toggle_pin(&mut self, id: u32) {
        if let Some(b) = self.items.iter_mut().find(|b| b.id == id) { b.pinned = !b.pinned; }
    }

    pub fn record_visit(&mut self, id: u32) {
        if let Some(b) = self.items.iter_mut().find(|b| b.id == id) { b.visits += 1; }
    }

    fn render(&self, selected: Option<u32>) -> Vec<(String, Color)> {
        let mut lines = Vec::new();
        // Pinned first, then by category
        let mut pinned: Vec<&Bookmark> = self.items.iter().filter(|b| b.pinned).collect();
        let mut rest:   Vec<&Bookmark> = self.items.iter().filter(|b| !b.pinned).collect();
        pinned.sort_by(|a, b| a.title.cmp(&b.title));
        rest.sort_by(|a, b| a.category.name().cmp(b.category.name()).then(a.title.cmp(&b.title)));

        if !pinned.is_empty() {
            lines.push((String::from("  📌 Pinned"), ACCENT_CYAN));
            for bm in &pinned {
                let sel = if selected == Some(bm.id) { "▶ " } else { "  " };
                let pin = if bm.pinned { "📌 " } else { "   " };
                lines.push((format!("{}{}{:<36} {}", sel, pin, bm.title, bm.url), bm.category.color()));
            }
            lines.push((String::new(), TEXT_PRIMARY));
        }

        let mut last_cat: Option<BmCategory> = None;
        for bm in &rest {
            if last_cat != Some(bm.category) {
                lines.push((format!("  ── {} ──────────────", bm.category.name()), bm.category.color()));
                last_cat = Some(bm.category);
            }
            let sel = if selected == Some(bm.id) { "▶ " } else { "  " };
            lines.push((format!("{}  {:<36} {} ({}×)", sel, bm.title, bm.url, bm.visits), TEXT_PRIMARY));
        }
        lines
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  DOWNLOADS
// ════════════════════════════════════════════════════════════════════════════

#[derive(Clone, PartialEq, Debug)]
pub enum DlState {
    Queued,
    InProgress(u8),   // 0–100 %
    Complete,
    Failed(String),
}

#[derive(Clone)]
pub struct Download {
    pub id:        u32,
    pub filename:  String,
    pub url:       String,
    pub size_kb:   u32,
    pub state:     DlState,
    pub tick:      u32, // internal progress timer
}

impl Download {
    fn new(id: u32, filename: &str, url: &str, size_kb: u32) -> Self {
        Self { id, filename: filename.into(), url: url.into(), size_kb, state: DlState::Queued, tick: 0 }
    }

    /// Advance simulated download progress by one tick (~100ms).
    pub fn advance(&mut self) {
        match &self.state {
            DlState::Queued => { self.state = DlState::InProgress(0); }
            DlState::InProgress(pct) => {
                self.tick += 1;
                let new_pct = (self.tick * 3).min(100) as u8;
                if new_pct >= 100 { self.state = DlState::Complete; }
                else { self.state = DlState::InProgress(new_pct); }
            }
            _ => {}
        }
    }

    fn render_line(&self, selected: bool) -> (String, Color) {
        let sel = if selected { "▶ " } else { "  " };
        let (state_str, color) = match &self.state {
            DlState::Queued        => (String::from("  [queued  ]"), TEXT_SECONDARY),
            DlState::InProgress(p) => {
                let bar: String = (0..20).map(|i| if i < (*p as usize / 5) { '█' } else { '░' }).collect();
                (format!(" [{}] {:>3}%", bar, p), ACCENT_CYAN)
            }
            DlState::Complete      => (String::from("  [✓ done  ]"), ACCENT_GREEN),
            DlState::Failed(e)     => (format!("  [✗ {}]", e), ACCENT_RED),
        };
        (format!("{}{:<24} {:>6} KB {}", sel, self.filename, self.size_kb, state_str), color)
    }
}

pub struct DownloadManager {
    pub items:   Vec<Download>,
    pub next_id: u32,
}

impl DownloadManager {
    fn new() -> Self {
        let mut dm = Self { items: Vec::new(), next_id: 1 };
        dm.seed();
        dm
    }

    fn seed(&mut self) {
        let demos: &[(&str, &str, u32)] = &[
            ("smart-os-v0.27.0.iso", "https://smartos.local/iso",  512_000),
            ("kernel-docs.pdf",      "https://smartos.local/docs",  2_048),
            ("demo-video.smv",       "https://smartos.local/media", 8_192),
        ];
        for (name, url, size) in demos {
            let id = self.next_id; self.next_id += 1;
            let mut dl = Download::new(id, name, url, *size);
            // Seed first as complete, second in-progress, third queued
            match id {
                1 => dl.state = DlState::Complete,
                2 => { dl.state = DlState::InProgress(47); dl.tick = 15; }
                _ => {}
            }
            self.items.push(dl);
        }
    }

    pub fn add(&mut self, filename: &str, url: &str, size_kb: u32) -> u32 {
        let id = self.next_id; self.next_id += 1;
        self.items.push(Download::new(id, filename, url, size_kb));
        id
    }

    pub fn clear_done(&mut self) {
        self.items.retain(|d| !matches!(d.state, DlState::Complete | DlState::Failed(_)));
    }

    /// Advance all in-progress downloads by one tick.
    pub fn tick_all(&mut self) {
        for dl in self.items.iter_mut() { dl.advance(); }
    }

    fn render(&self, selected: Option<u32>) -> Vec<(String, Color)> {
        if self.items.is_empty() {
            return vec![(String::from("  (no downloads)"), TEXT_SECONDARY)];
        }
        self.items.iter()
            .map(|dl| dl.render_line(selected == Some(dl.id)))
            .collect()
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  HISTORY
// ════════════════════════════════════════════════════════════════════════════

#[derive(Clone)]
pub struct HistoryEntry {
    pub id:        u32,
    pub url:       String,
    pub title:     String,
    pub timestamp: String,
}

pub struct BrowserHistory {
    pub entries: Vec<HistoryEntry>,
    pub next_id: u32,
    pub search:  String,
}

impl BrowserHistory {
    fn new() -> Self {
        let mut h = Self { entries: Vec::new(), next_id: 1, search: String::new() };
        h.seed();
        h
    }

    fn seed(&mut self) {
        let demos: &[(&str, &str, &str)] = &[
            ("https://smartos.local/",       "Smart OS",           "2026-05-31 09:00"),
            ("https://smartos.local/docs",   "Kernel Docs",        "2026-05-31 09:05"),
            ("https://doc.rust-lang.org/",   "Rust Reference",     "2026-05-31 09:30"),
            ("https://smartos.local/apps",   "App Store",          "2026-05-31 10:00"),
            ("https://news.example.com/",    "News Feed",          "2026-05-31 11:00"),
            ("https://tracker.example.com/", "Project Tracker",    "2026-05-31 11:45"),
            ("https://blog.example.com/",    "Personal Blog",      "2026-05-31 12:30"),
        ];
        for (url, title, ts) in demos {
            self.push(url, title, ts);
        }
    }

    pub fn push(&mut self, url: &str, title: &str, timestamp: &str) {
        let id = self.next_id; self.next_id += 1;
        self.entries.push(HistoryEntry { id, url: url.into(), title: title.into(), timestamp: timestamp.into() });
    }

    pub fn clear(&mut self) { self.entries.clear(); }

    /// Returns entries matching `self.search` (case-insensitive substring).
    fn filtered(&self) -> Vec<&HistoryEntry> {
        if self.search.is_empty() {
            self.entries.iter().rev().collect()
        } else {
            let q = self.search.to_lowercase();
            self.entries.iter().rev()
                .filter(|e| e.url.to_lowercase().contains(q.as_str())
                         || e.title.to_lowercase().contains(q.as_str()))
                .collect()
        }
    }

    fn render(&self, selected: Option<u32>) -> Vec<(String, Color)> {
        let items = self.filtered();
        if items.is_empty() {
            return vec![(String::from("  (no history)"), TEXT_SECONDARY)];
        }
        let mut lines = Vec::new();
        if !self.search.is_empty() {
            lines.push((format!("  🔍 Filter: \"{}\"  ({} results)", self.search, items.len()), ACCENT_CYAN));
        }
        for e in items {
            let sel = if selected == Some(e.id) { "▶ " } else { "  " };
            lines.push((
                format!("{}  {} | {:<36} {}", sel, e.timestamp, e.title, e.url),
                TEXT_PRIMARY,
            ));
        }
        lines
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  READING MODE — HTML → clean text
// ════════════════════════════════════════════════════════════════════════════

/// Strip HTML tags and decode common entities from `html`.
pub fn extract_readable(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut in_script = false;
    let mut buf = String::new();

    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        if in_tag {
            buf.push(ch);
            if ch == '>' {
                // Check if entering/leaving script/style
                let tag = buf.to_lowercase();
                if tag.starts_with("<script") || tag.starts_with("<style") { in_script = true; }
                if tag.starts_with("</script") || tag.starts_with("</style") { in_script = false; }
                // Block tags → newline
                if matches_block_tag(&buf) { out.push('\n'); }
                in_tag = false;
                buf.clear();
            }
        } else if ch == '<' {
            in_tag = true;
            buf.clear();
            buf.push('<');
        } else if !in_script {
            if ch == '&' {
                // Entity decode
                let start = i;
                let end = bytes[i..].iter().position(|&b| b == b';').map(|p| i + p + 1).unwrap_or(i + 1);
                let entity = &html[start..end.min(html.len())];
                out.push_str(decode_entity(entity));
                i = end;
                continue;
            } else {
                out.push(ch);
            }
        }
        i += 1;
    }
    out
}

fn matches_block_tag(tag: &str) -> bool {
    let t = tag.to_lowercase();
    let t = t.trim_start_matches('<').trim_start_matches('/');
    matches!(t.split_once(|c: char| !c.is_alphabetic()).map(|(t,_)| t).unwrap_or(t),
        "p"|"div"|"br"|"h1"|"h2"|"h3"|"h4"|"h5"|"h6"|"li"|"tr"|"blockquote"|"pre"|"article"|"section")
}

fn decode_entity(entity: &str) -> &'static str {
    match entity {
        "&amp;"  => "&",  "&lt;"  => "<",  "&gt;"  => ">",
        "&quot;" => "\"", "&apos;" => "'", "&nbsp;" => " ",
        "&mdash;" => "—", "&ndash;" => "–", "&hellip;" => "…",
        "&copy;"  => "©", "&reg;"  => "®", "&trade;" => "™",
        _ => " ",
    }
}

/// Estimate reading time (words / 200 wpm → minutes).
pub fn estimate_reading_time(text: &str) -> u32 {
    let words = text.split_whitespace().count();
    ((words as u32 + 199) / 200).max(1)
}

/// Render clean reader lines from HTML source.
pub fn render_reader(html: &str, url: &str) -> Vec<(String, Color)> {
    let text = extract_readable(html);
    let mins = estimate_reading_time(&text);
    let mut lines = Vec::new();
    lines.push((format!("  📖  Reading Mode  |  {} min read  |  {}", mins, url), ACCENT_CYAN));
    lines.push((String::from("  ─────────────────────────────────────────────────────────"), BORDER_INACTIVE));
    lines.push((String::new(), TEXT_PRIMARY));

    for para in text.lines() {
        let p = para.trim();
        if p.is_empty() { lines.push((String::new(), TEXT_PRIMARY)); continue; }
        // Wrap at ~80 chars
        let mut rem = p;
        while !rem.is_empty() {
            if rem.len() <= 80 {
                lines.push((format!("  {}", rem), TEXT_PRIMARY));
                break;
            }
            let split = rem[..80].rfind(' ').unwrap_or(80);
            lines.push((format!("  {}", &rem[..split]), TEXT_PRIMARY));
            rem = rem[split..].trim_start();
        }
    }
    lines
}

// ════════════════════════════════════════════════════════════════════════════
//  App state
// ════════════════════════════════════════════════════════════════════════════

#[derive(PartialEq, Clone, Copy)]
pub enum BrowserV2View { Bookmarks, Downloads, History, Reader }

pub struct BrowserV2State {
    pub window_id:   WindowId,
    pub view:        BrowserV2View,
    pub bookmarks:   BookmarkStore,
    pub downloads:   DownloadManager,
    pub history:     BrowserHistory,
    pub reader_html: String,
    pub reader_url:  String,
    pub selected_bm: Option<u32>,
    pub selected_dl: Option<u32>,
    pub selected_hs: Option<u32>,
    pub dl_tick:     u64,   // timer for download simulation
    pub status:      String,
}

pub static STATE: Mutex<Option<BrowserV2State>> = Mutex::new(None);

// ─── Content builder ─────────────────────────────────────────────────────────

fn build_content(s: &BrowserV2State) -> Vec<(String, Color)> {
    match s.view {
        BrowserV2View::Bookmarks => s.bookmarks.render(s.selected_bm),
        BrowserV2View::Downloads => s.downloads.render(s.selected_dl),
        BrowserV2View::History   => s.history.render(s.selected_hs),
        BrowserV2View::Reader    => render_reader(&s.reader_html, &s.reader_url),
    }
}

// ─── App thread ──────────────────────────────────────────────────────────────

pub fn run() {
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() { Some(d) => d, None => return };

        let mut win = crate::gui::window::Window::new("Browser v2", 80, 55, 760, 520, ACCENT_ORANGE);
        win.use_widgets = true;

        win.widgets.push(Widget::new(0, 0,   0, 88, 22,
            WidgetKind::Button(Button::new("Bookmarks", ACCENT_CYAN,    AppCommand::ButtonClicked(0)))));
        win.widgets.push(Widget::new(1, 92,  0, 88, 22,
            WidgetKind::Button(Button::new("Downloads", ACCENT_GREEN,   AppCommand::ButtonClicked(1)))));
        win.widgets.push(Widget::new(2, 184, 0, 72, 22,
            WidgetKind::Button(Button::new("History",   ACCENT_ORANGE,  AppCommand::ButtonClicked(2)))));
        win.widgets.push(Widget::new(3, 260, 0, 72, 22,
            WidgetKind::Button(Button::new("Reader",    ACCENT_MAGENTA, AppCommand::ButtonClicked(3)))));
        // Action buttons (context-sensitive)
        win.widgets.push(Widget::new(4, 336, 0, 64, 22,
            WidgetKind::Button(Button::new("Add/Clear", ACCENT_CYAN,    AppCommand::ButtonClicked(4)))));
        win.widgets.push(Widget::new(5, 404, 0, 60, 22,
            WidgetKind::Button(Button::new("Delete",    ACCENT_RED,     AppCommand::ButtonClicked(5)))));
        win.widgets.push(Widget::new(6, 468, 0, 52, 22,
            WidgetKind::Button(Button::new("Pin",       ACCENT_ORANGE,  AppCommand::ButtonClicked(6)))));
        // Status label
        win.widgets.push(Widget::new(7, 0, 25, 760, 16,
            WidgetKind::Label(StaticLabel::new("Bookmarks", TEXT_SECONDARY))));
        // Content area
        win.widgets.push(Widget::new(8, 0, 44, 760, 476,
            WidgetKind::ScrollText(ScrollableText::new(4000))));

        win.focused_widget = Some(8);
        let id = win.id;
        desk.wm.add(win);
        id
    };

    let reader_html = String::from(DEMO_ARTICLE_HTML);

    *STATE.lock() = Some(BrowserV2State {
        window_id,
        view:        BrowserV2View::Bookmarks,
        bookmarks:   BookmarkStore::new(),
        downloads:   DownloadManager::new(),
        history:     BrowserHistory::new(),
        reader_html,
        reader_url:  String::from("https://smartos.local/article"),
        selected_bm: None,
        selected_dl: None,
        selected_hs: None,
        dl_tick:     0,
        status:      String::from("Bookmarks — 7 saved"),
    });

    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                match action {
                    // Tab switches
                    WidgetAction::Execute(AppCommand::ButtonClicked(0)) => { s.view = BrowserV2View::Bookmarks; }
                    WidgetAction::Execute(AppCommand::ButtonClicked(1)) => { s.view = BrowserV2View::Downloads; }
                    WidgetAction::Execute(AppCommand::ButtonClicked(2)) => { s.view = BrowserV2View::History; }
                    WidgetAction::Execute(AppCommand::ButtonClicked(3)) => { s.view = BrowserV2View::Reader; }
                    // Add / Clear (context-sensitive)
                    WidgetAction::Execute(AppCommand::ButtonClicked(4)) => {
                        match s.view {
                            BrowserV2View::Bookmarks => {
                                s.bookmarks.add("New Bookmark", "https://example.com/", BmCategory::Misc);
                            }
                            BrowserV2View::Downloads => { s.downloads.clear_done(); }
                            BrowserV2View::History   => { s.history.clear(); }
                            BrowserV2View::Reader    => {}
                        }
                    }
                    // Delete selected
                    WidgetAction::Execute(AppCommand::ButtonClicked(5)) => {
                        match s.view {
                            BrowserV2View::Bookmarks => {
                                if let Some(id) = s.selected_bm { s.bookmarks.remove(id); s.selected_bm = None; }
                            }
                            BrowserV2View::Downloads => {
                                if let Some(id) = s.selected_dl {
                                    s.downloads.items.retain(|d| d.id != id); s.selected_dl = None;
                                }
                            }
                            _ => {}
                        }
                    }
                    // Pin bookmark
                    WidgetAction::Execute(AppCommand::ButtonClicked(6)) => {
                        if s.view == BrowserV2View::Bookmarks {
                            if let Some(id) = s.selected_bm { s.bookmarks.toggle_pin(id); }
                        }
                    }
                    // Line click → select item
                    WidgetAction::Execute(AppCommand::LineClicked(row)) => {
                        match s.view {
                            BrowserV2View::Bookmarks => {
                                let items: Vec<u32> = s.bookmarks.items.iter().map(|b| b.id).collect();
                                if let Some(&id) = items.get(row) {
                                    s.selected_bm = Some(id);
                                    s.bookmarks.record_visit(id);
                                }
                            }
                            BrowserV2View::Downloads => {
                                if let Some(dl) = s.downloads.items.get(row) { s.selected_dl = Some(dl.id); }
                            }
                            BrowserV2View::History => {
                                let ids: Vec<u32> = s.history.filtered().iter().map(|e| e.id).collect();
                                if let Some(&id) = ids.get(row) { s.selected_hs = Some(id); }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }

                // Update status line
                s.status = match s.view {
                    BrowserV2View::Bookmarks => format!("Bookmarks — {} saved", s.bookmarks.items.len()),
                    BrowserV2View::Downloads => {
                        let done = s.downloads.items.iter().filter(|d| d.state == DlState::Complete).count();
                        format!("Downloads — {}/{} complete", done, s.downloads.items.len())
                    }
                    BrowserV2View::History => format!("History — {} entries", s.history.entries.len()),
                    BrowserV2View::Reader  => String::from("Reader — clean article view"),
                };
            }
        }

        // Tick downloads every ~30 frames
        {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                s.dl_tick += 1;
                if s.dl_tick % 30 == 0 { s.downloads.tick_all(); }
            }
        }

        crate::process::scheduler::yield_now();
    }
}

// ─── Sync to window ──────────────────────────────────────────────────────────

pub fn sync_to_window(win: &mut Window) {
    let (status, lines) = {
        let guard = STATE.lock();
        let s = match guard.as_ref() { Some(x) => x, None => return };
        (s.status.clone(), build_content(s))
    };
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 7) {
        if let WidgetKind::Label(ref mut lbl) = w.kind { lbl.text = status; }
    }
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 8) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = lines; }
    }
    win.dirty = true;
}

// ─── Demo article ─────────────────────────────────────────────────────────────

const DEMO_ARTICLE_HTML: &str = "\
<html><body>\
<h1>Smart OS v0.27.0 Released</h1>\
<p>The Smart OS team is pleased to announce version 0.27.0, featuring the new \
Browser v2 with bookmarks, downloads, history, and reading mode.</p>\
<h2>What&apos;s New</h2>\
<p>Browser v2 introduces a comprehensive companion panel alongside the main \
browsing engine. Users can now manage bookmarks across four categories &mdash; \
Work, Personal, Tech, and Misc &mdash; with pin support for quick access.</p>\
<p>The download manager tracks file retrieval with a real-time progress bar, \
state transitions from Queued through InProgress to Complete or Failed.</p>\
<h2>Reading Mode</h2>\
<p>Reading mode strips navigation, ads, and boilerplate from any page, \
delivering clean article text with an estimated reading time. HTML entity \
decoding handles &amp;amp;, &amp;lt;, &amp;gt;, &amp;nbsp; and Unicode \
punctuation like &amp;mdash; and &amp;hellip;</p>\
<p>Smart OS continues to be built entirely in Rust with no external crates, \
running in a no_std kernel environment on bare metal x86&ndash;64.</p>\
</body></html>";

// ─── Self-test ────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // 1. Bookmark store CRUD
    let mut bm = BookmarkStore::new();
    let before = bm.items.len();
    let id = bm.add("Test", "https://test.com/", BmCategory::Tech);
    if bm.items.len() != before + 1 { ok = false; }
    bm.record_visit(id);
    if bm.items.iter().find(|b| b.id == id).unwrap().visits != 1 { ok = false; }
    bm.toggle_pin(id);
    if !bm.items.iter().find(|b| b.id == id).unwrap().pinned { ok = false; }
    bm.remove(id);
    if bm.items.iter().any(|b| b.id == id) { ok = false; }

    // 2. Download state machine
    let mut dl = Download::new(1, "file.bin", "http://x.com/file", 1024);
    if dl.state != DlState::Queued { ok = false; }
    dl.advance(); // Queued → InProgress(0)
    if !matches!(dl.state, DlState::InProgress(_)) { ok = false; }
    // Run to completion
    for _ in 0..40 { dl.advance(); }
    if dl.state != DlState::Complete { ok = false; }

    // 3. Download manager clear_done
    let mut dm = DownloadManager::new();
    let before_count = dm.items.len();
    dm.clear_done();
    // The seeded manager has 1 Complete and 1 InProgress and 1 Queued
    if dm.items.len() >= before_count { ok = false; }

    // 4. History push + search
    let mut h = BrowserHistory::new();
    let before = h.entries.len();
    h.push("https://newsite.com/", "New Site", "2026-05-31 15:00");
    if h.entries.len() != before + 1 { ok = false; }
    h.search = String::from("newsite");
    if h.filtered().len() != 1 { ok = false; }
    h.clear();
    if !h.entries.is_empty() { ok = false; }

    // 5. HTML text extraction
    let html = "<p>Hello <b>World</b>!</p><p>Line two.</p>";
    let text = extract_readable(html);
    if !text.contains("Hello") { ok = false; }
    if !text.contains("World") { ok = false; }
    if !text.contains("Line two") { ok = false; }
    // Should NOT contain tags
    if text.contains('<') { ok = false; }

    // 6. Entity decoding
    let html2 = "<p>A &amp; B &lt; C &gt; D &nbsp; E &mdash; F</p>";
    let text2 = extract_readable(html2);
    if !text2.contains('&') { ok = false; }
    if !text2.contains('<') && text2.contains("&lt;") { ok = false; } // &lt; decoded to <
    // Actually &lt; should decode to <, but < in text won't start a tag:
    // re-check: extract_readable decodes &lt; → "<" — but < in non-tag context is fine
    if text2.contains("&amp;") { ok = false; } // should be decoded

    // 7. Reading time estimate
    let long_text = "word ".repeat(400);
    let mins = estimate_reading_time(&long_text);
    if mins != 2 { ok = false; } // 400 words / 200 wpm = 2 min

    let short_text = "hello world";
    if estimate_reading_time(short_text) != 1 { ok = false; } // min 1

    // 8. BmCategory names
    if BmCategory::Work.name() != "Work" { ok = false; }
    if BmCategory::Tech.name() != "Tech" { ok = false; }

    ok
}

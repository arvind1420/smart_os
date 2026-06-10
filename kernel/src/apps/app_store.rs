/// Smart OS — App Store (Phase 69, v0.29.0)
///
/// A graphical package manager front-end.  Bridges the existing `pkgmgr`
/// back-end (install/uninstall/list) with a GUI window that lets the user
/// browse the catalogue, search, install and remove packages.
///
/// Layout
/// ──────
/// Toolbar: [Search input] [⌕ Search] [↓ Install] [✕ Remove] [⟳ Refresh]
/// Left panel  (widget 6, ScrollText) — package list with ● / ○ indicators
/// Right panel (widget 7, ScrollText) — details for the selected package
/// Status bar  (widget 8, Label)      — last action result

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;

use crate::gui::window::{Window, WindowId};
use crate::gui::desktop::DESKTOP;
use crate::gui::theme::*;
use crate::gui::widget::{
    Widget, WidgetKind, Button, StaticLabel, ScrollableText,
    TextInput, AppCommand, WidgetAction,
};

// ═══════════════════════════════════════════════════════════════════════════
//  Catalogue — extended package listing beyond pkgmgr's seed entries
// ═══════════════════════════════════════════════════════════════════════════

/// A catalogue entry shown in the store (may or may not be in pkgmgr yet).
#[derive(Clone)]
pub struct CatalogueEntry {
    pub id:          String,   // package identifier (matches pkgmgr key)
    pub name:        String,
    pub version:     String,
    pub category:    Category,
    pub description: String,
    pub author:      String,
    pub size_kb:     u32,
    pub stars:       u8,       // 0-5
    pub installed:   bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Category { System, Productivity, Media, Developer, Utility, Game }

impl Category {
    fn name(self) -> &'static str {
        match self {
            Category::System       => "System",
            Category::Productivity => "Productivity",
            Category::Media        => "Media",
            Category::Developer    => "Developer",
            Category::Utility      => "Utility",
            Category::Game         => "Game",
        }
    }
    fn accent(self) -> Color {
        match self {
            Category::System       => ACCENT_CYAN,
            Category::Productivity => ACCENT_GREEN,
            Category::Media        => ACCENT_MAGENTA,
            Category::Developer    => ACCENT_ORANGE,
            Category::Utility      => TEXT_SECONDARY,
            Category::Game         => ACCENT_RED,
        }
    }
}

impl CatalogueEntry {
    fn stars_str(&self) -> String {
        let filled = self.stars.min(5) as usize;
        let empty  = 5 - filled;
        let mut s = String::new();
        for _ in 0..filled { s.push('★'); }
        for _ in 0..empty  { s.push('☆'); }
        s
    }
}

/// Build the default catalogue (seeds pkgmgr entries + extra store items).
fn default_catalogue() -> Vec<CatalogueEntry> {
    // Pull installed state from pkgmgr for known packages
    let installed_list = crate::apps::pkgmgr::list_packages();
    let is_installed = |id: &str| -> bool {
        installed_list.iter().any(|(n, _, _, inst)| n == id && *inst)
    };

    vec![
        CatalogueEntry { id: "core-utils".to_string(),  name: "Core Utils".to_string(),
            version: "1.0.0".to_string(), category: Category::System,
            description: "Fundamental shell utilities: true, false, echo, cat, ls, sh.".to_string(),
            author: "Smart OS Team".to_string(), size_kb: 48, stars: 5,
            installed: is_installed("core-utils") },

        CatalogueEntry { id: "fork-test".to_string(), name: "Fork Test".to_string(),
            version: "1.0.0".to_string(), category: Category::Developer,
            description: "fork/exec/waitpid integration test binary.".to_string(),
            author: "Smart OS Team".to_string(), size_kb: 12, stars: 3,
            installed: is_installed("fork-test") },

        CatalogueEntry { id: "calculator".to_string(), name: "Calculator".to_string(),
            version: "2.0.0".to_string(), category: Category::Utility,
            description: "GUI RPN and algebraic calculator.".to_string(),
            author: "Smart OS Team".to_string(), size_kb: 64, stars: 4,
            installed: is_installed("calculator") },

        CatalogueEntry { id: "text-editor".to_string(), name: "Text Editor".to_string(),
            version: "1.3.0".to_string(), category: Category::Productivity,
            description: "Lightweight in-kernel text editor with syntax highlighting.".to_string(),
            author: "Smart OS Team".to_string(), size_kb: 96, stars: 4,
            installed: is_installed("text-editor") },

        CatalogueEntry { id: "code-editor".to_string(), name: "Code Editor v2".to_string(),
            version: "2.0.0".to_string(), category: Category::Developer,
            description: "Multi-tab code editor with Rust/JS/C/Python tokeniser and find+replace.".to_string(),
            author: "Smart OS Team".to_string(), size_kb: 192, stars: 5,
            installed: is_installed("code-editor") },

        CatalogueEntry { id: "media-player".to_string(), name: "Media Player".to_string(),
            version: "1.1.0".to_string(), category: Category::Media,
            description: "WAV/PCM audio player with waveform visualiser and playlist support.".to_string(),
            author: "Smart OS Team".to_string(), size_kb: 128, stars: 4,
            installed: is_installed("media-player") },

        CatalogueEntry { id: "image-viewer".to_string(), name: "Image Viewer".to_string(),
            version: "1.0.0".to_string(), category: Category::Media,
            description: "PNG/JPEG/BMP viewer with zoom, pan and slideshow.".to_string(),
            author: "Smart OS Team".to_string(), size_kb: 80, stars: 4,
            installed: is_installed("image-viewer") },

        CatalogueEntry { id: "pdf-reader".to_string(), name: "PDF Reader".to_string(),
            version: "1.0.0".to_string(), category: Category::Productivity,
            description: "PDF viewer with xref scanning and BT/ET text extraction.".to_string(),
            author: "Smart OS Team".to_string(), size_kb: 72, stars: 3,
            installed: is_installed("pdf-reader") },

        CatalogueEntry { id: "video-player".to_string(), name: "Video Player".to_string(),
            version: "1.0.0".to_string(), category: Category::Media,
            description: "SmartVideo .smv player with 30fps colour-bar demo.".to_string(),
            author: "Smart OS Team".to_string(), size_kb: 96, stars: 3,
            installed: is_installed("video-player") },

        CatalogueEntry { id: "email-client".to_string(), name: "Email Client".to_string(),
            version: "1.0.0".to_string(), category: Category::Productivity,
            description: "In-kernel email client with SMTP/POP3 stubs, folders and thread view.".to_string(),
            author: "Smart OS Team".to_string(), size_kb: 110, stars: 3,
            installed: is_installed("email-client") },

        CatalogueEntry { id: "office".to_string(), name: "Office Suite".to_string(),
            version: "1.0.0".to_string(), category: Category::Productivity,
            description: "Writer, Calc and Slides with Markdown rendering and formula engine.".to_string(),
            author: "Smart OS Team".to_string(), size_kb: 200, stars: 4,
            installed: is_installed("office") },

        CatalogueEntry { id: "browser".to_string(), name: "Web Browser".to_string(),
            version: "3.0.0".to_string(), category: Category::Utility,
            description: "Full-stack browser: HTML5/CSS3/JS, Canvas, WebGL, WebSocket, Service Workers.".to_string(),
            author: "Smart OS Team".to_string(), size_kb: 512, stars: 5,
            installed: is_installed("browser") },

        CatalogueEntry { id: "dashboard".to_string(), name: "System Dashboard".to_string(),
            version: "1.0.0".to_string(), category: Category::System,
            description: "CPU/mem/net/process live dashboard with profiler stats.".to_string(),
            author: "Smart OS Team".to_string(), size_kb: 88, stars: 4,
            installed: is_installed("dashboard") },

        CatalogueEntry { id: "smartmine".to_string(), name: "SmartMine".to_string(),
            version: "0.9.0".to_string(), category: Category::Game,
            description: "Minesweeper clone for Smart OS.".to_string(),
            author: "Community".to_string(), size_kb: 32, stars: 4,
            installed: false },

        CatalogueEntry { id: "solitaire".to_string(), name: "Solitaire".to_string(),
            version: "0.8.0".to_string(), category: Category::Game,
            description: "Classic Klondike solitaire card game.".to_string(),
            author: "Community".to_string(), size_kb: 44, stars: 3,
            installed: false },

        CatalogueEntry { id: "paint".to_string(), name: "Smart Paint".to_string(),
            version: "0.5.0".to_string(), category: Category::Utility,
            description: "Simple raster drawing app with brush, fill and shape tools.".to_string(),
            author: "Community".to_string(), size_kb: 60, stars: 3,
            installed: false },
    ]
}

// ═══════════════════════════════════════════════════════════════════════════
//  App state
// ═══════════════════════════════════════════════════════════════════════════

pub struct AppStoreState {
    pub window_id:  WindowId,
    pub catalogue:  Vec<CatalogueEntry>,
    pub filtered:   Vec<usize>,   // indices into catalogue
    pub selected:   usize,        // index into filtered
    pub query:      String,
    pub status:     String,
    pub dirty:      bool,
}

impl AppStoreState {
    fn apply_filter(&mut self) {
        let q = self.query.to_lowercase();
        self.filtered = self.catalogue.iter().enumerate()
            .filter(|(_, e)| {
                q.is_empty()
                || e.name.to_lowercase().contains(&q)
                || e.id.to_lowercase().contains(&q)
                || e.description.to_lowercase().contains(&q)
                || e.category.name().to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect();
        if self.selected >= self.filtered.len() && !self.filtered.is_empty() {
            self.selected = 0;
        }
    }

    fn selected_entry(&self) -> Option<&CatalogueEntry> {
        self.filtered.get(self.selected).and_then(|&i| self.catalogue.get(i))
    }
}

pub static STATE: Mutex<Option<AppStoreState>> = Mutex::new(None);

// ═══════════════════════════════════════════════════════════════════════════
//  Window creation
// ═══════════════════════════════════════════════════════════════════════════

fn create_window() -> WindowId {
    let mut dg = DESKTOP.lock();
    let desk = dg.as_mut().expect("desktop not init");
    let mut win = Window::new("App Store", 80, 40, 820, 560, ACCENT_CYAN);
    win.use_widgets = true;

    // 0: search text input
    win.widgets.push(Widget::new(0,   4,  4, 300, 26,
        WidgetKind::TextInput(TextInput::new("Search packages...", ACCENT_CYAN))));
    // 1: Search button
    win.widgets.push(Widget::new(1, 308,  4,  80, 26,
        WidgetKind::Button(Button::new("⌕ Search",  ACCENT_CYAN,   AppCommand::ButtonClicked(1)))));
    // 2: Install
    win.widgets.push(Widget::new(2, 392,  4,  90, 26,
        WidgetKind::Button(Button::new("↓ Install",  ACCENT_GREEN,  AppCommand::ButtonClicked(2)))));
    // 3: Remove
    win.widgets.push(Widget::new(3, 486,  4,  80, 26,
        WidgetKind::Button(Button::new("✕ Remove",   ACCENT_RED,    AppCommand::ButtonClicked(3)))));
    // 4: Refresh
    win.widgets.push(Widget::new(4, 570,  4,  90, 26,
        WidgetKind::Button(Button::new("⟳ Refresh",  TEXT_SECONDARY, AppCommand::ButtonClicked(4)))));

    // 5: Status label
    win.widgets.push(Widget::new(5,   4, 36, 800, 16,
        WidgetKind::Label(StaticLabel::new("App Store — browse and manage packages.", TEXT_SECONDARY))));

    // 6: Package list (left scroll)
    win.widgets.push(Widget::new(6,   4, 58, 310, 468,
        WidgetKind::ScrollText(ScrollableText::new(128))));

    // 7: Detail panel (right scroll)
    win.widgets.push(Widget::new(7, 320, 58, 488, 468,
        WidgetKind::ScrollText(ScrollableText::new(64))));

    win.focused_widget = Some(0);
    let id = win.id;
    desk.wm.add(win);
    id
}

// ═══════════════════════════════════════════════════════════════════════════
//  Rendering helpers
// ═══════════════════════════════════════════════════════════════════════════

fn build_list(s: &AppStoreState) -> Vec<(String, Color)> {
    if s.filtered.is_empty() {
        return vec![("  No packages match the search.".to_string(), TEXT_MUTED)];
    }
    s.filtered.iter().enumerate().map(|(pos, &idx)| {
        let e = &s.catalogue[idx];
        let mark  = if e.installed { "●" } else { "○" };
        let sel   = if pos == s.selected { "►" } else { " " };
        let line  = format!("{}{} {} v{}", sel, mark, e.name, e.version);
        let color = if pos == s.selected {
            ACCENT_CYAN
        } else if e.installed {
            ACCENT_GREEN
        } else {
            TEXT_PRIMARY
        };
        (line, color)
    }).collect()
}

fn build_detail(s: &AppStoreState) -> Vec<(String, Color)> {
    let e = match s.selected_entry() {
        Some(e) => e,
        None => return vec![("  Select a package to see details.".to_string(), TEXT_MUTED)],
    };
    let mut v: Vec<(String, Color)> = Vec::new();
    v.push((format!("  {}", e.name), TEXT_PRIMARY));
    v.push((format!("  Version : {}", e.version), TEXT_SECONDARY));
    v.push((format!("  Author  : {}", e.author), TEXT_SECONDARY));
    v.push((format!("  Category: {}", e.category.name()), e.category.accent()));
    v.push((format!("  Size    : {} KB", e.size_kb), TEXT_SECONDARY));
    v.push((format!("  Rating  : {}", e.stars_str()), ACCENT_ORANGE));
    v.push((format!("  Status  : {}", if e.installed { "Installed" } else { "Not installed" }),
        if e.installed { ACCENT_GREEN } else { TEXT_MUTED }));
    v.push((String::new(), TEXT_MUTED));
    v.push(("  Description".to_string(), TEXT_SECONDARY));
    v.push(("  ─────────────────────────────────────────────".to_string(), TEXT_MUTED));
    // Wrap description at ~50 chars
    let words: Vec<&str> = e.description.split_whitespace().collect();
    let mut line = String::from("  ");
    for word in &words {
        if line.len() + word.len() + 1 > 52 {
            v.push((line.clone(), TEXT_PRIMARY));
            line = format!("  {}", word);
        } else {
            if line.len() > 2 { line.push(' '); }
            line.push_str(word);
        }
    }
    if line.len() > 2 { v.push((line, TEXT_PRIMARY)); }
    v.push((String::new(), TEXT_MUTED));
    if e.installed {
        v.push(("  [Installed — press ✕ Remove to uninstall]".to_string(), ACCENT_GREEN));
    } else {
        v.push(("  [Not installed — press ↓ Install]".to_string(), TEXT_MUTED));
    }
    v
}

// ═══════════════════════════════════════════════════════════════════════════
//  Sync
// ═══════════════════════════════════════════════════════════════════════════

pub fn sync_to_window(win: &mut Window) {
    let guard = STATE.lock();
    let s = match guard.as_ref() { Some(x) => x, None => return };
    if !s.dirty { return; }

    // Status
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 5) {
        if let WidgetKind::Label(ref mut lbl) = w.kind {
            lbl.text = format!("App Store — {} packages ({} installed) | {}",
                s.catalogue.len(),
                s.catalogue.iter().filter(|e| e.installed).count(),
                s.status);
        }
    }
    // List
    let list_lines = build_list(s);
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 6) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = list_lines; }
    }
    // Detail
    let detail_lines = build_detail(s);
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 7) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = detail_lines; }
    }
    win.dirty = true;
}

// ═══════════════════════════════════════════════════════════════════════════
//  Kernel thread
// ═══════════════════════════════════════════════════════════════════════════

pub fn run() {
    let window_id = create_window();
    let mut cat = default_catalogue();
    let total = cat.len();
    let mut s = AppStoreState {
        window_id,
        filtered: (0..total).collect(),
        catalogue: cat,
        selected: 0,
        query: String::new(),
        status: format!("{} packages available.", total),
        dirty: true,
    };
    *STATE.lock() = Some(s);

    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                match action {
                    // Enter pressed in search text input → capture query + search
                    WidgetAction::Execute(AppCommand::TextSubmitted(text)) => {
                        s.query = text;
                        s.apply_filter();
                        s.status = format!("{} result(s).", s.filtered.len());
                        s.dirty = true;
                    }
                    // Search button → re-run filter with current query
                    WidgetAction::Execute(AppCommand::ButtonClicked(1)) => {
                        s.apply_filter();
                        s.status = format!("{} result(s).", s.filtered.len());
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(2)) => {
                        // Install
                        if let Some(e) = s.selected_entry().cloned() {
                            if e.installed {
                                s.status = format!("{} is already installed.", e.name);
                            } else {
                                match crate::apps::pkgmgr::install(&e.id) {
                                    Ok(msg) => {
                                        if let Some(idx) = s.filtered.get(s.selected).copied() {
                                            s.catalogue[idx].installed = true;
                                        }
                                        s.status = msg;
                                    }
                                    Err(e_str) => { s.status = format!("Install failed: {}", e_str); }
                                }
                            }
                            s.dirty = true;
                        }
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(3)) => {
                        // Remove
                        if let Some(e) = s.selected_entry().cloned() {
                            if !e.installed {
                                s.status = format!("{} is not installed.", e.name);
                            } else {
                                match crate::apps::pkgmgr::remove(&e.id) {
                                    Ok(msg) => {
                                        if let Some(idx) = s.filtered.get(s.selected).copied() {
                                            s.catalogue[idx].installed = false;
                                        }
                                        s.status = msg;
                                    }
                                    Err(e_str) => { s.status = format!("Remove failed: {}", e_str); }
                                }
                            }
                            s.dirty = true;
                        }
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(4)) => {
                        // Refresh catalogue installed flags
                        let installed = crate::apps::pkgmgr::list_packages();
                        for e in s.catalogue.iter_mut() {
                            e.installed = installed.iter().any(|(n, _, _, inst)| n == &e.id && *inst);
                        }
                        s.apply_filter();
                        s.status = "Catalogue refreshed.".to_string();
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::LineClicked(row)) => {
                        if row < s.filtered.len() { s.selected = row; s.dirty = true; }
                    }
                    _ => {}
                }
                s.dirty = false;
            }
        }
        crate::process::scheduler::yield_now();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Self-test  (8 tests)
// ═══════════════════════════════════════════════════════════════════════════

pub fn self_test() -> bool {
    let mut ok = true;

    // T1: default_catalogue not empty
    let cat = default_catalogue();
    if cat.is_empty() { ok = false; }

    // T2: at least one category per major type
    let has_sys  = cat.iter().any(|e| e.category == Category::System);
    let has_prod = cat.iter().any(|e| e.category == Category::Productivity);
    let has_med  = cat.iter().any(|e| e.category == Category::Media);
    let has_dev  = cat.iter().any(|e| e.category == Category::Developer);
    if !has_sys || !has_prod || !has_med || !has_dev { ok = false; }

    // T3: stars clamped 0-5
    for e in &cat {
        if e.stars > 5 { ok = false; break; }
    }

    // T4: stars_str length == 5
    for e in &cat {
        if e.stars_str().chars().count() != 5 { ok = false; break; }
    }

    // T5: filter — empty query returns all
    let mut s = AppStoreState {
        window_id: 0u64,
        catalogue: cat.clone(),
        filtered: (0..cat.len()).collect(),
        selected: 0,
        query: String::new(),
        status: String::new(),
        dirty: false,
    };
    s.apply_filter();
    if s.filtered.len() != cat.len() { ok = false; }

    // T6: filter by name
    s.query = "browser".to_string();
    s.apply_filter();
    if s.filtered.is_empty() { ok = false; }
    if !cat[s.filtered[0]].name.to_lowercase().contains("browser")
       && !cat[s.filtered[0]].description.to_lowercase().contains("browser") {
        ok = false;
    }

    // T7: filter by category name
    s.query = "game".to_string();
    s.apply_filter();
    if s.filtered.is_empty() { ok = false; }
    if !s.filtered.iter().all(|&i| cat[i].category == Category::Game) { ok = false; }

    // T8: build_list / build_detail don't panic on valid state
    s.query.clear();
    s.apply_filter();
    s.selected = 0;
    let list   = build_list(&s);
    let detail = build_detail(&s);
    if list.is_empty() || detail.is_empty() { ok = false; }

    ok
}

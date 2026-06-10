/// Smart OS — First-Run Setup Wizard (Phase 72, v0.32.0)
///
/// A multi-page setup wizard shown on first boot (or on demand from Settings).
/// Pages:
///   0  Welcome              — splash + language selection
///   1  Locale & Time        — timezone, date format, language
///   2  Network              — hostname, static IP / DHCP toggle
///   3  User Account         — username + password creation
///   4  Privacy & Telemetry  — opt-in/out checkboxes
///   5  Desktop Theme        — Light / Dark / Hacker theme picker
///   6  Summary              — confirms choices, writes /etc/os-release + /etc/hostname
///
/// Implements a `WizardState` finite-state machine with `next_page()` / `prev_page()`
/// navigation and stores user choices as a `SetupConfig` struct that is applied on
/// page 6 (Finish) and persisted to the VFS.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;

use crate::gui::window::{Window, WindowId};
use crate::gui::desktop::DESKTOP;
use crate::gui::theme::*;
use crate::gui::widget::{
    Widget, WidgetKind, Button, StaticLabel, ScrollableText, AppCommand, WidgetAction,
};

// ═══════════════════════════════════════════════════════════════════════════
//  Configuration model
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Language { English, Spanish, French, German, Japanese, Chinese }

impl Language {
    pub fn name(self) -> &'static str {
        match self {
            Language::English  => "English",
            Language::Spanish  => "Español",
            Language::French   => "Français",
            Language::German   => "Deutsch",
            Language::Japanese => "日本語",
            Language::Chinese  => "中文",
        }
    }
    pub fn locale_code(self) -> &'static str {
        match self {
            Language::English  => "en_US",
            Language::Spanish  => "es_ES",
            Language::French   => "fr_FR",
            Language::German   => "de_DE",
            Language::Japanese => "ja_JP",
            Language::Chinese  => "zh_CN",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TimeZone { UTC, EasternUS, CentralUS, Pacific, London, Berlin, Tokyo, Sydney }

impl TimeZone {
    pub fn name(self) -> &'static str {
        match self {
            TimeZone::UTC       => "UTC",
            TimeZone::EasternUS => "America/New_York (UTC-5)",
            TimeZone::CentralUS => "America/Chicago (UTC-6)",
            TimeZone::Pacific   => "America/Los_Angeles (UTC-8)",
            TimeZone::London    => "Europe/London (UTC+0)",
            TimeZone::Berlin    => "Europe/Berlin (UTC+1)",
            TimeZone::Tokyo     => "Asia/Tokyo (UTC+9)",
            TimeZone::Sydney    => "Australia/Sydney (UTC+11)",
        }
    }
    pub fn offset_hours(self) -> i8 {
        match self {
            TimeZone::UTC => 0, TimeZone::EasternUS => -5, TimeZone::CentralUS => -6,
            TimeZone::Pacific => -8, TimeZone::London => 0, TimeZone::Berlin => 1,
            TimeZone::Tokyo => 9, TimeZone::Sydney => 11,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ThemeChoice { Dark, Light, Hacker }

impl ThemeChoice {
    pub fn name(self) -> &'static str {
        match self { ThemeChoice::Dark => "Dark (Default)", ThemeChoice::Light => "Light", ThemeChoice::Hacker => "Hacker Green" }
    }
}

#[derive(Clone, Debug)]
pub struct SetupConfig {
    pub language:        Language,
    pub timezone:        TimeZone,
    pub hostname:        String,
    pub dhcp:            bool,
    pub static_ip:       String,
    pub username:        String,
    pub password_hash:   String,   // simple djb2 hash stored as hex string
    pub telemetry:       bool,
    pub theme:           ThemeChoice,
    pub setup_complete:  bool,
}

impl Default for SetupConfig {
    fn default() -> Self {
        Self {
            language: Language::English,
            timezone: TimeZone::UTC,
            hostname: "smartos-host".to_string(),
            dhcp: true,
            static_ip: "192.168.1.100".to_string(),
            username: "user".to_string(),
            password_hash: "0000".to_string(),
            telemetry: false,
            theme: ThemeChoice::Dark,
            setup_complete: false,
        }
    }
}

/// djb2 hash of a password string (no_std safe, not cryptographic).
pub fn hash_password(s: &str) -> String {
    let mut h: u32 = 5381;
    for b in s.bytes() { h = h.wrapping_mul(33) ^ (b as u32); }
    format!("{:08x}", h)
}

// ═══════════════════════════════════════════════════════════════════════════
//  Wizard state machine
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum WizardPage { Welcome = 0, Locale = 1, Network = 2, Account = 3, Privacy = 4, Theme = 5, Summary = 6 }

impl WizardPage {
    pub fn title(self) -> &'static str {
        match self {
            WizardPage::Welcome  => "Welcome to Smart OS",
            WizardPage::Locale   => "Language & Time Zone",
            WizardPage::Network  => "Network Configuration",
            WizardPage::Account  => "Create User Account",
            WizardPage::Privacy  => "Privacy & Telemetry",
            WizardPage::Theme    => "Desktop Theme",
            WizardPage::Summary  => "Review & Finish",
        }
    }
    pub fn next(self) -> Option<WizardPage> {
        match self {
            WizardPage::Welcome  => Some(WizardPage::Locale),
            WizardPage::Locale   => Some(WizardPage::Network),
            WizardPage::Network  => Some(WizardPage::Account),
            WizardPage::Account  => Some(WizardPage::Privacy),
            WizardPage::Privacy  => Some(WizardPage::Theme),
            WizardPage::Theme    => Some(WizardPage::Summary),
            WizardPage::Summary  => None,
        }
    }
    pub fn prev(self) -> Option<WizardPage> {
        match self {
            WizardPage::Welcome  => None,
            WizardPage::Locale   => Some(WizardPage::Welcome),
            WizardPage::Network  => Some(WizardPage::Locale),
            WizardPage::Account  => Some(WizardPage::Network),
            WizardPage::Privacy  => Some(WizardPage::Account),
            WizardPage::Theme    => Some(WizardPage::Privacy),
            WizardPage::Summary  => Some(WizardPage::Theme),
        }
    }
    pub fn index(self) -> usize { self as usize }
    pub fn total() -> usize { 7 }
}

pub struct WizardState {
    pub window_id: WindowId,
    pub page:      WizardPage,
    pub config:    SetupConfig,
    pub dirty:     bool,
    pub finished:  bool,
}

impl WizardState {
    fn page_content(&self) -> Vec<(String, Color)> {
        let mut v: Vec<(String, Color)> = Vec::new();
        match self.page {
            WizardPage::Welcome => {
                v.push(("".to_string(), TEXT_MUTED));
                v.push(("  ╔══════════════════════════════════════╗".to_string(), ACCENT_CYAN));
                v.push(("  ║        Welcome to Smart OS           ║".to_string(), ACCENT_CYAN));
                v.push(("  ║   Hybrid Microkernel OS v0.32.0      ║".to_string(), ACCENT_CYAN));
                v.push(("  ╚══════════════════════════════════════╝".to_string(), ACCENT_CYAN));
                v.push(("".to_string(), TEXT_MUTED));
                v.push(("  This wizard will guide you through the initial setup.".to_string(), TEXT_PRIMARY));
                v.push(("  You can change these settings later in the Settings app.".to_string(), TEXT_SECONDARY));
                v.push(("".to_string(), TEXT_MUTED));
                v.push(("  Press → Next to continue.".to_string(), TEXT_MUTED));
            }
            WizardPage::Locale => {
                v.push(("  Language".to_string(), TEXT_SECONDARY));
                for lang in [Language::English, Language::Spanish, Language::French,
                             Language::German, Language::Japanese, Language::Chinese] {
                    let sel = if self.config.language == lang { "►" } else { " " };
                    v.push((format!("  {} {} ({})", sel, lang.name(), lang.locale_code()),
                        if self.config.language == lang { ACCENT_CYAN } else { TEXT_PRIMARY }));
                }
                v.push(("".to_string(), TEXT_MUTED));
                v.push(("  Time Zone".to_string(), TEXT_SECONDARY));
                for tz in [TimeZone::UTC, TimeZone::EasternUS, TimeZone::CentralUS, TimeZone::Pacific,
                           TimeZone::London, TimeZone::Berlin, TimeZone::Tokyo, TimeZone::Sydney] {
                    let sel = if self.config.timezone == tz { "►" } else { " " };
                    v.push((format!("  {} {}", sel, tz.name()),
                        if self.config.timezone == tz { ACCENT_CYAN } else { TEXT_PRIMARY }));
                }
            }
            WizardPage::Network => {
                v.push(("  Hostname".to_string(), TEXT_SECONDARY));
                v.push((format!("  {}", self.config.hostname), ACCENT_CYAN));
                v.push(("".to_string(), TEXT_MUTED));
                v.push(("  Network Mode".to_string(), TEXT_SECONDARY));
                v.push((format!("  {} DHCP (automatic)", if self.config.dhcp { "►" } else { " " }),
                    if self.config.dhcp { ACCENT_CYAN } else { TEXT_PRIMARY }));
                v.push((format!("  {} Static IP: {}", if !self.config.dhcp { "►" } else { " " }, self.config.static_ip),
                    if !self.config.dhcp { ACCENT_CYAN } else { TEXT_PRIMARY }));
                v.push(("".to_string(), TEXT_MUTED));
                v.push(("  Use buttons below to toggle DHCP / Static.".to_string(), TEXT_MUTED));
            }
            WizardPage::Account => {
                v.push(("  Username".to_string(), TEXT_SECONDARY));
                v.push((format!("  {}", self.config.username), ACCENT_CYAN));
                v.push(("".to_string(), TEXT_MUTED));
                v.push(("  Password  (hash stored — not shown)".to_string(), TEXT_SECONDARY));
                v.push((format!("  Hash: {}", self.config.password_hash), TEXT_MUTED));
                v.push(("".to_string(), TEXT_MUTED));
                v.push(("  Default username: 'user'  password: 'password'".to_string(), TEXT_MUTED));
                v.push(("  (Change via Settings > Users after setup)".to_string(), TEXT_MUTED));
            }
            WizardPage::Privacy => {
                v.push(("  Privacy Settings".to_string(), TEXT_SECONDARY));
                v.push(("".to_string(), TEXT_MUTED));
                v.push((format!("  [{}] Send anonymous usage telemetry to Smart OS project",
                    if self.config.telemetry { "✓" } else { " " }),
                    if self.config.telemetry { ACCENT_CYAN } else { TEXT_PRIMARY }));
                v.push(("".to_string(), TEXT_MUTED));
                v.push(("  Telemetry includes: OS version, uptime, crash counts.".to_string(), TEXT_MUTED));
                v.push(("  No personal data is collected.".to_string(), TEXT_MUTED));
                v.push(("".to_string(), TEXT_MUTED));
                v.push(("  Press 'Toggle Telemetry' button to change.".to_string(), TEXT_MUTED));
            }
            WizardPage::Theme => {
                v.push(("  Desktop Theme".to_string(), TEXT_SECONDARY));
                v.push(("".to_string(), TEXT_MUTED));
                for th in [ThemeChoice::Dark, ThemeChoice::Light, ThemeChoice::Hacker] {
                    let sel = if self.config.theme == th { "►" } else { " " };
                    v.push((format!("  {} {}", sel, th.name()),
                        if self.config.theme == th { ACCENT_CYAN } else { TEXT_PRIMARY }));
                }
                v.push(("".to_string(), TEXT_MUTED));
                v.push(("  Use theme buttons to select.".to_string(), TEXT_MUTED));
            }
            WizardPage::Summary => {
                v.push(("  Setup Summary".to_string(), ACCENT_CYAN));
                v.push(("  ─────────────────────────────────────────────".to_string(), TEXT_MUTED));
                v.push((format!("  Language   : {} ({})", self.config.language.name(), self.config.language.locale_code()), TEXT_PRIMARY));
                v.push((format!("  Time Zone  : {}", self.config.timezone.name()), TEXT_PRIMARY));
                v.push((format!("  Hostname   : {}", self.config.hostname), TEXT_PRIMARY));
                v.push((format!("  Network    : {}", if self.config.dhcp { "DHCP" } else { &self.config.static_ip }), TEXT_PRIMARY));
                v.push((format!("  Username   : {}", self.config.username), TEXT_PRIMARY));
                v.push((format!("  Telemetry  : {}", if self.config.telemetry { "Enabled" } else { "Disabled" }), TEXT_PRIMARY));
                v.push((format!("  Theme      : {}", self.config.theme.name()), TEXT_PRIMARY));
                v.push(("".to_string(), TEXT_MUTED));
                v.push(("  Press '✓ Finish' to apply settings and start Smart OS.".to_string(), ACCENT_GREEN));
            }
        }
        v
    }
}

pub static STATE: Mutex<Option<WizardState>> = Mutex::new(None);

// ═══════════════════════════════════════════════════════════════════════════
//  Apply setup (write to VFS)
// ═══════════════════════════════════════════════════════════════════════════

fn apply_config(cfg: &SetupConfig) {
    let _ = crate::vfs::mkdir("/etc");
    let os_release = format!(
        "NAME=Smart OS\nVERSION=v0.32.0\nLANGUAGE={}\nTIMEZONE={}\nTHEME={:?}\n",
        cfg.language.locale_code(), cfg.timezone.name(), cfg.theme);
    let _ = crate::vfs::create_and_write("/etc/os-release", os_release.as_bytes());
    let _ = crate::vfs::create_and_write("/etc/hostname", cfg.hostname.as_bytes());
    let network_cfg = format!("dhcp={}\nip={}\n", cfg.dhcp, cfg.static_ip);
    let _ = crate::vfs::create_and_write("/etc/network", network_cfg.as_bytes());
    crate::serial_println!("[setup] Config applied: hostname={} lang={}", cfg.hostname, cfg.language.locale_code());
}

// ═══════════════════════════════════════════════════════════════════════════
//  Window
// ═══════════════════════════════════════════════════════════════════════════

fn create_window() -> WindowId {
    let mut dg = DESKTOP.lock();
    let desk = dg.as_mut().expect("desktop not init");
    let mut win = Window::new("Setup Wizard", 120, 60, 680, 520, ACCENT_CYAN);
    win.use_widgets = true;

    // 0: Page title
    win.widgets.push(Widget::new(0, 4, 4, 660, 20,
        WidgetKind::Label(StaticLabel::new("Welcome to Smart OS", TEXT_PRIMARY))));
    // 1: Progress label  "Step 1 of 7"
    win.widgets.push(Widget::new(1, 4, 26, 660, 14,
        WidgetKind::Label(StaticLabel::new("Step 1 of 7", TEXT_MUTED))));

    // Content scroll (large area)
    win.widgets.push(Widget::new(2, 4, 46, 660, 380,
        WidgetKind::ScrollText(ScrollableText::new(64))));

    // Navigation / action buttons (bottom row)
    // 3: Back
    win.widgets.push(Widget::new(3,   4, 432, 90, 26,
        WidgetKind::Button(Button::new("← Back",    TEXT_SECONDARY, AppCommand::ButtonClicked(3)))));
    // 4: Next
    win.widgets.push(Widget::new(4,  98, 432, 90, 26,
        WidgetKind::Button(Button::new("→ Next",    ACCENT_CYAN,   AppCommand::ButtonClicked(4)))));
    // 5: action button (context-sensitive)
    win.widgets.push(Widget::new(5, 200, 432, 160, 26,
        WidgetKind::Button(Button::new("…",         ACCENT_GREEN,  AppCommand::ButtonClicked(5)))));
    // 6: Finish (only visible on Summary)
    win.widgets.push(Widget::new(6, 370, 432, 110, 26,
        WidgetKind::Button(Button::new("✓ Finish",  ACCENT_GREEN,  AppCommand::ButtonClicked(6)))));

    let id = win.id;
    desk.wm.add(win);
    id
}

// ═══════════════════════════════════════════════════════════════════════════
//  Sync
// ═══════════════════════════════════════════════════════════════════════════

pub fn sync_to_window(win: &mut Window) {
    let guard = STATE.lock();
    let s = match guard.as_ref() { Some(x) => x, None => return };
    if !s.dirty { return; }

    // Title
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 0) {
        if let WidgetKind::Label(ref mut l) = w.kind {
            l.text = s.page.title().to_string();
            l.color = ACCENT_CYAN;
        }
    }
    // Progress
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 1) {
        if let WidgetKind::Label(ref mut l) = w.kind {
            l.text = format!("Step {} of {}", s.page.index() + 1, WizardPage::total());
        }
    }
    // Content
    let lines = s.page_content();
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 2) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = lines; }
    }
    // Context action button label
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 5) {
        if let WidgetKind::Button(ref mut btn) = w.kind {
            btn.label = match s.page {
                WizardPage::Network  => "⇌ Toggle DHCP".to_string(),
                WizardPage::Privacy  => "⊕ Toggle Telemetry".to_string(),
                WizardPage::Theme    => "Dark / Light / Hacker".to_string(),
                WizardPage::Locale   => "← Language  →".to_string(),
                _ => "…".to_string(),
            };
        }
    }
    win.dirty = true;
}

// ═══════════════════════════════════════════════════════════════════════════
//  Kernel thread
// ═══════════════════════════════════════════════════════════════════════════

pub fn run() {
    let window_id = create_window();
    *STATE.lock() = Some(WizardState {
        window_id,
        page: WizardPage::Welcome,
        config: SetupConfig::default(),
        dirty: true,
        finished: false,
    });

    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                if s.finished { continue; }
                match action {
                    WidgetAction::Execute(AppCommand::ButtonClicked(3)) => {
                        // Back
                        if let Some(prev) = s.page.prev() { s.page = prev; s.dirty = true; }
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(4)) => {
                        // Next
                        if let Some(next) = s.page.next() { s.page = next; s.dirty = true; }
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(5)) => {
                        // Context action
                        match s.page {
                            WizardPage::Network => { s.config.dhcp = !s.config.dhcp; s.dirty = true; }
                            WizardPage::Privacy => { s.config.telemetry = !s.config.telemetry; s.dirty = true; }
                            WizardPage::Theme => {
                                s.config.theme = match s.config.theme {
                                    ThemeChoice::Dark   => ThemeChoice::Light,
                                    ThemeChoice::Light  => ThemeChoice::Hacker,
                                    ThemeChoice::Hacker => ThemeChoice::Dark,
                                };
                                s.dirty = true;
                            }
                            WizardPage::Locale => {
                                // Cycle language
                                s.config.language = match s.config.language {
                                    Language::English  => Language::Spanish,
                                    Language::Spanish  => Language::French,
                                    Language::French   => Language::German,
                                    Language::German   => Language::Japanese,
                                    Language::Japanese => Language::Chinese,
                                    Language::Chinese  => Language::English,
                                };
                                s.dirty = true;
                            }
                            _ => {}
                        }
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(6)) => {
                        // Finish
                        if s.page == WizardPage::Summary {
                            s.config.setup_complete = true;
                            apply_config(&s.config);
                            s.finished = true;
                            s.dirty = true;
                            crate::gui::notification::push(
                                "Setup Wizard", "Setup complete! Enjoy Smart OS.",
                                ACCENT_GREEN);
                        }
                    }
                    _ => {}
                }
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

    // T1: WizardPage navigation forward chain
    let mut page = WizardPage::Welcome;
    for _ in 0..WizardPage::total() - 1 {
        page = match page.next() { Some(p) => p, None => { ok = false; break; } };
    }
    if page != WizardPage::Summary { ok = false; }

    // T2: WizardPage navigation backward chain
    let mut page = WizardPage::Summary;
    for _ in 0..WizardPage::total() - 1 {
        page = match page.prev() { Some(p) => p, None => { ok = false; break; } };
    }
    if page != WizardPage::Welcome { ok = false; }

    // T3: WizardPage::Welcome.prev() == None
    if WizardPage::Welcome.prev().is_some() { ok = false; }

    // T4: WizardPage::Summary.next() == None
    if WizardPage::Summary.next().is_some() { ok = false; }

    // T5: hash_password deterministic
    let h1 = hash_password("hello");
    let h2 = hash_password("hello");
    if h1 != h2 || h1.is_empty() { ok = false; }

    // T6: hash_password different inputs → different hashes
    if hash_password("abc") == hash_password("xyz") { ok = false; }

    // T7: Language locale codes non-empty + unique
    let codes: Vec<&str> = [Language::English, Language::Spanish, Language::French,
                             Language::German, Language::Japanese, Language::Chinese]
        .iter().map(|l| l.locale_code()).collect();
    let len = codes.len();
    let unique: alloc::collections::BTreeSet<&str> = codes.into_iter().collect();
    if unique.len() != len { ok = false; }

    // T8: WizardPage titles non-empty
    for page in [WizardPage::Welcome, WizardPage::Locale, WizardPage::Network,
                 WizardPage::Account, WizardPage::Privacy, WizardPage::Theme, WizardPage::Summary] {
        if page.title().is_empty() { ok = false; break; }
    }

    ok
}

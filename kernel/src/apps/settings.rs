/// Smart OS Settings — System control panel application.
///
/// Tabbed interface: System, Display, Network, Language, About.
/// Auto-refreshes periodically.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::{Window, WindowId};
use crate::gui::widget::*;
use crate::gui::desktop::DESKTOP;

/// Active settings tab.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingsTab {
    System,
    Display,
    Network,
    Language,
    About,
}

/// Settings application state.
pub struct SettingsState {
    pub window_id: WindowId,
    pub active_tab: SettingsTab,
    pub dirty: bool,
}

pub static STATE: Mutex<Option<SettingsState>> = Mutex::new(None);

/// Settings thread entry point.
pub fn run() {
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() {
            Some(d) => d,
            None => return,
        };

        let mut win = Window::new("Settings", 200, 80, 460, 340, ACCENT_PURPLE);
        win.use_widgets = true;

        // Widget 0-4: Tab buttons (5 tabs × 84 px + 4 gaps × 4 px = 436 px < 460 px)
        let tab_w = 84;
        let tab_h = 22;
        win.widgets.push(Widget::new(0, 0, 0, tab_w, tab_h,
            WidgetKind::Button(Button::new("System",   ACCENT_GREEN,   AppCommand::ButtonClicked(0)))));
        win.widgets.push(Widget::new(1, (tab_w + 4), 0, tab_w, tab_h,
            WidgetKind::Button(Button::new("Display",  ACCENT_CYAN,    AppCommand::ButtonClicked(1)))));
        win.widgets.push(Widget::new(2, (tab_w + 4) * 2, 0, tab_w, tab_h,
            WidgetKind::Button(Button::new("Network",  ACCENT_ORANGE,  AppCommand::ButtonClicked(2)))));
        win.widgets.push(Widget::new(3, (tab_w + 4) * 3, 0, tab_w, tab_h,
            WidgetKind::Button(Button::new("Language", ACCENT_MAGENTA, AppCommand::ButtonClicked(3)))));
        win.widgets.push(Widget::new(4, (tab_w + 4) * 4, 0, tab_w, tab_h,
            WidgetKind::Button(Button::new("About",    ACCENT_PURPLE,  AppCommand::ButtonClicked(4)))));

        // Widget 5: Content area (scrollable text)
        win.widgets.push(Widget::new(5, 0, 28, 440, 280,
            WidgetKind::ScrollText(ScrollableText::new(200))));

        win.focused_widget = Some(5);
        let id = win.id;
        desk.wm.add(win);
        id
    };

    *STATE.lock() = Some(SettingsState {
        window_id,
        active_tab: SettingsTab::System,
        dirty: true,
    });

    let mut refresh_counter = 0u32;
    loop {
        // Handle tab button clicks
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            match action {
                WidgetAction::Execute(AppCommand::ButtonClicked(idx)) => {
                    let mut guard = STATE.lock();
                    if let Some(ref mut state) = *guard {
                        state.active_tab = match idx {
                            0 => SettingsTab::System,
                            1 => SettingsTab::Display,
                            2 => SettingsTab::Network,
                            3 => SettingsTab::Language,
                            _ => SettingsTab::About,
                        };
                        state.dirty = true;
                    }
                }
                _ => {}
            }
        }

        // Auto-refresh every ~2 seconds (200 yields at ~100Hz)
        refresh_counter += 1;
        if refresh_counter >= 200 {
            refresh_counter = 0;
            if let Some(ref mut state) = *STATE.lock() {
                state.dirty = true;
            }
        }

        crate::process::scheduler::yield_now();
    }
}

/// Sync settings state to the window (called from render loop).
pub fn sync_to_window(window: &mut Window) {
    let mut guard = STATE.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return,
    };

    if !state.dirty { return; }
    state.dirty = false;

    let lines = match state.active_tab {
        SettingsTab::System   => build_system_info(),
        SettingsTab::Display  => build_display_info(),
        SettingsTab::Network  => build_network_info(),
        SettingsTab::Language => build_language_info(),
        SettingsTab::About    => build_about_info(),
    };

    if let Some(w) = window.widgets.get_mut(5) {
        if let WidgetKind::ScrollText(ref mut scroll) = w.kind {
            scroll.lines.clear();
            for (text, color) in lines {
                scroll.push_line(&text, color);
            }
        }
    }
}

fn build_system_info() -> Vec<(String, Color)> {
    let uptime = crate::drivers::timer::uptime_secs();
    let (heap_used, heap_free) = crate::memory::heap::heap_stats();
    let threads = crate::process::scheduler::ready_count() + 1;
    let cpus = crate::arch::x86_64::smp::cpu_count();
    let hours = uptime / 3600;
    let mins = (uptime % 3600) / 60;
    let secs = uptime % 60;

    let mut lines = Vec::new();
    lines.push((String::from("  ── System Information ──"), ACCENT_GREEN));
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((format!("  Kernel:    Smart OS v0.20.0"), TEXT_PRIMARY));
    lines.push((format!("  CPUs:      {} core(s)", cpus), TEXT_PRIMARY));
    lines.push((format!("  Threads:   {} active", threads), TEXT_PRIMARY));
    lines.push((format!("  Uptime:    {:02}:{:02}:{:02}", hours, mins, secs), TEXT_PRIMARY));
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((String::from("  ── Memory ──"), ACCENT_GREEN));
    lines.push((format!("  Heap used: {} KB", heap_used / 1024), TEXT_PRIMARY));
    lines.push((format!("  Heap free: {} KB", heap_free / 1024), TEXT_PRIMARY));
    let total = heap_used + heap_free;
    let pct = if total > 0 { heap_used * 100 / total } else { 0 };
    lines.push((format!("  Usage:     {}%", pct), if pct > 80 { ACCENT_RED } else { TEXT_PRIMARY }));

    // RTC time
    let dt = crate::drivers::rtc::now();
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((format!("  Date:      {:04}-{:02}-{:02}", dt.year, dt.month, dt.day), TEXT_PRIMARY));
    lines.push((format!("  Time:      {:02}:{:02}:{:02}", dt.hour, dt.minute, dt.second), TEXT_PRIMARY));
    lines
}

fn build_display_info() -> Vec<(String, Color)> {
    let (sw, sh) = {
        let (w, h) = crate::gui::compositor::fb_resolution();
        (w as usize, h as usize)
    };
    let mut lines = Vec::new();
    lines.push((String::from("  ── Display Information ──"), ACCENT_CYAN));
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((format!("  Resolution: {}x{}", sw, sh), TEXT_PRIMARY));
    lines.push((String::from("  Renderer:   Software (CPU)"), TEXT_PRIMARY));
    lines.push((String::from("  Buffer:     Double-buffered"), TEXT_PRIMARY));
    lines.push((String::from("  Font:       TrueType (14px anti-aliased)"), TEXT_PRIMARY));
    lines.push((String::from("              8x16 bitmap fallback"), TEXT_PRIMARY));
    lines.push((String::from("  Theme:      Cyberpunk Neon Dark"), TEXT_PRIMARY));
    lines
}

fn build_network_info() -> Vec<(String, Color)> {
    let mut lines = Vec::new();
    lines.push((String::from("  ── Network Stack ──"), ACCENT_ORANGE));
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((format!("  IP:       {}.{}.{}.{}",
        crate::net::LOCAL_IP[0], crate::net::LOCAL_IP[1],
        crate::net::LOCAL_IP[2], crate::net::LOCAL_IP[3]), TEXT_PRIMARY));
    lines.push((format!("  Gateway:  {}.{}.{}.{}",
        crate::net::GATEWAY_IP[0], crate::net::GATEWAY_IP[1],
        crate::net::GATEWAY_IP[2], crate::net::GATEWAY_IP[3]), TEXT_PRIMARY));
    lines.push((format!("  Subnet:   {}.{}.{}.{}",
        crate::net::SUBNET_MASK[0], crate::net::SUBNET_MASK[1],
        crate::net::SUBNET_MASK[2], crate::net::SUBNET_MASK[3]), TEXT_PRIMARY));
    lines.push((String::new(), TEXT_PRIMARY));

    let tcp_conns = crate::net::tcp::connection_count();
    let tcp_listen = crate::net::tcp::listener_count();
    lines.push((format!("  TCP:      {} connection(s), {} listener(s)", tcp_conns, tcp_listen), TEXT_PRIMARY));
    lines.push((format!("  DNS:      Cache {} entries", crate::net::dns::cache_count()), TEXT_PRIMARY));
    lines
}

fn build_language_info() -> Vec<(String, Color)> {
    use crate::i18n::{LocaleId, locale, get_locale, format_number, format_date, format_currency, format_time};
    let current = get_locale();
    let loc     = locale(current);
    let mut lines = Vec::new();

    lines.push((String::from("  \u{2500}\u{2500} Language & Region \u{2500}\u{2500}"), ACCENT_MAGENTA));
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((format!("  Locale:     {}", loc.code),                              ACCENT_CYAN));
    lines.push((format!("  Language:   {}", loc.name),                              TEXT_PRIMARY));
    lines.push((format!("  Direction:  {}", if loc.is_rtl { "RTL" } else { "LTR" }), TEXT_PRIMARY));
    lines.push((format!("  Time:       {}", if loc.time_24h { "24-hour" } else { "12-hour AM/PM" }), TEXT_PRIMARY));
    lines.push((String::new(), TEXT_PRIMARY));

    lines.push((String::from("  \u{2500}\u{2500} Format Examples \u{2500}\u{2500}"), ACCENT_CYAN));
    lines.push((format!("  Number:     {}", format_number(1_234_567, current)),     TEXT_PRIMARY));
    lines.push((format!("  Date:       {}", format_date(2024, 12, 25, current)),    TEXT_PRIMARY));
    lines.push((format!("  Time:       {}", format_time(14, 30, 0, current)),       TEXT_PRIMARY));
    lines.push((format!("  Currency:   {}", format_currency(9999, current)),        TEXT_PRIMARY));
    lines.push((String::new(), TEXT_PRIMARY));

    lines.push((String::from("  \u{2500}\u{2500} Available Locales \u{2500}\u{2500}"), ACCENT_CYAN));
    for &id in LocaleId::all() {
        let l      = locale(id);
        let active = id == current;
        let marker = if active { "  \u{25BA} " } else { "    " };
        let color  = if active { ACCENT_GREEN } else { TEXT_SECONDARY };
        lines.push((format!("{}[{}]  {}", marker, l.code, l.name), color));
    }

    lines
}

fn build_about_info() -> Vec<(String, Color)> {
    let mut lines = Vec::new();
    lines.push((String::from("  \u{2500}\u{2500} About Smart OS \u{2500}\u{2500}"), ACCENT_MAGENTA));
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((String::from("  Smart OS v0.20.0"), ACCENT_CYAN));
    lines.push((String::from("  A real bootable x86_64 operating system"), TEXT_PRIMARY));
    lines.push((String::from("  Written in Rust (no_std, bare metal)"), TEXT_PRIMARY));
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((String::from("  \u{2500}\u{2500} Architecture \u{2500}\u{2500}"), ACCENT_CYAN));
    lines.push((String::from("  Hybrid microkernel"), TEXT_PRIMARY));
    lines.push((String::from("  UEFI + BIOS bootable"), TEXT_PRIMARY));
    lines.push((String::from("  SmartPack binary format"), TEXT_PRIMARY));
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((String::from("  \u{2500}\u{2500} Recent Phases \u{2500}\u{2500}"), ACCENT_CYAN));
    lines.push((String::from("  58: AC'97 Audio Engine"),     TEXT_SECONDARY));
    lines.push((String::from("  59: TrueType Font Rendering"), TEXT_SECONDARY));
    lines.push((String::from("  60: Unicode & Localization"),  TEXT_SECONDARY));
    lines
}

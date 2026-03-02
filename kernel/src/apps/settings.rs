/// Smart OS Settings — System control panel application.
///
/// Tabbed interface showing System, Display, Network, and About info.
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

        // Widget 0-3: Tab buttons
        let tab_w = 100;
        let tab_h = 22;
        win.widgets.push(Widget::new(0, 0, 0, tab_w, tab_h,
            WidgetKind::Button(Button::new("System", ACCENT_GREEN, AppCommand::ButtonClicked(0)))));
        win.widgets.push(Widget::new(1, tab_w + 4, 0, tab_w, tab_h,
            WidgetKind::Button(Button::new("Display", ACCENT_CYAN, AppCommand::ButtonClicked(1)))));
        win.widgets.push(Widget::new(2, (tab_w + 4) * 2, 0, tab_w, tab_h,
            WidgetKind::Button(Button::new("Network", ACCENT_ORANGE, AppCommand::ButtonClicked(2)))));
        win.widgets.push(Widget::new(3, (tab_w + 4) * 3, 0, tab_w, tab_h,
            WidgetKind::Button(Button::new("About", ACCENT_MAGENTA, AppCommand::ButtonClicked(3)))));

        // Widget 4: Content area (scrollable text)
        win.widgets.push(Widget::new(4, 0, 28, 440, 280,
            WidgetKind::ScrollText(ScrollableText::new(200))));

        win.focused_widget = Some(4);
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
        SettingsTab::System => build_system_info(),
        SettingsTab::Display => build_display_info(),
        SettingsTab::Network => build_network_info(),
        SettingsTab::About => build_about_info(),
    };

    if let Some(w) = window.widgets.get_mut(4) {
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
    lines.push((format!("  Kernel:    Smart OS v0.9.0"), TEXT_PRIMARY));
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
    let (sw, sh) = crate::gui::compositor::screen_size();
    let mut lines = Vec::new();
    lines.push((String::from("  ── Display Information ──"), ACCENT_CYAN));
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((format!("  Resolution: {}x{}", sw, sh), TEXT_PRIMARY));
    lines.push((String::from("  Renderer:   Software (CPU)"), TEXT_PRIMARY));
    lines.push((String::from("  Buffer:     Double-buffered"), TEXT_PRIMARY));
    lines.push((String::from("  Font:       8x16 bitmap (small)"), TEXT_PRIMARY));
    lines.push((String::from("              10x20 bitmap (large)"), TEXT_PRIMARY));
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

fn build_about_info() -> Vec<(String, Color)> {
    let mut lines = Vec::new();
    lines.push((String::from("  ── About Smart OS ──"), ACCENT_MAGENTA));
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((String::from("  Smart OS v0.9.0"), ACCENT_CYAN));
    lines.push((String::from("  A real bootable x86_64 operating system"), TEXT_PRIMARY));
    lines.push((String::from("  Written in Rust (no_std, bare metal)"), TEXT_PRIMARY));
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((String::from("  ── Architecture ──"), ACCENT_CYAN));
    lines.push((String::from("  Hybrid microkernel"), TEXT_PRIMARY));
    lines.push((String::from("  UEFI + BIOS bootable"), TEXT_PRIMARY));
    lines.push((String::from("  SmartPack binary format"), TEXT_PRIMARY));
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((String::from("  ── Phases Complete ──"), ACCENT_CYAN));
    lines.push((String::from("  1: Boot + SmartPack"), TEXT_SECONDARY));
    lines.push((String::from("  2: Drivers + IPC + GUI"), TEXT_SECONDARY));
    lines.push((String::from("  3: AI + SmartFS + Plugins"), TEXT_SECONDARY));
    lines.push((String::from("  4: Apps + Widgets"), TEXT_SECONDARY));
    lines.push((String::from("  5: User-space (ring-3)"), TEXT_SECONDARY));
    lines.push((String::from("  6: Network + Storage + SMP"), TEXT_SECONDARY));
    lines.push((String::from("  7: AI Intelligence"), TEXT_SECONDARY));
    lines.push((String::from("  8: TCP + USB + FAT32"), TEXT_SECONDARY));
    lines.push((String::from("  9: Desktop OS Polish"), TEXT_SECONDARY));
    lines
}

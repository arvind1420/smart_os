//! Accessibility Engine for Smart OS — Phase 43.
//!
//! Provides a kernel-level accessibility bus (A-Bus) for UI traversal
//! and a background screen reader daemon.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use crate::gui::window::WindowId;

#[derive(Debug, Clone)]
pub enum UIRole {
    Window,
    Button,
    TextInput,
    Label,
    Icon,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct UINode {
    pub role: UIRole,
    pub label: String,
    pub focused: bool,
}

/// Global Accessibility Bus state.
pub struct AccessibilityBus {
    pub enabled: bool,
    pub last_focused_desc: String,
}

pub static ABUS: Mutex<AccessibilityBus> = Mutex::new(AccessibilityBus {
    enabled: true,
    last_focused_desc: String::new(),
});

/// Notify the accessibility bus that focus has changed.
pub fn on_focus_changed(role: UIRole, label: &str) {
    let mut abus = ABUS.lock();
    if !abus.enabled { return; }

    let desc = format!("{}, {:?}", label, role);
    if desc != abus.last_focused_desc {
        abus.last_focused_desc = desc.clone();
        // The screen reader thread will pick this up or we can call TTS directly
        crate::ai::speech::say(&desc);
    }
}

/// Initialize the accessibility engine.
pub fn init() {
    crate::serial_println!("[accessibility] Accessibility Engine (A-Bus) initialized.");
    // In a full implementation, we'd spawn the screen-reader daemon here.
}

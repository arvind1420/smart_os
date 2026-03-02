/// Theming System for Smart OS.
///
/// Phase 10: User-selectable color themes. Provides multiple
/// built-in themes (Cyberpunk, Ocean, Forest, Sunset, Monochrome)
/// and the ability to switch at runtime.
///
/// Theme colors override the static constants in theme.rs
/// at runtime via accessor functions.

use core::sync::atomic::{AtomicUsize, Ordering};
use super::theme::Color;
use alloc::vec::Vec;
use crate::serial_println;

/// A complete theme definition.
#[derive(Debug, Clone, Copy)]
pub struct ThemeColors {
    pub bg_primary: Color,
    pub bg_secondary: Color,
    pub bg_panel: Color,
    pub bg_taskbar: Color,
    pub bg_titlebar: Color,
    pub bg_titlebar_active: Color,
    pub accent_primary: Color,
    pub accent_secondary: Color,
    pub accent_success: Color,
    pub accent_warning: Color,
    pub accent_error: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub border_glow: Color,
    pub border_inactive: Color,
}

/// Built-in theme definitions.
pub const THEME_CYBERPUNK: ThemeColors = ThemeColors {
    bg_primary: Color::rgb(8, 8, 16),
    bg_secondary: Color::rgb(14, 14, 28),
    bg_panel: Color::rgb(18, 18, 36),
    bg_taskbar: Color::rgb(10, 10, 22),
    bg_titlebar: Color::rgb(20, 12, 40),
    bg_titlebar_active: Color::rgb(30, 15, 60),
    accent_primary: Color::rgb(0, 255, 255),
    accent_secondary: Color::rgb(255, 0, 255),
    accent_success: Color::rgb(0, 255, 128),
    accent_warning: Color::rgb(255, 160, 0),
    accent_error: Color::rgb(255, 50, 50),
    text_primary: Color::rgb(230, 230, 245),
    text_secondary: Color::rgb(140, 140, 170),
    text_muted: Color::rgb(80, 80, 110),
    border_glow: Color::rgb(0, 180, 200),
    border_inactive: Color::rgb(40, 40, 60),
};

pub const THEME_OCEAN: ThemeColors = ThemeColors {
    bg_primary: Color::rgb(5, 15, 30),
    bg_secondary: Color::rgb(10, 25, 45),
    bg_panel: Color::rgb(15, 30, 55),
    bg_taskbar: Color::rgb(5, 12, 25),
    bg_titlebar: Color::rgb(10, 30, 60),
    bg_titlebar_active: Color::rgb(15, 45, 80),
    accent_primary: Color::rgb(64, 160, 255),
    accent_secondary: Color::rgb(100, 200, 255),
    accent_success: Color::rgb(50, 220, 150),
    accent_warning: Color::rgb(255, 180, 50),
    accent_error: Color::rgb(255, 80, 80),
    text_primary: Color::rgb(220, 235, 255),
    text_secondary: Color::rgb(130, 160, 200),
    text_muted: Color::rgb(70, 100, 140),
    border_glow: Color::rgb(50, 140, 220),
    border_inactive: Color::rgb(30, 50, 80),
};

pub const THEME_FOREST: ThemeColors = ThemeColors {
    bg_primary: Color::rgb(10, 18, 10),
    bg_secondary: Color::rgb(15, 28, 15),
    bg_panel: Color::rgb(20, 35, 20),
    bg_taskbar: Color::rgb(8, 14, 8),
    bg_titlebar: Color::rgb(15, 35, 15),
    bg_titlebar_active: Color::rgb(20, 50, 20),
    accent_primary: Color::rgb(80, 220, 80),
    accent_secondary: Color::rgb(160, 255, 100),
    accent_success: Color::rgb(50, 255, 100),
    accent_warning: Color::rgb(240, 200, 50),
    accent_error: Color::rgb(255, 70, 70),
    text_primary: Color::rgb(220, 240, 220),
    text_secondary: Color::rgb(140, 170, 140),
    text_muted: Color::rgb(80, 110, 80),
    border_glow: Color::rgb(60, 180, 60),
    border_inactive: Color::rgb(35, 55, 35),
};

pub const THEME_SUNSET: ThemeColors = ThemeColors {
    bg_primary: Color::rgb(20, 8, 8),
    bg_secondary: Color::rgb(35, 14, 14),
    bg_panel: Color::rgb(45, 18, 18),
    bg_taskbar: Color::rgb(18, 6, 6),
    bg_titlebar: Color::rgb(50, 15, 25),
    bg_titlebar_active: Color::rgb(70, 20, 35),
    accent_primary: Color::rgb(255, 120, 50),
    accent_secondary: Color::rgb(255, 80, 120),
    accent_success: Color::rgb(100, 230, 100),
    accent_warning: Color::rgb(255, 200, 50),
    accent_error: Color::rgb(255, 50, 50),
    text_primary: Color::rgb(255, 230, 220),
    text_secondary: Color::rgb(200, 150, 140),
    text_muted: Color::rgb(140, 90, 80),
    border_glow: Color::rgb(255, 100, 60),
    border_inactive: Color::rgb(60, 30, 30),
};

pub const THEME_MONOCHROME: ThemeColors = ThemeColors {
    bg_primary: Color::rgb(10, 10, 10),
    bg_secondary: Color::rgb(20, 20, 20),
    bg_panel: Color::rgb(28, 28, 28),
    bg_taskbar: Color::rgb(8, 8, 8),
    bg_titlebar: Color::rgb(25, 25, 25),
    bg_titlebar_active: Color::rgb(40, 40, 40),
    accent_primary: Color::rgb(200, 200, 200),
    accent_secondary: Color::rgb(160, 160, 160),
    accent_success: Color::rgb(180, 255, 180),
    accent_warning: Color::rgb(255, 255, 150),
    accent_error: Color::rgb(255, 120, 120),
    text_primary: Color::rgb(230, 230, 230),
    text_secondary: Color::rgb(150, 150, 150),
    text_muted: Color::rgb(90, 90, 90),
    border_glow: Color::rgb(160, 160, 160),
    border_inactive: Color::rgb(50, 50, 50),
};

/// All built-in themes.
const THEMES: [ThemeColors; 5] = [
    THEME_CYBERPUNK,
    THEME_OCEAN,
    THEME_FOREST,
    THEME_SUNSET,
    THEME_MONOCHROME,
];

/// Theme names.
const THEME_NAMES: [&str; 5] = ["Cyberpunk", "Ocean", "Forest", "Sunset", "Monochrome"];

/// Current active theme index.
static CURRENT_THEME: AtomicUsize = AtomicUsize::new(0);

/// Get the current active theme colors.
pub fn current_theme() -> ThemeColors {
    let idx = CURRENT_THEME.load(Ordering::Relaxed);
    THEMES[idx.min(THEMES.len() - 1)]
}

/// Get the current theme name.
pub fn current_theme_name() -> &'static str {
    let idx = CURRENT_THEME.load(Ordering::Relaxed);
    THEME_NAMES[idx.min(THEME_NAMES.len() - 1)]
}

/// Switch to a theme by index.
pub fn set_theme(index: usize) {
    if index < THEMES.len() {
        let prev = CURRENT_THEME.swap(index, Ordering::Relaxed);
        if prev != index {
            serial_println!("[theme] Switched to '{}' theme.", THEME_NAMES[index]);
        }
    }
}

/// Switch to the next theme (cycles).
pub fn next_theme() -> usize {
    let cur = CURRENT_THEME.load(Ordering::Relaxed);
    let next = (cur + 1) % THEMES.len();
    set_theme(next);
    next
}

/// Get the number of available themes.
pub fn theme_count() -> usize {
    THEMES.len()
}

/// List all available themes.
pub fn list_themes() -> Vec<(usize, &'static str)> {
    THEME_NAMES.iter().enumerate().map(|(i, &n)| (i, n)).collect()
}

/// Initialize the theming system.
pub fn init() {
    serial_println!("[theme] Theming system initialized ({} themes).", THEMES.len());
}

/// Convenience: get the accent color from the active theme.
pub fn accent() -> Color {
    current_theme().accent_primary
}

/// Convenience: get the background color from the active theme.
pub fn background() -> Color {
    current_theme().bg_primary
}

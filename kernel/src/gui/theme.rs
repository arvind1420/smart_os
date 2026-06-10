/// Smart OS Modern Dark Theme.
///
/// Windows 11-inspired dark mode with clean surfaces and
/// vivid accent colors. Keeps neon accents for personality.

/// A color in RGB format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Blend this color toward another by factor (0-255).
    pub fn blend(self, other: Color, factor: u8) -> Color {
        let f = factor as u16;
        let inv = 255 - f as u16;
        Color {
            r: ((self.r as u16 * inv + other.r as u16 * f) / 255) as u8,
            g: ((self.g as u16 * inv + other.g as u16 * f) / 255) as u8,
            b: ((self.b as u16 * inv + other.b as u16 * f) / 255) as u8,
        }
    }

    /// Dim the color by a factor (0-255, 255 = full brightness).
    pub fn dim(self, factor: u8) -> Color {
        Color {
            r: (self.r as u16 * factor as u16 / 255) as u8,
            g: (self.g as u16 * factor as u16 / 255) as u8,
            b: (self.b as u16 * factor as u16 / 255) as u8,
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  MODERN DARK THEME — Windows 11-inspired
// ═══════════════════════════════════════════════════════════════

/// Dark surface — primary background (deep charcoal)
pub const BG_PRIMARY: Color = Color::rgb(18, 18, 20);
/// Elevated surface — secondary panels
pub const BG_SECONDARY: Color = Color::rgb(28, 28, 32);
/// Card/window content background
pub const BG_PANEL: Color = Color::rgb(36, 36, 40);
/// Taskbar background (frosted-style dark)
pub const BG_TASKBAR: Color = Color::rgb(22, 22, 26);
/// Inactive window title bar
pub const BG_TITLEBAR: Color = Color::rgb(42, 42, 46);
/// Active window title bar (slight blue tint)
pub const BG_TITLEBAR_ACTIVE: Color = Color::rgb(40, 44, 58);

/// Windows blue — primary system accent
pub const ACCENT_BLUE: Color = Color::rgb(0, 120, 212);
/// Sky blue — lighter accent
pub const ACCENT_CYAN: Color = Color::rgb(0, 188, 242);
/// Magenta — secondary accent
pub const ACCENT_MAGENTA: Color = Color::rgb(200, 60, 200);
/// Success green
pub const ACCENT_GREEN: Color = Color::rgb(22, 198, 12);
/// Warning amber
pub const ACCENT_ORANGE: Color = Color::rgb(255, 185, 0);
/// Error/close red
pub const ACCENT_RED: Color = Color::rgb(196, 43, 28);
/// Purple — highlight
pub const ACCENT_PURPLE: Color = Color::rgb(136, 23, 152);

/// Bright white text
pub const TEXT_PRIMARY: Color = Color::rgb(242, 242, 242);
/// Secondary text (slightly dimmed)
pub const TEXT_SECONDARY: Color = Color::rgb(160, 160, 168);
/// Muted/hint text
pub const TEXT_MUTED: Color = Color::rgb(100, 100, 108);

/// Active window border (accent blue)
pub const BORDER_GLOW: Color = Color::rgb(0, 120, 212);
/// Inactive border (subtle gray)
pub const BORDER_INACTIVE: Color = Color::rgb(58, 58, 64);

// ═══════════════════════════════════════════════════════════════
//  Interactive Widget Colors
// ═══════════════════════════════════════════════════════════════

/// Text input background
pub const BG_INPUT: Color = Color::rgb(28, 28, 32);
/// Focused text input background
pub const BG_INPUT_FOCUSED: Color = Color::rgb(34, 34, 42);
/// Button background
pub const BG_BUTTON: Color = Color::rgb(44, 44, 52);
/// Button hover
pub const BG_BUTTON_HOVER: Color = Color::rgb(58, 58, 68);
/// Text cursor color
pub const CURSOR_COLOR: Color = Color::rgb(0, 120, 212);
/// Scrollbar track background
pub const SCROLLBAR_BG: Color = Color::rgb(30, 30, 34);
/// Scrollbar thumb
pub const SCROLLBAR_FG: Color = Color::rgb(80, 80, 90);

// ═══════════════════════════════════════════════════════════════
//  Layout Constants
// ═══════════════════════════════════════════════════════════════

/// Taskbar height in pixels (48 for modern look).
pub const TASKBAR_HEIGHT: usize = 48;
/// Window title bar height (28 for modern look).
pub const TITLEBAR_HEIGHT: usize = 28;
/// Window border width.
pub const BORDER_WIDTH: usize = 1;
/// Window glow/shadow size.
pub const GLOW_SIZE: usize = 2;
/// Widget internal padding.
pub const WIDGET_PADDING: usize = 4;
/// Text input widget height.
pub const INPUT_HEIGHT: usize = 24;
/// Button widget height.
pub const BUTTON_HEIGHT: usize = 28;

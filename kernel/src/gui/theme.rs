/// Smart OS Cyberpunk/Neon Dark Theme.
///
/// A futuristic visual theme with deep dark backgrounds,
/// neon accent colors, and glowing effects.

/// A color in RGB format.
#[derive(Debug, Clone, Copy)]
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
//  NEON DARK THEME — Cyberpunk aesthetic
// ═══════════════════════════════════════════════════════════════

/// Deep space black — primary background
pub const BG_PRIMARY: Color = Color::rgb(8, 8, 16);
/// Slightly lighter — secondary panels
pub const BG_SECONDARY: Color = Color::rgb(14, 14, 28);
/// Panel/card background
pub const BG_PANEL: Color = Color::rgb(18, 18, 36);
/// Taskbar background
pub const BG_TASKBAR: Color = Color::rgb(10, 10, 22);
/// Window title bar
pub const BG_TITLEBAR: Color = Color::rgb(20, 12, 40);
/// Active window title bar
pub const BG_TITLEBAR_ACTIVE: Color = Color::rgb(30, 15, 60);

/// Neon cyan — primary accent
pub const ACCENT_CYAN: Color = Color::rgb(0, 255, 255);
/// Neon magenta — secondary accent
pub const ACCENT_MAGENTA: Color = Color::rgb(255, 0, 255);
/// Neon green — success/active
pub const ACCENT_GREEN: Color = Color::rgb(0, 255, 128);
/// Neon orange — warning
pub const ACCENT_ORANGE: Color = Color::rgb(255, 160, 0);
/// Neon red — error/close
pub const ACCENT_RED: Color = Color::rgb(255, 50, 50);
/// Neon blue — info
pub const ACCENT_BLUE: Color = Color::rgb(60, 120, 255);
/// Neon purple — highlight
pub const ACCENT_PURPLE: Color = Color::rgb(180, 60, 255);

/// Bright white text
pub const TEXT_PRIMARY: Color = Color::rgb(230, 230, 245);
/// Dimmed text
pub const TEXT_SECONDARY: Color = Color::rgb(140, 140, 170);
/// Muted text
pub const TEXT_MUTED: Color = Color::rgb(80, 80, 110);

/// Window border (glowing effect — dimmed accent)
pub const BORDER_GLOW: Color = Color::rgb(0, 180, 200);
/// Inactive border
pub const BORDER_INACTIVE: Color = Color::rgb(40, 40, 60);

// ═══════════════════════════════════════════════════════════════
//  Interactive Widget Colors
// ═══════════════════════════════════════════════════════════════

/// Text input background
pub const BG_INPUT: Color = Color::rgb(12, 12, 30);
/// Focused text input background
pub const BG_INPUT_FOCUSED: Color = Color::rgb(18, 18, 45);
/// Button background
pub const BG_BUTTON: Color = Color::rgb(20, 15, 45);
/// Button hover
pub const BG_BUTTON_HOVER: Color = Color::rgb(30, 20, 65);
/// Neon text cursor color
pub const CURSOR_COLOR: Color = Color::rgb(0, 255, 200);
/// Scrollbar track background
pub const SCROLLBAR_BG: Color = Color::rgb(15, 15, 30);
/// Scrollbar thumb
pub const SCROLLBAR_FG: Color = Color::rgb(60, 60, 100);

// ═══════════════════════════════════════════════════════════════
//  Layout Constants
// ═══════════════════════════════════════════════════════════════

/// Taskbar height in pixels.
pub const TASKBAR_HEIGHT: usize = 32;
/// Window title bar height.
pub const TITLEBAR_HEIGHT: usize = 24;
/// Window border width.
pub const BORDER_WIDTH: usize = 1;
/// Window corner radius (visual only, rendered as glow).
pub const GLOW_SIZE: usize = 2;
/// Widget internal padding.
pub const WIDGET_PADDING: usize = 4;
/// Text input widget height.
pub const INPUT_HEIGHT: usize = 22;
/// Button widget height.
pub const BUTTON_HEIGHT: usize = 24;

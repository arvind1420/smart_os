/// Smart OS Widgets — Reusable UI components with neon aesthetics.
///
/// Panels, labels, status bars, progress indicators, and system info displays.

use alloc::string::String;
use alloc::format;
use super::theme::*;
use super::compositor::Compositor;

/// A neon-accented panel with optional title.
pub struct Panel {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub title: Option<String>,
    pub accent: Color,
    pub bg: Color,
}

impl Panel {
    pub fn new(x: usize, y: usize, width: usize, height: usize, accent: Color) -> Self {
        Self {
            x,
            y,
            width,
            height,
            title: None,
            accent,
            bg: BG_PANEL,
        }
    }

    pub fn with_title(mut self, title: &str) -> Self {
        self.title = Some(String::from(title));
        self
    }

    pub fn render(&self, comp: &mut Compositor) {
        // Background
        comp.fill_rect(self.x, self.y, self.width, self.height, self.bg);

        // Neon border (single pixel, accented)
        comp.draw_rect(self.x, self.y, self.width, self.height, self.accent.dim(120));

        // Top accent line (brighter)
        comp.hline(self.x, self.y, self.width, self.accent);

        // Title if present
        if let Some(ref title) = self.title {
            let title_x = self.x + 8;
            let title_y = self.y + 4;
            comp.draw_text(title_x, title_y, title, self.accent);
        }
    }
}

/// A text label.
pub struct Label {
    pub x: usize,
    pub y: usize,
    pub text: String,
    pub color: Color,
}

impl Label {
    pub fn new(x: usize, y: usize, text: &str, color: Color) -> Self {
        Self {
            x,
            y,
            text: String::from(text),
            color,
        }
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = String::from(text);
    }

    pub fn render(&self, comp: &mut Compositor) {
        comp.draw_text(self.x, self.y, &self.text, self.color);
    }
}

/// A horizontal progress bar with neon fill.
pub struct ProgressBar {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub value: f32, // 0.0 - 1.0
    pub accent: Color,
}

impl ProgressBar {
    pub fn new(x: usize, y: usize, width: usize, height: usize, accent: Color) -> Self {
        Self {
            x,
            y,
            width,
            height,
            value: 0.0,
            accent,
        }
    }

    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(0.0, 1.0);
    }

    pub fn render(&self, comp: &mut Compositor) {
        // Background track
        comp.fill_rect(self.x, self.y, self.width, self.height, BG_SECONDARY);
        comp.draw_rect(self.x, self.y, self.width, self.height, self.accent.dim(60));

        // Filled portion with gradient
        let fill_w = (self.width as f32 * self.value) as usize;
        if fill_w > 0 {
            comp.draw_gradient_h(
                self.x,
                self.y,
                fill_w,
                self.height,
                self.accent.dim(180),
                self.accent,
            );
        }
    }
}

/// System info display widget — shows OS stats in a panel.
pub struct SystemInfoWidget {
    pub x: usize,
    pub y: usize,
    pub width: usize,
}

impl SystemInfoWidget {
    pub fn new(x: usize, y: usize, width: usize) -> Self {
        Self { x, y, width }
    }

    pub fn render(&self, comp: &mut Compositor, uptime_secs: u64, heap_used: usize, heap_free: usize, threads: usize) {
        let panel_height = 110;

        // Panel background
        let panel = Panel::new(self.x, self.y, self.width, panel_height, ACCENT_CYAN)
            .with_title("SYSTEM");
        panel.render(comp);

        let text_x = self.x + 10;
        let mut y = self.y + 24;

        // Uptime
        let mins = uptime_secs / 60;
        let secs = uptime_secs % 60;
        let uptime_str = format!("UPTIME  {:02}:{:02}", mins, secs);
        comp.draw_text(text_x, y, &uptime_str, ACCENT_GREEN);
        y += 20;

        // Memory usage
        let total = heap_used + heap_free;
        let usage_pct = if total > 0 {
            (heap_used as f32 / total as f32) * 100.0
        } else {
            0.0
        };
        let mem_str = format!("MEMORY  {:.0}% ({}/{}K)", usage_pct, heap_used / 1024, total / 1024);
        comp.draw_text(text_x, y, &mem_str, ACCENT_CYAN);
        y += 20;

        // Memory bar
        let bar = ProgressBar {
            x: text_x,
            y,
            width: self.width - 20,
            height: 6,
            value: usage_pct / 100.0,
            accent: if usage_pct > 80.0 { ACCENT_RED } else { ACCENT_CYAN },
        };
        bar.render(comp);
        y += 14;

        // Thread count
        let thread_str = format!("THREADS {}", threads);
        comp.draw_text(text_x, y, &thread_str, ACCENT_MAGENTA);
    }
}

/// The taskbar at the bottom of the screen.
pub struct Taskbar {
    pub screen_width: usize,
    pub screen_height: usize,
}

impl Taskbar {
    pub fn new(screen_width: usize, screen_height: usize) -> Self {
        Self {
            screen_width,
            screen_height,
        }
    }

    pub fn render(
        &self,
        comp: &mut Compositor,
        window_list: &[(u64, &str, bool, Color)],
        _uptime_secs: u64,
    ) {
        let bar_y = self.screen_height.saturating_sub(TASKBAR_HEIGHT);
        let bar_h = TASKBAR_HEIGHT;
        let icon_size = 32usize;
        let icon_pad = (bar_h.saturating_sub(icon_size)) / 2;
        let text_y = bar_y + (bar_h.saturating_sub(16)) / 2;

        // ── Taskbar background (frosted dark) ──
        comp.fill_rect(0, bar_y, self.screen_width, bar_h, BG_TASKBAR);
        // Subtle top separator
        comp.hline(0, bar_y, self.screen_width, BORDER_INACTIVE);

        // ── Start Button (left side) ──
        let start_w = 48usize;
        let start_x = 4usize;
        let start_icon_x = start_x + (start_w - icon_size) / 2;
        let start_icon_y = bar_y + icon_pad;
        // Draw 4-quadrant Windows-like logo
        let q = icon_size / 2 - 1;
        comp.fill_rect(start_icon_x,     start_icon_y,     q, q, ACCENT_BLUE);
        comp.fill_rect(start_icon_x + q + 2, start_icon_y, q, q, ACCENT_GREEN.dim(200));
        comp.fill_rect(start_icon_x,     start_icon_y + q + 2, q, q, ACCENT_ORANGE.dim(200));
        comp.fill_rect(start_icon_x + q + 2, start_icon_y + q + 2, q, q, ACCENT_RED.dim(200));

        // ── Search bar (next to Start) ──
        let search_x = start_x + start_w + 6;
        let search_w = 160usize;
        let search_y = bar_y + icon_pad;
        let search_h = icon_size;
        comp.fill_rect(search_x, search_y, search_w, search_h, BG_SECONDARY);
        comp.draw_rect(search_x, search_y, search_w, search_h, BORDER_INACTIVE);
        comp.draw_text(search_x + 6, search_y + (search_h.saturating_sub(16)) / 2,
            "Search...", TEXT_MUTED);

        // ── Center: Running app buttons ──
        let apps_area_start = search_x + search_w + 8;
        let apps_area_end = self.screen_width.saturating_sub(200);
        let mut btn_x = apps_area_start;

        for &(_, title, active, accent) in window_list {
            let btn_w = (title.len() * 7 + 20).min(120);
            if btn_x + btn_w > apps_area_end { break; }

            let btn_y = bar_y + icon_pad;
            let btn_h = icon_size;

            if active {
                // Active: filled with accent tint + bottom accent bar
                comp.fill_rect(btn_x, btn_y, btn_w, btn_h, accent.dim(35));
                comp.draw_rect(btn_x, btn_y, btn_w, btn_h, accent.dim(80));
                // Bottom accent bar
                comp.hline(btn_x, bar_y + bar_h - 3, btn_w, accent);
                comp.hline(btn_x, bar_y + bar_h - 4, btn_w, accent.dim(140));
                comp.draw_text(btn_x + 8, btn_y + (btn_h.saturating_sub(16)) / 2, title, TEXT_PRIMARY);
            } else {
                // Inactive: subtle border on hover (static: just dim text)
                comp.fill_rect(btn_x, btn_y, btn_w, btn_h, BG_SECONDARY);
                comp.draw_rect(btn_x, btn_y, btn_w, btn_h, BORDER_INACTIVE);
                // Dim dot indicator (window exists but not active)
                comp.fill_rect(btn_x + btn_w / 2 - 2, bar_y + bar_h - 4, 4, 2, TEXT_MUTED);
                comp.draw_text(btn_x + 8, btn_y + (btn_h.saturating_sub(16)) / 2, title, TEXT_SECONDARY);
            }
            btn_x += btn_w + 4;
        }

        // ── System Tray (right side) ──
        let tray_right = self.screen_width.saturating_sub(4);

        // Clock + date
        let dt = crate::drivers::rtc::now();
        let clock_str = format!("{:02}:{:02}", dt.hour, dt.minute);
        let date_str = format!("{:04}-{:02}-{:02}", dt.year, dt.month, dt.day);
        let clock_w = clock_str.len() * 8;
        let date_w = date_str.len() * 8;
        let tray_w = clock_w.max(date_w) + 16;
        let tray_x = tray_right.saturating_sub(tray_w);

        comp.draw_text(tray_x + 8, bar_y + 6, &clock_str, TEXT_PRIMARY);
        comp.draw_text(tray_x + 8, bar_y + 24, &date_str, TEXT_SECONDARY);

        // Tray separator
        comp.vline(tray_x - 6, bar_y + 8, bar_h - 16, BORDER_INACTIVE);

        // Network/notification indicator (just a colored dot)
        let notif_x = tray_x.saturating_sub(20);
        comp.fill_rect(notif_x, bar_y + icon_pad + 8, 8, 8, ACCENT_BLUE.dim(180));

        // Status dot (uptime-based color: green=ok)
        let status_x = notif_x.saturating_sub(16);
        comp.fill_rect(status_x, bar_y + icon_pad + 8, 8, 8, ACCENT_GREEN.dim(180));

        // Frame/thread info (dev overlay — small, muted)
        let _ = text_y;
    }
}

/// A large centered title for splash/boot screens.
pub fn draw_splash_title(comp: &mut Compositor, screen_w: usize, screen_h: usize) {
    let title = "SMART OS";
    let subtitle = "Cyberpunk Desktop Environment v0.9.0";

    // Center the title
    let title_w = title.len() * 8;
    let title_x = (screen_w.saturating_sub(title_w)) / 2;
    let title_y = screen_h / 3;

    // Draw title with glow effect
    // Shadow/glow layers
    comp.draw_text(title_x + 1, title_y + 1, title, ACCENT_CYAN.dim(60));
    comp.draw_text(title_x, title_y, title, ACCENT_CYAN);

    // Accent underline
    let line_w = title_w + 40;
    let line_x = (screen_w.saturating_sub(line_w)) / 2;
    comp.draw_gradient_h(line_x, title_y + 20, line_w, 2, ACCENT_MAGENTA.dim(40), ACCENT_CYAN);
    comp.draw_gradient_h(line_x, title_y + 22, line_w, 1, ACCENT_CYAN, ACCENT_MAGENTA.dim(40));

    // Subtitle
    let sub_w = subtitle.len() * 8;
    let sub_x = (screen_w.saturating_sub(sub_w)) / 2;
    comp.draw_text(sub_x, title_y + 30, subtitle, TEXT_SECONDARY);
}

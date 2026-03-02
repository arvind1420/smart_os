/// Alt+Tab Window Switching for Smart OS.
///
/// Phase 10: Shows a visual window switcher overlay when Alt+Tab is pressed.
/// Cycles through open windows, highlights the selected one, and switches
/// focus on release.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use super::compositor::Compositor;
use super::theme::*;
use super::window::WindowId;

/// Whether the Alt+Tab switcher is currently visible.
static ALT_TAB_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Current selection index in the window list.
static SELECTED_INDEX: Mutex<usize> = Mutex::new(0);

/// Cached window list for the current Alt+Tab session.
static WINDOW_LIST: Mutex<Vec<(WindowId, String)>> = Mutex::new(Vec::new());

/// Is Alt+Tab currently active?
pub fn is_active() -> bool {
    ALT_TAB_ACTIVE.load(Ordering::Relaxed)
}

/// Start an Alt+Tab switching session.
///
/// Captures the current window list and shows the switcher.
pub fn start(windows: Vec<(WindowId, String)>) {
    if windows.is_empty() {
        return;
    }

    *WINDOW_LIST.lock() = windows;
    *SELECTED_INDEX.lock() = 0;
    ALT_TAB_ACTIVE.store(true, Ordering::Relaxed);
}

/// Move to the next window in the Alt+Tab list.
pub fn next() {
    let list = WINDOW_LIST.lock();
    let len = list.len();
    if len == 0 { return; }

    let mut idx = SELECTED_INDEX.lock();
    *idx = (*idx + 1) % len;
}

/// Move to the previous window in the Alt+Tab list.
pub fn prev() {
    let list = WINDOW_LIST.lock();
    let len = list.len();
    if len == 0 { return; }

    let mut idx = SELECTED_INDEX.lock();
    if *idx == 0 {
        *idx = len - 1;
    } else {
        *idx -= 1;
    }
}

/// Finish Alt+Tab and return the selected window ID.
pub fn finish() -> Option<WindowId> {
    if !ALT_TAB_ACTIVE.load(Ordering::Relaxed) {
        return None;
    }

    ALT_TAB_ACTIVE.store(false, Ordering::Relaxed);

    let list = WINDOW_LIST.lock();
    let idx = *SELECTED_INDEX.lock();

    list.get(idx).map(|(id, _)| *id)
}

/// Cancel Alt+Tab without switching.
pub fn cancel() {
    ALT_TAB_ACTIVE.store(false, Ordering::Relaxed);
}

/// Render the Alt+Tab overlay.
pub fn render(comp: &mut Compositor, screen_w: usize, screen_h: usize) {
    if !ALT_TAB_ACTIVE.load(Ordering::Relaxed) {
        return;
    }

    let list = WINDOW_LIST.lock();
    let selected = *SELECTED_INDEX.lock();

    if list.is_empty() {
        return;
    }

    // Calculate overlay dimensions
    let item_width = 160;
    let item_height = 40;
    let padding = 8;
    let max_visible = 6;
    let visible_count = list.len().min(max_visible);

    let overlay_w = item_width + padding * 2;
    let overlay_h = visible_count * (item_height + padding) + padding;

    // Center on screen
    let ox = (screen_w.saturating_sub(overlay_w)) / 2;
    let oy = (screen_h.saturating_sub(overlay_h)) / 2;

    // Draw semi-transparent background
    let bg = Color::rgb(10, 10, 30);
    comp.fill_rect(ox, oy, overlay_w, overlay_h, bg);

    // Border glow
    comp.hline(ox, oy, overlay_w, ACCENT_CYAN.dim(120));
    comp.hline(ox, oy + overlay_h - 1, overlay_w, ACCENT_CYAN.dim(120));
    comp.vline(ox, oy, overlay_h, ACCENT_CYAN.dim(120));
    comp.vline(ox + overlay_w - 1, oy, overlay_h, ACCENT_CYAN.dim(120));

    // Title
    comp.draw_text(ox + padding, oy + 2, "Switch Window", ACCENT_CYAN);

    // Draw window items
    for (i, (_, title)) in list.iter().enumerate().take(max_visible) {
        let ix = ox + padding;
        let iy = oy + padding + 14 + i * (item_height + padding);

        if i == selected {
            // Highlighted item
            comp.fill_rect(ix, iy, item_width, item_height, BG_TITLEBAR_ACTIVE);
            comp.hline(ix, iy, item_width, ACCENT_CYAN);
            comp.hline(ix, iy + item_height - 1, item_width, ACCENT_CYAN);
            comp.vline(ix, iy, item_height, ACCENT_CYAN);
            comp.vline(ix + item_width - 1, iy, item_height, ACCENT_CYAN);

            // Window title
            let display = if title.len() > 18 {
                &title[..18]
            } else {
                title
            };
            comp.draw_text(ix + 8, iy + 12, display, TEXT_PRIMARY);

            // Arrow indicator
            comp.draw_text(ix + item_width - 20, iy + 12, ">", ACCENT_CYAN);
        } else {
            // Normal item
            comp.fill_rect(ix, iy, item_width, item_height, BG_PANEL);

            let display = if title.len() > 18 {
                &title[..18]
            } else {
                title
            };
            comp.draw_text(ix + 8, iy + 12, display, TEXT_SECONDARY);
        }
    }

    // Show count if more items than visible
    if list.len() > max_visible {
        let more = alloc::format!("... +{} more", list.len() - max_visible);
        let my = oy + overlay_h - 14;
        comp.draw_text(ox + padding, my, &more, TEXT_MUTED);
    }
}

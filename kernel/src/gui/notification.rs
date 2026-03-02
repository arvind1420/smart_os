/// Toast notification system for Smart OS.
///
/// Displays up to 3 temporary notifications stacked in the top-right corner.
/// Each notification auto-expires after its duration elapses.

use alloc::string::String;
use alloc::collections::VecDeque;
use spin::Mutex;
use super::compositor::Compositor;
use super::theme::*;

/// Default notification display duration in milliseconds.
const DEFAULT_DURATION_MS: u64 = 5000;

/// Maximum number of notifications rendered simultaneously.
const MAX_VISIBLE: usize = 3;

/// Notification dimensions and layout.
const NOTIF_WIDTH: usize = 260;
const NOTIF_HEIGHT: usize = 50;
const NOTIF_MARGIN_RIGHT: usize = 10;
const NOTIF_START_Y: usize = 40;
const NOTIF_SPACING: usize = 55;

/// A single toast notification.
pub struct Notification {
    pub title: String,
    pub message: String,
    pub color: Color,
    pub created_at: u64,
    pub duration_ms: u64,
}

/// Global notification queue.
pub static NOTIFICATIONS: Mutex<VecDeque<Notification>> = Mutex::new(VecDeque::new());

/// Push a notification with the default 5-second duration.
pub fn push(title: &str, message: &str, color: Color) {
    push_with_duration(title, message, color, DEFAULT_DURATION_MS);
}

/// Push a notification with a custom duration in milliseconds.
pub fn push_with_duration(title: &str, message: &str, color: Color, duration_ms: u64) {
    let notif = Notification {
        title: String::from(title),
        message: String::from(message),
        color,
        created_at: crate::drivers::timer::uptime_ms(),
        duration_ms,
    };
    NOTIFICATIONS.lock().push_back(notif);
}

/// Render active notifications on the compositor.
///
/// Draws up to 3 toast popups stacked in the top-right corner of the screen.
/// Expired notifications are removed automatically.
pub fn render(comp: &mut Compositor, screen_w: usize, _screen_h: usize) {
    let current_ms = crate::drivers::timer::uptime_ms();

    let mut queue = NOTIFICATIONS.lock();

    // Remove expired notifications from the front.
    while let Some(front) = queue.front() {
        if current_ms.saturating_sub(front.created_at) > front.duration_ms {
            queue.pop_front();
        } else {
            break;
        }
    }

    // Render up to MAX_VISIBLE notifications.
    let visible_count = queue.len().min(MAX_VISIBLE);
    let base_x = screen_w.saturating_sub(NOTIF_WIDTH + NOTIF_MARGIN_RIGHT);

    for i in 0..visible_count {
        let notif = &queue[i];
        let y = NOTIF_START_Y + i * NOTIF_SPACING;

        // Background panel
        comp.fill_rect(base_x, y, NOTIF_WIDTH, NOTIF_HEIGHT, BG_PANEL);

        // Glow border (1px in notification's accent color)
        // Top and bottom edges
        comp.fill_rect(base_x, y, NOTIF_WIDTH, 1, notif.color);
        comp.fill_rect(base_x, y + NOTIF_HEIGHT - 1, NOTIF_WIDTH, 1, notif.color);
        // Left and right edges
        comp.fill_rect(base_x, y, 1, NOTIF_HEIGHT, notif.color);
        comp.fill_rect(base_x + NOTIF_WIDTH - 1, y, 1, NOTIF_HEIGHT, notif.color);

        // Title text (accent color, offset inside border)
        let text_x = base_x + 8;
        let title_y = y + 6;
        comp.draw_text(text_x, title_y, &notif.title, notif.color);

        // Message text (primary text color)
        let msg_y = y + 24;
        comp.draw_text(text_x, msg_y, &notif.message, TEXT_PRIMARY);
    }
}

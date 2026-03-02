/// Smart OS Desktop Environment — Main desktop composition.
///
/// Manages the overall desktop layout: background, taskbar, system info panel,
/// window management, and rendering loop.
/// Phase 4: Apps create their own windows; render loop syncs app state.

use alloc::format;
use spin::Mutex;
use super::compositor::{Compositor, COMPOSITOR};
use super::theme::*;
use super::window::WindowManager;
use super::widgets::{Taskbar, SystemInfoWidget, draw_splash_title};

/// Global desktop state.
pub static DESKTOP: Mutex<Option<Desktop>> = Mutex::new(None);

/// The desktop environment.
pub struct Desktop {
    /// Window manager.
    pub wm: WindowManager,
    /// Whether the splash screen has been shown.
    splash_shown: bool,
    /// Frame counter.
    frame_count: u64,
}

impl Desktop {
    fn new() -> Self {
        Self {
            wm: WindowManager::new(),
            splash_shown: false,
            frame_count: 0,
        }
    }
}

/// Initialize the desktop environment.
/// In Phase 4, the desktop starts empty — app threads create their own windows.
pub fn init() {
    let desktop = Desktop::new();
    *DESKTOP.lock() = Some(desktop);
}

/// Render one frame of the desktop.
pub fn render() {
    // We need both locks — compositor and desktop.
    // Take compositor lock first to avoid deadlock ordering issues.
    let mut comp_guard = COMPOSITOR.lock();
    let comp = match comp_guard.as_mut() {
        Some(c) => c,
        None => return,
    };

    let mut desktop_guard = DESKTOP.lock();
    let desktop = match desktop_guard.as_mut() {
        Some(d) => d,
        None => return,
    };

    let screen_w = comp.width;
    let screen_h = comp.height;

    desktop.frame_count += 1;

    // ── Phase 1: Clear background ──
    comp.clear(BG_PRIMARY);

    // ── Phase 2: Draw background pattern (subtle grid) ──
    draw_background_grid(comp, screen_w, screen_h);

    // ── Phase 3: Splash title (top area) ──
    if !desktop.splash_shown {
        draw_splash_title(comp, screen_w, screen_h);
        if desktop.frame_count > 1 {
            desktop.splash_shown = true;
        }
        return; // Show splash for one frame
    }

    // ── Phase 4: Sync app state to windows ──
    // Terminal
    if let Some(win) = desktop.wm.get_mut_by_title("Terminal") {
        crate::apps::terminal::sync_to_window(win);
    }
    // File Manager
    if let Some(win) = desktop.wm.get_mut_by_title("Files") {
        crate::apps::file_manager::sync_to_window(win);
    }
    // Text Editor
    if let Some(win) = desktop.wm.get_mut_by_title("Editor") {
        crate::apps::editor::sync_to_window(win);
    }
    // System Monitor updates its own content_lines directly
    // Calculator
    if let Some(win) = desktop.wm.get_mut_by_title("Calc") {
        crate::apps::calculator::sync_to_window(win);
    }
    // Task Manager
    if let Some(win) = desktop.wm.get_mut_by_title("Tasks") {
        crate::apps::task_manager::sync_to_window(win);
    }
    // Settings
    if let Some(win) = desktop.wm.get_mut_by_title("Settings") {
        crate::apps::settings::sync_to_window(win);
    }

    // ── Phase 5: Render all windows ──
    desktop.wm.render_all(comp);

    // ── Phase 6: System info widget (top-right corner) ──
    let uptime = crate::drivers::timer::uptime_secs();
    let (heap_used, heap_free) = crate::memory::heap::heap_stats();
    let threads = crate::process::scheduler::ready_count() + 1; // +1 for current
    let info_w = 240;
    let info_x = screen_w.saturating_sub(info_w + 10);
    let info_widget = SystemInfoWidget::new(info_x, 10, info_w);
    info_widget.render(comp, uptime, heap_used, heap_free, threads);

    // ── Phase 7: Taskbar ──
    let taskbar = Taskbar::new(screen_w, screen_h);
    let window_list = desktop.wm.window_list();
    taskbar.render(comp, &window_list, uptime);

    // ── Phase 8: Status line (above taskbar) ──
    let status_y = screen_h.saturating_sub(TASKBAR_HEIGHT + 18);
    let status_text = format!(
        " Frame #{} | {} threads | Heap {}/{}K ",
        desktop.frame_count,
        threads,
        heap_used / 1024,
        (heap_used + heap_free) / 1024,
    );
    comp.draw_text(4, status_y, &status_text, TEXT_MUTED);

    // ── Phase 9a: Toast notifications (above windows, below cursor) ──
    super::notification::render(comp, screen_w, screen_h);

    // ── Phase 9b: Context menu (above everything except cursor) ──
    super::context_menu::render(comp);

    // ── Phase 9c: Mouse cursor (always on top) ──
    let (mx, my) = crate::drivers::mouse::position();
    super::mouse_cursor::draw_cursor(comp, mx as usize, my as usize);
}

/// Draw a subtle cyberpunk grid pattern on the background.
fn draw_background_grid(comp: &mut Compositor, width: usize, height: usize) {
    let grid_color = Color::rgb(12, 12, 24); // Very subtle
    let grid_spacing = 40;

    // Vertical lines
    let mut x = 0;
    while x < width {
        comp.vline(x, 0, height, grid_color);
        x += grid_spacing;
    }

    // Horizontal lines
    let mut y = 0;
    while y < height {
        comp.hline(0, y, width, grid_color);
        y += grid_spacing;
    }
}

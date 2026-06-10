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
    /// Ghost snap preview (x, y, w, h).
    pub ghost_snap: Option<(usize, usize, usize, usize)>,
}

impl Desktop {
    fn new() -> Self {
        Self {
            wm: WindowManager::new(),
            splash_shown: false,
            frame_count: 0,
            ghost_snap: None,
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

    // ── Phase 1: Draw wallpaper (dark gradient, no grid) ──
    draw_wallpaper(comp, screen_w, screen_h);

    // ── Phase 2.5: Draw ghost snap preview ──
    if let Some((gx, gy, gw, gh)) = desktop.ghost_snap {
        comp.fill_rect(gx, gy, gw, gh, Color::rgb(0, 100, 128));
        comp.draw_rect(gx, gy, gw, gh, Color::rgb(0, 200, 255));
    }

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
    // Browser
    if let Some(win) = desktop.wm.get_mut_by_title("Browser") {
        crate::apps::browser::sync_to_window(win);
    }
    // Image Viewer
    if let Some(win) = desktop.wm.get_mut_by_title("Image Viewer") {
        crate::apps::image_viewer::sync_to_window(win);
    }
    // PDF Reader
    if let Some(win) = desktop.wm.get_mut_by_title("PDF Reader") {
        crate::apps::pdf_reader::sync_to_window(win);
    }
    // Video Player
    if let Some(win) = desktop.wm.get_mut_by_title("Video Player") {
        crate::apps::video_player::sync_to_window(win);
    }
    // Email Client
    if let Some(win) = desktop.wm.get_mut_by_title("Email") {
        crate::apps::email_client::sync_to_window(win);
    }
    // Office Suite
    if let Some(win) = desktop.wm.get_mut_by_title("Office") {
        crate::apps::office::sync_to_window(win);
    }
    // Browser v2 (bookmarks / downloads / history / reader)
    if let Some(win) = desktop.wm.get_mut_by_title("Browser v2") {
        crate::apps::browser_v2::sync_to_window(win);
    }
    // USB Mass Storage Manager
    if let Some(win) = desktop.wm.get_mut_by_title("USB Storage") {
        crate::apps::usb_storage::sync_to_window(win);
    }
    // App Store
    if let Some(win) = desktop.wm.get_mut_by_title("App Store") {
        crate::apps::app_store::sync_to_window(win);
    }
    // Power Manager
    if let Some(win) = desktop.wm.get_mut_by_title("Power") {
        crate::apps::power_manager::sync_to_window(win);
    }
    // Crash Recovery
    if let Some(win) = desktop.wm.get_mut_by_title("Crash Recovery") {
        crate::apps::crash_recovery::sync_to_window(win);
    }
    // Setup Wizard
    if let Some(win) = desktop.wm.get_mut_by_title("Setup Wizard") {
        crate::apps::setup_wizard::sync_to_window(win);
    }
    // Accessibility
    if let Some(win) = desktop.wm.get_mut_by_title("Accessibility") {
        crate::apps::accessibility::sync_to_window(win);
    }
    // Printer
    if let Some(win) = desktop.wm.get_mut_by_title("Printer") {
        crate::apps::printer::sync_to_window(win);
    }
    // Update Manager
    if let Some(win) = desktop.wm.get_mut_by_title("Updates") {
        crate::apps::update_manager::sync_to_window(win);
    }
    // Cloud Sync
    if let Some(win) = desktop.wm.get_mut_by_title("Cloud Sync") {
        crate::apps::cloud_sync::sync_to_window(win);
    }
    // Gaming
    if let Some(win) = desktop.wm.get_mut_by_title("Gaming") {
        crate::apps::gaming::sync_to_window(win);
    }

    // ── Phase 61: WM2 tiling — re-arrange windows if layout is not Float ──
    if super::wm2::get_layout() != super::wm2::TilingLayout::Float {
        super::wm2::apply_tiling(&mut desktop.wm, screen_w, screen_h);
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

    // Phase 8 slot: status info moved into taskbar system tray

    // ── Phase 9a: Toast notifications (above windows, below cursor) ──
    super::notification::render(comp, screen_w, screen_h);

    // ── Phase 9b: Context menu (above everything except cursor) ──
    super::context_menu::render(comp);

    // ── Phase 60: CJK IME Candidate overlay ──
    super::ime::render(comp);

    // ── Phase 9c: Mouse cursor (always on top) ──
    let (mx, my) = crate::drivers::mouse::position();
    super::mouse_cursor::draw_cursor(comp, mx as usize, my as usize);
}

/// Draw a modern dark gradient wallpaper.
fn draw_wallpaper(comp: &mut Compositor, width: usize, height: usize) {
    // Top: slightly lighter (dark blue-gray), bottom: darkest
    let top_color = Color::rgb(26, 28, 38);
    let bottom_color = Color::rgb(14, 14, 18);
    let usable_h = height.saturating_sub(crate::gui::theme::TASKBAR_HEIGHT);

    for y in 0..usable_h {
        let factor = (y * 255 / usable_h.max(1)) as u8;
        let row_color = top_color.blend(bottom_color, factor);
        comp.hline(0, y, width, row_color);
    }

    // Taskbar area — solid dark
    comp.fill_rect(0, usable_h, width, crate::gui::theme::TASKBAR_HEIGHT, Color::rgb(14, 14, 18));

    // Subtle radial-ish glow in center (just a lighter ellipse at center-top)
    let cx = width / 2;
    let glow = Color::rgb(30, 35, 50);
    if height > 200 {
        for dy in 0..80usize {
            let spread = (80 - dy) * width / 160;
            let gx = cx.saturating_sub(spread);
            let gw = spread * 2;
            let alpha = ((80 - dy) as u8).saturating_mul(2);
            comp.hline(gx, dy, gw, top_color.blend(glow, alpha));
        }
    }
}

/// Smart OS Window Manager — Neon-bordered floating windows.
///
/// Each window has a title bar with close/minimize buttons,
/// a glowing neon border, and a content area for widgets or text.

use alloc::string::String;
use alloc::vec::Vec;
use super::theme::*;
use super::compositor::Compositor;
use super::widget::{Widget, WidgetEvent, WidgetAction};

/// Unique window identifier.
pub type WindowId = u64;

static NEXT_WINDOW_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

fn alloc_window_id() -> WindowId {
    NEXT_WINDOW_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

/// Window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Normal,
    Minimized,
    Maximized,
}

/// Minimum window dimensions.
pub const MIN_WIDTH: usize = 200;
pub const MIN_HEIGHT: usize = 100;

/// A desktop window with title bar and neon glow borders.
pub struct Window {
    pub id: WindowId,
    pub title: String,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub state: WindowState,
    pub active: bool,
    /// Accent color for this window's glow border.
    pub accent: Color,
    /// Window content lines (simple text content — legacy mode).
    pub content_lines: Vec<String>,
    /// Whether this window is visible.
    pub visible: bool,

    // ── Phase 4: Widget support ──

    /// Interactive widgets in the content area.
    pub widgets: Vec<Widget>,
    /// Which widget has keyboard focus (by id).
    pub focused_widget: Option<u8>,
    /// If true, render widgets instead of content_lines.
    pub use_widgets: bool,

    // ── Phase 9: Snap/resize support ──

    /// Saved bounds before snapping/maximizing (x, y, w, h).
    pub pre_snap_bounds: Option<(usize, usize, usize, usize)>,
}

impl Window {
    /// Create a new window.
    pub fn new(title: &str, x: usize, y: usize, width: usize, height: usize, accent: Color) -> Self {
        Self {
            id: alloc_window_id(),
            title: String::from(title),
            x,
            y,
            width,
            height,
            state: WindowState::Normal,
            active: false,
            accent,
            content_lines: Vec::new(),
            visible: true,
            widgets: Vec::new(),
            focused_widget: None,
            use_widgets: false,
            pre_snap_bounds: None,
        }
    }

    /// Add a text line to the window content (legacy mode).
    pub fn add_line(&mut self, text: &str) {
        self.content_lines.push(String::from(text));
    }

    /// Clear all content.
    pub fn clear_content(&mut self) {
        self.content_lines.clear();
    }

    /// Total height including title bar and borders.
    pub fn total_height(&self) -> usize {
        self.height + TITLEBAR_HEIGHT + BORDER_WIDTH * 2
    }

    /// Total width including borders.
    pub fn total_width(&self) -> usize {
        self.width + BORDER_WIDTH * 2
    }

    /// Maximize the window to fill the screen (saves current bounds for restore).
    pub fn maximize(&mut self, screen_w: usize, screen_h: usize) {
        if self.state != WindowState::Maximized {
            self.pre_snap_bounds = Some((self.x, self.y, self.width, self.height));
        }
        self.x = 0;
        self.y = 0;
        self.width = screen_w.saturating_sub(BORDER_WIDTH * 2);
        self.height = screen_h.saturating_sub(TITLEBAR_HEIGHT + BORDER_WIDTH * 2 + TASKBAR_HEIGHT);
        self.state = WindowState::Maximized;
    }

    /// Restore the window from maximized/snapped state to its previous bounds.
    pub fn restore(&mut self) {
        if let Some((x, y, w, h)) = self.pre_snap_bounds.take() {
            self.x = x;
            self.y = y;
            self.width = w;
            self.height = h;
        }
        self.state = WindowState::Normal;
        self.visible = true;
    }

    /// Snap the window to a region of the screen (saves current bounds for unsnap).
    pub fn snap_to(&mut self, x: usize, y: usize, w: usize, h: usize) {
        if self.pre_snap_bounds.is_none() {
            self.pre_snap_bounds = Some((self.x, self.y, self.width, self.height));
        }
        self.x = x;
        self.y = y;
        self.width = w.saturating_sub(BORDER_WIDTH * 2);
        self.height = h.saturating_sub(TITLEBAR_HEIGHT + BORDER_WIDTH * 2);
        self.state = WindowState::Normal; // Snapped is rendered as Normal
    }

    // ── Widget management ──

    /// Add a widget to the window's content area.
    pub fn add_widget(&mut self, widget: Widget) {
        self.widgets.push(widget);
        self.use_widgets = true;
    }

    /// Cycle keyboard focus to the next focusable widget.
    pub fn focus_next(&mut self) {
        let focusable: Vec<u8> = self.widgets.iter()
            .filter(|w| w.is_focusable() && w.visible)
            .map(|w| w.id)
            .collect();

        if focusable.is_empty() { return; }

        let current_idx = self.focused_widget
            .and_then(|fid| focusable.iter().position(|&id| id == fid));

        let next_idx = match current_idx {
            Some(idx) => (idx + 1) % focusable.len(),
            None => 0,
        };

        let new_focus = focusable[next_idx];

        // Update focus states
        for w in &mut self.widgets {
            let was_focused = w.focused;
            w.focused = w.id == new_focus;
            if was_focused && !w.focused {
                w.handle_event(&WidgetEvent::FocusLost);
            } else if !was_focused && w.focused {
                w.handle_event(&WidgetEvent::FocusGained);
            }
        }
        self.focused_widget = Some(new_focus);
    }

    /// Set focus to a specific widget by ID.
    pub fn focus_widget(&mut self, id: u8) {
        for w in &mut self.widgets {
            let was_focused = w.focused;
            w.focused = w.id == id;
            if was_focused && !w.focused {
                w.handle_event(&WidgetEvent::FocusLost);
            } else if !was_focused && w.focused {
                w.handle_event(&WidgetEvent::FocusGained);
            }
        }
        self.focused_widget = Some(id);
    }

    /// Route a key press to the focused widget.
    pub fn dispatch_key(&mut self, ascii: u8, scancode: u8) -> Option<WidgetAction> {
        let focus_id = self.focused_widget?;
        let widget = self.widgets.iter_mut().find(|w| w.id == focus_id)?;
        let action = widget.handle_event(&WidgetEvent::KeyPress { ascii, scancode });
        match action {
            WidgetAction::None => None,
            other => Some(other),
        }
    }

    /// Route a mouse click to the appropriate widget (coordinates relative to content area).
    pub fn dispatch_click(&mut self, rel_x: usize, rel_y: usize) -> Option<WidgetAction> {
        // Find which widget was clicked
        let clicked_id = self.widgets.iter()
            .filter(|w| w.visible)
            .find(|w| w.contains(rel_x, rel_y))
            .map(|w| w.id);

        if let Some(id) = clicked_id {
            // Set focus to clicked widget
            self.focus_widget(id);

            // Dispatch click event with coordinates relative to widget
            let widget = self.widgets.iter_mut().find(|w| w.id == id)?;
            let wx = rel_x.saturating_sub(widget.x);
            let wy = rel_y.saturating_sub(widget.y);
            let action = widget.handle_event(&WidgetEvent::MouseClick { x: wx, y: wy });
            match action {
                WidgetAction::None => None,
                other => Some(other),
            }
        } else {
            None
        }
    }

    /// Get a mutable reference to a widget by ID.
    pub fn get_widget_mut(&mut self, id: u8) -> Option<&mut Widget> {
        self.widgets.iter_mut().find(|w| w.id == id)
    }

    /// Render this window to the compositor.
    pub fn render(&self, comp: &mut Compositor) {
        if !self.visible || self.state == WindowState::Minimized {
            return;
        }

        let border_color = if self.active { BORDER_GLOW } else { BORDER_INACTIVE };
        let titlebar_bg = if self.active { BG_TITLEBAR_ACTIVE } else { BG_TITLEBAR };

        let wx = self.x;
        let wy = self.y;
        let tw = self.total_width();
        let th = self.total_height();

        // ── Glow border (neon effect — outer layers) ──
        if self.active {
            comp.draw_glow_border(wx, wy, tw, th, self.accent, GLOW_SIZE);
        }

        // ── Window border ──
        comp.draw_rect(wx, wy, tw, th, border_color);

        // ── Title bar background ──
        let tb_x = wx + BORDER_WIDTH;
        let tb_y = wy + BORDER_WIDTH;
        let tb_w = self.width;
        comp.fill_rect(tb_x, tb_y, tb_w, TITLEBAR_HEIGHT, titlebar_bg);

        // ── Title bar accent line (top of title bar — thin neon strip) ──
        comp.hline(tb_x, tb_y, tb_w, self.accent.dim(if self.active { 255 } else { 80 }));

        // ── Window title text ──
        let title_x = tb_x + 8;
        let title_y = tb_y + (TITLEBAR_HEIGHT.saturating_sub(16)) / 2;
        let title_color = if self.active { TEXT_PRIMARY } else { TEXT_SECONDARY };
        comp.draw_text(title_x, title_y, &self.title, title_color);

        // ── Close button (neon red dot) ──
        let close_x = tb_x + tb_w - 16;
        let close_y = tb_y + (TITLEBAR_HEIGHT.saturating_sub(8)) / 2;
        comp.fill_rect(close_x, close_y, 8, 8, ACCENT_RED);

        // ── Minimize button (neon orange dot) ──
        let min_x = close_x - 14;
        comp.fill_rect(min_x, close_y, 8, 8, ACCENT_ORANGE);

        // ── Maximize button (neon green dot) ──
        let max_x = min_x - 14;
        comp.fill_rect(max_x, close_y, 8, 8, ACCENT_GREEN);

        // ── Content area background ──
        let content_x = tb_x;
        let content_y = tb_y + TITLEBAR_HEIGHT;
        comp.fill_rect(content_x, content_y, self.width, self.height, BG_PANEL);

        // ── Content separator line ──
        comp.hline(content_x, content_y, self.width, border_color.dim(120));

        // ── Render content ──
        if self.use_widgets {
            self.render_widgets(comp, content_x, content_y);
        } else {
            self.render_content_lines(comp, content_x, content_y);
        }

        // ── Resize grip (bottom-right corner, 3 small dots) ──
        let grip_color = if self.active { BORDER_GLOW } else { BORDER_INACTIVE };
        let gx = wx + tw - 10;
        let gy = wy + th - 10;
        comp.fill_rect(gx + 6, gy + 6, 2, 2, grip_color);
        comp.fill_rect(gx + 2, gy + 6, 2, 2, grip_color);
        comp.fill_rect(gx + 6, gy + 2, 2, 2, grip_color);
    }

    /// Render legacy content_lines text.
    fn render_content_lines(&self, comp: &mut Compositor, content_x: usize, content_y: usize) {
        let text_x = content_x + 6;
        let mut text_y = content_y + 4;
        let max_lines = (self.height.saturating_sub(8)) / 18;
        for (i, line) in self.content_lines.iter().enumerate() {
            if i >= max_lines {
                break;
            }
            comp.draw_text(text_x, text_y, line, TEXT_PRIMARY);
            text_y += 18;
        }
    }

    /// Render all widgets in the content area.
    fn render_widgets(&self, comp: &mut Compositor, content_x: usize, content_y: usize) {
        for widget in &self.widgets {
            widget.render(comp, content_x, content_y);
        }
    }
}

/// The window manager tracks all open windows and z-order.
pub struct WindowManager {
    /// All windows, ordered by z-index (last = topmost).
    pub windows: Vec<Window>,
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
        }
    }

    /// Add a window to the manager.
    pub fn add(&mut self, window: Window) {
        self.windows.push(window);
    }

    /// Set the active window (deactivates all others).
    pub fn set_active(&mut self, id: WindowId) {
        for win in &mut self.windows {
            win.active = win.id == id;
        }
    }

    /// Get active window ID.
    pub fn active_id(&self) -> Option<WindowId> {
        self.windows.iter().find(|w| w.active).map(|w| w.id)
    }

    /// Get window by ID (mutable).
    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    /// Get window by title (mutable).
    pub fn get_mut_by_title(&mut self, title: &str) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.title == title)
    }

    /// Render all windows in z-order.
    pub fn render_all(&self, comp: &mut Compositor) {
        for window in &self.windows {
            window.render(comp);
        }
    }

    /// Get window titles for taskbar.
    pub fn window_list(&self) -> Vec<(WindowId, &str, bool, Color)> {
        self.windows
            .iter()
            .filter(|w| w.visible)
            .map(|w| (w.id, w.title.as_str(), w.active, w.accent))
            .collect()
    }

    /// Bring a window to the front (topmost z-order) and make it active.
    pub fn bring_to_front(&mut self, id: WindowId) {
        if let Some(pos) = self.windows.iter().position(|w| w.id == id) {
            let win = self.windows.remove(pos);
            self.windows.push(win);
        }
        self.set_active(id);
    }

    /// Hide a window.
    pub fn hide(&mut self, id: WindowId) {
        if let Some(win) = self.get_mut(id) {
            win.visible = false;
            win.active = false;
        }
    }
}

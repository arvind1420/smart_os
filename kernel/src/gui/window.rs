/// Smart OS Window Manager — Neon-bordered floating windows.
///
/// Each window has a title bar with close/minimize buttons,
/// a glowing neon border, and a content area for widgets or text.

use alloc::string::String;
use alloc::vec::Vec;
use super::theme::*;
use super::compositor::Compositor;
use super::widget::{Widget, WidgetEvent, WidgetAction};
use crate::drivers::drm::FramebufferObj;

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

    /// Hardware-backed framebuffer for this window (if DRM active).
    pub hardware_fb: Option<FramebufferObj>,
    /// Whether the window deco/content needs to be re-rendered to hardware_fb.
    pub dirty: bool,

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
    /// Raw image pixel buffer: (x, y, w, h, pixels)
    pub image_buffer: Option<(usize, usize, usize, usize, Vec<u8>)>,
}

impl Window {
    /// Create a new window.
    pub fn new(title: &str, x: usize, y: usize, width: usize, height: usize, accent: Color) -> Self {
        let mut hardware_fb = None;
        
        // Try to allocate a hardware framebuffer for this window
        let mut drm = crate::drivers::drm::DRM.lock();
        if let Some(ref mut driver) = drm.active_driver {
            // Allocate a buffer slightly larger than content to account for borders/title
            let tw = width + BORDER_WIDTH * 2;
            let th = height + TITLEBAR_HEIGHT + BORDER_WIDTH * 2;
            if let Ok(fb) = driver.alloc_framebuffer(tw as u32, th as u32, 0) {
                hardware_fb = Some(fb);
            }
        }

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
            image_buffer: None,
            hardware_fb,
            dirty: true,
        }
    }

    /// Add a text line to the window content (legacy mode).
    pub fn add_line(&mut self, text: &str) {
        self.content_lines.push(String::from(text));
        self.dirty = true;
    }

    /// Clear all content.
    pub fn clear_content(&mut self) {
        self.content_lines.clear();
        self.dirty = true;
    }

    /// Total height including title bar and borders.
    pub fn total_height(&self) -> usize {
        self.height + TITLEBAR_HEIGHT + BORDER_WIDTH * 2
    }

    /// Total width including borders.
    pub fn total_width(&self) -> usize {
        self.width + BORDER_WIDTH * 2
    }

    /// Resize the window content area, handling hardware framebuffer allocation if needed.
    pub fn resize(&mut self, new_width: usize, new_height: usize) {
        if self.width == new_width && self.height == new_height {
            return;
        }
        self.width = new_width;
        self.height = new_height;
        self.dirty = true;

        if self.hardware_fb.is_some() {
            let mut drm = crate::drivers::drm::DRM.lock();
            if let Some(ref mut driver) = drm.active_driver {
                let tw = new_width + BORDER_WIDTH * 2;
                let th = new_height + TITLEBAR_HEIGHT + BORDER_WIDTH * 2;
                if let Ok(fb) = driver.alloc_framebuffer(tw as u32, th as u32, 0) {
                    self.hardware_fb = Some(fb);
                }
            }
        }
    }

    /// Maximize the window to fill the screen (saves current bounds for restore).
    pub fn maximize(&mut self, screen_w: usize, screen_h: usize) {
        if self.state != WindowState::Maximized {
            self.pre_snap_bounds = Some((self.x, self.y, self.width, self.height));
        }
        self.x = 0;
        self.y = 0;
        let new_w = screen_w.saturating_sub(BORDER_WIDTH * 2);
        let new_h = screen_h.saturating_sub(TITLEBAR_HEIGHT + BORDER_WIDTH * 2 + TASKBAR_HEIGHT);
        self.resize(new_w, new_h);
        self.state = WindowState::Maximized;
    }

    /// Restore the window from maximized/snapped state to its previous bounds.
    pub fn restore(&mut self) {
        if let Some((x, y, w, h)) = self.pre_snap_bounds.take() {
            self.x = x;
            self.y = y;
            self.resize(w, h);
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
        let new_w = w.saturating_sub(BORDER_WIDTH * 2);
        let new_h = h.saturating_sub(TITLEBAR_HEIGHT + BORDER_WIDTH * 2);
        self.resize(new_w, new_h);
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
        let mut new_label = alloc::string::String::new();
        let mut new_role = super::accessibility::UIRole::Unknown;

        for w in &mut self.widgets {
            let was_focused = w.focused;
            w.focused = w.id == new_focus;
            if !was_focused && w.focused {
                // Focus Gained
                new_label = match &w.kind {
                    super::widget::WidgetKind::Button(b) => b.label.clone(),
                    super::widget::WidgetKind::TextInput(t) => t.placeholder.clone(),
                    _ => alloc::format!("Widget {}", w.id),
                };
                new_role = match &w.kind {
                    super::widget::WidgetKind::Button(_) => super::accessibility::UIRole::Button,
                    super::widget::WidgetKind::TextInput(_) => super::accessibility::UIRole::TextInput,
                    _ => super::accessibility::UIRole::Unknown,
                };
                w.handle_event(&WidgetEvent::FocusGained);
            } else if was_focused && !w.focused {
                w.handle_event(&WidgetEvent::FocusLost);
            }
        }
        self.focused_widget = Some(new_focus);

        // Notify A-Bus (Phase 43)
        super::accessibility::on_focus_changed(new_role, &new_label);
    }

    /// Set focus to a specific widget by ID.
    pub fn focus_widget(&mut self, id: u8) {
        let mut new_label = alloc::string::String::new();
        let mut new_role = super::accessibility::UIRole::Unknown;

        for w in &mut self.widgets {
            let was_focused = w.focused;
            w.focused = w.id == id;
            if !was_focused && w.focused {
                // Focus Gained
                new_label = match &w.kind {
                    super::widget::WidgetKind::Button(b) => b.label.clone(),
                    super::widget::WidgetKind::TextInput(t) => t.placeholder.clone(),
                    _ => alloc::format!("Widget {}", w.id),
                };
                new_role = match &w.kind {
                    super::widget::WidgetKind::Button(_) => super::accessibility::UIRole::Button,
                    super::widget::WidgetKind::TextInput(_) => super::accessibility::UIRole::TextInput,
                    _ => super::accessibility::UIRole::Unknown,
                };
                w.handle_event(&WidgetEvent::FocusGained);
            } else if was_focused && !w.focused {
                w.handle_event(&WidgetEvent::FocusLost);
            }
        }
        self.focused_widget = Some(id);

        // Notify A-Bus (Phase 43)
        super::accessibility::on_focus_changed(new_role, &new_label);
    }

    /// Route a key press to the focused widget.
    pub fn dispatch_key(&mut self, key: char, scancode: u8) -> Option<WidgetAction> {
        let focus_id = self.focused_widget?;
        let widget = self.widgets.iter_mut().find(|w| w.id == focus_id)?;
        let action = widget.handle_event(&WidgetEvent::KeyPress { key, scancode });
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
    pub fn render(&mut self, comp: &mut Compositor) {
        if !self.visible || self.state == WindowState::Minimized {
            return;
        }

        // Draw soft drop shadow first (only if NOT maximized!)
        if self.state != WindowState::Maximized {
            let tw = self.total_width();
            let th = self.total_height();
            if self.active {
                // Active window shadow: offset = 6, blur = 10, max_alpha = 140
                comp.draw_soft_shadow(self.x, self.y, tw, th, 6, 6, 10, 140);
            } else {
                // Inactive window shadow: offset = 3, blur = 5, max_alpha = 80
                comp.draw_soft_shadow(self.x, self.y, tw, th, 3, 3, 5, 80);
            }
        }

        // Hardware Acceleration Path: Render to window's own FB then blit to screen
        if let Some(fb) = self.hardware_fb {
            if self.dirty {
                // Temporarily target the window's FB for rendering
                comp.push_target(Some(fb));
                
                // Render at relative (0,0)
                self.render_internal(comp, 0, 0);
                
                comp.pop_target();
                self.dirty = false;
            }
            
            // Blit the cached window FB to the main screen back buffer
            comp.hardware_blit(fb.id, self.x as u32, self.y as u32, self.total_width() as u32, self.total_height() as u32);
            return;
        }

        // Software Path: Render directly to screen back buffer
        self.render_internal(comp, self.x, self.y);
    }

    /// Internal render logic — can be used for both direct and cached rendering.
    fn render_internal(&self, comp: &mut Compositor, wx: usize, wy: usize) {
        let border_color = if self.active { BORDER_GLOW } else { BORDER_INACTIVE };
        let titlebar_bg = if self.active { BG_TITLEBAR_ACTIVE } else { BG_TITLEBAR };

        let tw = self.total_width();
        let th = self.total_height();

        // ── Glow border (neon effect) ──
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

        // ── Accent line at very top of title bar (1px) ──
        if self.active {
            comp.hline(tb_x, tb_y, tb_w, self.accent.dim(180));
        }

        // ── Colored dot + window title ──
        let dot_x = tb_x + 6;
        let title_y = tb_y + (TITLEBAR_HEIGHT.saturating_sub(16)) / 2;
        comp.fill_rect(dot_x, title_y + 4, 8, 8, self.accent.dim(if self.active { 200 } else { 80 }));
        let title_x = dot_x + 12;
        let title_color = if self.active { TEXT_PRIMARY } else { TEXT_SECONDARY };
        comp.draw_text(title_x, title_y, &self.title, title_color);

        // ── Window control buttons (right-aligned, modern style) ──
        // Button width 40px each: close, maximize, minimize
        let btn_h = TITLEBAR_HEIGHT;
        let btn_w = 40usize;
        let btn_y = tb_y;

        // Close button (rightmost) — red on hover, subtle otherwise
        let close_x = tb_x + tb_w.saturating_sub(btn_w);
        comp.fill_rect(close_x, btn_y, btn_w, btn_h, ACCENT_RED.dim(if self.active { 200 } else { 60 }));
        // Draw × symbol
        let cx = close_x + btn_w / 2 - 4;
        let cy = btn_y + btn_h / 2 - 4;
        comp.draw_text(cx, cy, "x", TEXT_PRIMARY);

        // Maximize button
        let max_x = close_x.saturating_sub(btn_w);
        comp.fill_rect(max_x, btn_y, btn_w, btn_h, BG_TITLEBAR_ACTIVE.dim(if self.active { 180 } else { 80 }));
        let mx = max_x + btn_w / 2 - 4;
        comp.draw_rect(mx, btn_y + btn_h / 2 - 4, 9, 8, if self.active { TEXT_PRIMARY } else { TEXT_SECONDARY });

        // Minimize button
        let min_x = max_x.saturating_sub(btn_w);
        comp.fill_rect(min_x, btn_y, btn_w, btn_h, BG_TITLEBAR_ACTIVE.dim(if self.active { 180 } else { 80 }));
        let mnx = min_x + btn_w / 2 - 4;
        comp.hline(mnx, btn_y + btn_h / 2 + 2, 9, if self.active { TEXT_PRIMARY } else { TEXT_SECONDARY });

        // ── Content area background ──
        let content_x = tb_x;
        let content_y = tb_y + TITLEBAR_HEIGHT;
        comp.fill_rect(content_x, content_y, self.width, self.height, BG_PANEL);

        // ── Content separator line ──
        comp.hline(content_x, content_y, self.width, border_color.dim(120));

        // ── Render content ──
        if self.use_widgets {
            self.render_widgets(comp, content_x, content_y);
            // Also blit image_buffer overlay (e.g. image viewer canvas below toolbar)
            if let Some((ix, iy, img_w, img_h, ref img_pixels)) = self.image_buffer {
                let max_h = self.height.saturating_sub(iy);
                let max_w = self.width.saturating_sub(ix);
                let draw_h = img_h.min(max_h);
                let draw_w = img_w.min(max_w);
                for py in 0..draw_h {
                    let dest_y = content_y + iy + py;
                    for px in 0..draw_w {
                        let dest_x = content_x + ix + px;
                        let off = (py * img_w + px) * 4;
                        if off + 3 < img_pixels.len() {
                            let r = img_pixels[off];
                            let g = img_pixels[off+1];
                            let b = img_pixels[off+2];
                            comp.set_pixel(dest_x, dest_y, Color { r, g, b });
                        }
                    }
                }
            }
        } else {
            if let Some((ix, iy, img_w, img_h, ref img_pixels)) = self.image_buffer {
                let max_h = self.height.saturating_sub(iy);
                let max_w = self.width.saturating_sub(ix);
                let draw_h = img_h.min(max_h);
                let draw_w = img_w.min(max_w);
                for py in 0..draw_h {
                    let dest_y = content_y + iy + py;
                    for px in 0..draw_w {
                        let dest_x = content_x + ix + px;
                        let off = (py * img_w + px) * 4;
                        if off + 3 < img_pixels.len() {
                            let r = img_pixels[off];
                            let g = img_pixels[off+1];
                            let b = img_pixels[off+2];
                            comp.set_pixel(dest_x, dest_y, Color { r, g, b });
                        }
                    }
                }
            }
            self.render_content_lines(comp, content_x, content_y);
        }

        // ── Resize grip ──
        let grip_color = if self.active { BORDER_GLOW } else { BORDER_INACTIVE };
        let gx = wx + tw - 10;
        let gy = wy + th - 10;
        comp.fill_rect(gx + 6, gy + 6, 2, 2, grip_color);
        comp.fill_rect(gx + 2, gy + 6, 2, 2, grip_color);
        comp.fill_rect(gx + 6, gy + 2, 2, 2, grip_color);
    }

    /// Render legacy content_lines text with absolute position parsing.
    fn render_content_lines(&self, comp: &mut Compositor, content_x: usize, content_y: usize) {
        let text_x = content_x + 6;
        let mut text_y = content_y + 4;
        let max_lines = (self.height.saturating_sub(8)) / 18;
        
        for (i, line) in self.content_lines.iter().enumerate() {
            if line.starts_with("@ttf:") {
                let s = &line[5..];
                if let Some(pos) = s.find(' ') {
                    let meta = &s[..pos];
                    let mut parts = meta.split(':');
                    let rx: usize = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
                    let ry: usize = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
                    let size: usize = parts.next().and_then(|p| p.parse().ok()).unwrap_or(14);
                    let text = &s[pos+1..];
                    comp.draw_text_ttf(content_x + rx, content_y + ry, text, TEXT_PRIMARY, size as f32);
                }
                continue;
            }
            if line.starts_with('@') {
                // Format: @x:y text
                if let Some(pos) = line.find(' ') {
                    let coords = &line[1..pos];
                    if let Some(colon) = coords.find(':') {
                        let rx: usize = coords[..colon].parse().unwrap_or(0);
                        let ry: usize = coords[colon+1..].parse().unwrap_or(0);
                        comp.draw_text(content_x + rx, content_y + ry, &line[pos+1..], TEXT_PRIMARY);
                    }
                }
                continue;
            }
            if line.starts_with("!rect") {
                // Format: !rect x:y wxh #rrggbb
                let s = &line[6..];
                if let Some(space1) = s.find(' ') {
                    let coords = &s[..space1];
                    let rest = &s[space1 + 1..];
                    if let Some(colon) = coords.find(':') {
                        let rx: usize = coords[..colon].parse().unwrap_or(0);
                        let ry: usize = coords[colon+1..].parse().unwrap_or(0);
                        if let Some(space2) = rest.find(' ') {
                            let dims = &rest[..space2];
                            let col_str = &rest[space2 + 1..];
                            if let Some(cross) = dims.find('x') {
                                let rw: usize = dims[..cross].parse().unwrap_or(0);
                                let rh: usize = dims[cross+1..].parse().unwrap_or(0);
                                if col_str.starts_with('#') && col_str.len() == 7 {
                                    let r = u8::from_str_radix(&col_str[1..3], 16).unwrap_or(0);
                                    let g = u8::from_str_radix(&col_str[3..5], 16).unwrap_or(0);
                                    let b = u8::from_str_radix(&col_str[5..7], 16).unwrap_or(0);
                                    comp.fill_rect(content_x + rx, content_y + ry, rw, rh, Color { r, g, b });
                                }
                            }
                        }
                    }
                }
                continue;
            }

            if i >= max_lines { break; }
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

    /// Add a window to the manager. If no window is currently active, this one becomes active.
    pub fn add(&mut self, mut window: Window) {
        let has_active = self.windows.iter().any(|w| w.active);
        if !has_active {
            window.active = true;
        }
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
    pub fn render_all(&mut self, comp: &mut Compositor) {
        for window in &mut self.windows {
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

    /// Automatically focus the topmost visible, non-minimized window.
    pub fn auto_focus_next(&mut self) {
        let next_active = self.windows.iter()
            .rev()
            .find(|w| w.visible && w.state != WindowState::Minimized)
            .map(|w| w.id);

        if let Some(id) = next_active {
            self.set_active(id);
        } else {
            // Deactivate all if none found
            for win in &mut self.windows {
                win.active = false;
            }
        }
    }
}

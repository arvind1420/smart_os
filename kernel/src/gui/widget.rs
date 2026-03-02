/// Interactive widget system for Smart OS GUI.
///
/// Provides Button, TextInput, ScrollableText, and Label widgets
/// using an enum-based approach (no dyn Trait) for simplicity and Send+Sync safety.

use alloc::string::String;
use alloc::vec::Vec;
use super::compositor::Compositor;
use super::theme::*;

// ═══════════════════════════════════════════════════════════════
//  Events and Actions
// ═══════════════════════════════════════════════════════════════

/// Events that widgets can receive.
#[derive(Debug, Clone)]
pub enum WidgetEvent {
    MouseClick { x: usize, y: usize },
    KeyPress { ascii: u8, scancode: u8 },
    FocusGained,
    FocusLost,
}

/// Commands emitted by widgets when activated.
#[derive(Debug, Clone)]
pub enum AppCommand {
    /// Text submitted from a TextInput (Enter pressed).
    TextSubmitted(String),
    /// A button with this ID was clicked.
    ButtonClicked(u8),
    /// Navigate to a path (file manager).
    NavigateTo(String),
    /// Scroll request.
    ScrollUp,
    ScrollDown,
    /// A line in a scrollable text was clicked.
    LineClicked(usize),
    /// Raw key event (for text editor). (ascii, scancode)
    RawKey(u8, u8),
}

/// Action returned by a widget after handling an event.
#[derive(Debug, Clone)]
pub enum WidgetAction {
    None,
    Redraw,
    Execute(AppCommand),
}

// ═══════════════════════════════════════════════════════════════
//  Widget Container
// ═══════════════════════════════════════════════════════════════

/// A positioned widget in a window's content area.
pub struct Widget {
    pub id: u8,
    /// Position relative to window content area.
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub focused: bool,
    pub visible: bool,
    pub kind: WidgetKind,
}

/// The concrete widget type.
pub enum WidgetKind {
    Button(Button),
    TextInput(TextInput),
    ScrollText(ScrollableText),
    Label(StaticLabel),
    RawInput(RawInputWidget),
}

impl Widget {
    /// Create a new widget at the given position.
    pub fn new(id: u8, x: usize, y: usize, width: usize, height: usize, kind: WidgetKind) -> Self {
        Self { id, x, y, width, height, focused: false, visible: true, kind }
    }

    /// Render this widget to the compositor at absolute coordinates.
    pub fn render(&self, comp: &mut Compositor, abs_x: usize, abs_y: usize) {
        if !self.visible { return; }
        let wx = abs_x + self.x;
        let wy = abs_y + self.y;
        match &self.kind {
            WidgetKind::Button(b) => b.render(comp, wx, wy, self.width, self.height, self.focused),
            WidgetKind::TextInput(t) => t.render(comp, wx, wy, self.width, self.height, self.focused),
            WidgetKind::ScrollText(s) => s.render(comp, wx, wy, self.width, self.height),
            WidgetKind::Label(l) => l.render(comp, wx, wy),
            WidgetKind::RawInput(r) => r.render(comp, wx, wy, self.width, self.height),
        }
    }

    /// Handle an event, returning an action.
    pub fn handle_event(&mut self, event: &WidgetEvent) -> WidgetAction {
        if !self.visible { return WidgetAction::None; }
        match &mut self.kind {
            WidgetKind::Button(b) => b.handle_event(event),
            WidgetKind::TextInput(t) => t.handle_event(event),
            WidgetKind::ScrollText(s) => s.handle_event(event),
            WidgetKind::Label(_) => WidgetAction::None,
            WidgetKind::RawInput(r) => r.handle_event(event),
        }
    }

    /// Whether this widget can receive keyboard focus.
    pub fn is_focusable(&self) -> bool {
        matches!(self.kind, WidgetKind::TextInput(_) | WidgetKind::Button(_) | WidgetKind::RawInput(_))
    }

    /// Check if a point (relative to content area) hits this widget.
    pub fn contains(&self, px: usize, py: usize) -> bool {
        px >= self.x && px < self.x + self.width && py >= self.y && py < self.y + self.height
    }
}

// ═══════════════════════════════════════════════════════════════
//  Button
// ═══════════════════════════════════════════════════════════════

/// A clickable button with a label and accent color.
pub struct Button {
    pub label: String,
    pub accent: Color,
    pub command: AppCommand,
}

impl Button {
    pub fn new(label: &str, accent: Color, command: AppCommand) -> Self {
        Self { label: String::from(label), accent, command }
    }

    fn render(&self, comp: &mut Compositor, x: usize, y: usize, w: usize, h: usize, focused: bool) {
        let bg = if focused { BG_BUTTON_HOVER } else { BG_BUTTON };
        comp.fill_rect(x, y, w, h, bg);
        let border = if focused { self.accent } else { BORDER_INACTIVE };
        comp.draw_rect(x, y, w, h, border);

        // Center the label text
        let text_w = self.label.len() * 8;
        let tx = x + (w.saturating_sub(text_w)) / 2;
        let ty = y + (h.saturating_sub(16)) / 2;
        comp.draw_text(tx, ty, &self.label, self.accent);
    }

    fn handle_event(&mut self, event: &WidgetEvent) -> WidgetAction {
        match event {
            WidgetEvent::MouseClick { .. } | WidgetEvent::KeyPress { ascii: b'\n', .. } => {
                WidgetAction::Execute(self.command.clone())
            }
            _ => WidgetAction::None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  TextInput
// ═══════════════════════════════════════════════════════════════

/// A single-line editable text input.
pub struct TextInput {
    pub text: String,
    pub cursor_pos: usize,
    pub placeholder: String,
    pub accent: Color,
    pub max_len: usize,
}

impl TextInput {
    pub fn new(placeholder: &str, accent: Color) -> Self {
        Self {
            text: String::new(),
            cursor_pos: 0,
            placeholder: String::from(placeholder),
            accent,
            max_len: 200,
        }
    }

    fn render(&self, comp: &mut Compositor, x: usize, y: usize, w: usize, h: usize, focused: bool) {
        // Background
        let bg = if focused { BG_INPUT_FOCUSED } else { BG_INPUT };
        comp.fill_rect(x, y, w, h, bg);

        // Border
        let border = if focused { self.accent } else { BORDER_INACTIVE };
        comp.draw_rect(x, y, w, h, border);

        // Prompt character
        let prompt_x = x + WIDGET_PADDING;
        let text_y = y + (h.saturating_sub(16)) / 2;
        comp.draw_text(prompt_x, text_y, ">", self.accent);

        let text_x = prompt_x + 12; // After "> "

        if self.text.is_empty() && !focused {
            // Placeholder text
            comp.draw_text(text_x, text_y, &self.placeholder, TEXT_MUTED);
        } else {
            // Actual text — show visible portion
            let max_chars = (w.saturating_sub(24)) / 8; // Available chars
            let display_start = if self.cursor_pos > max_chars {
                self.cursor_pos - max_chars
            } else {
                0
            };
            let display_end = (display_start + max_chars).min(self.text.len());
            let display_text = &self.text[display_start..display_end];
            comp.draw_text(text_x, text_y, display_text, TEXT_PRIMARY);

            // Cursor
            if focused {
                let cursor_offset = self.cursor_pos.saturating_sub(display_start);
                let cx = text_x + cursor_offset * 8;
                comp.fill_rect(cx, text_y, 2, 16, CURSOR_COLOR);
            }
        }
    }

    fn handle_event(&mut self, event: &WidgetEvent) -> WidgetAction {
        match event {
            WidgetEvent::KeyPress { ascii, scancode } => {
                match *ascii {
                    b'\n' => {
                        // Submit
                        let submitted = self.text.clone();
                        self.text.clear();
                        self.cursor_pos = 0;
                        WidgetAction::Execute(AppCommand::TextSubmitted(submitted))
                    }
                    0x08 => {
                        // Backspace
                        if self.cursor_pos > 0 {
                            self.cursor_pos -= 1;
                            self.text.remove(self.cursor_pos);
                        }
                        WidgetAction::Redraw
                    }
                    0x1B => {
                        // Escape — clear input
                        self.text.clear();
                        self.cursor_pos = 0;
                        WidgetAction::Redraw
                    }
                    0 => {
                        // Non-ASCII key — check scancode for arrow keys
                        match *scancode {
                            0x4B => { // Left arrow
                                if self.cursor_pos > 0 { self.cursor_pos -= 1; }
                                WidgetAction::Redraw
                            }
                            0x4D => { // Right arrow
                                if self.cursor_pos < self.text.len() { self.cursor_pos += 1; }
                                WidgetAction::Redraw
                            }
                            0x47 => { // Home
                                self.cursor_pos = 0;
                                WidgetAction::Redraw
                            }
                            0x4F => { // End
                                self.cursor_pos = self.text.len();
                                WidgetAction::Redraw
                            }
                            _ => WidgetAction::None,
                        }
                    }
                    0x03 => {
                        // Ctrl+C — copy entire input text to clipboard
                        if !self.text.is_empty() {
                            crate::gui::clipboard::copy(&self.text);
                        }
                        WidgetAction::Redraw
                    }
                    0x16 => {
                        // Ctrl+V — paste from clipboard
                        if let Some(clip) = crate::gui::clipboard::paste() {
                            for ch in clip.chars() {
                                if ch >= ' ' && (ch as u32) < 0x7F && self.text.len() < self.max_len {
                                    self.text.insert(self.cursor_pos, ch);
                                    self.cursor_pos += 1;
                                }
                            }
                        }
                        WidgetAction::Redraw
                    }
                    0x18 => {
                        // Ctrl+X — cut (copy + clear)
                        if !self.text.is_empty() {
                            crate::gui::clipboard::cut(&self.text);
                            self.text.clear();
                            self.cursor_pos = 0;
                        }
                        WidgetAction::Redraw
                    }
                    ch if ch >= 0x20 && ch < 0x7F => {
                        // Printable ASCII
                        if self.text.len() < self.max_len {
                            self.text.insert(self.cursor_pos, ch as char);
                            self.cursor_pos += 1;
                        }
                        WidgetAction::Redraw
                    }
                    _ => WidgetAction::None,
                }
            }
            _ => WidgetAction::None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  ScrollableText
// ═══════════════════════════════════════════════════════════════

/// A scrollable multi-line text display.
pub struct ScrollableText {
    /// Lines of text with individual colors.
    pub lines: Vec<(String, Color)>,
    /// Current scroll offset (first visible line).
    pub scroll_offset: usize,
    /// Maximum lines to retain (ring buffer limit).
    pub max_lines: usize,
}

impl ScrollableText {
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: Vec::new(),
            scroll_offset: 0,
            max_lines,
        }
    }

    /// Add a line, auto-scrolling if at bottom.
    pub fn push_line(&mut self, text: &str, color: Color) {
        let at_bottom = self.is_at_bottom();
        self.lines.push((String::from(text), color));
        // Trim if over limit
        if self.lines.len() > self.max_lines {
            self.lines.remove(0);
            if self.scroll_offset > 0 {
                self.scroll_offset -= 1;
            }
        }
        // Auto-scroll to bottom if user was already at bottom
        if at_bottom {
            self.scroll_to_bottom();
        }
    }

    /// Clear all lines.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.scroll_offset = 0;
    }

    /// Set all lines at once (replaces existing content).
    pub fn set_lines(&mut self, lines: Vec<(String, Color)>) {
        self.lines = lines;
        // Clamp scroll offset
        let visible = self.visible_line_count(200); // Approximate
        if self.scroll_offset + visible > self.lines.len() {
            self.scroll_to_bottom();
        }
    }

    fn visible_line_count(&self, widget_height: usize) -> usize {
        widget_height.saturating_sub(4) / 18 // 16px font + 2px spacing
    }

    fn is_at_bottom(&self) -> bool {
        // Conservative check
        self.lines.len() <= self.scroll_offset + 30 // Approximate visible lines
    }

    fn scroll_to_bottom(&mut self) {
        let visible = 20; // Approximate; will be exact during render
        self.scroll_offset = self.lines.len().saturating_sub(visible);
    }

    /// Public scroll to bottom (used by apps syncing state).
    pub fn scroll_to_bottom_pub(&mut self) {
        self.scroll_to_bottom();
    }

    fn render(&self, comp: &mut Compositor, x: usize, y: usize, w: usize, h: usize) {
        // Background
        comp.fill_rect(x, y, w, h, BG_INPUT);

        let text_x = x + 4;
        let mut text_y = y + 2;
        let max_lines = h.saturating_sub(4) / 18;
        let max_chars = w.saturating_sub(16) / 8; // Leave room for scrollbar

        for i in 0..max_lines {
            let line_idx = self.scroll_offset + i;
            if line_idx >= self.lines.len() { break; }
            let (ref text, color) = self.lines[line_idx];
            // Truncate long lines
            if text.len() > max_chars {
                comp.draw_text(text_x, text_y, &text[..max_chars], color);
            } else {
                comp.draw_text(text_x, text_y, text, color);
            }
            text_y += 18;
        }

        // Scrollbar (if content exceeds visible area)
        if self.lines.len() > max_lines && max_lines > 0 {
            let sb_x = x + w - 6;
            let sb_y = y + 2;
            let sb_h = h - 4;
            comp.fill_rect(sb_x, sb_y, 4, sb_h, SCROLLBAR_BG);

            // Thumb position and size
            let total = self.lines.len();
            let thumb_h = ((max_lines as u64 * sb_h as u64) / total as u64).max(8) as usize;
            let thumb_y = sb_y + (self.scroll_offset as u64 * (sb_h - thumb_h) as u64 / total.saturating_sub(max_lines).max(1) as u64) as usize;
            comp.fill_rect(sb_x, thumb_y, 4, thumb_h, SCROLLBAR_FG);
        }
    }

    fn handle_event(&mut self, event: &WidgetEvent) -> WidgetAction {
        match event {
            WidgetEvent::KeyPress { scancode: 0x49, .. } => {
                // Page Up
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
                WidgetAction::Redraw
            }
            WidgetEvent::KeyPress { scancode: 0x51, .. } => {
                // Page Down
                self.scroll_offset = (self.scroll_offset + 10).min(self.lines.len().saturating_sub(1));
                WidgetAction::Redraw
            }
            WidgetEvent::MouseClick { x: _, y: click_y } => {
                // Calculate which line was clicked (relative to widget)
                let line_in_view = *click_y / 18;
                let abs_line = self.scroll_offset + line_in_view;
                if abs_line < self.lines.len() {
                    WidgetAction::Execute(AppCommand::LineClicked(abs_line))
                } else {
                    WidgetAction::None
                }
            }
            _ => WidgetAction::None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  StaticLabel
// ═══════════════════════════════════════════════════════════════

/// A non-interactive text label.
pub struct StaticLabel {
    pub text: String,
    pub color: Color,
}

impl StaticLabel {
    pub fn new(text: &str, color: Color) -> Self {
        Self { text: String::from(text), color }
    }

    fn render(&self, comp: &mut Compositor, x: usize, y: usize) {
        comp.draw_text(x, y, &self.text, self.color);
    }
}

// ═══════════════════════════════════════════════════════════════
//  RawInput (for Text Editor)
// ═══════════════════════════════════════════════════════════════

/// A widget that captures all key events and forwards them as RawKey actions.
/// Used by the text editor to receive direct keyboard input.
pub struct RawInputWidget {
    /// Lines to display (set by the app).
    pub display_lines: Vec<(String, Color)>,
    /// Scroll offset.
    pub scroll_offset: usize,
}

impl RawInputWidget {
    pub fn new() -> Self {
        Self {
            display_lines: Vec::new(),
            scroll_offset: 0,
        }
    }

    /// Set display content.
    pub fn set_lines(&mut self, lines: Vec<(String, Color)>) {
        self.display_lines = lines;
    }

    fn render(&self, comp: &mut Compositor, x: usize, y: usize, w: usize, h: usize) {
        // Background
        comp.fill_rect(x, y, w, h, BG_INPUT);

        let text_x = x + 4;
        let mut text_y = y + 2;
        let max_lines = h.saturating_sub(4) / 18;
        let max_chars = w.saturating_sub(12) / 8;

        for i in 0..max_lines {
            let line_idx = self.scroll_offset + i;
            if line_idx >= self.display_lines.len() { break; }
            let (ref text, color) = self.display_lines[line_idx];
            if text.len() > max_chars {
                comp.draw_text(text_x, text_y, &text[..max_chars], color);
            } else {
                comp.draw_text(text_x, text_y, text, color);
            }
            text_y += 18;
        }

        // Scrollbar
        if self.display_lines.len() > max_lines && max_lines > 0 {
            let sb_x = x + w - 6;
            let sb_y = y + 2;
            let sb_h = h - 4;
            comp.fill_rect(sb_x, sb_y, 4, sb_h, SCROLLBAR_BG);
            let total = self.display_lines.len();
            let thumb_h = ((max_lines as u64 * sb_h as u64) / total as u64).max(8) as usize;
            let thumb_y = sb_y + (self.scroll_offset as u64 * (sb_h - thumb_h) as u64 / total.saturating_sub(max_lines).max(1) as u64) as usize;
            comp.fill_rect(sb_x, thumb_y, 4, thumb_h, SCROLLBAR_FG);
        }
    }

    fn handle_event(&mut self, event: &WidgetEvent) -> WidgetAction {
        match event {
            WidgetEvent::KeyPress { ascii, scancode } => {
                WidgetAction::Execute(AppCommand::RawKey(*ascii, *scancode))
            }
            WidgetEvent::MouseClick { x: _, y: click_y } => {
                let line_in_view = *click_y / 18;
                let abs_line = self.scroll_offset + line_in_view;
                if abs_line < self.display_lines.len() {
                    WidgetAction::Execute(AppCommand::LineClicked(abs_line))
                } else {
                    WidgetAction::None
                }
            }
            _ => WidgetAction::None,
        }
    }
}

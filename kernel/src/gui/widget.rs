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
    KeyPress { key: char, scancode: u8 },
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
    /// Up arrow in TextInput → scroll back through command history.
    HistoryPrev,
    /// Down arrow in TextInput → scroll forward through command history.
    HistoryNext,
    /// Tab key in TextInput → complete current word. Carries current input text.
    TabComplete(String),
    /// Ctrl+C in TextInput → interrupt foreground command.
    Interrupt,
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
    /// A web page viewport: paints a list of pre-computed render commands
    /// produced by the HTML/CSS layout pipeline (see apps::browser_render).
    WebPage(WebPage),
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
            WidgetKind::WebPage(p) => p.render(comp, wx, wy, self.width, self.height),
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
            WidgetKind::WebPage(p) => p.handle_event(event),
        }
    }

    /// Whether this widget can receive keyboard focus.
    pub fn is_focusable(&self) -> bool {
        matches!(self.kind,
            WidgetKind::TextInput(_) | WidgetKind::Button(_) | WidgetKind::RawInput(_) | WidgetKind::WebPage(_))
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

        // Center the label text — use TTF measurement when available
        const FONT_PX: f32 = 14.0;
        let text_w = comp.measure_text(&self.label, FONT_PX);
        let tx = x + (w.saturating_sub(text_w)) / 2;
        let ty = y + (h.saturating_sub(14)) / 2;
        comp.draw_text(tx, ty, &self.label, self.accent);
    }

    fn handle_event(&mut self, event: &WidgetEvent) -> WidgetAction {
        match event {
            WidgetEvent::MouseClick { .. } | WidgetEvent::KeyPress { key: '\n', .. } => {
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
    pub masked: bool,   // show '*' instead of actual characters (password fields)
}

impl TextInput {
    pub fn new(placeholder: &str, accent: Color) -> Self {
        Self {
            text: String::new(),
            cursor_pos: 0,
            placeholder: String::from(placeholder),
            accent,
            max_len: 200,
            masked: false,
        }
    }

    pub fn new_password(placeholder: &str, accent: Color) -> Self {
        Self { masked: true, ..Self::new(placeholder, accent) }
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
            // Actual text — show visible portion (masked as '*' for password fields)
            let char_count = self.text.chars().count();
            let display_chars: Vec<char> = if self.masked {
                "*".repeat(char_count).chars().collect()
            } else {
                self.text.chars().collect()
            };
            let max_chars = (w.saturating_sub(24)) / 8; // Available chars
            let display_start = if self.cursor_pos > max_chars {
                self.cursor_pos - max_chars
            } else {
                0
            };
            let display_end = (display_start + max_chars).min(display_chars.len());
            let display_text: String = display_chars[display_start..display_end].iter().collect();
            comp.draw_text(text_x, text_y, &display_text, TEXT_PRIMARY);

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
            WidgetEvent::KeyPress { key, scancode } => {
                let mut chars: Vec<char> = self.text.chars().collect();
                match *key {
                    '\n' => {
                        // Submit — keep text in place so the app thread can read it
                        // via the widget state (clearing it here would race with the
                        // app thread that reads credentials after dequeuing the action).
                        let submitted = self.text.clone();
                        WidgetAction::Execute(AppCommand::TextSubmitted(submitted))
                    }
                    '\x08' => {
                        // Backspace
                        if self.cursor_pos > 0 {
                            self.cursor_pos -= 1;
                            chars.remove(self.cursor_pos);
                            self.text = chars.into_iter().collect();
                        }
                        WidgetAction::Redraw
                    }
                    '\x1B' => {
                        // Escape — clear input
                        self.text.clear();
                        self.cursor_pos = 0;
                        WidgetAction::Redraw
                    }
                    '\x09' => {
                        // Tab — request completion with current text
                        WidgetAction::Execute(AppCommand::TabComplete(self.text.clone()))
                    }
                    '\0' => {
                        // Non-ASCII key — check scancode for arrow keys
                        match *scancode {
                            0x48 => { // Up arrow → previous history entry
                                WidgetAction::Execute(AppCommand::HistoryPrev)
                            }
                            0x50 => { // Down arrow → next history entry
                                WidgetAction::Execute(AppCommand::HistoryNext)
                            }
                            0x4B => { // Left arrow
                                if self.cursor_pos > 0 { self.cursor_pos -= 1; }
                                WidgetAction::Redraw
                            }
                            0x4D => { // Right arrow
                                if self.cursor_pos < chars.len() { self.cursor_pos += 1; }
                                WidgetAction::Redraw
                            }
                            0x47 => { // Home
                                self.cursor_pos = 0;
                                WidgetAction::Redraw
                            }
                            0x4F => { // End
                                self.cursor_pos = chars.len();
                                WidgetAction::Redraw
                            }
                            _ => WidgetAction::None,
                        }
                    }
                    '\x03' => {
                        // Ctrl+C — interrupt foreground command
                        WidgetAction::Execute(AppCommand::Interrupt)
                    }
                    '\x16' => {
                        // Ctrl+V — paste from clipboard
                        if let Some(clip) = crate::gui::clipboard::paste() {
                            for ch in clip.chars() {
                                if ch >= ' ' && chars.len() < self.max_len {
                                    chars.insert(self.cursor_pos, ch);
                                    self.cursor_pos += 1;
                                }
                            }
                            self.text = chars.into_iter().collect();
                        }
                        WidgetAction::Redraw
                    }
                    '\x18' => {
                        // Ctrl+X — cut (copy + clear)
                        if !self.text.is_empty() {
                            crate::gui::clipboard::cut(&self.text);
                            self.text.clear();
                            self.cursor_pos = 0;
                        }
                        WidgetAction::Redraw
                    }
                    ch if ch >= ' ' => {
                        // Printable Unicode
                        if chars.len() < self.max_len {
                            chars.insert(self.cursor_pos, ch);
                            self.cursor_pos += 1;
                            self.text = chars.into_iter().collect();
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
            // Truncate long lines (use char boundary to handle multi-byte UTF-8)
            let char_count = text.chars().count();
            if char_count > max_chars {
                let byte_end = text.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(text.len());
                comp.draw_text(text_x, text_y, &text[..byte_end], color);
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
            WidgetEvent::KeyPress { key, scancode } => {
                WidgetAction::Execute(AppCommand::RawKey(*key as u8, *scancode))
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

// ═══════════════════════════════════════════════════════════════
//  WebPage — paints HTML/CSS layout commands
// ═══════════════════════════════════════════════════════════════

/// A single rendering instruction in a web-page viewport.
///
/// All coordinates are in "document" space (relative to the top of the page);
/// the WebPage widget subtracts its scroll offset and clips to its viewport
/// before forwarding to the compositor.
#[derive(Clone)]
pub enum RenderCmd {
    /// Fill a rectangle with a solid color (used for backgrounds).
    Rect { x: i32, y: i32, w: u32, h: u32, color: Color },
    /// 1-pixel outline rectangle (used for borders).
    Border { x: i32, y: i32, w: u32, h: u32, color: Color },
    /// Draw a text run.  `scale=1` is the native 8×16 bitmap; `scale=2` is 16×32, etc.
    Text { x: i32, y: i32, text: String, color: Color, scale: u8 },
    /// Horizontal rule.
    HRule { x: i32, y: i32, w: u32, color: Color },
    /// Blit a decoded RGBA image.  `data` is row-major, 4 bytes/pixel.
    Image { x: i32, y: i32, w: u32, h: u32, data: alloc::sync::Arc<Vec<u8>> },
    /// Pending image – fetch URL outside the layout engine, then replace with Image.
    ImagePlaceholder { url: String, alt: String, x: i32, y: i32, w: u32, h: u32 },
}

/// A scrollable HTML/CSS rendered page.
pub struct WebPage {
    /// Pre-built display list.
    pub commands: Vec<RenderCmd>,
    /// Total height of the document in pixels.
    pub content_height: u32,
    /// Current scroll offset in pixels (top of viewport in document space).
    pub scroll_offset: u32,
    /// Background fill drawn before commands.
    pub background: Color,
}

impl WebPage {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            content_height: 0,
            scroll_offset: 0,
            background: BG_INPUT,
        }
    }

    /// Replace the page contents.
    pub fn set_page(&mut self, cmds: Vec<RenderCmd>, height: u32, bg: Color) {
        self.commands = cmds;
        self.content_height = height;
        self.background = bg;
        self.scroll_offset = 0;
    }

    fn max_scroll(&self, widget_height: usize) -> u32 {
        self.content_height.saturating_sub(widget_height as u32)
    }

    fn render(&self, comp: &mut Compositor, x: usize, y: usize, w: usize, h: usize) {
        // 1. Page background.
        comp.fill_rect(x, y, w, h, self.background);

        let scroll = self.scroll_offset as i32;
        let view_top    = scroll;
        let view_bottom = scroll + h as i32;
        let scrollbar_w = if self.content_height > h as u32 { 6 } else { 0 };
        let content_w   = w.saturating_sub(scrollbar_w);

        // 2. Walk display list, clipping y against [view_top, view_bottom).
        for cmd in &self.commands {
            match cmd {
                RenderCmd::Rect { x: cx, y: cy, w: cw, h: ch, color } => {
                    let top    = *cy;
                    let bottom = cy.saturating_add(*ch as i32);
                    if bottom <= view_top || top >= view_bottom { continue; }
                    let dx = x as i32 + *cx;
                    let dy = y as i32 + (top - scroll);
                    let dw = (*cw).min(content_w as u32);
                    let dh = *ch;
                    if dx < x as i32 || dy < y as i32 {
                        // Clip: trim if extending off the top/left.
                        let cl_x = (dx).max(x as i32);
                        let cl_y = (dy).max(y as i32);
                        let cl_w = (dx + dw as i32 - cl_x).max(0) as u32;
                        let cl_h = (dy + dh as i32 - cl_y).max(0) as u32;
                        comp.fill_rect(cl_x as usize, cl_y as usize, cl_w as usize, cl_h as usize, *color);
                    } else {
                        comp.fill_rect(dx as usize, dy as usize, dw as usize, dh as usize, *color);
                    }
                }
                RenderCmd::Border { x: cx, y: cy, w: cw, h: ch, color } => {
                    let top    = *cy;
                    let bottom = cy.saturating_add(*ch as i32);
                    if bottom <= view_top || top >= view_bottom { continue; }
                    let dx = x as i32 + *cx;
                    let dy = y as i32 + (top - scroll);
                    if dx >= 0 && dy >= 0 {
                        comp.draw_rect(dx as usize, dy as usize, *cw as usize, *ch as usize, *color);
                    }
                }
                RenderCmd::Text { x: cx, y: cy, text, color, scale } => {
                    let scl = (*scale).max(1) as usize;
                    let line_h = 16 * scl as i32;
                    let top    = *cy;
                    let bottom = cy.saturating_add(line_h);
                    if bottom <= view_top || top >= view_bottom { continue; }
                    let dx = x as i32 + *cx;
                    let dy = y as i32 + (top - scroll);
                    if dx >= 0 && dy >= 0 {
                        // Truncate text to fit visible content width.
                        let max_chars = content_w
                            .saturating_sub((cx.max(&0).unsigned_abs() as usize).min(content_w))
                            / (8 * scl);
                        let char_count = text.chars().count();
                        if char_count > max_chars && max_chars > 0 {
                            let byte_end = text.char_indices()
                                .nth(max_chars)
                                .map(|(i, _)| i)
                                .unwrap_or(text.len());
                            comp.draw_text_scaled(dx as usize, dy as usize, &text[..byte_end], *color, scl);
                        } else {
                            comp.draw_text_scaled(dx as usize, dy as usize, text, *color, scl);
                        }
                    }
                }
                RenderCmd::HRule { x: cx, y: cy, w: cw, color } => {
                    let top = *cy;
                    if top < view_top || top >= view_bottom { continue; }
                    let dx = x as i32 + *cx;
                    let dy = y as i32 + (top - scroll);
                    if dx >= 0 && dy >= 0 {
                        comp.fill_rect(dx as usize, dy as usize, (*cw as usize).min(content_w), 1, *color);
                    }
                }
                RenderCmd::Image { x: cx, y: cy, w: iw, h: ih, data } => {
                    let top    = *cy;
                    let bottom = cy.saturating_add(*ih as i32);
                    if bottom <= view_top || top >= view_bottom { continue; }
                    let dx = x as i32 + *cx;
                    let dy = y as i32 + (top - scroll);
                    if dx < 0 || dy < 0 { continue; }
                    paint_rgba(comp, dx as usize, dy as usize, *iw, *ih, data);
                }
                RenderCmd::ImagePlaceholder { x: cx, y: cy, w: iw, h: ih, alt, .. } => {
                    // Draw a gray box with a thin border while the image loads.
                    let top    = *cy;
                    let bottom = cy.saturating_add(*ih as i32);
                    if bottom <= view_top || top >= view_bottom { continue; }
                    let dx = x as i32 + *cx;
                    let dy = y as i32 + (top - scroll);
                    if dx < 0 || dy < 0 { continue; }
                    let placeholder_bg  = Color { r: 60,  g: 60,  b: 70  };
                    let placeholder_bdr = Color { r: 100, g: 100, b: 115 };
                    let placeholder_txt = Color { r: 160, g: 160, b: 175 };
                    comp.fill_rect(dx as usize, dy as usize, *iw as usize, *ih as usize, placeholder_bg);
                    // Border
                    comp.fill_rect(dx as usize, dy as usize, *iw as usize, 1, placeholder_bdr);
                    comp.fill_rect(dx as usize, (dy as usize + *ih as usize).saturating_sub(1), *iw as usize, 1, placeholder_bdr);
                    comp.fill_rect(dx as usize, dy as usize, 1, *ih as usize, placeholder_bdr);
                    comp.fill_rect((dx as usize + *iw as usize).saturating_sub(1), dy as usize, 1, *ih as usize, placeholder_bdr);
                    // Alt text centered (best-effort)
                    if !alt.is_empty() && *iw >= 20 && *ih >= 16 {
                        let text_y = dy + (*ih as i32 / 2).saturating_sub(8);
                        let max_chars = (*iw / 8).max(1) as usize;
                        let label: String = if alt.len() <= max_chars {
                            alt.clone()
                        } else {
                            alloc::format!("{}…", &alt[..max_chars.saturating_sub(1)])
                        };
                        comp.draw_text(dx as usize + 4, text_y.max(dy) as usize, &label, placeholder_txt);
                    }
                }
            }
        }

        // 3. Scroll bar.
        if self.content_height > h as u32 {
            let sb_x = x + w - 6;
            let sb_y = y + 2;
            let sb_h = h.saturating_sub(4);
            comp.fill_rect(sb_x, sb_y, 4, sb_h, SCROLLBAR_BG);
            let total  = self.content_height as u64;
            let view_h = h as u64;
            let thumb_h = ((view_h * sb_h as u64) / total).max(8) as usize;
            let max_scroll = (total - view_h).max(1);
            let thumb_y = sb_y + ((self.scroll_offset as u64 * (sb_h - thumb_h) as u64) / max_scroll) as usize;
            comp.fill_rect(sb_x, thumb_y, 4, thumb_h, SCROLLBAR_FG);
        }
    }

    fn handle_event(&mut self, event: &WidgetEvent) -> WidgetAction {
        match event {
            // Page Up
            WidgetEvent::KeyPress { scancode: 0x49, .. } => {
                self.scroll_offset = self.scroll_offset.saturating_sub(200);
                WidgetAction::Redraw
            }
            // Page Down
            WidgetEvent::KeyPress { scancode: 0x51, .. } => {
                let max = self.content_height.saturating_sub(50);
                self.scroll_offset = (self.scroll_offset + 200).min(max);
                WidgetAction::Redraw
            }
            // Arrow Up
            WidgetEvent::KeyPress { scancode: 0x48, .. } => {
                self.scroll_offset = self.scroll_offset.saturating_sub(40);
                WidgetAction::Redraw
            }
            // Arrow Down
            WidgetEvent::KeyPress { scancode: 0x50, .. } => {
                let max = self.content_height.saturating_sub(50);
                self.scroll_offset = (self.scroll_offset + 40).min(max);
                WidgetAction::Redraw
            }
            // Home
            WidgetEvent::KeyPress { scancode: 0x47, .. } => {
                self.scroll_offset = 0;
                WidgetAction::Redraw
            }
            // End
            WidgetEvent::KeyPress { scancode: 0x4F, .. } => {
                self.scroll_offset = self.content_height.saturating_sub(50);
                WidgetAction::Redraw
            }
            _ => WidgetAction::None,
        }
    }

    /// Clamp scroll offset against current widget height (called by the app
    /// after a re-layout when the widget dimensions are known).
    pub fn clamp_scroll(&mut self, widget_height: usize) {
        let max = self.max_scroll(widget_height);
        if self.scroll_offset > max {
            self.scroll_offset = max;
        }
    }
}

/// Paint a row-major RGBA8888 buffer to the compositor at (x, y).
/// Skips fully transparent pixels and treats partial alpha as opaque to keep
/// the painter cheap; we don't blend against the back-buffer.
fn paint_rgba(comp: &mut Compositor, x: usize, y: usize, w: u32, h: u32, data: &[u8]) {
    let expected = (w * h * 4) as usize;
    if data.len() < expected { return; }
    for row in 0..h as usize {
        for col in 0..w as usize {
            let i = (row * w as usize + col) * 4;
            let r = data[i];
            let g = data[i + 1];
            let b = data[i + 2];
            let a = data[i + 3];
            if a < 64 { continue; }
            comp.set_pixel(x + col, y + row, Color { r, g, b });
        }
    }
}

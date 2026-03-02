/// Text Editor Application for Smart OS.
///
/// A simple but functional text editor with cursor navigation, line editing,
/// file I/O, and keyboard shortcuts (Ctrl+S save, Ctrl+O open, Ctrl+Q quit).

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::{Window, WindowId};
use crate::gui::widget::*;
use crate::gui::desktop::DESKTOP;

// ═══════════════════════════════════════════════════════════════
//  State
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditorMode {
    Normal,
    Command,
}

pub struct EditorState {
    pub window_id: WindowId,
    pub file_path: String,
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub scroll_offset: usize,
    pub dirty: bool,
    pub mode: EditorMode,
    pub status_msg: String,
    pub needs_redraw: bool,
}

pub static STATE: Mutex<Option<EditorState>> = Mutex::new(None);

/// File path to open on startup (set by terminal `edit` command).
pub static INITIAL_FILE: Mutex<Option<String>> = Mutex::new(None);

/// Set the initial file path for the editor to open.
pub fn set_initial_file(path: &str) {
    *INITIAL_FILE.lock() = Some(String::from(path));
}

// ═══════════════════════════════════════════════════════════════
//  Entry Point
// ═══════════════════════════════════════════════════════════════

/// Text editor main loop (spawned as a kernel thread).
pub fn run() {
    // Create editor window
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() {
            Some(d) => d,
            None => return,
        };

        let win_w = 500;
        let win_h = 380;

        let mut win = Window::new("Editor", 120, 60, win_w, win_h, ACCENT_CYAN);
        win.use_widgets = true;

        // Widget 0: Status bar (label)
        let status_widget = Widget::new(
            0, 0, 0, win_w, 20,
            WidgetKind::Label(StaticLabel::new("Editor: [new]", ACCENT_CYAN)),
        );
        win.widgets.push(status_widget);

        // Widget 1: Main text area (RawInput — captures all keys)
        let text_area_h = win_h - 80;
        let mut raw = Widget::new(
            1, 0, 22, win_w, text_area_h,
            WidgetKind::RawInput(RawInputWidget::new()),
        );
        raw.focused = true;
        win.widgets.push(raw);

        // Widget 2: Command bar (TextInput — for Ctrl+O, Ctrl+G prompts)
        let cmd_bar = Widget::new(
            2, 0, 22 + text_area_h + 2, win_w, 24,
            WidgetKind::TextInput(TextInput::new("Ctrl+S save | Ctrl+O open | Ctrl+Q quit", ACCENT_CYAN)),
        );
        win.widgets.push(cmd_bar);

        win.focused_widget = Some(1);
        let id = win.id;
        desk.wm.add(win);
        id
    };

    // Initialize editor state
    let mut initial_lines = vec![String::new()];
    let mut initial_path = String::from("[new]");

    // Try to load initial file
    if let Some(path) = INITIAL_FILE.lock().take() {
        match load_file_content(&path) {
            Ok(lines) => {
                initial_lines = lines;
                initial_path = path;
            }
            Err(_) => {
                initial_path = path;
            }
        }
    }

    let state = EditorState {
        window_id,
        file_path: initial_path,
        lines: initial_lines,
        cursor_line: 0,
        cursor_col: 0,
        scroll_offset: 0,
        dirty: false,
        mode: EditorMode::Normal,
        status_msg: String::new(),
        needs_redraw: true,
    };
    *STATE.lock() = Some(state);

    // Main event loop
    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut state_guard = STATE.lock();
            if let Some(ref mut state) = *state_guard {
                match action {
                    WidgetAction::Execute(AppCommand::RawKey(ascii, scancode)) => {
                        if state.mode == EditorMode::Normal {
                            handle_key(state, ascii, scancode);
                        }
                    }
                    WidgetAction::Execute(AppCommand::TextSubmitted(text)) => {
                        // Command bar response
                        handle_command(state, &text);
                        // Return focus to text area
                        state.mode = EditorMode::Normal;
                        state.needs_redraw = true;
                    }
                    _ => {}
                }
            }
        }
        crate::process::scheduler::yield_now();
    }
}

// ═══════════════════════════════════════════════════════════════
//  Key Handling
// ═══════════════════════════════════════════════════════════════

fn handle_key(state: &mut EditorState, ascii: u8, scancode: u8) {
    match ascii {
        // Ctrl+S (save)
        0x13 => {
            save_file(state);
        }
        // Ctrl+O (open)
        0x0F => {
            state.mode = EditorMode::Command;
            state.status_msg = String::from("Open file:");
            state.needs_redraw = true;
        }
        // Ctrl+Q (quit) — we can't really close, just clear
        0x11 => {
            state.status_msg = String::from("Cannot close (kernel thread)");
            state.needs_redraw = true;
        }
        // Ctrl+G (goto line)
        0x07 => {
            state.mode = EditorMode::Command;
            state.status_msg = String::from("Goto line:");
            state.needs_redraw = true;
        }
        // Ctrl+C (copy current line)
        0x03 => {
            if state.cursor_line < state.lines.len() {
                let line = state.lines[state.cursor_line].clone();
                crate::gui::clipboard::copy(&line);
                state.status_msg = String::from("Line copied");
                state.needs_redraw = true;
            }
        }
        // Ctrl+V (paste from clipboard)
        0x16 => {
            if let Some(clip) = crate::gui::clipboard::paste() {
                for ch in clip.chars() {
                    if ch == '\n' {
                        split_line(state);
                    } else if ch >= ' ' && (ch as u32) < 0x7F {
                        insert_char(state, ch);
                    }
                }
                state.dirty = true;
                state.status_msg = String::from("Pasted");
                state.needs_redraw = true;
            }
        }
        // Ctrl+X (cut current line)
        0x18 => {
            if state.cursor_line < state.lines.len() {
                let line = state.lines.remove(state.cursor_line);
                crate::gui::clipboard::cut(&line);
                if state.lines.is_empty() {
                    state.lines.push(String::new());
                }
                if state.cursor_line >= state.lines.len() {
                    state.cursor_line = state.lines.len() - 1;
                }
                state.cursor_col = 0;
                state.dirty = true;
                state.status_msg = String::from("Line cut");
                state.needs_redraw = true;
            }
        }
        // Enter
        0x0D | 0x0A => {
            split_line(state);
        }
        // Backspace
        0x08 => {
            delete_backward(state);
        }
        // Printable ASCII
        ch if ch >= 0x20 && ch < 0x7F => {
            insert_char(state, ch as char);
        }
        // Non-ASCII — handle via scancode
        0 => {
            match scancode {
                0x48 => move_cursor(state, Direction::Up),
                0x50 => move_cursor(state, Direction::Down),
                0x4B => move_cursor(state, Direction::Left),
                0x4D => move_cursor(state, Direction::Right),
                0x47 => move_cursor(state, Direction::Home),
                0x4F => move_cursor(state, Direction::End),
                0x49 => move_cursor(state, Direction::PageUp),
                0x51 => move_cursor(state, Direction::PageDown),
                0x53 => delete_forward(state),
                _ => {}
            }
        }
        _ => {}
    }
}

fn handle_command(state: &mut EditorState, text: &str) {
    if state.status_msg.starts_with("Open file") {
        // Load file
        match load_file_content(text) {
            Ok(lines) => {
                state.lines = lines;
                state.file_path = String::from(text);
                state.cursor_line = 0;
                state.cursor_col = 0;
                state.scroll_offset = 0;
                state.dirty = false;
                state.status_msg = format!("Loaded: {}", text);
            }
            Err(e) => {
                state.status_msg = format!("Error: {}", e);
            }
        }
    } else if state.status_msg.starts_with("Goto line") {
        // Parse line number
        if let Ok(line_num) = text.parse::<usize>() {
            let target = line_num.saturating_sub(1).min(state.lines.len().saturating_sub(1));
            state.cursor_line = target;
            state.cursor_col = 0;
            ensure_cursor_visible(state);
            state.status_msg = format!("Line {}", target + 1);
        } else {
            state.status_msg = String::from("Invalid line number");
        }
    }
    state.needs_redraw = true;
}

// ═══════════════════════════════════════════════════════════════
//  Text Editing Operations
// ═══════════════════════════════════════════════════════════════

fn insert_char(state: &mut EditorState, ch: char) {
    if state.cursor_line < state.lines.len() {
        let col = state.cursor_col.min(state.lines[state.cursor_line].len());
        state.lines[state.cursor_line].insert(col, ch);
        state.cursor_col = col + 1;
        state.dirty = true;
        state.needs_redraw = true;
    }
}

fn delete_backward(state: &mut EditorState) {
    if state.cursor_col > 0 && state.cursor_line < state.lines.len() {
        state.cursor_col -= 1;
        state.lines[state.cursor_line].remove(state.cursor_col);
        state.dirty = true;
        state.needs_redraw = true;
    } else if state.cursor_col == 0 && state.cursor_line > 0 {
        // Join with previous line
        let current = state.lines.remove(state.cursor_line);
        state.cursor_line -= 1;
        state.cursor_col = state.lines[state.cursor_line].len();
        state.lines[state.cursor_line].push_str(&current);
        state.dirty = true;
        state.needs_redraw = true;
    }
}

fn delete_forward(state: &mut EditorState) {
    if state.cursor_line < state.lines.len() {
        let line_len = state.lines[state.cursor_line].len();
        if state.cursor_col < line_len {
            state.lines[state.cursor_line].remove(state.cursor_col);
            state.dirty = true;
            state.needs_redraw = true;
        } else if state.cursor_line + 1 < state.lines.len() {
            // Join with next line
            let next = state.lines.remove(state.cursor_line + 1);
            state.lines[state.cursor_line].push_str(&next);
            state.dirty = true;
            state.needs_redraw = true;
        }
    }
}

fn split_line(state: &mut EditorState) {
    if state.cursor_line < state.lines.len() {
        let col = state.cursor_col.min(state.lines[state.cursor_line].len());
        let rest = state.lines[state.cursor_line][col..].to_string();
        state.lines[state.cursor_line].truncate(col);
        state.cursor_line += 1;
        state.lines.insert(state.cursor_line, rest);
        state.cursor_col = 0;
        state.dirty = true;
        state.needs_redraw = true;
        ensure_cursor_visible(state);
    }
}

enum Direction { Up, Down, Left, Right, Home, End, PageUp, PageDown }

fn move_cursor(state: &mut EditorState, dir: Direction) {
    match dir {
        Direction::Up => {
            if state.cursor_line > 0 {
                state.cursor_line -= 1;
                clamp_col(state);
            }
        }
        Direction::Down => {
            if state.cursor_line + 1 < state.lines.len() {
                state.cursor_line += 1;
                clamp_col(state);
            }
        }
        Direction::Left => {
            if state.cursor_col > 0 {
                state.cursor_col -= 1;
            } else if state.cursor_line > 0 {
                state.cursor_line -= 1;
                state.cursor_col = state.lines[state.cursor_line].len();
            }
        }
        Direction::Right => {
            let line_len = if state.cursor_line < state.lines.len() {
                state.lines[state.cursor_line].len()
            } else { 0 };
            if state.cursor_col < line_len {
                state.cursor_col += 1;
            } else if state.cursor_line + 1 < state.lines.len() {
                state.cursor_line += 1;
                state.cursor_col = 0;
            }
        }
        Direction::Home => {
            state.cursor_col = 0;
        }
        Direction::End => {
            if state.cursor_line < state.lines.len() {
                state.cursor_col = state.lines[state.cursor_line].len();
            }
        }
        Direction::PageUp => {
            state.cursor_line = state.cursor_line.saturating_sub(20);
            clamp_col(state);
        }
        Direction::PageDown => {
            state.cursor_line = (state.cursor_line + 20).min(state.lines.len().saturating_sub(1));
            clamp_col(state);
        }
    }
    ensure_cursor_visible(state);
    state.needs_redraw = true;
}

fn clamp_col(state: &mut EditorState) {
    if state.cursor_line < state.lines.len() {
        state.cursor_col = state.cursor_col.min(state.lines[state.cursor_line].len());
    }
}

fn ensure_cursor_visible(state: &mut EditorState) {
    if state.cursor_line < state.scroll_offset {
        state.scroll_offset = state.cursor_line;
    }
    // Approximate visible lines (will be exact in render)
    let visible = 20;
    if state.cursor_line >= state.scroll_offset + visible {
        state.scroll_offset = state.cursor_line - visible + 1;
    }
}

// ═══════════════════════════════════════════════════════════════
//  File I/O
// ═══════════════════════════════════════════════════════════════

fn save_file(state: &mut EditorState) {
    if state.file_path == "[new]" || state.file_path.is_empty() {
        state.mode = EditorMode::Command;
        state.status_msg = String::from("Open file:");
        state.needs_redraw = true;
        return;
    }

    let content = state.lines.join("\n");
    match crate::vfs::create_and_write(&state.file_path, content.as_bytes()) {
        Ok(()) => {
            state.dirty = false;
            state.status_msg = format!("Saved: {}", state.file_path);
        }
        Err(e) => {
            state.status_msg = format!("Save error: {}", e);
        }
    }
    state.needs_redraw = true;
}

fn load_file_content(path: &str) -> Result<Vec<String>, &'static str> {
    let fd = crate::vfs::open(path)?;
    let mut buf = [0u8; 8192];
    let n = crate::vfs::read(fd, &mut buf)?;
    crate::vfs::close(fd).ok();

    let text = core::str::from_utf8(&buf[..n]).unwrap_or("");
    let lines: Vec<String> = text.lines().map(String::from).collect();
    if lines.is_empty() {
        Ok(alloc::vec![String::new()])
    } else {
        Ok(lines)
    }
}

// ═══════════════════════════════════════════════════════════════
//  Render Sync
// ═══════════════════════════════════════════════════════════════

/// Sync editor state to the window widgets (called from render loop).
pub fn sync_to_window(window: &mut crate::gui::window::Window) {
    let mut state_guard = STATE.lock();
    let state = match state_guard.as_mut() {
        Some(s) => s,
        None => return,
    };

    if !state.needs_redraw {
        return;
    }
    state.needs_redraw = false;

    // Update status bar (widget 0)
    let dirty_marker = if state.dirty { " [+]" } else { "" };
    let status_text = format!(
        " {} | Ln {}, Col {}{} | {}",
        state.file_path,
        state.cursor_line + 1,
        state.cursor_col + 1,
        dirty_marker,
        state.status_msg,
    );
    if let Some(w) = window.widgets.get_mut(0) {
        if let WidgetKind::Label(ref mut label) = w.kind {
            label.text = status_text;
            label.color = if state.dirty { ACCENT_ORANGE } else { ACCENT_CYAN };
        }
    }

    // Update text area (widget 1)
    if let Some(w) = window.widgets.get_mut(1) {
        if let WidgetKind::RawInput(ref mut raw) = w.kind {
            let display = build_display_lines(state);
            raw.scroll_offset = state.scroll_offset;
            raw.display_lines = display;
        }
    }

    // Focus management
    if state.mode == EditorMode::Command {
        // Focus command bar
        if let Some(w) = window.widgets.get_mut(1) { w.focused = false; }
        if let Some(w) = window.widgets.get_mut(2) { w.focused = true; }
    } else {
        // Focus text area
        if let Some(w) = window.widgets.get_mut(1) { w.focused = true; }
        if let Some(w) = window.widgets.get_mut(2) { w.focused = false; }
    }
}

/// Build display lines with line numbers and cursor indicator.
fn build_display_lines(state: &EditorState) -> Vec<(String, Color)> {
    let line_count = state.lines.len();
    let gutter_w = if line_count >= 1000 { 5 } else if line_count >= 100 { 4 } else { 3 };

    let mut display = Vec::with_capacity(line_count);
    for (i, line) in state.lines.iter().enumerate() {
        let is_cursor_line = i == state.cursor_line;
        let line_num = format!("{:>width$} ", i + 1, width = gutter_w);

        // Insert cursor marker
        let display_line = if is_cursor_line {
            let col = state.cursor_col.min(line.len());
            let mut text = String::with_capacity(gutter_w + 1 + line.len() + 1);
            text.push_str(&line_num);
            text.push_str(&line[..col]);
            text.push('|'); // cursor indicator
            text.push_str(&line[col..]);
            text
        } else {
            format!("{}{}", line_num, line)
        };

        let color = if is_cursor_line { ACCENT_GREEN } else { TEXT_PRIMARY };
        display.push((display_line, color));
    }

    display
}

/// Use a `to_string` helper for usize in no_std.
#[allow(dead_code)]
trait ParseHelper {
    fn parse_usize(&self) -> Result<usize, ()>;
}

impl ParseHelper for str {
    fn parse_usize(&self) -> Result<usize, ()> {
        let mut result = 0usize;
        for &b in self.as_bytes() {
            if b >= b'0' && b <= b'9' {
                result = result.checked_mul(10).ok_or(())?;
                result = result.checked_add((b - b'0') as usize).ok_or(())?;
            } else {
                return Err(());
            }
        }
        Ok(result)
    }
}

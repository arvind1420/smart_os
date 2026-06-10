//! CJK Input Method Editor (IME) subsystem for Smart OS.
//!
//! Phonetic pinyin/romaji composition mapping to CJK candidates, with overlay rendering.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use super::compositor::Compositor;
use super::theme::*;

static DICTIONARY: &[(&str, &[&str])] = &[
    ("nihao", &["你好", "拟好", "泥豪"]),
    ("shijie", &["世界", "视界", "逝界"]),
    ("zhongwen", &["中文", "重温", "众文"]),
    ("smartos", &["智能系统", "聪颖系统"]),
    ("browser", &["浏览器", "浏览"]),
    ("os", &["操作系统", "系统"]),
    ("audio", &["音频", "声音"]),
    ("font", &["字体", "字形"]),
    ("nihon", &["日本", "日本語", "二本"]),
    ("konnichiwa", &["こんにちは", "今日波"]),
];

pub struct ImeState {
    pub active: bool,
    pub composition: String,
    pub candidates: Vec<String>,
    pub selected: usize,
}

impl ImeState {
    const fn new() -> Self {
        Self {
            active: false,
            composition: String::new(),
            candidates: Vec::new(),
            selected: 0,
        }
    }

    fn update_candidates(&mut self) {
        self.candidates.clear();
        self.selected = 0;
        if self.composition.is_empty() { return; }

        for &(key, list) in DICTIONARY {
            if key.starts_with(&self.composition) {
                for &cand in list {
                    self.candidates.push(String::from(cand));
                }
            }
        }
        // Limit to 9 candidates max
        self.candidates.truncate(9);
    }

    fn commit_candidate(&mut self, index: usize) {
        let text_to_commit = if index < self.candidates.len() {
            self.candidates[index].clone()
        } else {
            self.composition.clone()
        };

        if !text_to_commit.is_empty() {
            // Write directly to focused TextInput widget in active window
            use super::desktop::DESKTOP;
            let mut desktop = DESKTOP.lock();
            if let Some(ref mut desk) = *desktop {
                if let Some(wid) = desk.wm.active_id() {
                    if let Some(win) = desk.wm.get_mut(wid) {
                        if let Some(focus_id) = win.focused_widget {
                            if let Some(widget) = win.get_widget_mut(focus_id) {
                                if let super::widget::WidgetKind::TextInput(ref mut ti) = widget.kind {
                                    let commit_chars: Vec<char> = text_to_commit.chars().collect();
                                    let new_text: String = ti.text.chars().take(ti.cursor_pos)
                                        .chain(commit_chars.iter().cloned())
                                        .chain(ti.text.chars().skip(ti.cursor_pos)).collect();
                                    ti.text = new_text;
                                    ti.cursor_pos += commit_chars.len();
                                    win.dirty = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        self.composition.clear();
        self.candidates.clear();
    }
}

pub static IME: Mutex<ImeState> = Mutex::new(ImeState::new());

/// Intercept key presses for IME. Returns true if the key was intercepted.
pub fn handle_key(key: char, scancode: u8) -> bool {
    // Ctrl+Space to toggle CJK IME mode
    // (scancode for Space is 0x39, check ctrl status via input mods)
    if scancode == 0x39 && super::input::ctrl_held() {
        let mut state = IME.lock();
        state.active = !state.active;
        state.composition.clear();
        state.candidates.clear();
        crate::serial_println!("[ime] Toggle CJK IME active={}", state.active);
        return true;
    }

    let mut state = IME.lock();
    if !state.active {
        return false;
    }

    // Process composition keys
    if !state.composition.is_empty() {
        match key {
            '\x1B' => { // Escape: cancel composition
                state.composition.clear();
                state.candidates.clear();
                return true;
            }
            '\x08' => { // Backspace: remove last character
                state.composition.pop();
                state.update_candidates();
                return true;
            }
            ' ' => { // Space: commit first candidate or composition
                state.commit_candidate(0);
                return true;
            }
            '1'..='9' => { // Numeric selection of candidate
                let idx = (key as usize) - ('1' as usize);
                state.commit_candidate(idx);
                return true;
            }
            _ => {}
        }
    }

    // Add lowercase letters to composition
    if key >= 'a' && key <= 'z' {
        state.composition.push(key);
        state.update_candidates();
        return true;
    }

    // Intercept other keys if composition is active to prevent typing raw letters
    if !state.composition.is_empty() {
        return true;
    }

    false
}

/// Retrieve absolute position of the focused input widget on screen.
fn get_focused_input_pos() -> Option<(usize, usize, usize, usize)> {
    use super::desktop::DESKTOP;
    use super::theme::{BORDER_WIDTH, TITLEBAR_HEIGHT};
    use super::widget::WidgetKind;

    let desktop = DESKTOP.lock();
    let desk = desktop.as_ref()?;
    let active_id = desk.wm.active_id()?;
    let window = desk.wm.windows.iter().find(|w| w.id == active_id)?;
    let focus_id = window.focused_widget?;
    let widget = window.widgets.iter().find(|w| w.id == focus_id)?;

    if let WidgetKind::TextInput(_) = &widget.kind {
        let abs_x = window.x + BORDER_WIDTH + widget.x;
        let abs_y = window.y + BORDER_WIDTH + TITLEBAR_HEIGHT + widget.y;
        Some((abs_x, abs_y, widget.width, widget.height))
    } else {
        None
    }
}

/// Render CJK IME candidate window overlay.
pub fn render(comp: &mut Compositor) {
    let state = IME.lock();
    if !state.active || state.composition.is_empty() {
        return;
    }

    // 1. Locate overlay placement (below input field or floating default)
    let (ox, oy, iw, ih) = match get_focused_input_pos() {
        Some((x, y, w, h)) => (x, y + h + 2, w, 24),
        None => (20, comp.height.saturating_sub(60), 300, 24),
    };

    // Build candidate display string, e.g. "nihao | 1.你好 2.拟好 3.泥豪"
    let mut label = String::new();
    label.push('[');
    label.push_str(&state.composition);
    label.push_str("] ");

    for (i, cand) in state.candidates.iter().enumerate() {
        label.push_str(&alloc::format!("{}.{} ", i + 1, cand));
    }

    let text_w = label.chars().count() * 8 + 16;
    let box_w = iw.max(text_w);
    let box_h = ih;

    // Draw Candidate box background
    comp.fill_rect(ox, oy, box_w, box_h, BG_PANEL);
    comp.draw_rect(ox, oy, box_w, box_h, BORDER_GLOW);

    // Draw candidate text
    comp.draw_text(ox + 8, oy + (box_h.saturating_sub(16)) / 2, &label, TEXT_PRIMARY);
}

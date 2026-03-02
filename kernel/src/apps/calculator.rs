/// Smart OS Calculator --- Simple integer calculator application.
///
/// Provides a 4x4 button grid (digits, operators, clear, equals)
/// with a display label showing the current number or result.
/// Uses i64 integer arithmetic, evaluated left-to-right.

use alloc::format;
use alloc::string::String;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::{Window, WindowId};
use crate::gui::widget::*;
use crate::gui::desktop::DESKTOP;

/// Calculator application state.
pub struct CalcState {
    pub window_id: WindowId,
    /// Current display string (the number being entered or result).
    pub display: String,
    /// Left-hand accumulator for pending operations.
    pub accumulator: i64,
    /// Pending operator: '+', '-', '*', or '/'.
    pub pending_op: Option<char>,
    /// Whether the next digit press should start a new number.
    pub new_input: bool,
    /// Whether state changed and needs GUI sync.
    pub dirty: bool,
}

pub static STATE: Mutex<Option<CalcState>> = Mutex::new(None);

// Button layout: each button has a widget ID (1..=16) and a label.
// Row 1 (y=40):  7  8  9  /    -> widget IDs 1,2,3,4
// Row 2 (y=95):  4  5  6  *    -> widget IDs 5,6,7,8
// Row 3 (y=150): 1  2  3  -    -> widget IDs 9,10,11,12
// Row 4 (y=205): C  0  =  +    -> widget IDs 13,14,15,16
const BTN_W: usize = 50;
const BTN_H: usize = 50;
const GAP: usize = 5;
const GRID_X: usize = 2;
const DISPLAY_H: usize = 36;

const BUTTON_LABELS: [&str; 16] = [
    "7", "8", "9", "/",
    "4", "5", "6", "*",
    "1", "2", "3", "-",
    "C", "0", "=", "+",
];

/// Calculator thread entry point.
pub fn run() {
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() {
            Some(d) => d,
            None => return,
        };

        let mut win = Window::new("Calc", 680, 100, 220, 320, ACCENT_MAGENTA);
        win.use_widgets = true;

        // Widget 0: Display label (right-aligned text showing current number)
        let display_label = Widget::new(0, 4, 4, 212, DISPLAY_H,
            WidgetKind::Label(StaticLabel::new("0", TEXT_PRIMARY)));
        win.add_widget(display_label);

        // Widgets 1..=16: Calculator buttons in 4x4 grid
        for row in 0..4u8 {
            for col in 0..4u8 {
                let idx = (row * 4 + col) as usize;
                let widget_id = (idx + 1) as u8;
                let bx = GRID_X + col as usize * (BTN_W + GAP);
                let by = DISPLAY_H + 8 + row as usize * (BTN_H + GAP);
                let label = BUTTON_LABELS[idx];

                // Color: digits white, operators magenta, C red, = green
                let accent = match label {
                    "+" | "-" | "*" | "/" => ACCENT_MAGENTA,
                    "C" => ACCENT_RED,
                    "=" => ACCENT_GREEN,
                    _ => TEXT_PRIMARY,
                };

                let btn = Widget::new(widget_id, bx, by, BTN_W, BTN_H,
                    WidgetKind::Button(Button::new(label, accent,
                        AppCommand::ButtonClicked(widget_id))));
                win.add_widget(btn);
            }
        }

        let id = win.id;
        desk.wm.add(win);
        id
    };

    // Initialize calculator state
    *STATE.lock() = Some(CalcState {
        window_id,
        display: String::from("0"),
        accumulator: 0,
        pending_op: None,
        new_input: true,
        dirty: true,
    });

    // Main loop: poll for button clicks
    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            match action {
                WidgetAction::Execute(AppCommand::ButtonClicked(btn_id)) => {
                    let idx = btn_id.wrapping_sub(1) as usize;
                    if idx < 16 {
                        let label = BUTTON_LABELS[idx];
                        let mut state = STATE.lock();
                        if let Some(ref mut s) = *state {
                            handle_button(s, label);
                            s.dirty = true;
                        }
                    }
                }
                _ => {}
            }
        }

        // Yield CPU
        for _ in 0..5 {
            crate::process::scheduler::yield_now();
        }
    }
}

/// Process a button press.
fn handle_button(s: &mut CalcState, label: &str) {
    match label {
        "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
            if s.new_input {
                s.display = String::from(label);
                s.new_input = false;
            } else {
                // Prevent absurdly long numbers
                if s.display.len() < 15 {
                    // Replace leading "0" unless it's "0" followed by more digits
                    if s.display == "0" {
                        s.display = String::from(label);
                    } else {
                        s.display.push_str(label);
                    }
                }
            }
        }
        "+" | "-" | "*" | "/" => {
            evaluate_pending(s);
            s.pending_op = Some(label.as_bytes()[0] as char);
            s.new_input = true;
        }
        "=" => {
            evaluate_pending(s);
            s.pending_op = None;
            s.new_input = true;
        }
        "C" => {
            s.display = String::from("0");
            s.accumulator = 0;
            s.pending_op = None;
            s.new_input = true;
        }
        _ => {}
    }
}

/// Evaluate the pending operation (accumulator op display) and store the result.
fn evaluate_pending(s: &mut CalcState) {
    let current = parse_display(&s.display);
    if let Some(op) = s.pending_op {
        let result = match op {
            '+' => s.accumulator.saturating_add(current),
            '-' => s.accumulator.saturating_sub(current),
            '*' => s.accumulator.saturating_mul(current),
            '/' => {
                if current != 0 { s.accumulator / current } else { 0 }
            }
            _ => current,
        };
        s.accumulator = result;
        s.display = format!("{}", result);
    } else {
        // No pending op: just move display value into accumulator
        s.accumulator = current;
    }
}

/// Parse the display string as an i64.
fn parse_display(text: &str) -> i64 {
    // Handle negative numbers and parse
    let mut result: i64 = 0;
    let mut negative = false;
    for (i, b) in text.bytes().enumerate() {
        if b == b'-' && i == 0 {
            negative = true;
        } else if b >= b'0' && b <= b'9' {
            result = result.saturating_mul(10).saturating_add((b - b'0') as i64);
        }
    }
    if negative { -result } else { result }
}

/// Sync calculator state to window widgets (called from render loop).
pub fn sync_to_window(window: &mut Window) {
    let mut state = STATE.lock();
    let s = match state.as_mut() {
        Some(s) => s,
        None => return,
    };
    if !s.dirty { return; }
    s.dirty = false;

    // Update the display label (widget 0)
    if let Some(widget) = window.get_widget_mut(0) {
        if let WidgetKind::Label(ref mut lbl) = widget.kind {
            // Right-align: pad with spaces so text appears on the right
            let max_chars = 24; // roughly 212px / 8px per char
            let display_len = s.display.len();
            if display_len < max_chars {
                let padding = max_chars - display_len;
                let mut padded = String::with_capacity(max_chars);
                for _ in 0..padding {
                    padded.push(' ');
                }
                padded.push_str(&s.display);
                lbl.text = padded;
            } else {
                lbl.text = s.display.clone();
            }
        }
    }
}

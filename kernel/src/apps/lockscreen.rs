/// Lock screen overlay for Smart OS.
///
/// Activated by Ctrl+Alt+L (detected in the GUI render thread).
/// Shows a centred card with a masked password field over the desktop.
/// Calls `session::unlock(password)` on submit.
///
/// Widget layout:
///   0 – password TextInput (masked)
///   1 – "Unlock" Button  (AppCommand::ButtonClicked(0))
///   2 – error StaticLabel

use alloc::string::String;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::{Window, WindowId};
use crate::gui::widget::*;
use crate::gui::desktop::DESKTOP;

// ── State ─────────────────────────────────────────────────────────────────────

struct LockState {
    window_id: WindowId,
    error:     String,
    dirty:     bool,
    unlocked:  bool,
}

static STATE: Mutex<Option<LockState>> = Mutex::new(None);

// ── Public API ────────────────────────────────────────────────────────────────

pub fn is_showing() -> bool { STATE.lock().is_some() }

/// Show the lock screen (runs synchronously until unlocked).
pub fn show() {
    if STATE.lock().is_some() { return; }

    crate::session::lock();
    let window_id = create_lock_window();
    *STATE.lock() = Some(LockState {
        window_id,
        error: String::new(),
        dirty: true,
        unlocked: false,
    });

    loop {
        let (wid, done) = match *STATE.lock() {
            Some(ref s) => (s.window_id, s.unlocked),
            None => break,
        };
        if done { break; }

        if let Some(action) = crate::gui::input::poll_action(wid) {
            handle_action(action, wid);
        }
        sync(wid);

        for _ in 0..8 {
            crate::process::scheduler::yield_now();
        }
    }

    // Remove overlay
    if let Some(ref s) = *STATE.lock() {
        let mut desktop = DESKTOP.lock();
        if let Some(ref mut d) = *desktop {
            d.wm.windows.retain(|w| w.id != s.window_id);
        }
    }
    *STATE.lock() = None;
}

// ── Window builder ────────────────────────────────────────────────────────────

fn create_lock_window() -> WindowId {
    let (sw, sh) = crate::gui::compositor::screen_size();
    let sw = if sw == 0 { 1024 } else { sw };
    let sh = if sh == 0 { 768  } else { sh };

    let card_w = 300usize;
    let card_h = 180usize;
    let cx = (sw.saturating_sub(card_w)) / 2;
    let cy = (sh.saturating_sub(card_h)) / 2;

    let username = crate::session::current_user().unwrap_or_else(|| String::from("user"));

    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() { Some(d) => d, None => panic!("no desktop") };

    let title = alloc::format!("Locked — {}", username);
    let mut win = Window::new(&title, cx, cy, card_w, card_h, ACCENT_ORANGE);
    win.use_widgets = true;

    let pad = 20usize;
    let field_w = card_w - pad * 2;

    win.widgets.push(Widget::new(0, pad, 40, field_w, 28,
        WidgetKind::TextInput(TextInput::new_password("Password", ACCENT_CYAN))));
    win.widgets.push(Widget::new(1, pad, 80, field_w, 32,
        WidgetKind::Button(Button::new("Unlock", ACCENT_GREEN, AppCommand::ButtonClicked(0)))));
    win.widgets.push(Widget::new(2, pad, 122, field_w, 18,
        WidgetKind::Label(StaticLabel::new("", ACCENT_RED))));

    win.focused_widget = Some(0);
    let id = win.id;
    desk.wm.add(win);
    id
}

// ── Event handler ─────────────────────────────────────────────────────────────

fn handle_action(action: WidgetAction, window_id: WindowId) {
    match action {
        WidgetAction::Execute(AppCommand::ButtonClicked(0)) |
        WidgetAction::Execute(AppCommand::TextSubmitted(_)) => try_unlock(window_id),
        _ => {}
    }
}

fn try_unlock(window_id: WindowId) {
    let password = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() { Some(d) => d, None => return };
        let win  = match desk.wm.get_mut(window_id) { Some(w) => w, None => return };
        win.widgets.iter()
            .find(|w| w.id == 0)
            .and_then(|w| if let WidgetKind::TextInput(ref t) = w.kind { Some(t.text.clone()) } else { None })
            .unwrap_or_default()
    };

    if crate::session::unlock(&password) {
        if let Some(ref mut s) = *STATE.lock() { s.unlocked = true; }
    } else {
        if let Some(ref mut s) = *STATE.lock() {
            s.error = String::from("Incorrect password");
            s.dirty = true;
        }
    }
}

fn sync(window_id: WindowId) {
    let (dirty, error) = match *STATE.lock() {
        Some(ref s) => (s.dirty, s.error.clone()),
        None => return,
    };
    if !dirty { return; }

    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() { Some(d) => d, None => return };
    let win  = match desk.wm.get_mut(window_id) { Some(w) => w, None => return };
    if let Some(widget) = win.get_widget_mut(2) {
        if let WidgetKind::Label(ref mut lbl) = widget.kind {
            lbl.text = error;
        }
    }
    win.dirty = true;
    if let Some(ref mut s) = *STATE.lock() { s.dirty = false; }
}

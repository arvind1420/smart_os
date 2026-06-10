/// Graphical login screen for Smart OS.
///
/// Full-screen modal shown at boot.  On successful authentication it starts a
/// session via `session::start_session` and spawns desktop applications.
///
/// Widget layout (inside card window):
///   0 – username TextInput
///   1 – password TextInput (masked)
///   2 – "Login" Button  (AppCommand::ButtonClicked(0))
///   3 – error StaticLabel

use alloc::format;
use alloc::string::String;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::Window;
use crate::gui::window::WindowId;
use crate::gui::widget::*;
use crate::gui::desktop::DESKTOP;

// ── State ─────────────────────────────────────────────────────────────────────

struct LoginState {
    window_id: WindowId,
    error_msg: String,
    dirty:     bool,
    logged_in: bool,
}

static STATE: Mutex<Option<LoginState>> = Mutex::new(None);

// ── Public signal ─────────────────────────────────────────────────────────────

static LOGIN_SUCCESS: Mutex<bool> = Mutex::new(false);

pub fn login_succeeded() -> bool { *LOGIN_SUCCESS.lock() }

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run() {
    let window_id = create_login_window();

    *STATE.lock() = Some(LoginState {
        window_id,
        error_msg: String::new(),
        dirty: true,
        logged_in: false,
    });

    loop {
        let (wid, done) = match *STATE.lock() {
            Some(ref s) => (s.window_id, s.logged_in),
            None => break,
        };
        if done { break; }

        if let Some(action) = crate::gui::input::poll_action(wid) {
            handle_action(action, wid);
        }

        sync_to_window(wid);

        for _ in 0..10 {
            crate::process::scheduler::yield_now();
        }
    }

    // Login window was already destroyed in attempt_login before spawn_desktop().
    // Purge any leftover state entry just in case.
    *STATE.lock() = None;

    // Wait until the Browser window is actually registered in the desktop,
    // then focus it.  Retry for up to ~5 s (50 000 yields ≈ 5 s at ~100 µs each).
    for _ in 0..50_000usize {
        crate::process::scheduler::yield_now();
        // Check if browser window exists yet
        let found = {
            let desktop = crate::gui::desktop::DESKTOP.lock();
            if let Some(ref desk) = *desktop {
                desk.wm.windows.iter().any(|w| w.visible && w.title.starts_with("Browser"))
            } else {
                false
            }
        };
        if found {
            crate::gui::compositor::focus_window_by_title("Browser");
            break;
        }
    }
}

// ── Window builder ────────────────────────────────────────────────────────────

fn create_login_window() -> WindowId {
    let (sw, sh) = crate::gui::compositor::screen_size();
    let sw = if sw == 0 { 1024 } else { sw };
    let sh = if sh == 0 { 768  } else { sh };

    let card_w = 320usize;
    let card_h = 220usize;
    let cx = (sw.saturating_sub(card_w)) / 2;
    let cy = (sh.saturating_sub(card_h)) / 2;

    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() { Some(d) => d, None => panic!("no desktop") };

    let mut win = Window::new("Smart OS Login", cx, cy, card_w, card_h, ACCENT_BLUE);
    win.use_widgets = true;

    let pad = 20usize;
    let field_h = 28usize;
    let field_w = card_w - pad * 2;

    // Widget 0: username
    let mut uname = TextInput::new("Username", ACCENT_BLUE);
    uname.max_len = 32;
    win.widgets.push(Widget::new(0, pad, 50, field_w, field_h,
        WidgetKind::TextInput(uname)));

    // Widget 1: password (masked)
    win.widgets.push(Widget::new(1, pad, 90, field_w, field_h,
        WidgetKind::TextInput(TextInput::new_password("Password", ACCENT_CYAN))));

    // Widget 2: Login button
    win.widgets.push(Widget::new(2, pad, 130, field_w, 32,
        WidgetKind::Button(Button::new("Login", ACCENT_GREEN, AppCommand::ButtonClicked(0)))));

    // Widget 3: error label
    win.widgets.push(Widget::new(3, pad, 172, field_w, 18,
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
        WidgetAction::Execute(AppCommand::TextSubmitted(_)) => attempt_login(window_id),
        _ => {}
    }
}

fn attempt_login(window_id: WindowId) {
    let (username, password) = read_credentials(window_id);

    if crate::users::authenticate(&username, &password) {
        if crate::session::start_session(&username) {
            // Deactivate login window BEFORE spawning apps so that the first
            // app window created by spawn_desktop() auto-gets active=true.
            crate::gui::compositor::destroy_window(window_id);
            spawn_desktop();
            *LOGIN_SUCCESS.lock() = true;
            if let Some(ref mut s) = *STATE.lock() {
                s.logged_in = true;
            }
        }
    } else {
        let msg = if username.is_empty() {
            String::from("Enter your username")
        } else {
            format!("Wrong password for '{}'", username)
        };
        if let Some(ref mut s) = *STATE.lock() {
            s.error_msg = msg;
            s.dirty = true;
        }
    }
}

fn read_credentials(window_id: WindowId) -> (String, String) {
    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() { Some(d) => d, None => return (String::new(), String::new()) };
    let win  = match desk.wm.get_mut(window_id) { Some(w) => w, None => return (String::new(), String::new()) };

    let username = win.widgets.iter()
        .find(|w| w.id == 0)
        .and_then(|w| if let WidgetKind::TextInput(ref t) = w.kind { Some(t.text.clone()) } else { None })
        .unwrap_or_default();

    let password = win.widgets.iter()
        .find(|w| w.id == 1)
        .and_then(|w| if let WidgetKind::TextInput(ref t) = w.kind { Some(t.text.clone()) } else { None })
        .unwrap_or_default();

    (username, password)
}

fn spawn_desktop() {
    use crate::process::scheduler::{spawn, spawn_large};
    spawn("terminal",     crate::apps::terminal::run,      7);
    spawn("file-manager", crate::apps::file_manager::run,  8);
    spawn("sysmon",       crate::apps::sysmon::run,        9);
    // Browser makes multiple sequential HTTPS connections (HTML + images).
    // TLS 1.3 + TCP + HTTP client stack up to >200 KB deep; needs 512 KB stack.
    spawn_large("browser", crate::apps::browser::run,     10);
    spawn("editor",       crate::apps::editor::run,       11);
    spawn("calculator",   crate::apps::calculator::run,   12);
    spawn("task-manager", crate::apps::task_manager::run, 13);
    spawn("settings",     crate::apps::settings::run,     14);
    spawn("usb-storage",  crate::apps::usb_storage::run,  15);
    spawn("app-store",    crate::apps::app_store::run,    16);
    spawn("power-mgr",    crate::apps::power_manager::run,   17);
    spawn("crash-rec",    crate::apps::crash_recovery::run,  18);
    spawn("setup-wiz",    crate::apps::setup_wizard::run,    19);
    spawn("a11y",         crate::apps::accessibility::run,   20);
    spawn("printer",      crate::apps::printer::run,         21);
    spawn("update-mgr",   crate::apps::update_manager::run,  22);
    spawn("cloud-sync",   crate::apps::cloud_sync::run,      23);
    spawn("gaming",       crate::apps::gaming::run,          24);
    // Phase 78-79: browser sandbox renderer threads (one per tab slot 0-2)
    spawn("renderer-0", crate::apps::browser_sandbox::run_renderer_tab0, 6);
    spawn("renderer-1", crate::apps::browser_sandbox::run_renderer_tab1, 6);
    spawn("renderer-2", crate::apps::browser_sandbox::run_renderer_tab2, 6);
}

// ── Display sync ──────────────────────────────────────────────────────────────

fn sync_to_window(window_id: WindowId) {
    let (dirty, error_msg) = match *STATE.lock() {
        Some(ref s) => (s.dirty, s.error_msg.clone()),
        None => return,
    };
    if !dirty { return; }

    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() { Some(d) => d, None => return };
    let win  = match desk.wm.get_mut(window_id) { Some(w) => w, None => return };

    if let Some(widget) = win.get_widget_mut(3) {
        if let WidgetKind::Label(ref mut lbl) = widget.kind {
            lbl.text = error_msg;
        }
    }
    win.dirty = true;

    if let Some(ref mut s) = *STATE.lock() { s.dirty = false; }
}

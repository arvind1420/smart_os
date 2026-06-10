/// Mouse and keyboard input processing for GUI window management.
///
/// Handles mouse events: window focus, dragging, close/minimize/maximize buttons,
/// widget clicks, window resize (edge/corner drag), right-click context menus,
/// taskbar click handling, and window edge-snapping.
/// Handles keyboard events: routing to focused window's focused widget.
/// Provides an action queue for app threads to poll widget actions.

use alloc::collections::VecDeque;
use alloc::vec;
use spin::Mutex;
use super::window::{WindowId, MIN_WIDTH, MIN_HEIGHT};
use super::desktop::DESKTOP;
use super::theme::{TITLEBAR_HEIGHT, BORDER_WIDTH, TASKBAR_HEIGHT};
use super::widget::WidgetAction;
use crate::drivers::mouse::MouseEvent;

// ── Modifier key state ────────────────────────────────────────────────────────

// PS/2 scancodes for modifier keys (Set 1, press codes)
const SC_LCTRL:  u8 = 0x1D;
const SC_LALT:   u8 = 0x38;
const SC_LSHIFT: u8 = 0x2A;
const SC_RSHIFT: u8 = 0x36;

struct Modifiers { ctrl: bool, alt: bool, shift: bool }
static MODS: Mutex<Modifiers> = Mutex::new(Modifiers { ctrl: false, alt: false, shift: false });

fn update_mods(scancode: u8, pressed: bool) {
    let mut m = MODS.lock();
    // Release scancodes are press | 0x80
    let base = scancode & 0x7F;
    match base {
        SC_LCTRL  => m.ctrl  = pressed,
        SC_LALT   => m.alt   = pressed,
        SC_LSHIFT | SC_RSHIFT => m.shift = pressed,
        _ => {}
    }
}

pub fn ctrl_held()  -> bool { MODS.lock().ctrl  }
pub fn alt_held()   -> bool { MODS.lock().alt   }
pub fn shift_held() -> bool { MODS.lock().shift }

/// Hit test result — which part of a window was clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitRegion {
    TitleBar,
    CloseButton,
    MinimizeButton,
    MaximizeButton,
    Content,
    ResizeLeft,
    ResizeRight,
    ResizeTop,
    ResizeBottom,
    ResizeTopLeft,
    ResizeTopRight,
    ResizeBottomLeft,
    ResizeBottomRight,
}

/// The edge being resized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// GUI input state machine.
pub struct InputState {
    /// Currently dragging a window (title bar move).
    pub dragging: Option<DragState>,
    /// Currently resizing a window.
    pub resizing: Option<ResizeState>,
    /// Last known mouse position.
    pub mouse_x: i32,
    pub mouse_y: i32,
    /// Previous left button state (for detecting clicks).
    pub prev_left: bool,
    /// Previous right button state.
    pub prev_right: bool,
    /// Timestamp of last left click (for double-click detection).
    pub last_click_tick: u64,
    /// Window of last click (for double-click on same window).
    pub last_click_win: Option<WindowId>,
}

/// State for an active window drag operation.
pub struct DragState {
    pub window_id: WindowId,
    /// Offset from window top-left corner to the mouse position.
    pub offset_x: i32,
    pub offset_y: i32,
}

/// State for an active window resize operation.
pub struct ResizeState {
    pub window_id: WindowId,
    pub edge: ResizeEdge,
    /// Original window bounds at drag start.
    pub orig_x: usize,
    pub orig_y: usize,
    pub orig_w: usize,
    pub orig_h: usize,
    /// Mouse position at drag start.
    pub start_mx: i32,
    pub start_my: i32,
}

impl InputState {
    pub const fn new() -> Self {
        Self {
            dragging: None,
            resizing: None,
            mouse_x: 0,
            mouse_y: 0,
            prev_left: false,
            prev_right: false,
            last_click_tick: 0,
            last_click_win: None,
        }
    }
}

pub static INPUT_STATE: Mutex<InputState> = Mutex::new(InputState::new());

// ═══════════════════════════════════════════════════════════════
//  Action Queue — widget actions for app threads to poll
// ═══════════════════════════════════════════════════════════════

static ACTION_QUEUE: Mutex<VecDeque<(WindowId, WidgetAction)>> = Mutex::new(VecDeque::new());

/// Push a widget action to the queue.
fn queue_action(window_id: WindowId, action: WidgetAction) {
    let mut queue = ACTION_QUEUE.lock();
    // Cap queue size to avoid unbounded growth
    if queue.len() < 256 {
        queue.push_back((window_id, action));
    }
}

/// Poll for the next action for a specific window (called by app threads).
pub fn poll_action(window_id: WindowId) -> Option<WidgetAction> {
    let mut queue = ACTION_QUEUE.lock();
    if let Some(pos) = queue.iter().position(|(id, _)| *id == window_id) {
        Some(queue.remove(pos).unwrap().1)
    } else {
        None
    }
}

// ═══════════════════════════════════════════════════════════════
//  Keyboard Event Handling
// ═══════════════════════════════════════════════════════════════

/// Process a keyboard event, routing it to the focused window's widget tree.
pub fn handle_key_event(event: crate::drivers::keyboard::KeyEvent) {
    // Always update modifier state (press and release)
    update_mods(event.scancode, event.pressed);

    if !event.pressed { return; }

    // Intercept keyboard input for CJK IME stub
    let key = match event.ascii {
        Some(ascii) => ascii as char,
        None => '\0',
    };
    if super::ime::handle_key(key, event.scancode) {
        return;
    }

    // Ctrl+Alt+L — lock screen
    if ctrl_held() && alt_held() {
        if event.ascii == Some(b'l') || event.ascii == Some(b'L') || event.scancode == 0x26 {
            if crate::session::is_logged_in() && !crate::apps::lockscreen::is_showing() {
                crate::process::scheduler::spawn("lockscreen", crate::apps::lockscreen::show, 16);
            }
            return;
        }
    }

    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() {
        Some(d) => d,
        None => { crate::serial_println!("[kbd] no desktop"); return; }
    };

    // Keyboard window management: Alt+Arrows or Ctrl+Alt+Arrows
    if alt_held() {
        if let Some(active_id) = desk.wm.active_id() {
            let screen_dims = {
                let comp = super::compositor::COMPOSITOR.lock();
                comp.as_ref().map(|c| (c.width, c.height))
            };
            if let Some((sw, sh)) = screen_dims {
                let snap_h = sh.saturating_sub(TASKBAR_HEIGHT);
                let mut action_taken = false;
                let mut toggle_minimize = false;

                if let Some(win) = desk.wm.get_mut(active_id) {
                    match event.scancode {
                        0x4B => { // Left Arrow: Snap Left
                            win.snap_to(0, 0, sw / 2, snap_h);
                            action_taken = true;
                        }
                        0x4D => { // Right Arrow: Snap Right
                            win.snap_to(sw / 2, 0, sw / 2, snap_h);
                            action_taken = true;
                        }
                        0x48 => { // Up Arrow: Maximize
                            win.maximize(sw, sh);
                            action_taken = true;
                        }
                        0x50 => { // Down Arrow: Restore or Minimize
                            if win.state == super::window::WindowState::Maximized || win.pre_snap_bounds.is_some() {
                                win.restore();
                            } else {
                                win.state = super::window::WindowState::Minimized;
                                win.active = false;
                                toggle_minimize = true;
                            }
                            action_taken = true;
                        }
                        _ => {}
                    }
                }

                if action_taken {
                    if toggle_minimize {
                        desk.wm.auto_focus_next();
                    }
                    return;
                }
            }
        }
    }

    // Alt+Tab — cycle through visible windows
    if alt_held() && event.scancode == 0x0F {
        let visible_ids: alloc::vec::Vec<super::window::WindowId> = desk.wm.windows.iter()
            .filter(|w| w.visible)
            .map(|w| w.id)
            .collect();
        if !visible_ids.is_empty() {
            let next_id = if let Some(active) = desk.wm.active_id() {
                if let Some(pos) = visible_ids.iter().position(|&id| id == active) {
                    visible_ids[(pos + 1) % visible_ids.len()]
                } else {
                    visible_ids[0]
                }
            } else {
                visible_ids[0]
            };
            desk.wm.bring_to_front(next_id);
            crate::serial_println!("[kbd] Alt+Tab → window {}", next_id);
        }
        return;
    }

    let active_id = match desk.wm.active_id() {
        Some(id) => id,
        None => {
            // No active window — activate the first visible one
            let first_id = desk.wm.windows.iter()
                .filter(|w| w.visible)
                .next()
                .map(|w| w.id);
            match first_id {
                Some(id) => { desk.wm.set_active(id); id }
                None => { crate::serial_println!("[kbd] no active window"); return; }
            }
        }
    };
    crate::serial_println!("[kbd] routing to window {}", active_id);

    let window = match desk.wm.get_mut(active_id) {
        Some(w) => w,
        None => return,
    };

    if !window.use_widgets { return; }

    // Tab key cycles focus between widgets
    if event.ascii == Some(b'\t') {
        window.focus_next();
        return;
    }

    // Route other keys to the focused widget
    let key = match event.ascii {
        Some(ascii) => ascii as char,
        None => '\0',
    };
    let scancode = event.scancode;

    if let Some(action) = window.dispatch_key(key, scancode) {
        queue_action(active_id, action);
    }
}

// ═══════════════════════════════════════════════════════════════
//  Mouse Event Handling
// ═══════════════════════════════════════════════════════════════

/// Resize detection threshold (pixels from edge).
const RESIZE_MARGIN: i32 = 6;

/// Double-click threshold (timer ticks, ~100Hz so 40 = 400ms).
const DBLCLICK_TICKS: u64 = 40;

/// Process a mouse event, updating window positions and focus.
pub fn handle_mouse_event(event: MouseEvent) {
    let (mx, my) = crate::drivers::mouse::position();

    let mut input = INPUT_STATE.lock();
    input.mouse_x = mx;
    input.mouse_y = my;

    let left_pressed = event.left;
    let right_pressed = event.right;
    let left_clicked = left_pressed && !input.prev_left;
    let left_released = !left_pressed && input.prev_left;
    let right_clicked = right_pressed && !input.prev_right;
    input.prev_left = left_pressed;
    input.prev_right = right_pressed;

    // ── Update context menu hover on every mouse move ──
    super::context_menu::update_hover(mx, my);

    // ── Handle resize in progress ──
    if left_pressed {
        if let Some(ref resize) = input.resizing {
            let win_id = resize.window_id;
            let dx = mx - resize.start_mx;
            let dy = my - resize.start_my;

            let mut new_x = resize.orig_x;
            let mut new_y = resize.orig_y;
            let mut new_w = resize.orig_w;
            let mut new_h = resize.orig_h;

            match resize.edge {
                ResizeEdge::Right => {
                    new_w = (resize.orig_w as i32 + dx).max(MIN_WIDTH as i32) as usize;
                }
                ResizeEdge::Bottom => {
                    new_h = (resize.orig_h as i32 + dy).max(MIN_HEIGHT as i32) as usize;
                }
                ResizeEdge::BottomRight => {
                    new_w = (resize.orig_w as i32 + dx).max(MIN_WIDTH as i32) as usize;
                    new_h = (resize.orig_h as i32 + dy).max(MIN_HEIGHT as i32) as usize;
                }
                ResizeEdge::Left => {
                    let right_fixed = resize.orig_x + resize.orig_w;
                    new_x = (resize.orig_x as i32 + dx).min((right_fixed - MIN_WIDTH) as i32).max(0) as usize;
                    new_w = right_fixed - new_x;
                }
                ResizeEdge::Top => {
                    let bottom_fixed = resize.orig_y + resize.orig_h;
                    new_y = (resize.orig_y as i32 + dy).min((bottom_fixed - MIN_HEIGHT) as i32).max(0) as usize;
                    new_h = bottom_fixed - new_y;
                }
                ResizeEdge::TopLeft => {
                    let right_fixed = resize.orig_x + resize.orig_w;
                    new_x = (resize.orig_x as i32 + dx).min((right_fixed - MIN_WIDTH) as i32).max(0) as usize;
                    new_w = right_fixed - new_x;

                    let bottom_fixed = resize.orig_y + resize.orig_h;
                    new_y = (resize.orig_y as i32 + dy).min((bottom_fixed - MIN_HEIGHT) as i32).max(0) as usize;
                    new_h = bottom_fixed - new_y;
                }
                ResizeEdge::TopRight => {
                    new_w = (resize.orig_w as i32 + dx).max(MIN_WIDTH as i32) as usize;

                    let bottom_fixed = resize.orig_y + resize.orig_h;
                    new_y = (resize.orig_y as i32 + dy).min((bottom_fixed - MIN_HEIGHT) as i32).max(0) as usize;
                    new_h = bottom_fixed - new_y;
                }
                ResizeEdge::BottomLeft => {
                    let right_fixed = resize.orig_x + resize.orig_w;
                    new_x = (resize.orig_x as i32 + dx).min((right_fixed - MIN_WIDTH) as i32).max(0) as usize;
                    new_w = right_fixed - new_x;

                    new_h = (resize.orig_h as i32 + dy).max(MIN_HEIGHT as i32) as usize;
                }
            }

            drop(input);

            let mut desktop = DESKTOP.lock();
            if let Some(ref mut desk) = *desktop {
                if let Some(win) = desk.wm.get_mut(win_id) {
                    win.x = new_x;
                    win.y = new_y;
                    win.resize(new_w, new_h);
                }
            }
            return;
        }
    }

    // ── Handle drag in progress ──
    if left_pressed {
        if let Some(ref mut drag) = input.dragging {
            let win_id = drag.window_id;
            let mut offset_x = drag.offset_x;
            let mut offset_y = drag.offset_y;

            let mut restore_needed = false;
            let mut old_w = 1;
            {
                let mut desktop_guard = DESKTOP.lock();
                if let Some(ref mut desk) = *desktop_guard {
                    if let Some(win) = desk.wm.get_mut(win_id) {
                        if win.state == super::window::WindowState::Maximized || win.pre_snap_bounds.is_some() {
                            restore_needed = true;
                            old_w = win.total_width();
                            win.restore();
                        }
                    }
                }
            }

            if restore_needed {
                let mut desktop_guard = DESKTOP.lock();
                let new_w = if let Some(ref mut desk) = *desktop_guard {
                    desk.wm.get_mut(win_id).map(|w| w.total_width()).unwrap_or(1)
                } else {
                    1
                };
                drag.offset_x = (drag.offset_x as usize * new_w / old_w.max(1)) as i32;
                offset_x = drag.offset_x;
            }

            let new_x = (mx - offset_x).max(0) as usize;
            let new_y = (my - offset_y).max(0) as usize;
            drop(input);

            let mut desktop_guard = DESKTOP.lock();
            if let Some(ref mut desk) = *desktop_guard {
                if let Some(win) = desk.wm.get_mut(win_id) {
                    win.x = new_x;
                    win.y = new_y;

                    // Update ghost snap preview
                    let screen_dims = {
                        let comp = super::compositor::COMPOSITOR.lock();
                        comp.as_ref().map(|c| (c.width, c.height))
                    };
                    if let Some((sw, sh)) = screen_dims {
                        let snap_h = sh.saturating_sub(TASKBAR_HEIGHT);
                        if mx <= 5 {
                            desk.ghost_snap = Some((0, 0, sw / 2, snap_h));
                        } else if mx >= (sw as i32 - 5) {
                            desk.ghost_snap = Some((sw / 2, 0, sw / 2, snap_h));
                        } else if my <= 5 {
                            desk.ghost_snap = Some((0, 0, sw, snap_h));
                        } else {
                            desk.ghost_snap = None;
                        }
                    }
                }
            }
            return;
        }
    }

    // ── Handle right-click (context menu) ──
    if right_clicked {
        drop(input);
        handle_right_click(mx, my);
        return;
    }

    // ── Handle left click ──
    if left_clicked {
        // If context menu is visible, check if we clicked on it
        if super::context_menu::is_visible() {
            if let Some(action) = super::context_menu::handle_click(mx, my) {
                drop(input);
                super::context_menu::hide();
                dispatch_menu_action(action);
                return;
            } else {
                // Clicked outside menu — close it
                super::context_menu::hide();
            }
        }

        let current_tick = crate::drivers::timer::ticks();

        // Get screen dimensions for snapping
        let screen_dims = {
            let comp = super::compositor::COMPOSITOR.lock();
            comp.as_ref().map(|c| (c.width, c.height))
        };

        // Hit test against taskbar first
        if let Some((sw, sh)) = screen_dims {
            let taskbar_y = sh.saturating_sub(TASKBAR_HEIGHT) as i32;
            if my >= taskbar_y {
                drop(input);
                handle_taskbar_click(mx, my, sw, sh);
                return;
            }
        }

        // Hit test against windows
        let hit = {
            let desktop = DESKTOP.lock();
            if let Some(ref desk) = *desktop {
                hit_test_windows(&desk.wm, mx, my)
            } else {
                None
            }
        };

        if let Some((win_id, region)) = hit {
            match region {
                HitRegion::CloseButton => {
                    drop(input);
                    let mut desktop = DESKTOP.lock();
                    if let Some(ref mut desk) = *desktop {
                        desk.wm.hide(win_id);
                        desk.wm.auto_focus_next();
                    }
                    return;
                }
                HitRegion::MinimizeButton => {
                    // Actually minimize the window
                    drop(input);
                    let mut desktop = DESKTOP.lock();
                    if let Some(ref mut desk) = *desktop {
                        if let Some(win) = desk.wm.get_mut(win_id) {
                            win.state = super::window::WindowState::Minimized;
                            win.active = false;
                        }
                        desk.wm.auto_focus_next();
                    }
                    return;
                }
                HitRegion::MaximizeButton => {
                    // Toggle maximize/restore
                    drop(input);
                    if let Some((sw, sh)) = screen_dims {
                        let mut desktop = DESKTOP.lock();
                        if let Some(ref mut desk) = *desktop {
                            if let Some(win) = desk.wm.get_mut(win_id) {
                                if win.state == super::window::WindowState::Maximized {
                                    win.restore();
                                } else {
                                    win.maximize(sw, sh);
                                }
                            }
                        }
                    }
                    return;
                }
                HitRegion::TitleBar => {
                    // Double-click detection for maximize/restore
                    let is_double = current_tick.saturating_sub(input.last_click_tick) < DBLCLICK_TICKS
                        && input.last_click_win == Some(win_id);
                    input.last_click_tick = current_tick;
                    input.last_click_win = Some(win_id);

                    if is_double {
                        // Double-click titlebar: toggle maximize
                        drop(input);
                        if let Some((sw, sh)) = screen_dims {
                            let mut desktop = DESKTOP.lock();
                            if let Some(ref mut desk) = *desktop {
                                if let Some(win) = desk.wm.get_mut(win_id) {
                                    if win.state == super::window::WindowState::Maximized {
                                        win.restore();
                                    } else {
                                        win.maximize(sw, sh);
                                    }
                                }
                                desk.wm.bring_to_front(win_id);
                            }
                        }
                        return;
                    }

                    // Single click: start dragging + bring to front
                    let (win_x, win_y) = {
                        let desktop = DESKTOP.lock();
                        if let Some(ref desk) = *desktop {
                            desk.wm.windows.iter()
                                .find(|w| w.id == win_id)
                                .map(|w| (w.x as i32, w.y as i32))
                                .unwrap_or((0, 0))
                        } else {
                            (0, 0)
                        }
                    };

                    input.dragging = Some(DragState {
                        window_id: win_id,
                        offset_x: mx - win_x,
                        offset_y: my - win_y,
                    });
                    drop(input);

                    let mut desktop = DESKTOP.lock();
                    if let Some(ref mut desk) = *desktop {
                        desk.wm.bring_to_front(win_id);
                    }
                    return;
                }
                HitRegion::ResizeLeft | HitRegion::ResizeRight | HitRegion::ResizeTop | HitRegion::ResizeBottom |
                HitRegion::ResizeTopLeft | HitRegion::ResizeTopRight | HitRegion::ResizeBottomLeft | HitRegion::ResizeBottomRight => {
                    // Start resize operation
                    let (orig_x, orig_y, orig_w, orig_h) = {
                        let desktop = DESKTOP.lock();
                        if let Some(ref desk) = *desktop {
                            desk.wm.windows.iter()
                                .find(|w| w.id == win_id)
                                .map(|w| (w.x, w.y, w.width, w.height))
                                .unwrap_or((0, 0, 300, 200))
                        } else {
                            (0, 0, 300, 200)
                        }
                    };
                    let edge = match region {
                        HitRegion::ResizeLeft => ResizeEdge::Left,
                        HitRegion::ResizeRight => ResizeEdge::Right,
                        HitRegion::ResizeTop => ResizeEdge::Top,
                        HitRegion::ResizeBottom => ResizeEdge::Bottom,
                        HitRegion::ResizeTopLeft => ResizeEdge::TopLeft,
                        HitRegion::ResizeTopRight => ResizeEdge::TopRight,
                        HitRegion::ResizeBottomLeft => ResizeEdge::BottomLeft,
                        HitRegion::ResizeBottomRight => ResizeEdge::BottomRight,
                        _ => unreachable!(),
                    };
                    input.resizing = Some(ResizeState {
                        window_id: win_id,
                        edge,
                        orig_x,
                        orig_y,
                        orig_w,
                        orig_h,
                        start_mx: mx,
                        start_my: my,
                    });
                    drop(input);

                    let mut desktop = DESKTOP.lock();
                    if let Some(ref mut desk) = *desktop {
                        desk.wm.bring_to_front(win_id);
                    }
                    return;
                }
                HitRegion::Content => {
                    // Bring to front + dispatch click to widgets
                    input.last_click_tick = current_tick;
                    input.last_click_win = Some(win_id);
                    drop(input);

                    let mut desktop = DESKTOP.lock();
                    if let Some(ref mut desk) = *desktop {
                        desk.wm.bring_to_front(win_id);

                        if let Some(win) = desk.wm.get_mut(win_id) {
                            if win.use_widgets {
                                let content_x = win.x + BORDER_WIDTH;
                                let content_y = win.y + BORDER_WIDTH + TITLEBAR_HEIGHT;
                                let rel_x = (mx as usize).saturating_sub(content_x);
                                let rel_y = (my as usize).saturating_sub(content_y);
                                if let Some(action) = win.dispatch_click(rel_x, rel_y) {
                                    queue_action(win_id, action);
                                }
                            }
                        }
                    }
                    return;
                }
            }
        }
    }

    // ── Handle left release — end drag or resize, check snapping ──
    if left_released {
        // Check for window snapping on drag release
        if let Some(ref _drag) = input.dragging {
            let win_id = _drag.window_id;
            let screen_dims = {
                let comp = super::compositor::COMPOSITOR.lock();
                comp.as_ref().map(|c| (c.width, c.height))
            };
            if let Some((sw, sh)) = screen_dims {
                let snap_h = sh.saturating_sub(TASKBAR_HEIGHT);
                drop(input);
                let mut desktop = DESKTOP.lock();
                if let Some(ref mut desk) = *desktop {
                    desk.ghost_snap = None;
                    if let Some(win) = desk.wm.get_mut(win_id) {
                        // Snap to edges
                        if mx <= 5 {
                            // Left half
                            win.snap_to(0, 0, sw / 2, snap_h);
                        } else if mx >= (sw as i32 - 5) {
                            // Right half
                            win.snap_to(sw / 2, 0, sw / 2, snap_h);
                        } else if my <= 5 {
                            // Top — maximize
                            win.maximize(sw, sh);
                        }
                    }
                }
                // Re-acquire input to clear drag
                let mut input = INPUT_STATE.lock();
                input.dragging = None;
                return;
            }
        }
        input.dragging = None;
        input.resizing = None;
    }
}

// ═══════════════════════════════════════════════════════════════
//  Right-Click Context Menu
// ═══════════════════════════════════════════════════════════════

fn handle_right_click(mx: i32, my: i32) {
    use super::context_menu::*;

    // First check if we right-clicked on a window
    let hit = {
        let desktop = DESKTOP.lock();
        if let Some(ref desk) = *desktop {
            hit_test_windows(&desk.wm, mx, my)
        } else {
            None
        }
    };

    let items = if let Some((_, region)) = hit {
        match region {
            HitRegion::TitleBar | HitRegion::MinimizeButton | HitRegion::MaximizeButton | HitRegion::CloseButton => {
                // Titlebar context menu
                vec![
                    menu_item("Minimize", MenuAction::Minimize),
                    menu_item("Maximize", MenuAction::Maximize),
                    menu_item("Close", MenuAction::Close),
                ]
            }
            _ => {
                // Window content context menu
                let has_clip = crate::gui::clipboard::has_content();
                let mut items = vec![
                    menu_item("Copy", MenuAction::Copy),
                    if has_clip { menu_item("Paste", MenuAction::Paste) } else { menu_item_disabled("Paste", MenuAction::Paste) },
                    menu_item("Cut", MenuAction::Cut),
                ];
                items.push(menu_item("Refresh", MenuAction::Refresh));
                items
            }
        }
    } else {
        // Desktop context menu (no window hit)
        vec![
            menu_item("New File", MenuAction::NewFile),
            menu_item("Refresh", MenuAction::Refresh),
            menu_item("Properties", MenuAction::Properties),
        ]
    };

    show(mx as usize, my as usize, items);
}

/// Dispatch a context menu action.
fn dispatch_menu_action(action: super::context_menu::MenuAction) {
    use super::context_menu::MenuAction;

    match action {
        MenuAction::Close => {
            let mut desktop = DESKTOP.lock();
            if let Some(ref mut desk) = *desktop {
                if let Some(id) = desk.wm.active_id() {
                    desk.wm.hide(id);
                }
            }
        }
        MenuAction::Minimize => {
            let mut desktop = DESKTOP.lock();
            if let Some(ref mut desk) = *desktop {
                if let Some(id) = desk.wm.active_id() {
                    if let Some(win) = desk.wm.get_mut(id) {
                        win.state = super::window::WindowState::Minimized;
                        win.active = false;
                    }
                }
            }
        }
        MenuAction::Maximize => {
            let screen_dims = {
                let comp = super::compositor::COMPOSITOR.lock();
                comp.as_ref().map(|c| (c.width, c.height))
            };
            if let Some((sw, sh)) = screen_dims {
                let mut desktop = DESKTOP.lock();
                if let Some(ref mut desk) = *desktop {
                    if let Some(id) = desk.wm.active_id() {
                        if let Some(win) = desk.wm.get_mut(id) {
                            if win.state == super::window::WindowState::Maximized {
                                win.restore();
                            } else {
                                win.maximize(sw, sh);
                            }
                        }
                    }
                }
            }
        }
        MenuAction::Copy => {
            // Copy from the active window's focused widget (TextInput)
            let mut desktop = DESKTOP.lock();
            if let Some(ref mut desk) = *desktop {
                if let Some(id) = desk.wm.active_id() {
                    if let Some(win) = desk.wm.get_mut(id) {
                        if let Some(fid) = win.focused_widget {
                            if let Some(w) = win.get_widget_mut(fid) {
                                if let super::widget::WidgetKind::TextInput(ref ti) = w.kind {
                                    if !ti.text.is_empty() {
                                        crate::gui::clipboard::copy(&ti.text);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        MenuAction::Paste => {
            // Simulate Ctrl+V by dispatching to focused widget
            let mut desktop = DESKTOP.lock();
            if let Some(ref mut desk) = *desktop {
                if let Some(id) = desk.wm.active_id() {
                    if let Some(win) = desk.wm.get_mut(id) {
                        // Dispatch a Ctrl+V key event (ascii 0x16)
                        if let Some(action) = win.dispatch_key('\x16', 0) {
                            queue_action(id, action);
                        }
                    }
                }
            }
        }
        _ => {} // Other actions: no-op for now
    }
}

// ═══════════════════════════════════════════════════════════════
//  Taskbar Click Handling
// ═══════════════════════════════════════════════════════════════

fn handle_taskbar_click(mx: i32, _my: i32, screen_w: usize, _screen_h: usize) {
    let apps_area_start = 226;
    let apps_area_end = screen_w.saturating_sub(200);

    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() {
        Some(d) => d,
        None => return,
    };

    let mut btn_x = apps_area_start;
    let mut target_id: Option<WindowId> = None;

    // Use window_list to get identical list and order
    let window_list = desk.wm.window_list();

    for &(win_id, title, _, _) in &window_list {
        let btn_w = (title.len() * 7 + 20).min(120);
        if btn_x + btn_w > apps_area_end {
            break;
        }
        if mx >= btn_x as i32 && mx < (btn_x + btn_w) as i32 {
            target_id = Some(win_id);
            break;
        }
        btn_x += btn_w + 4;
    }

    if let Some(id) = target_id {
        let mut toggle_minimize = false;
        if let Some(win) = desk.wm.get_mut(id) {
            // If it is active and not minimized, minimize it!
            if win.active && win.state != super::window::WindowState::Minimized {
                win.state = super::window::WindowState::Minimized;
                win.active = false;
                toggle_minimize = true;
            } else {
                win.state = super::window::WindowState::Normal;
                win.visible = true;
            }
        }
        if toggle_minimize {
            desk.wm.auto_focus_next();
        } else {
            desk.wm.bring_to_front(id);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  Hit Testing
// ═══════════════════════════════════════════════════════════════

/// Hit test all windows (reverse z-order for topmost first).
fn hit_test_windows(
    wm: &super::window::WindowManager,
    mx: i32,
    my: i32,
) -> Option<(WindowId, HitRegion)> {
    use super::window::WindowState;

    for window in wm.windows.iter().rev() {
        if !window.visible || window.state == WindowState::Minimized {
            continue;
        }

        let wx = window.x as i32;
        let wy = window.y as i32;
        let tw = window.total_width() as i32;
        let th = window.total_height() as i32;

        // Check if mouse is within window bounds
        if mx < wx || mx >= wx + tw || my < wy || my >= wy + th {
            continue;
        }

        // Determine which region was hit
        let rel_x = mx - wx;
        let rel_y = my - wy;

        // Check resize edges first, but ONLY if NOT maximized!
        if window.state != WindowState::Maximized {
            // Check corners first (top-left, top-right, bottom-left, bottom-right)
            if rel_x < RESIZE_MARGIN && rel_y < RESIZE_MARGIN {
                return Some((window.id, HitRegion::ResizeTopLeft));
            }
            if rel_x >= tw - RESIZE_MARGIN && rel_y < RESIZE_MARGIN {
                return Some((window.id, HitRegion::ResizeTopRight));
            }
            if rel_x < RESIZE_MARGIN && rel_y >= th - RESIZE_MARGIN {
                return Some((window.id, HitRegion::ResizeBottomLeft));
            }
            if rel_x >= tw - RESIZE_MARGIN && rel_y >= th - RESIZE_MARGIN {
                return Some((window.id, HitRegion::ResizeBottomRight));
            }

            // Check edges (left, right, top, bottom)
            if rel_x < RESIZE_MARGIN {
                return Some((window.id, HitRegion::ResizeLeft));
            }
            if rel_x >= tw - RESIZE_MARGIN {
                return Some((window.id, HitRegion::ResizeRight));
            }
            if rel_y < RESIZE_MARGIN {
                return Some((window.id, HitRegion::ResizeTop));
            }
            if rel_y >= th - RESIZE_MARGIN {
                return Some((window.id, HitRegion::ResizeBottom));
            }
        }

        // Title bar region (first TITLEBAR_HEIGHT + BORDER_WIDTH pixels)
        let tb_height = (TITLEBAR_HEIGHT + BORDER_WIDTH) as i32;
        if rel_y < tb_height {
            // Check close button (rightmost, 40px wide)
            let right_limit = tw - BORDER_WIDTH as i32;
            if rel_x >= right_limit - 40 && rel_x < right_limit {
                return Some((window.id, HitRegion::CloseButton));
            }
            // Check maximize button (middle, 40px wide)
            if rel_x >= right_limit - 80 && rel_x < right_limit - 40 {
                return Some((window.id, HitRegion::MaximizeButton));
            }
            // Check minimize button (left, 40px wide)
            if rel_x >= right_limit - 120 && rel_x < right_limit - 80 {
                return Some((window.id, HitRegion::MinimizeButton));
            }
            return Some((window.id, HitRegion::TitleBar));
        }

        return Some((window.id, HitRegion::Content));
    }

    None
}

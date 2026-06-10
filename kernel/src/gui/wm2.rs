//! Window Manager v2 — Phase 61: Window Manager v2 (v0.21.0).
//!
//! Extends the existing floating-window manager with:
//!   • Tiling layout engine  (Float / HorizSplit / VertSplit / Monocle / Grid2x2)
//!   • Focus history ring    (Alt+Tab backward-compatible)
//!   • Keyboard window ops   (snap left/right/top/bottom, maximize, restore)
//!   • Window cycling        (cycle_focus_fwd / cycle_focus_bwd)
//!
//! All state lives in `WM2` (Mutex<Option<Wm2State>>).  Call `init()` once
//! during boot; thereafter every frame that a tiling layout is active should
//! call `apply_tiling()` so newly spawned windows are arranged correctly.

#![allow(dead_code)]

use alloc::vec::Vec;
use spin::Mutex;

use super::theme::{BORDER_WIDTH, TITLEBAR_HEIGHT, TASKBAR_HEIGHT};
use super::window::{WindowId, WindowManager, WindowState};

// ─── Tiling layout modes ─────────────────────────────────────────────────────

/// How visible windows are arranged on the workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TilingLayout {
    /// Traditional floating windows — no automatic arrangement.
    Float,
    /// Each window occupies a vertical column (left-to-right).
    HorizSplit,
    /// Each window occupies a horizontal row (top-to-bottom).
    VertSplit,
    /// The focused window fills the workspace; others are stacked behind.
    Monocle,
    /// Up to four windows in a 2×2 grid; extras stack beneath slot 3.
    Grid2x2,
}

impl TilingLayout {
    /// Cycle to the next layout mode.
    pub fn next(self) -> Self {
        match self {
            TilingLayout::Float      => TilingLayout::HorizSplit,
            TilingLayout::HorizSplit => TilingLayout::VertSplit,
            TilingLayout::VertSplit  => TilingLayout::Monocle,
            TilingLayout::Monocle   => TilingLayout::Grid2x2,
            TilingLayout::Grid2x2   => TilingLayout::Float,
        }
    }

    /// Human-readable name for display.
    pub fn name(self) -> &'static str {
        match self {
            TilingLayout::Float      => "Floating",
            TilingLayout::HorizSplit => "Horizontal Split",
            TilingLayout::VertSplit  => "Vertical Split",
            TilingLayout::Monocle   => "Monocle",
            TilingLayout::Grid2x2   => "Grid 2×2",
        }
    }
}

// ─── WM2 state ───────────────────────────────────────────────────────────────

pub struct Wm2State {
    /// Active tiling layout (Float = no tiling).
    pub layout: TilingLayout,
    /// Focus history ring: most-recently-focused window ID is at the tail.
    /// Capacity is capped at FOCUS_HISTORY_CAP.
    pub focus_history: Vec<WindowId>,
    /// Current Alt+Tab position within focus_history.
    pub cycle_idx: usize,
    /// Whether a tiling re-layout is pending on the next render frame.
    pub layout_dirty: bool,
}

const FOCUS_HISTORY_CAP: usize = 32;

pub static WM2: Mutex<Option<Wm2State>> = Mutex::new(None);

/// Initialise the WM2 subsystem.  Must be called once before any other API.
pub fn init() {
    *WM2.lock() = Some(Wm2State {
        layout: TilingLayout::Float,
        focus_history: Vec::new(),
        cycle_idx: 0,
        layout_dirty: false,
    });
}

// ─── Layout queries ──────────────────────────────────────────────────────────

/// Returns the current tiling layout.
pub fn get_layout() -> TilingLayout {
    WM2.lock().as_ref().map(|s| s.layout).unwrap_or(TilingLayout::Float)
}

/// Set a specific tiling layout and mark a re-tile as pending.
pub fn set_layout(layout: TilingLayout) {
    if let Some(ref mut s) = *WM2.lock() {
        s.layout = layout;
        s.layout_dirty = true;
    }
}

/// Advance to the next layout mode (cycles through all modes).
pub fn cycle_layout() -> TilingLayout {
    let mut guard = WM2.lock();
    if let Some(ref mut s) = *guard {
        s.layout = s.layout.next();
        s.layout_dirty = true;
        s.layout
    } else {
        TilingLayout::Float
    }
}

/// Returns `true` and clears the dirty flag if a re-tile is pending.
pub fn take_layout_dirty() -> bool {
    if let Some(ref mut s) = *WM2.lock() {
        let d = s.layout_dirty;
        s.layout_dirty = false;
        d
    } else {
        false
    }
}

// ─── Focus history ────────────────────────────────────────────────────────────

/// Record that `id` has received focus.  Deduplicates; keeps the most recent
/// occurrence at the tail.  Older entries beyond the cap are evicted.
pub fn push_focus(id: WindowId) {
    if let Some(ref mut s) = *WM2.lock() {
        s.focus_history.retain(|&x| x != id);
        s.focus_history.push(id);
        while s.focus_history.len() > FOCUS_HISTORY_CAP {
            s.focus_history.remove(0);
        }
        let n = s.focus_history.len();
        s.cycle_idx = n.saturating_sub(1);
    }
}

/// Remove `id` from the focus history (call on window close).
pub fn remove_focus(id: WindowId) {
    if let Some(ref mut s) = *WM2.lock() {
        s.focus_history.retain(|&x| x != id);
        let n = s.focus_history.len();
        if s.cycle_idx >= n && n > 0 {
            s.cycle_idx = n - 1;
        }
    }
}

/// Step one window backward in focus history and return the targeted window ID.
/// Wraps around at the beginning.
pub fn cycle_focus_bwd() -> Option<WindowId> {
    let mut guard = WM2.lock();
    let s = guard.as_mut()?;
    let n = s.focus_history.len();
    if n == 0 { return None; }
    if s.cycle_idx > 0 {
        s.cycle_idx -= 1;
    } else {
        s.cycle_idx = n - 1;
    }
    Some(s.focus_history[s.cycle_idx])
}

/// Step one window forward in focus history and return the targeted window ID.
pub fn cycle_focus_fwd() -> Option<WindowId> {
    let mut guard = WM2.lock();
    let s = guard.as_mut()?;
    let n = s.focus_history.len();
    if n == 0 { return None; }
    s.cycle_idx = (s.cycle_idx + 1) % n;
    Some(s.focus_history[s.cycle_idx])
}

// ─── Tiling engine ────────────────────────────────────────────────────────────

/// Rearrange all visible, non-minimized windows in `wm` according to the
/// current tiling layout.  Uses `screen_w × screen_h` as the screen size
/// (taskbar height is subtracted from the workspace automatically).
///
/// Floating layout is a no-op (windows keep their current positions).
/// Calling this on every frame is cheap because it only moves/resizes when
/// the layout is not Float.
pub fn apply_tiling(wm: &mut WindowManager, screen_w: usize, screen_h: usize) {
    let layout = get_layout();
    if layout == TilingLayout::Float { return; }

    let work_h = screen_h.saturating_sub(TASKBAR_HEIGHT);
    let work_w = screen_w;

    // Collect IDs of visible, non-minimized windows in z-order (front last).
    let ids: Vec<WindowId> = wm.windows
        .iter()
        .filter(|w| w.visible && w.state != WindowState::Minimized)
        .map(|w| w.id)
        .collect();

    let n = ids.len();
    if n == 0 { return; }

    match layout {
        // ── Horizontal split — equal-width vertical columns ─────────
        TilingLayout::HorizSplit => {
            let col_w = work_w / n;
            for (i, &id) in ids.iter().enumerate() {
                if let Some(win) = wm.get_mut(id) {
                    let x = i * col_w;
                    let w = if i + 1 == n { work_w - x } else { col_w };
                    win.x = x;
                    win.y = 0;
                    let cw = w.saturating_sub(BORDER_WIDTH * 2);
                    let ch = work_h.saturating_sub(TITLEBAR_HEIGHT + BORDER_WIDTH * 2);
                    win.resize(cw, ch);
                    win.state = WindowState::Normal;
                    win.dirty = true;
                }
            }
        }

        // ── Vertical split — equal-height horizontal rows ────────────
        TilingLayout::VertSplit => {
            let row_h = work_h / n;
            for (i, &id) in ids.iter().enumerate() {
                if let Some(win) = wm.get_mut(id) {
                    let y = i * row_h;
                    let h = if i + 1 == n { work_h - y } else { row_h };
                    win.x = 0;
                    win.y = y;
                    let cw = work_w.saturating_sub(BORDER_WIDTH * 2);
                    let ch = h.saturating_sub(TITLEBAR_HEIGHT + BORDER_WIDTH * 2);
                    win.resize(cw, ch);
                    win.state = WindowState::Normal;
                    win.dirty = true;
                }
            }
        }

        // ── Monocle — topmost window maximized, others stacked behind ─
        TilingLayout::Monocle => {
            for (i, &id) in ids.iter().enumerate() {
                if let Some(win) = wm.get_mut(id) {
                    if i + 1 == n {
                        // Topmost (active) window → maximize
                        win.maximize(screen_w, screen_h);
                    } else {
                        // Others: small offset so title bars are still reachable
                        win.x = 16 + i * 4;
                        win.y = 16 + i * 4;
                        win.state = WindowState::Normal;
                        win.dirty = true;
                    }
                }
            }
        }

        // ── Grid 2×2 — up to four windows in quadrants ───────────────
        TilingLayout::Grid2x2 => {
            let hw = work_w / 2;
            let hh = work_h / 2;
            // positions: TL, TR, BL, BR
            let slots: [(usize, usize, usize, usize); 4] = [
                (0,  0,  hw,           hh),
                (hw, 0,  work_w - hw,  hh),
                (0,  hh, hw,           work_h - hh),
                (hw, hh, work_w - hw,  work_h - hh),
            ];
            for (i, &id) in ids.iter().take(4).enumerate() {
                let (sx, sy, sw, sh) = slots[i];
                if let Some(win) = wm.get_mut(id) {
                    win.x = sx;
                    win.y = sy;
                    let cw = sw.saturating_sub(BORDER_WIDTH * 2);
                    let ch = sh.saturating_sub(TITLEBAR_HEIGHT + BORDER_WIDTH * 2);
                    win.resize(cw, ch);
                    win.state = WindowState::Normal;
                    win.dirty = true;
                }
            }
            // Any extra windows: stack in top-left corner (small cascade)
            for (i, &id) in ids.iter().skip(4).enumerate() {
                if let Some(win) = wm.get_mut(id) {
                    win.x = 24 + i * 6;
                    win.y = 24 + i * 6;
                    win.dirty = true;
                }
            }
        }

        TilingLayout::Float => {} // unreachable (handled above)
    }
}

// ─── Keyboard window ops ─────────────────────────────────────────────────────

/// Snap the active window to the left half of the screen.
pub fn snap_active_left(wm: &mut WindowManager, screen_w: usize, screen_h: usize) {
    if let Some(id) = wm.active_id() {
        let work_h = screen_h.saturating_sub(TASKBAR_HEIGHT);
        let half_w = screen_w / 2;
        if let Some(win) = wm.get_mut(id) {
            win.snap_to(0, 0, half_w, work_h);
        }
    }
}

/// Snap the active window to the right half of the screen.
pub fn snap_active_right(wm: &mut WindowManager, screen_w: usize, screen_h: usize) {
    if let Some(id) = wm.active_id() {
        let work_h = screen_h.saturating_sub(TASKBAR_HEIGHT);
        let half_w = screen_w / 2;
        if let Some(win) = wm.get_mut(id) {
            win.snap_to(half_w, 0, screen_w - half_w, work_h);
        }
    }
}

/// Snap the active window to the top half.
pub fn snap_active_top(wm: &mut WindowManager, screen_w: usize, screen_h: usize) {
    if let Some(id) = wm.active_id() {
        let work_h = screen_h.saturating_sub(TASKBAR_HEIGHT);
        let half_h = work_h / 2;
        if let Some(win) = wm.get_mut(id) {
            win.snap_to(0, 0, screen_w, half_h);
        }
    }
}

/// Snap the active window to the bottom half.
pub fn snap_active_bottom(wm: &mut WindowManager, screen_w: usize, screen_h: usize) {
    if let Some(id) = wm.active_id() {
        let work_h = screen_h.saturating_sub(TASKBAR_HEIGHT);
        let half_h = work_h / 2;
        if let Some(win) = wm.get_mut(id) {
            win.snap_to(0, half_h, screen_w, work_h - half_h);
        }
    }
}

/// Maximize or restore the active window (toggles).
pub fn toggle_maximize_active(wm: &mut WindowManager, screen_w: usize, screen_h: usize) {
    if let Some(id) = wm.active_id() {
        if let Some(win) = wm.get_mut(id) {
            if win.state == WindowState::Maximized {
                win.restore();
            } else {
                win.maximize(screen_w, screen_h);
            }
        }
    }
}

/// Minimize the active window and focus the next visible one.
pub fn minimize_active(wm: &mut WindowManager) {
    if let Some(id) = wm.active_id() {
        if let Some(win) = wm.get_mut(id) {
            win.state = WindowState::Minimized;
            win.active = false;
            win.visible = true; // Still in taskbar
        }
        remove_focus(id);
        wm.auto_focus_next();
    }
}

/// Close the active window and shift focus to the next.
pub fn close_active(wm: &mut WindowManager) {
    if let Some(id) = wm.active_id() {
        if let Some(pos) = wm.windows.iter().position(|w| w.id == id) {
            wm.windows.remove(pos);
        }
        remove_focus(id);
        wm.auto_focus_next();
    }
}

// ─── Window move helpers ──────────────────────────────────────────────────────

/// Move the active window by a delta, clamped to stay on-screen.
pub fn move_active(wm: &mut WindowManager, dx: i32, dy: i32, screen_w: usize, screen_h: usize) {
    if let Some(id) = wm.active_id() {
        if let Some(win) = wm.get_mut(id) {
            if win.state == WindowState::Maximized { return; }
            let new_x = (win.x as i32 + dx).max(0) as usize;
            let new_y = (win.y as i32 + dy).max(0) as usize;
            let max_x = screen_w.saturating_sub(win.total_width());
            let max_y = screen_h.saturating_sub(win.total_height() + TASKBAR_HEIGHT);
            win.x = new_x.min(max_x);
            win.y = new_y.min(max_y);
            win.dirty = true;
        }
    }
}

// ─── Self-test ────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // 1. Layout cycle
    let mut l = TilingLayout::Float;
    l = l.next(); if l != TilingLayout::HorizSplit { ok = false; }
    l = l.next(); if l != TilingLayout::VertSplit  { ok = false; }
    l = l.next(); if l != TilingLayout::Monocle   { ok = false; }
    l = l.next(); if l != TilingLayout::Grid2x2   { ok = false; }
    l = l.next(); if l != TilingLayout::Float      { ok = false; }

    // 2. Layout names
    if TilingLayout::Float.name()      != "Floating"          { ok = false; }
    if TilingLayout::HorizSplit.name() != "Horizontal Split"  { ok = false; }
    if TilingLayout::Monocle.name()    != "Monocle"           { ok = false; }

    // 3. Focus history push / cycle
    init();
    push_focus(1);
    push_focus(2);
    push_focus(3);
    // cycle_bwd: 3→2
    if cycle_focus_bwd() != Some(2) { ok = false; }
    // cycle_bwd: 2→1
    if cycle_focus_bwd() != Some(1) { ok = false; }
    // cycle_fwd: 1→2
    if cycle_focus_fwd() != Some(2) { ok = false; }

    // 4. Dedup in push_focus
    push_focus(2); // 2 already present → moves to tail
    {
        let guard = WM2.lock();
        if let Some(ref s) = *guard {
            // History should be [1, 3, 2] with 2 at tail (deduplicated)
            let tail = s.focus_history.last().copied();
            if tail != Some(2) { ok = false; }
        } else {
            ok = false;
        }
    }

    // 5. remove_focus
    remove_focus(3);
    {
        let guard = WM2.lock();
        if let Some(ref s) = *guard {
            if s.focus_history.contains(&3) { ok = false; }
        }
    }

    // 6. get/set layout + dirty flag
    set_layout(TilingLayout::Grid2x2);
    if get_layout() != TilingLayout::Grid2x2 { ok = false; }
    if !take_layout_dirty() { ok = false; }
    if take_layout_dirty() { ok = false; } // second call: flag cleared

    ok
}

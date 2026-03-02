/// Virtual Desktops / Workspaces for Smart OS.
///
/// Phase 10: Multiple desktop workspaces. Each workspace has its own
/// set of visible windows. Users can switch between workspaces using
/// keyboard shortcuts (Ctrl+1/2/3/4) or the taskbar.
///
/// Windows are assigned to a workspace. Switching workspaces hides
/// all windows on the current workspace and shows the new one.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;
use super::window::WindowId;
use crate::serial_println;

/// Maximum number of workspaces.
pub const MAX_WORKSPACES: usize = 4;

/// The current active workspace index (0-based).
static CURRENT_WORKSPACE: AtomicUsize = AtomicUsize::new(0);

/// Workspace names.
static WORKSPACE_NAMES: Mutex<[String; MAX_WORKSPACES]> = Mutex::new([
    String::new(), String::new(), String::new(), String::new(),
]);

/// Window → workspace assignment.
static WINDOW_WORKSPACE: Mutex<BTreeMap<WindowId, usize>> = Mutex::new(BTreeMap::new());

/// Initialize virtual desktop system.
pub fn init() {
    let mut names = WORKSPACE_NAMES.lock();
    names[0] = String::from("Main");
    names[1] = String::from("Work");
    names[2] = String::from("Dev");
    names[3] = String::from("Extra");

    serial_println!("[vdesktop] Virtual desktops initialized ({} workspaces).", MAX_WORKSPACES);
}

/// Get the current workspace index.
pub fn current() -> usize {
    CURRENT_WORKSPACE.load(Ordering::Relaxed)
}

/// Switch to a different workspace.
///
/// Returns the previous workspace index.
pub fn switch_to(workspace: usize) -> usize {
    if workspace >= MAX_WORKSPACES {
        return current();
    }

    let prev = CURRENT_WORKSPACE.swap(workspace, Ordering::Relaxed);

    if prev != workspace {
        serial_println!("[vdesktop] Switched from workspace {} to {}", prev, workspace);
    }

    prev
}

/// Switch to the next workspace (wraps around).
pub fn next_workspace() -> usize {
    let cur = current();
    let next = (cur + 1) % MAX_WORKSPACES;
    switch_to(next);
    next
}

/// Switch to the previous workspace (wraps around).
pub fn prev_workspace() -> usize {
    let cur = current();
    let prev = if cur == 0 { MAX_WORKSPACES - 1 } else { cur - 1 };
    switch_to(prev);
    prev
}

/// Assign a window to a workspace.
pub fn assign_window(window_id: WindowId, workspace: usize) {
    if workspace < MAX_WORKSPACES {
        WINDOW_WORKSPACE.lock().insert(window_id, workspace);
    }
}

/// Get the workspace a window is assigned to.
pub fn window_workspace(window_id: WindowId) -> usize {
    WINDOW_WORKSPACE.lock().get(&window_id).copied().unwrap_or(0)
}

/// Move a window to a different workspace.
pub fn move_window(window_id: WindowId, target_workspace: usize) {
    if target_workspace < MAX_WORKSPACES {
        WINDOW_WORKSPACE.lock().insert(window_id, target_workspace);
    }
}

/// Check if a window should be visible on the current workspace.
pub fn is_window_visible(window_id: WindowId) -> bool {
    let cur = current();
    let ws = WINDOW_WORKSPACE.lock().get(&window_id).copied().unwrap_or(0);
    ws == cur
}

/// Get all windows on a given workspace.
pub fn windows_on_workspace(workspace: usize) -> Vec<WindowId> {
    WINDOW_WORKSPACE.lock()
        .iter()
        .filter(|(_, ws)| **ws == workspace)
        .map(|(id, _)| *id)
        .collect()
}

/// Get workspace name.
pub fn workspace_name(workspace: usize) -> String {
    if workspace < MAX_WORKSPACES {
        WORKSPACE_NAMES.lock()[workspace].clone()
    } else {
        String::from("?")
    }
}

/// Set workspace name.
pub fn set_workspace_name(workspace: usize, name: &str) {
    if workspace < MAX_WORKSPACES {
        WORKSPACE_NAMES.lock()[workspace] = String::from(name);
    }
}

/// Remove a window from the workspace tracking (on window close).
pub fn remove_window(window_id: WindowId) {
    WINDOW_WORKSPACE.lock().remove(&window_id);
}

/// Get workspace info: Vec<(index, name, window_count, is_current)>.
pub fn workspace_info() -> Vec<(usize, String, usize, bool)> {
    let cur = current();
    let assignments = WINDOW_WORKSPACE.lock();
    let names = WORKSPACE_NAMES.lock();

    (0..MAX_WORKSPACES)
        .map(|i| {
            let count = assignments.values().filter(|ws| **ws == i).count();
            (i, names[i].clone(), count, i == cur)
        })
        .collect()
}

/// Count of windows on the current workspace.
pub fn current_window_count() -> usize {
    let cur = current();
    WINDOW_WORKSPACE.lock()
        .values()
        .filter(|ws| **ws == cur)
        .count()
}

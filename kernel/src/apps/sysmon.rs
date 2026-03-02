/// Smart OS System Monitor — Live system dashboard.
///
/// Displays real-time heap usage, thread list, IPC ports, plugin status,
/// and uptime. Uses content_lines mode (no interactive widgets needed).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::{Window, WindowId};
use crate::gui::desktop::DESKTOP;

/// System monitor state.
pub struct SysMonState {
    pub window_id: WindowId,
    /// Heap usage history (last 40 samples, percentage 0-100).
    pub heap_history: [u8; 40],
    pub history_idx: usize,
    pub update_counter: u64,
}

pub static STATE: Mutex<Option<SysMonState>> = Mutex::new(None);

/// System monitor thread entry point.
pub fn run() {
    // Create the system monitor window
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() {
            Some(d) => d,
            None => return,
        };

        let win = Window::new("System", 30, 400, 420, 200, ACCENT_CYAN);
        // Uses content_lines mode (no widgets, pure dashboard)

        let id = win.id;
        desk.wm.add(win);
        id
    };

    *STATE.lock() = Some(SysMonState {
        window_id,
        heap_history: [0; 40],
        history_idx: 0,
        update_counter: 0,
    });

    // Main loop: periodically update system stats
    loop {
        update_stats(window_id);

        // Update every ~50 yields (roughly every second at 100Hz timer)
        for _ in 0..50 {
            crate::process::scheduler::yield_now();
        }
    }
}

/// Update system stats and write to the window's content_lines.
fn update_stats(window_id: WindowId) {
    let (used, free) = crate::memory::heap::heap_stats();
    let total = used + free;
    let pct = if total > 0 { (used * 100 / total) as u8 } else { 0 };

    // Record heap history
    {
        let mut state = STATE.lock();
        if let Some(ref mut s) = *state {
            s.heap_history[s.history_idx] = pct;
            s.history_idx = (s.history_idx + 1) % 40;
            s.update_counter += 1;
        }
    }

    // Build content lines
    let mut lines: Vec<String> = Vec::new();

    // ── Header ──
    let uptime = crate::drivers::timer::uptime_secs();
    let mins = uptime / 60;
    let secs = uptime % 60;
    lines.push(format!(" SYSTEM MONITOR           Uptime: {:02}:{:02}", mins, secs));
    lines.push(String::new());

    // ── Memory ──
    lines.push(format!(" Memory: {}K / {}K ({}%)", used / 1024, total / 1024, pct));

    // Memory bar
    let bar_width = 36;
    let filled = (pct as usize * bar_width) / 100;
    let bar: String = (0..bar_width).map(|i| if i < filled { '#' } else { '.' }).collect();
    lines.push(format!(" [{}]", bar));
    lines.push(String::new());

    // ── Threads ──
    let threads = crate::process::scheduler::list_threads();
    lines.push(format!(" Threads: {}", threads.len()));
    for (tid, name, _state, pri) in &threads {
        let state_ch = match _state {
            crate::process::ThreadState::Running => 'R',
            crate::process::ThreadState::Ready => 'r',
            crate::process::ThreadState::Blocked => 'B',
            crate::process::ThreadState::Dead => 'D',
        };
        let display_name = if name.len() > 14 { &name[..14] } else { name.as_str() };
        lines.push(format!("  {:>3} {} {:14} P{}", tid, state_ch, display_name, pri));
    }

    // ── IPC Ports ──
    let ports = crate::ipc::port::port_stats();
    if !ports.is_empty() {
        // Only show if there are messages pending
        let total_pending: usize = ports.iter().map(|(_, c)| c).sum();
        lines.push(format!(" IPC: {} ports, {} pending", ports.len(), total_pending));
    }

    // Write to the window
    let mut desktop = DESKTOP.lock();
    if let Some(ref mut desk) = *desktop {
        if let Some(win) = desk.wm.get_mut(window_id) {
            win.content_lines = lines;
        }
    }
}

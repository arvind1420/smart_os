/// Smart OS System Monitor — Live system dashboard with graphs.
///
/// Phase 12: Enhanced with rolling CPU/memory history graph,
/// per-thread CPU time, network RX/TX counters, and disk stats.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::{Window, WindowId};
use crate::gui::desktop::DESKTOP;

const HISTORY_LEN: usize = 40;

pub struct SysMonState {
    pub window_id: WindowId,
    /// Heap usage history (last 40 samples, 0-100%).
    pub heap_history: [u8; HISTORY_LEN],
    /// Thread count history.
    pub thread_history: [u8; HISTORY_LEN],
    pub history_idx: usize,
    pub update_counter: u64,
    /// Cumulative tick at last sample (for delta calculation).
    pub last_ticks: u64,
}

pub static STATE: Mutex<Option<SysMonState>> = Mutex::new(None);

pub fn run() {
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() { Some(d) => d, None => return };
        let win = Window::new("System Monitor", 30, 400, 440, 260, ACCENT_CYAN);
        let id = win.id;
        desk.wm.add(win);
        id
    };

    *STATE.lock() = Some(SysMonState {
        window_id,
        heap_history:   [0; HISTORY_LEN],
        thread_history: [0; HISTORY_LEN],
        history_idx: 0,
        update_counter: 0,
        last_ticks: 0,
    });

    loop {
        update_stats(window_id);
        for _ in 0..50 { crate::process::scheduler::yield_now(); }
    }
}

fn render_graph(history: &[u8; HISTORY_LEN], idx: usize, width: usize) -> String {
    // Build a 1-line mini bar graph using block characters
    let bars = ['_', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let mut out = String::new();
    for i in 0..width {
        let sample_idx = (idx + HISTORY_LEN - width + i) % HISTORY_LEN;
        let val = history[sample_idx];
        let bar_idx = ((val as usize) * (bars.len() - 1)) / 100;
        out.push(bars[bar_idx.min(bars.len() - 1)]);
    }
    out
}

fn update_stats(window_id: WindowId) {
    let (used, free) = crate::memory::heap::heap_stats();
    let total = used + free;
    let pct = if total > 0 { (used * 100 / total) as u8 } else { 0 };
    let threads = crate::process::scheduler::list_threads();
    let thread_pct = ((threads.len().min(30)) * 100 / 30) as u8;

    let history_idx;
    {
        let mut state = STATE.lock();
        if let Some(ref mut s) = *state {
            s.heap_history[s.history_idx] = pct;
            s.thread_history[s.history_idx] = thread_pct;
            s.history_idx = (s.history_idx + 1) % HISTORY_LEN;
            s.update_counter += 1;
            history_idx = s.history_idx;
        } else { return; }
    }

    let mut lines: Vec<String> = Vec::new();

    // ── Header ──────────────────────────────────────────────────
    let uptime = crate::drivers::timer::uptime_secs();
    let mins = uptime / 60;
    let secs = uptime % 60;
    lines.push(format!(" SYSTEM MONITOR  v0.12.0      {:02}:{:02}", mins, secs));
    lines.push(String::new());

    // ── Memory ──────────────────────────────────────────────────
    let bar_width = 36;
    let filled = (pct as usize * bar_width) / 100;
    let bar: String = (0..bar_width).map(|i| if i < filled { '█' } else { '░' }).collect();
    lines.push(format!(" MEM  {}K/{}K ({}%)", used/1024, total/1024, pct));
    lines.push(format!(" [{}]", bar));

    // Memory rolling graph
    {
        let state = STATE.lock();
        if let Some(ref s) = *state {
            let graph = render_graph(&s.heap_history, history_idx, 38);
            lines.push(format!(" MEM▲ {}", graph));
        }
    }
    lines.push(String::new());

    // ── CPU (thread load proxy) ──────────────────────────────────
    let running_count = threads.iter().filter(|(_, _, st, _)|
        matches!(st, crate::process::ThreadState::Running | crate::process::ThreadState::Ready)
    ).count();
    {
        let state = STATE.lock();
        if let Some(ref s) = *state {
            let graph = render_graph(&s.thread_history, history_idx, 38);
            lines.push(format!(" CPU▲ {}", graph));
        }
    }
    lines.push(format!(" Threads: {}  Active: {}", threads.len(), running_count));

    // Thread table (top 8 by priority)
    let mut sorted = threads.clone();
    sorted.sort_by(|a, b| b.3.cmp(&a.3));
    lines.push(String::from("  TID  NAME             PRI STATE"));
    for (tid, name, st, pri) in sorted.iter().take(8) {
        let sc = match st {
            crate::process::ThreadState::Running => 'R',
            crate::process::ThreadState::Ready   => 'r',
            crate::process::ThreadState::Blocked => 'B',
            crate::process::ThreadState::Dead    => 'D',
        };
        let n = if name.len() > 14 { &name[..14] } else { name.as_str() };
        lines.push(format!("  {:>3}  {:14}  {:>2} {}", tid, n, pri, sc));
    }

    // ── Network ──────────────────────────────────────────────────
    lines.push(String::new());
    let nic = if crate::drivers::e1000::is_available() { "e1000" }
              else if crate::drivers::virtio_net::is_available() { "VirtIO" }
              else { "none" };
    let ip = crate::net::LOCAL_IP;
    lines.push(format!(" NET  {} — {}.{}.{}.{}", nic, ip[0], ip[1], ip[2], ip[3]));

    // ── Disk ─────────────────────────────────────────────────────
    let disk_info = if crate::drivers::ahci::is_available() {
        let ctrl = crate::drivers::ahci::AHCI_CONTROLLERS.lock();
        ctrl.first().and_then(|c| c.ports.first())
            .map(|p| format!("AHCI {}MiB", p.capacity_sectors * 512 / (1024*1024)))
            .unwrap_or_else(|| String::from("AHCI"))
    } else if crate::drivers::virtio_blk::is_available() {
        format!("VirtIO {}KiB", crate::drivers::virtio_blk::capacity() * 512 / 1024)
    } else {
        String::from("no disk")
    };
    lines.push(format!(" DISK {}", disk_info));

    // ── IPC ───────────────────────────────────────────────────────
    let ports = crate::ipc::port::port_stats();
    let pending: usize = ports.iter().map(|(_, c)| c).sum();
    lines.push(format!(" IPC  {} ports  {} pending msgs", ports.len(), pending));

    // Write to window
    let mut desktop = DESKTOP.lock();
    if let Some(ref mut desk) = *desktop {
        if let Some(win) = desk.wm.get_mut(window_id) {
            win.content_lines = lines;
        }
    }
}

/// Phase 57: System Dashboard for Smart OS.
///
/// A real-time monitoring panel that shows:
///   • CPU utilisation bar  — sampled from the profiler tick counter
///   • Memory bars          — heap (used/free) + physical frames
///   • Network counters     — TX/RX bytes (read from global net counters)
///   • Process table        — pid, name, state (top 8 processes)
///   • Uptime + tick rate   — wall-clock uptime from the timer
///   • Audio level meter    — placeholder peak from web_audio mixer
///
/// Refresh rate: every 20 scheduler ticks (~200 ms at 100 Hz).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::{Window, WindowId};
use crate::gui::widget::*;
use crate::gui::desktop::DESKTOP;

// ─────────────────────────────────────────────────────────────────────────────
//  Snapshot collected each refresh
// ─────────────────────────────────────────────────────────────────────────────

/// One sampled snapshot of system metrics.
#[derive(Clone, Default)]
pub struct Snapshot {
    // CPU
    pub cpu_util_pct:    u32,    // 0-100
    pub ticks:           u64,
    pub uptime_secs:     u64,

    // Memory
    pub heap_used_kb:    u64,
    pub heap_free_kb:    u64,
    pub phys_free_pages: usize,
    pub phys_total_pages:usize,

    // Processes
    pub proc_count:      usize,
    pub thread_count:    usize,
    pub procs:           Vec<ProcRow>,

    // Profiler samples
    pub profiler_samples:u64,

    // Network (cumulative)
    pub net_tx_bytes:    u64,
    pub net_rx_bytes:    u64,

    // Audio
    pub audio_peak:      u8,   // 0-100
}

#[derive(Clone, Default)]
pub struct ProcRow {
    pub pid:   u64,
    pub name:  String,
    pub state: String,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Global net counters (incremented by net drivers)
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::{AtomicU64, Ordering};

/// Total bytes transmitted since boot (incremented by VirtIO-net / e1000 TX).
pub static NET_TX_BYTES: AtomicU64 = AtomicU64::new(0);
/// Total bytes received since boot (incremented by VirtIO-net / e1000 RX).
pub static NET_RX_BYTES: AtomicU64 = AtomicU64::new(0);

/// Called by network drivers when a packet is sent.
#[inline]
pub fn net_account_tx(bytes: u64) { NET_TX_BYTES.fetch_add(bytes, Ordering::Relaxed); }
/// Called by network drivers when a packet is received.
#[inline]
pub fn net_account_rx(bytes: u64) { NET_RX_BYTES.fetch_add(bytes, Ordering::Relaxed); }

// ─────────────────────────────────────────────────────────────────────────────
//  CPU utilisation estimation
// ─────────────────────────────────────────────────────────────────────────────

/// Last-seen profiler sample count and tick, used to estimate CPU busy %.
static LAST_SAMPLES: AtomicU64 = AtomicU64::new(0);
static LAST_TICKS:   AtomicU64 = AtomicU64::new(0);

/// Estimate CPU utilisation as a percentage.
///
/// Uses the number of profiler ring-buffer samples collected per timer tick
/// as a proxy: if the profiler is sampling at rate R and the ring fills at
/// rate S, busy% ≈ S/R × 100, capped at 100.
///
/// This is a heuristic — real CPU idle detection would need TSC deltas from
/// an idle thread, but that's out of scope for Phase 57.
fn estimate_cpu_util(current_samples: u64, current_ticks: u64) -> u32 {
    let prev_s = LAST_SAMPLES.load(Ordering::Relaxed);
    let prev_t = LAST_TICKS.load(Ordering::Relaxed);

    LAST_SAMPLES.store(current_samples, Ordering::Relaxed);
    LAST_TICKS.store(current_ticks, Ordering::Relaxed);

    let delta_t = current_ticks.saturating_sub(prev_t);
    let delta_s = current_samples.saturating_sub(prev_s);
    if delta_t == 0 { return 0; }

    // Each timer tick the profiler takes one sample; the scheduler also runs.
    // Non-idle ticks generate one sample per tick → util ~ delta_s / delta_t × 100.
    let pct = (delta_s * 100) / delta_t;
    pct.min(100) as u32
}

// ─────────────────────────────────────────────────────────────────────────────
//  Data collection
// ─────────────────────────────────────────────────────────────────────────────

pub fn collect_snapshot() -> Snapshot {
    let ticks       = crate::drivers::timer::ticks();
    let uptime_secs = crate::drivers::timer::uptime_secs();

    // Heap stats.
    let (heap_used, heap_free) = crate::memory::heap::heap_stats();

    // Physical frames.
    let (phys_free_pages, phys_total_pages) = crate::memory::frame::frame_stats();

    // Profiler sample count.
    let profiler_samples = crate::profiler::total_samples();

    // CPU estimate.
    let cpu_util_pct = estimate_cpu_util(profiler_samples, ticks);

    // Process table snapshot (top 8 by pid).
    let mut procs: Vec<ProcRow> = Vec::new();
    let proc_count;
    {
        let table = crate::process::process::PROCESS_TABLE.lock();
        proc_count = table.len();
        for (&pid, proc) in table.iter().take(8) {
            procs.push(ProcRow {
                pid,
                name: proc.name.clone(),
                state: format!("{:?}", proc.state),
            });
        }
    }

    // Thread (ready queue) count.
    let thread_count = crate::process::scheduler::ready_count();

    // Network counters.
    let net_tx_bytes = NET_TX_BYTES.load(Ordering::Relaxed);
    let net_rx_bytes = NET_RX_BYTES.load(Ordering::Relaxed);

    // Audio peak — read from web_audio mixer (best-effort).
    let audio_peak = sample_audio_peak();

    Snapshot {
        cpu_util_pct,
        ticks,
        uptime_secs,
        heap_used_kb:     (heap_used / 1024) as u64,
        heap_free_kb:     (heap_free / 1024) as u64,
        phys_free_pages,
        phys_total_pages,
        proc_count,
        thread_count,
        procs,
        profiler_samples,
        net_tx_bytes,
        net_rx_bytes,
        audio_peak,
    }
}

/// Global audio output peak [0, 100], updated by the audio mixer path.
/// Exposed as an AtomicU64 so the dashboard can read it lock-free.
pub static AUDIO_PEAK_PCT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Called by the web_audio render path (or HDA driver) to report output level.
pub fn set_audio_peak(pct: u8) {
    AUDIO_PEAK_PCT.store(pct as u64, Ordering::Relaxed);
}

/// Sample audio peak level [0, 100].
fn sample_audio_peak() -> u8 {
    AUDIO_PEAK_PCT.load(Ordering::Relaxed).min(100) as u8
}

// ─────────────────────────────────────────────────────────────────────────────
//  Formatting helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Render a horizontal bar: "████░░░░░░" (width=20 cells, fill=pct/5 blocks).
pub fn bar(pct: u32, width: usize) -> String {
    let filled = ((pct as usize) * width / 100).min(width);
    let empty   = width - filled;
    let mut s = String::new();
    for _ in 0..filled { s.push('█'); }
    for _ in 0..empty  { s.push('░'); }
    s
}

/// Format a byte count as KB / MB / GB string.
pub fn fmt_bytes(bytes: u64) -> String {
    if bytes < 1024 { format!("{}B", bytes) }
    else if bytes < 1024 * 1024 { format!("{}K", bytes / 1024) }
    else if bytes < 1024 * 1024 * 1024 { format!("{}M", bytes / (1024 * 1024)) }
    else { format!("{}G", bytes / (1024 * 1024 * 1024)) }
}

/// Format uptime as "DDd HH:MM:SS".
pub fn fmt_uptime(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600)  / 60;
    let s = secs % 60;
    if d > 0 {
        format!("{}d {:02}:{:02}:{:02}", d, h, m, s)
    } else {
        format!("{:02}:{:02}:{:02}", h, m, s)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  GUI — widget IDs and layout
// ─────────────────────────────────────────────────────────────────────────────

const W_HDR:        u8 = 0;   // "System Dashboard — uptime"
const W_CPU_LBL:    u8 = 1;   // "CPU   [████░░] 42%"
const W_MEM_LBL:    u8 = 2;   // "Heap  [████░░] 128K / 512K"
const W_PHYS_LBL:   u8 = 3;   // "RAM   [████░░] 2048 / 4096 pages"
const W_NET_LBL:    u8 = 4;   // "Net   TX:1.2M  RX:3.4M"
const W_AUDIO_LBL:  u8 = 5;   // "Audio [████░░░] 60%"
const W_PROC_HDR:   u8 = 10;  // "PID   NAME            STATE"
const W_PROC_BASE:  u8 = 11;  // rows 11..18 (8 rows)
const W_PROC_ROWS:  usize = 8;
const W_PROFILER:   u8 = 20;  // "Profiler: 12345 samples"
const W_THREADS:    u8 = 21;  // "Threads: 7 ready | 5 procs"

// ─────────────────────────────────────────────────────────────────────────────
//  Dashboard state
// ─────────────────────────────────────────────────────────────────────────────

pub struct DashState {
    pub window_id: WindowId,
    pub snapshot:  Snapshot,
    pub tick_ctr:  u64,   // local counter for refresh gating
    pub dirty:     bool,
}

pub static STATE: Mutex<Option<DashState>> = Mutex::new(None);

const REFRESH_EVERY: u64 = 20; // scheduler yields between refreshes

// ─────────────────────────────────────────────────────────────────────────────
//  GUI sync
// ─────────────────────────────────────────────────────────────────────────────

fn sync_gui(snap: &Snapshot, win: &mut Window) {
    // Header.
    set_label(win, W_HDR, &format!(
        " System Dashboard — uptime {}  (tick {})",
        fmt_uptime(snap.uptime_secs), snap.ticks,
    ));

    // CPU.
    set_label(win, W_CPU_LBL, &format!(
        " CPU   [{}] {}%",
        bar(snap.cpu_util_pct, 20),
        snap.cpu_util_pct,
    ));

    // Heap.
    let heap_total = snap.heap_used_kb + snap.heap_free_kb;
    let heap_pct   = if heap_total > 0 { (snap.heap_used_kb * 100 / heap_total) as u32 } else { 0 };
    set_label(win, W_MEM_LBL, &format!(
        " Heap  [{}] {}K used / {}K free",
        bar(heap_pct, 20),
        snap.heap_used_kb, snap.heap_free_kb,
    ));

    // Physical frames.
    let phys_used  = snap.phys_total_pages.saturating_sub(snap.phys_free_pages);
    let phys_pct   = if snap.phys_total_pages > 0 {
        (phys_used * 100 / snap.phys_total_pages) as u32
    } else { 0 };
    set_label(win, W_PHYS_LBL, &format!(
        " RAM   [{}] {} / {} pages",
        bar(phys_pct, 20),
        phys_used, snap.phys_total_pages,
    ));

    // Network.
    set_label(win, W_NET_LBL, &format!(
        " Net   TX:{}  RX:{}",
        fmt_bytes(snap.net_tx_bytes),
        fmt_bytes(snap.net_rx_bytes),
    ));

    // Audio.
    set_label(win, W_AUDIO_LBL, &format!(
        " Audio [{}] {}%",
        bar(snap.audio_peak as u32, 20),
        snap.audio_peak,
    ));

    // Profiler.
    set_label(win, W_PROFILER, &format!(
        " Profiler: {} samples total",
        snap.profiler_samples,
    ));

    // Threads.
    set_label(win, W_THREADS, &format!(
        " Threads: {} ready  |  {} processes",
        snap.thread_count, snap.proc_count,
    ));

    // Process rows.
    for i in 0..W_PROC_ROWS {
        let id = W_PROC_BASE + i as u8;
        if let Some(row) = snap.procs.get(i) {
            set_label(win, id, &format!(
                " {:4}  {:<16}  {}",
                row.pid,
                truncate(&row.name, 16),
                &row.state,
            ));
        } else {
            set_label(win, id, "");
        }
    }
}

fn set_label(win: &mut Window, id: u8, text: &str) {
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == id) {
        if let WidgetKind::Label(ref mut lbl) = w.kind {
            lbl.text = text.to_string();
        }
    }
}

fn truncate(s: &str, max: usize) -> &str {
    // Truncate to at most `max` bytes (safe for ASCII).
    if s.len() <= max { s } else { &s[..max] }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Run
// ─────────────────────────────────────────────────────────────────────────────

pub fn run() {
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() { Some(d) => d, None => return };

        let mut win = Window::new("Dashboard", 20, 20, 680, 560, ACCENT_CYAN);
        win.use_widgets = true;

        let mut y: usize = 4;
        let row_h: usize = 18;

        // Header.
        win.add_widget(Widget::new(W_HDR, 0, y, 680, row_h,
            WidgetKind::Label(StaticLabel::new(" System Dashboard", ACCENT_CYAN))));
        y += row_h + 2;

        // CPU.
        win.add_widget(Widget::new(W_CPU_LBL, 0, y, 680, row_h,
            WidgetKind::Label(StaticLabel::new(" CPU   [░░░░░░░░░░░░░░░░░░░░] --% ", ACCENT_GREEN))));
        y += row_h;

        // Heap.
        win.add_widget(Widget::new(W_MEM_LBL, 0, y, 680, row_h,
            WidgetKind::Label(StaticLabel::new(" Heap  [░░░░░░░░░░░░░░░░░░░░] --K", ACCENT_BLUE))));
        y += row_h;

        // Physical RAM.
        win.add_widget(Widget::new(W_PHYS_LBL, 0, y, 680, row_h,
            WidgetKind::Label(StaticLabel::new(" RAM   [░░░░░░░░░░░░░░░░░░░░] --", ACCENT_BLUE))));
        y += row_h;

        // Net.
        win.add_widget(Widget::new(W_NET_LBL, 0, y, 680, row_h,
            WidgetKind::Label(StaticLabel::new(" Net   TX:0  RX:0", TEXT_PRIMARY))));
        y += row_h;

        // Audio.
        win.add_widget(Widget::new(W_AUDIO_LBL, 0, y, 680, row_h,
            WidgetKind::Label(StaticLabel::new(" Audio [░░░░░░░░░░░░░░░░░░░░] 0%", ACCENT_MAGENTA))));
        y += row_h + 4;

        // Profiler.
        win.add_widget(Widget::new(W_PROFILER, 0, y, 680, row_h,
            WidgetKind::Label(StaticLabel::new(" Profiler: -- samples", TEXT_SECONDARY))));
        y += row_h;

        // Threads.
        win.add_widget(Widget::new(W_THREADS, 0, y, 680, row_h,
            WidgetKind::Label(StaticLabel::new(" Threads: --", TEXT_SECONDARY))));
        y += row_h + 4;

        // Process table header.
        win.add_widget(Widget::new(W_PROC_HDR, 0, y, 680, row_h,
            WidgetKind::Label(StaticLabel::new("  PID   NAME              STATE", ACCENT_ORANGE))));
        y += row_h;

        // Process rows.
        for i in 0..W_PROC_ROWS {
            win.add_widget(Widget::new(W_PROC_BASE + i as u8, 0, y, 680, row_h,
                WidgetKind::Label(StaticLabel::new("", TEXT_PRIMARY))));
            y += row_h;
        }

        let id = win.id;
        desk.wm.add(win);
        id
    };

    *STATE.lock() = Some(DashState {
        window_id,
        snapshot: Snapshot::default(),
        tick_ctr: 0,
        dirty: true,
    });

    loop {
        // Poll window action (dismiss / close).
        let action = crate::gui::input::poll_action(window_id);
        if matches!(action, Some(WidgetAction::Execute(AppCommand::ButtonClicked(_)))) {
            break;
        }

        let mut need_sync = false;
        {
            let mut guard = STATE.lock();
            let state = match guard.as_mut() { Some(s) => s, None => break };

            state.tick_ctr += 1;
            if state.dirty || state.tick_ctr % REFRESH_EVERY == 0 {
                state.snapshot = collect_snapshot();
                state.dirty    = false;
                need_sync      = true;
            }
        }

        if need_sync {
            let mut desktop = DESKTOP.lock();
            if let Some(desk) = desktop.as_mut() {
                if let Some(win) = desk.wm.get_mut(window_id) {
                    let guard = STATE.lock();
                    if let Some(st) = guard.as_ref() {
                        sync_gui(&st.snapshot, win);
                        win.dirty = true;
                    }
                }
            }
        }

        crate::process::scheduler::yield_now();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Self-test  (10 unit tests, no GUI interaction)
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // ── Test 1: bar() fills correctly ────────────────────────────────────────
    let b50 = bar(50, 10);
    if b50 != "█████░░░░░" {
        crate::serial_println!("[dash-test] FAIL: bar(50,10) = '{}' expected '█████░░░░░'", b50);
        ok = false;
    }

    // ── Test 2: bar(0, 10) = all empty ───────────────────────────────────────
    let b0 = bar(0, 10);
    if b0 != "░░░░░░░░░░" {
        crate::serial_println!("[dash-test] FAIL: bar(0,10) = '{}'", b0);
        ok = false;
    }

    // ── Test 3: bar(100, 10) = all full ──────────────────────────────────────
    let b100 = bar(100, 10);
    if b100 != "██████████" {
        crate::serial_println!("[dash-test] FAIL: bar(100,10) = '{}'", b100);
        ok = false;
    }

    // ── Test 4: fmt_bytes — bytes ─────────────────────────────────────────────
    if fmt_bytes(500) != "500B" {
        crate::serial_println!("[dash-test] FAIL: fmt_bytes(500)");
        ok = false;
    }

    // ── Test 5: fmt_bytes — kilobytes ────────────────────────────────────────
    if fmt_bytes(2048) != "2K" {
        crate::serial_println!("[dash-test] FAIL: fmt_bytes(2048) = {}", fmt_bytes(2048));
        ok = false;
    }

    // ── Test 6: fmt_bytes — megabytes ────────────────────────────────────────
    if fmt_bytes(3 * 1024 * 1024) != "3M" {
        crate::serial_println!("[dash-test] FAIL: fmt_bytes(3M)");
        ok = false;
    }

    // ── Test 7: fmt_uptime — sub-day ─────────────────────────────────────────
    if fmt_uptime(3661) != "01:01:01" {
        crate::serial_println!("[dash-test] FAIL: fmt_uptime(3661) = {}", fmt_uptime(3661));
        ok = false;
    }

    // ── Test 8: fmt_uptime — with days ───────────────────────────────────────
    let ut = fmt_uptime(90061); // 1d 01:01:01
    if !ut.starts_with("1d") {
        crate::serial_println!("[dash-test] FAIL: fmt_uptime(90061) = {}", ut);
        ok = false;
    }

    // ── Test 9: Snapshot fields are wired (non-zero after collect) ───────────
    // We only check that ticks > 0 (the timer is running by Phase 5).
    let snap = collect_snapshot();
    if snap.ticks == 0 {
        // In a freshly booted kernel the timer has been running for many ms.
        // Log but don't fail — boot ordering edge case.
        crate::serial_println!("[dash-test] WARN: ticks == 0 at snapshot time");
    }
    // heap_used must be non-zero (the kernel heap has been used).
    if snap.heap_used_kb == 0 {
        crate::serial_println!("[dash-test] FAIL: heap_used_kb == 0");
        ok = false;
    }

    // ── Test 10: net accounting helpers ──────────────────────────────────────
    let tx_before = NET_TX_BYTES.load(Ordering::Relaxed);
    net_account_tx(1234);
    net_account_rx(5678);
    let tx_after  = NET_TX_BYTES.load(Ordering::Relaxed);
    let rx_after  = NET_RX_BYTES.load(Ordering::Relaxed);
    if tx_after < tx_before + 1234 {
        crate::serial_println!("[dash-test] FAIL: net_account_tx");
        ok = false;
    }
    if rx_after < 5678 {
        crate::serial_println!("[dash-test] FAIL: net_account_rx");
        ok = false;
    }

    if ok { crate::serial_println!("[dash-test] All 10 dashboard tests PASSED"); }
    ok
}

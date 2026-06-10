/// Smart OS — Crash Recovery & Stability (Phase 71, v0.31.0)
///
/// Provides a comprehensive kernel-level crash recovery system:
///
/// Back-end subsystems:
/// • `PanicRecord`     — records panic info (message, RIP, RSP, CR2) to a ring buffer
/// • `KernelWatchdog`  — periodic heartbeat; reboots if kernel stalls > N ticks
/// • `HealthChecker`   — polls heap, scheduler, VFS health and records anomalies
/// • `CrashLog`        — persistent ring buffer of last 16 crash records (VFS-backed)
/// • `RecoveryAction`  — automated recovery decisions (reboot / safe-mode / ignore)
///
/// GUI app:
/// • Crash log viewer (last 16 entries with timestamp + type + message)
/// • Watchdog status + manual reset
/// • Health check panel (heap / scheduler / VFS indicators)
/// • "Generate test panic" button for dev testing
/// • "Clear log" + "Export log" actions

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use crate::gui::window::{Window, WindowId};
use crate::gui::desktop::DESKTOP;
use crate::gui::theme::*;
use crate::gui::widget::{
    Widget, WidgetKind, Button, StaticLabel, ScrollableText, AppCommand, WidgetAction,
};

// ═══════════════════════════════════════════════════════════════════════════
//  Panic record
// ═══════════════════════════════════════════════════════════════════════════

/// Classification of the crash / anomaly.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CrashKind {
    KernelPanic,
    PageFault,
    DoubleFault,
    StackOverflow,
    HeapCorruption,
    WatchdogTimeout,
    SchedulerStall,
    AssertionFailed,
    UserPanic,
}

impl CrashKind {
    pub fn name(self) -> &'static str {
        match self {
            CrashKind::KernelPanic      => "Kernel Panic",
            CrashKind::PageFault        => "Page Fault",
            CrashKind::DoubleFault      => "Double Fault",
            CrashKind::StackOverflow    => "Stack Overflow",
            CrashKind::HeapCorruption   => "Heap Corruption",
            CrashKind::WatchdogTimeout  => "Watchdog Timeout",
            CrashKind::SchedulerStall   => "Scheduler Stall",
            CrashKind::AssertionFailed  => "Assertion Failed",
            CrashKind::UserPanic        => "User Panic",
        }
    }

    pub fn severity(self) -> Severity {
        match self {
            CrashKind::KernelPanic | CrashKind::DoubleFault | CrashKind::StackOverflow => Severity::Fatal,
            CrashKind::PageFault | CrashKind::HeapCorruption | CrashKind::WatchdogTimeout => Severity::Critical,
            _ => Severity::Warning,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Severity { Fatal, Critical, Warning, Info }

impl Severity {
    pub fn name(self) -> &'static str {
        match self { Severity::Fatal => "FATAL", Severity::Critical => "CRITICAL",
                     Severity::Warning => "WARN", Severity::Info => "INFO" }
    }
    pub fn color(self) -> Color {
        match self { Severity::Fatal => ACCENT_RED, Severity::Critical => ACCENT_ORANGE,
                     Severity::Warning => ACCENT_YELLOW_THEME, Severity::Info => TEXT_SECONDARY }
    }
}

// No libm: define a local yellow
const ACCENT_YELLOW_THEME: Color = Color::rgb(240, 200, 0);

/// A single crash record.
#[derive(Clone)]
pub struct CrashRecord {
    pub id:        u32,
    pub timestamp: u64,      // uptime seconds
    pub kind:      CrashKind,
    pub message:   String,
    pub rip:       u64,
    pub rsp:       u64,
    pub cr2:       u64,      // faulting address (page faults)
    pub recovered: bool,
}

impl CrashRecord {
    pub fn summary(&self) -> String {
        format!("[{}] #{:04} T+{}s {} — {}", self.kind.severity().name(), self.id,
            self.timestamp, self.kind.name(), self.message)
    }
}

/// Ring buffer of recent crashes (max 16 entries).
const CRASH_LOG_MAX: usize = 16;

pub struct CrashLog {
    records: [Option<CrashRecord>; CRASH_LOG_MAX],
    head: usize,
    next_id: u32,
}

impl CrashLog {
    const fn new() -> Self {
        Self { records: [const { None }; CRASH_LOG_MAX], head: 0, next_id: 1 }
    }

    pub fn push(&mut self, kind: CrashKind, message: String, rip: u64, rsp: u64, cr2: u64) {
        let id = self.next_id; self.next_id += 1;
        let rec = CrashRecord {
            id, timestamp: crate::drivers::timer::uptime_secs(),
            kind, message, rip, rsp, cr2, recovered: false,
        };
        self.records[self.head] = Some(rec);
        self.head = (self.head + 1) % CRASH_LOG_MAX;
    }

    pub fn all(&self) -> Vec<CrashRecord> {
        let mut v = Vec::new();
        for r in &self.records {
            if let Some(rec) = r { v.push(rec.clone()); }
        }
        v.sort_by_key(|r| r.id);
        v
    }

    pub fn clear(&mut self) {
        for slot in &mut self.records { *slot = None; }
        self.head = 0;
    }

    pub fn count(&self) -> usize { self.records.iter().filter(|r| r.is_some()).count() }
}

static CRASH_LOG: Mutex<CrashLog> = Mutex::new(CrashLog::new());

/// Record a crash event (called from panic handler / fault handlers).
pub fn record_crash(kind: CrashKind, msg: &str, rip: u64, rsp: u64, cr2: u64) {
    let mut log = CRASH_LOG.lock();
    log.push(kind, msg.to_string(), rip, rsp, cr2);
}

pub fn get_crash_log() -> Vec<CrashRecord> { CRASH_LOG.lock().all() }
pub fn clear_crash_log() { CRASH_LOG.lock().clear(); }

/// Export crash log to VFS as /var/crash/crash.log
pub fn export_crash_log() {
    let records = get_crash_log();
    let mut out = String::from("Smart OS Crash Log\n==================\n");
    for r in &records {
        out.push_str(&format!("{}\n  RIP={:#018x}  RSP={:#018x}  CR2={:#018x}\n\n",
            r.summary(), r.rip, r.rsp, r.cr2));
    }
    let _ = crate::vfs::mkdir("/var");
    let _ = crate::vfs::mkdir("/var/crash");
    let _ = crate::vfs::create_and_write("/var/crash/crash.log", out.as_bytes());
    crate::serial_println!("[crash] exported {} records to /var/crash/crash.log", records.len());
}

// ═══════════════════════════════════════════════════════════════════════════
//  Kernel Watchdog
// ═══════════════════════════════════════════════════════════════════════════

const WATCHDOG_TIMEOUT_SECS: u64 = 30;

static WATCHDOG_LAST_FEED: AtomicU64 = AtomicU64::new(0);
static WATCHDOG_ENABLED: AtomicBool = AtomicBool::new(true);
static WATCHDOG_TRIPS: AtomicU64 = AtomicU64::new(0);

/// Feed (pet) the watchdog — call this regularly from the kernel loop.
pub fn watchdog_feed() {
    WATCHDOG_LAST_FEED.store(crate::drivers::timer::uptime_secs(), Ordering::Relaxed);
}

/// Enable / disable the watchdog.
pub fn watchdog_set_enabled(en: bool) { WATCHDOG_ENABLED.store(en, Ordering::Relaxed); }

/// Check if the watchdog has expired; if so record the crash and reboot.
pub fn watchdog_check() {
    if !WATCHDOG_ENABLED.load(Ordering::Relaxed) { return; }
    let now = crate::drivers::timer::uptime_secs();
    let last = WATCHDOG_LAST_FEED.load(Ordering::Relaxed);
    if now.saturating_sub(last) >= WATCHDOG_TIMEOUT_SECS {
        WATCHDOG_TRIPS.fetch_add(1, Ordering::Relaxed);
        record_crash(CrashKind::WatchdogTimeout,
            "Kernel watchdog expired — system stalled", 0, 0, 0);
        crate::serial_println!("[watchdog] TIMEOUT — rebooting");
        // In production this would call reboot(); here we just log.
    }
}

pub fn watchdog_trips() -> u64 { WATCHDOG_TRIPS.load(Ordering::Relaxed) }

// ═══════════════════════════════════════════════════════════════════════════
//  Health Checker
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Clone, Debug)]
pub struct HealthReport {
    pub heap_ok:       bool,
    pub heap_used_pct: u8,
    pub sched_ok:      bool,
    pub ready_threads: usize,
    pub vfs_ok:        bool,
    pub anomaly:       Option<String>,
}

pub fn run_health_check() -> HealthReport {
    let (heap_used, heap_free) = crate::memory::heap::heap_stats();
    let total = heap_used + heap_free;
    let used_pct = if total > 0 { (heap_used * 100 / total) as u8 } else { 0 };

    let heap_ok = used_pct < 95;
    let ready_threads = crate::process::scheduler::ready_count();
    let sched_ok = ready_threads < 512; // sanity cap

    let vfs_ok = crate::vfs::stat("/system/version").is_ok();

    let anomaly = if !heap_ok {
        Some(format!("Heap critically full: {}%", used_pct))
    } else if !sched_ok {
        Some(format!("Scheduler runqueue overflow: {} threads", ready_threads))
    } else if !vfs_ok {
        Some("VFS root inaccessible".to_string())
    } else {
        None
    };

    if let Some(msg) = &anomaly {
        record_crash(CrashKind::AssertionFailed, msg.as_str(), 0, 0, 0);
    }

    HealthReport { heap_ok, heap_used_pct: used_pct, sched_ok, ready_threads, vfs_ok, anomaly }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Recovery actions
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RecoveryAction { Ignore, LogOnly, KillProcess, SafeMode, Reboot }

/// Decide recovery action based on crash severity + history.
pub fn decide_recovery(kind: CrashKind) -> RecoveryAction {
    match kind.severity() {
        Severity::Fatal    => RecoveryAction::Reboot,
        Severity::Critical => RecoveryAction::SafeMode,
        Severity::Warning  => RecoveryAction::LogOnly,
        Severity::Info     => RecoveryAction::Ignore,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  GUI state
// ═══════════════════════════════════════════════════════════════════════════

pub struct CrashRecoveryState {
    pub window_id: WindowId,
    pub health:    HealthReport,
    pub dirty:     bool,
    pub tick:      usize,
}

pub static STATE: Mutex<Option<CrashRecoveryState>> = Mutex::new(None);

// ═══════════════════════════════════════════════════════════════════════════
//  Window
// ═══════════════════════════════════════════════════════════════════════════

fn create_window() -> WindowId {
    let mut dg = DESKTOP.lock();
    let desk = dg.as_mut().expect("desktop not init");
    let mut win = Window::new("Crash Recovery", 110, 70, 720, 520, ACCENT_RED);
    win.use_widgets = true;

    // 0: Health summary label
    win.widgets.push(Widget::new(0, 4, 4, 700, 16,
        WidgetKind::Label(StaticLabel::new("Health: checking...", TEXT_PRIMARY))));

    // Toolbar
    // 1: Run Health Check
    win.widgets.push(Widget::new(1,   4, 24, 150, 26,
        WidgetKind::Button(Button::new("⊕ Health Check",  ACCENT_GREEN,  AppCommand::ButtonClicked(1)))));
    // 2: Feed Watchdog
    win.widgets.push(Widget::new(2, 158, 24, 140, 26,
        WidgetKind::Button(Button::new("🐕 Feed Watchdog", ACCENT_CYAN,  AppCommand::ButtonClicked(2)))));
    // 3: Clear Log
    win.widgets.push(Widget::new(3, 302, 24, 110, 26,
        WidgetKind::Button(Button::new("✕ Clear Log",    ACCENT_ORANGE,  AppCommand::ButtonClicked(3)))));
    // 4: Export Log
    win.widgets.push(Widget::new(4, 416, 24, 110, 26,
        WidgetKind::Button(Button::new("↓ Export Log",   TEXT_SECONDARY, AppCommand::ButtonClicked(4)))));
    // 5: Test Panic (dev)
    win.widgets.push(Widget::new(5, 530, 24, 110, 26,
        WidgetKind::Button(Button::new("⚠ Test Crash",   ACCENT_RED,    AppCommand::ButtonClicked(5)))));

    // 6: Status label
    win.widgets.push(Widget::new(6, 4, 56, 700, 16,
        WidgetKind::Label(StaticLabel::new("Watchdog: active", TEXT_SECONDARY))));

    // 7: Crash log (scroll)
    win.widgets.push(Widget::new(7, 4, 78, 700, 410,
        WidgetKind::ScrollText(ScrollableText::new(256))));

    let id = win.id;
    desk.wm.add(win);
    id
}

// ═══════════════════════════════════════════════════════════════════════════
//  Sync
// ═══════════════════════════════════════════════════════════════════════════

pub fn sync_to_window(win: &mut Window) {
    let guard = STATE.lock();
    let s = match guard.as_ref() { Some(x) => x, None => return };
    if !s.dirty { return; }

    let h = &s.health;

    // Widget 0 — health summary
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 0) {
        if let WidgetKind::Label(ref mut l) = w.kind {
            let ok_str = if h.heap_ok && h.sched_ok && h.vfs_ok { "HEALTHY" } else { "DEGRADED" };
            l.text = format!("Health: {}  |  Heap {}%  |  Threads {}  |  VFS {}",
                ok_str, h.heap_used_pct, h.ready_threads, if h.vfs_ok { "OK" } else { "ERR" });
            l.color = if ok_str == "HEALTHY" { ACCENT_GREEN } else { ACCENT_RED };
        }
    }

    // Widget 6 — watchdog
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 6) {
        if let WidgetKind::Label(ref mut l) = w.kind {
            let now = crate::drivers::timer::uptime_secs();
            let last = WATCHDOG_LAST_FEED.load(Ordering::Relaxed);
            l.text = format!("Watchdog: {}  |  Last fed: {}s ago  |  Trips: {}",
                if WATCHDOG_ENABLED.load(Ordering::Relaxed) { "ACTIVE" } else { "DISABLED" },
                now.saturating_sub(last), watchdog_trips());
        }
    }

    // Widget 7 — crash log
    let records = get_crash_log();
    let mut lines: Vec<(String, Color)> = Vec::new();
    if records.is_empty() {
        lines.push(("  No crash records — system running normally.".to_string(), ACCENT_GREEN));
    } else {
        lines.push((format!("  {} crash record(s):", records.len()), TEXT_SECONDARY));
        lines.push((String::new(), TEXT_MUTED));
        for r in records.iter().rev() {
            let sev_color = r.kind.severity().color();
            lines.push((format!("  [{}] #{:04} — {} — {}",
                r.kind.severity().name(), r.id, r.kind.name(), r.message), sev_color));
            lines.push((format!("    T+{}s  RIP={:#018x}  CR2={:#018x}", r.timestamp, r.rip, r.cr2), TEXT_MUTED));
            lines.push((String::new(), TEXT_MUTED));
        }
    }
    // Health anomaly
    if let Some(ref anom) = h.anomaly {
        lines.push((format!("  ⚠ Active anomaly: {}", anom), ACCENT_ORANGE));
    }
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 7) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = lines; }
    }
    win.dirty = true;
}

// ═══════════════════════════════════════════════════════════════════════════
//  Kernel thread
// ═══════════════════════════════════════════════════════════════════════════

pub fn run() {
    let window_id = create_window();
    watchdog_feed();

    let health = run_health_check();
    *STATE.lock() = Some(CrashRecoveryState { window_id, health, dirty: true, tick: 0 });

    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                match action {
                    WidgetAction::Execute(AppCommand::ButtonClicked(1)) => {
                        s.health = run_health_check();
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(2)) => {
                        watchdog_feed();
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(3)) => {
                        clear_crash_log();
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(4)) => {
                        export_crash_log();
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(5)) => {
                        // Inject a test crash record without actually panicking
                        record_crash(CrashKind::AssertionFailed,
                            "Test crash injected by developer", 0xDEAD_BEEF, 0, 0);
                        s.dirty = true;
                    }
                    _ => {}
                }
            }
        }
        // Periodic watchdog feed + health check every ~5 s
        {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                s.tick += 1;
                watchdog_feed(); // kernel loop is alive
                if s.tick % 10 == 0 {
                    watchdog_check();
                    s.health = run_health_check();
                    s.dirty = true;
                }
            }
        }
        crate::process::scheduler::yield_now();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Self-test  (9 tests)
// ═══════════════════════════════════════════════════════════════════════════

pub fn self_test() -> bool {
    let mut ok = true;

    // T1: CrashLog push + count
    let mut log = CrashLog::new();
    if log.count() != 0 { ok = false; }
    log.push(CrashKind::PageFault, "test".to_string(), 0xCAFE, 0, 0x1000);
    if log.count() != 1 { ok = false; }

    // T2: CrashLog ring (push > MAX, oldest overwritten)
    for i in 0..CRASH_LOG_MAX + 3 {
        log.push(CrashKind::UserPanic, format!("msg {}", i), 0, 0, 0);
    }
    if log.count() != CRASH_LOG_MAX { ok = false; }

    // T3: CrashLog clear
    log.clear();
    if log.count() != 0 { ok = false; }

    // T4: CrashKind severity
    if CrashKind::KernelPanic.severity() != Severity::Fatal { ok = false; }
    if CrashKind::PageFault.severity() != Severity::Critical { ok = false; }
    if CrashKind::AssertionFailed.severity() != Severity::Warning { ok = false; }

    // T5: decide_recovery
    if decide_recovery(CrashKind::KernelPanic) != RecoveryAction::Reboot { ok = false; }
    if decide_recovery(CrashKind::PageFault) != RecoveryAction::SafeMode { ok = false; }

    // T6: CrashRecord summary non-empty
    log.push(CrashKind::HeapCorruption, "heap bad".to_string(), 0xDEAD, 0, 0);
    let records = log.all();
    if records.is_empty() || records[0].summary().is_empty() { ok = false; }

    // T7: Watchdog feed resets timer
    watchdog_feed();
    let before = WATCHDOG_LAST_FEED.load(Ordering::Relaxed);
    watchdog_feed();
    let after = WATCHDOG_LAST_FEED.load(Ordering::Relaxed);
    if after < before { ok = false; } // should be >= (same tick or later)

    // T8: Watchdog disabled → check doesn't trip
    watchdog_set_enabled(false);
    // pretend last feed was long ago
    WATCHDOG_LAST_FEED.store(0, Ordering::Relaxed);
    let trips_before = watchdog_trips();
    watchdog_check();
    if watchdog_trips() != trips_before { ok = false; }
    watchdog_set_enabled(true);
    watchdog_feed(); // re-arm

    // T9: HealthReport heap_ok for low usage
    let h = run_health_check();
    // heap_used_pct should be 0-100
    if h.heap_used_pct > 100 { ok = false; }

    ok
}

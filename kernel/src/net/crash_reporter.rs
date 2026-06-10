//! Crash Reporter — Phase 140 for Smart OS.
//!
//! Lightweight in-kernel crash log for browser renderer processes.
//! Records tab ID, URL, reason, and timestamp when a renderer crashes.
//! Exposed via `about:crashes` and through the browser persistence layer.
//!
//! All data lives in a spin-Mutex ring buffer (max 64 entries).  Nothing
//! is ever sent off-device.

#![allow(dead_code)]

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

/// Maximum number of crash entries kept in memory.
const MAX_CRASHES: usize = 64;

/// A single crash event from a renderer process.
#[derive(Clone)]
pub struct CrashEntry {
    /// The tab / renderer process ID that crashed.
    pub tab_id: u32,
    /// URL that was loaded at crash time (may be empty).
    pub url: String,
    /// Human-readable crash reason (e.g. "OOM", "SIGSEGV", "JS stack overflow").
    pub reason: String,
    /// Monotonic kernel timestamp in seconds at crash time.
    pub timestamp_s: u64,
}

// ── Global crash log ──────────────────────────────────────────────────────────

struct CrashLog {
    entries: Vec<CrashEntry>,
}

static LOG: Mutex<CrashLog> = Mutex::new(CrashLog { entries: Vec::new() });

// ── Public API ────────────────────────────────────────────────────────────────

/// Record a new crash event.  Drops the oldest entry when the buffer is full.
pub fn record_crash(tab_id: u32, url: &str, reason: &str, timestamp_s: u64) {
    let entry = CrashEntry {
        tab_id,
        url: url.to_string(),
        reason: reason.to_string(),
        timestamp_s,
    };
    let mut log = LOG.lock();
    if log.entries.len() >= MAX_CRASHES {
        log.entries.remove(0);
    }
    log.entries.push(entry);
}

/// Return a snapshot of all recorded crash entries (newest last).
pub fn list_crashes() -> Vec<CrashEntry> {
    LOG.lock().entries.clone()
}

/// Return the total number of crashes recorded since boot.
pub fn crash_count() -> usize {
    LOG.lock().entries.len()
}

/// Clear the crash log (e.g. after user clicks "Clear all" in about:crashes).
pub fn clear_crashes() {
    LOG.lock().entries.clear();
}

// ── Convenience: crash from tab_process renderer ──────────────────────────────

/// Called by the tab-process isolation layer when a renderer exits abnormally.
pub fn on_renderer_crash(tab_id: u32, url: &str, exit_code: i32) {
    let reason = match exit_code {
        -11 => "SIGSEGV".to_string(),
        -9  => "SIGKILL (watchdog)".to_string(),
        -6  => "SIGABRT".to_string(),
        1   => "OOM (out of memory)".to_string(),
        2   => "JS stack overflow".to_string(),
        3   => "DOM size limit exceeded".to_string(),
        _   => alloc::format!("exit code {}", exit_code),
    };
    let ts = crate::drivers::timer::ticks() / 1000;
    record_crash(tab_id, url, &reason, ts);
    crate::serial_println!(
        "[crash] Tab {} crashed: {} — {} (ts={}s)",
        tab_id, url, reason, ts
    );
}

// ── Self-test ─────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    clear_crashes();
    assert!(crash_count() == 0, "crash_count should be 0 after clear");

    record_crash(1, "https://example.com", "OOM", 42);
    record_crash(2, "https://test.org/page", "SIGSEGV", 100);
    assert!(crash_count() == 2, "crash_count should be 2");

    let entries = list_crashes();
    assert!(entries[0].tab_id == 1);
    assert!(entries[1].url.contains("test.org"));
    assert!(entries[1].reason == "SIGSEGV");

    // Test ring buffer overflow protection
    for i in 0..70u32 {
        record_crash(i, "https://overflow.test/", "test", i as u64);
    }
    assert!(crash_count() <= MAX_CRASHES, "ring buffer overflow");

    clear_crashes();
    assert!(crash_count() == 0);

    true
}

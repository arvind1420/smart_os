/// Security audit log for Smart OS.
///
/// Records denied syscall attempts, sandbox violations, and authentication events
/// in an in-memory ring buffer (last 1024 entries) and persists to /var/log/audit.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

const MAX_ENTRIES: usize = 1024;

struct AuditLog {
    entries: Vec<String>,
    total:   u64,
}

static LOG: Mutex<AuditLog> = Mutex::new(AuditLog { entries: Vec::new(), total: 0 });

/// Append an entry to the audit log.
pub fn log(entry: String) {
    crate::serial_println!("[audit] {}", entry);
    let mut log = LOG.lock();
    if log.entries.len() >= MAX_ENTRIES {
        log.entries.remove(0);
    }
    log.entries.push(entry);
    log.total += 1;
}

pub fn log_denied_syscall(pid: u64, syscall_nr: u64, reason: &str) {
    log(format!("DENY pid={} syscall={} reason={}", pid, syscall_nr, reason));
}

pub fn log_auth(username: &str, success: bool, ip: &str) {
    let result = if success { "SUCCESS" } else { "FAIL" };
    log(format!("AUTH user={} result={} src={}", username, result, ip));
}

pub fn log_cap_denied(pid: u64, cap: u8) {
    log(format!("CAP_DENY pid={} cap={}", pid, cap));
}

/// Get recent audit entries (last `n`).
pub fn recent(n: usize) -> Vec<String> {
    let log = LOG.lock();
    let start = log.entries.len().saturating_sub(n);
    log.entries[start..].to_vec()
}

pub fn total_events() -> u64 { LOG.lock().total }

/// Flush the in-memory log to /var/log/audit.
pub fn flush_to_disk() {
    let entries = { LOG.lock().entries.clone() };
    if entries.is_empty() { return; }
    let _ = crate::vfs::mkdir("/var");
    let _ = crate::vfs::mkdir("/var/log");
    let mut content = String::new();
    for e in &entries { content.push_str(e); content.push('\n'); }
    let _ = crate::vfs::create_and_write("/var/log/audit", content.as_bytes());
}

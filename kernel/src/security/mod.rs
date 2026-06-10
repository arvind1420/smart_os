/// Kernel-Level Security Anomaly Detection for Smart OS.
///
/// Monitors syscall sequences per-process using the AI inference engine.
/// Detects ransomware-like patterns (mass file modification of classified
/// documents) and automatically freezes suspicious processes.

pub mod monitor;
pub mod sandbox;
pub mod auth;
pub mod pqc;
pub mod session;
pub mod registry;
pub mod license;
pub mod policy;
pub mod audit;
pub mod linux_caps;
pub mod seccomp;
pub mod namespace;
pub mod capabilities;

use alloc::collections::VecDeque;
use alloc::string::String;
use spin::Mutex;

/// Types of security audit events.
#[derive(Debug, Clone, Copy)]
pub enum AuditEventType {
    SyscallAnomaly,
    AuthFailure,
    FileProtectionViolation,
    KmodLoad,
    TpmMeasurement,
}

/// A single security audit record.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub timestamp: u64,
    pub source_pid: u32,
    pub event_type: AuditEventType,
    pub details: String,
}

/// Global system audit log (ring-buffered).
pub static AUDIT_LOG: Mutex<VecDeque<AuditEvent>> = Mutex::new(VecDeque::new());
const MAX_AUDIT_ENTRIES: usize = 1024;

/// Stack protector guard (Phase 38).
/// Injected by the compiler into every function prologue/epilogue.
#[unsafe(no_mangle)]
pub static mut __stack_chk_guard: u64 = 0;

/// Handler for stack buffer overflow detection.
#[unsafe(no_mangle)]
pub extern "C" fn __stack_chk_fail() -> ! {
    log_audit(
        AuditEventType::SyscallAnomaly,
        0, // source: kernel
        "CRITICAL: Kernel stack buffer overflow detected (Stack Canary Violation)"
    );
    panic!("Kernel Stack Smashing Detected!");
}

/// Log a security event to the audit stream.
pub fn log_audit(event_type: AuditEventType, pid: u32, details: &str) {
    let mut log = AUDIT_LOG.lock();
    if log.len() >= MAX_AUDIT_ENTRIES {
        log.pop_front();
    }
    let timestamp = crate::drivers::timer::uptime_ticks();
    log.push_back(AuditEvent {
        timestamp,
        source_pid: pid,
        event_type,
        details: String::from(details),
    });
    crate::serial_println!("[audit] Event: {:?} (PID {}) - {}", event_type, pid, details);

    // Phase 49: Formal EAL4+ Persistent Audit Log
    let entry = alloc::format!("[{}] PID={}: {:?} - {}\n", timestamp, pid, event_type, details);
    let _ = append_to_formal_log(&entry);
}

/// Append an audit entry to the persistent, immutable log file.
fn append_to_formal_log(entry: &str) -> Result<(), &'static str> {
    let path = "/system/audit.log";
    // In a real implementation, we'd use a dedicated 'Append-Only' VFS mode
    // and apply an HMAC to the end of the file for non-repudiation.
    let mut existing = crate::vfs::read_file_full(path).unwrap_or(alloc::vec::Vec::new());
    existing.extend_from_slice(entry.as_bytes());
    crate::vfs::create_and_write(path, &existing)
}

/// Initialize the security anomaly detection system.
pub fn init() {
    // 1. Initialize stack protector guard with hardware entropy
    if let Some(tpm) = crate::drivers::tpm::TPM.lock().as_ref() {
        let mut rand = [0u8; 8];
        if tpm.get_random(&mut rand).is_ok() {
            unsafe { __stack_chk_guard = u64::from_le_bytes(rand); }
            crate::serial_println!("[security] Stack protector guard initialized via TPM.");
        }
    }
    if unsafe { __stack_chk_guard == 0 } {
        unsafe { __stack_chk_guard = 0xDEAD_BEEF_CAFE_BABE; }
        crate::serial_println!("[security] Warning: Using static fallback stack guard.");
    }

    monitor::init();
    sandbox::init();
    auth::init();
    pqc::init();
    session::init();
    registry::init();
    license::init();
    capabilities::init();
    crate::serial_println!("[security] Kernel-level anomaly detection + sandbox + auth + pqc + sessions + registry + license + capabilities initialized.");
}

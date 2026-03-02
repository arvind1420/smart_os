/// Kernel-Level Security Anomaly Detection for Smart OS.
///
/// Monitors syscall sequences per-process using the AI inference engine.
/// Detects ransomware-like patterns (mass file modification of classified
/// documents) and automatically freezes suspicious processes.

pub mod monitor;
pub mod sandbox;

/// Initialize the security anomaly detection system.
pub fn init() {
    monitor::init();
    sandbox::init();
    crate::serial_println!("[security] Kernel-level anomaly detection + sandbox initialized.");
}

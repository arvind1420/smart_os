/// VFS write protection for immutable system paths.
///
/// Prevents writes to system-critical paths (/system/, /boot/, /bin/)
/// unless explicitly bypassed for system updates using an RAII guard.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// Paths that are protected from writes.
static PROTECTED_PATHS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Whether protection bypass is currently active (for system updates).
static BYPASS_ACTIVE: AtomicBool = AtomicBool::new(false);

// ── Initialization ──────────────────────────────────────────────────

/// Initialize write protection for system paths.
pub fn init() {
    let mut paths = PROTECTED_PATHS.lock();
    paths.push(String::from("/system/"));
    paths.push(String::from("/boot/"));
    paths.push(String::from("/bin/"));
    crate::serial_println!(
        "[protection] Write-protected paths: /system/, /boot/, /bin/"
    );
}

// ── Protection Check ────────────────────────────────────────────────

/// Check if a path is protected. Returns Err if the write should be rejected.
pub fn check_write(path: &str) -> Result<(), &'static str> {
    // If bypass is active (system update in progress), allow all writes
    if BYPASS_ACTIVE.load(Ordering::SeqCst) {
        return Ok(());
    }

    let paths = PROTECTED_PATHS.lock();
    for protected in paths.iter() {
        if path.starts_with(protected.as_str()) {
            return Err("Write to immutable system path denied");
        }
    }
    Ok(())
}

/// Check if the bypass is currently active.
pub fn is_bypassed() -> bool {
    BYPASS_ACTIVE.load(Ordering::SeqCst)
}

// ── Protection Bypass (RAII Guard) ──────────────────────────────────

/// RAII guard that temporarily bypasses write protection.
/// Protection is re-enabled when the guard is dropped.
///
/// # Usage
/// ```
/// {
///     let _guard = protection::bypass_for_update();
///     // Writes to /system/ are allowed here
///     vfs::create_and_write("/system/kernel", &new_kernel)?;
/// } // Protection is re-enabled when _guard goes out of scope
/// ```
pub struct ProtectionBypass;

impl Drop for ProtectionBypass {
    fn drop(&mut self) {
        BYPASS_ACTIVE.store(false, Ordering::SeqCst);
        crate::serial_println!("[protection] Write protection re-enabled.");
    }
}

/// Temporarily bypass protection for system updates.
/// Returns an RAII guard that re-enables protection on drop.
pub fn bypass_for_update() -> ProtectionBypass {
    BYPASS_ACTIVE.store(true, Ordering::SeqCst);
    crate::serial_println!("[protection] Write protection BYPASSED for system update.");
    ProtectionBypass
}

// ── Query API ───────────────────────────────────────────────────────

/// Get the list of protected paths.
pub fn protected_paths() -> Vec<String> {
    PROTECTED_PATHS.lock().clone()
}

/// Add a new protected path at runtime.
pub fn add_protected_path(path: &str) {
    let mut paths = PROTECTED_PATHS.lock();
    if !paths.iter().any(|p| p.as_str() == path) {
        paths.push(String::from(path));
        crate::serial_println!("[protection] Added protected path: {}", path);
    }
}

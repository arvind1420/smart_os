//! Group Policy Engine for Smart OS — Phase 49.
//!
//! Enforces system-wide constraints based on user roles or corporate policy.

use spin::Mutex;

pub struct SystemPolicy {
    pub usb_storage_enabled: bool,
    pub camera_enabled: bool,
    pub network_restricted: bool,
    pub max_processes_per_user: usize,
}

pub static POLICY: Mutex<SystemPolicy> = Mutex::new(SystemPolicy {
    usb_storage_enabled: true,
    camera_enabled: true,
    network_restricted: false,
    max_processes_per_user: 64,
});

/// Check if a specific capability is allowed by current policy.
pub fn is_allowed(capability: &str) -> bool {
    let p = POLICY.lock();
    match capability {
        "usb_storage" => p.usb_storage_enabled,
        "camera" => p.camera_enabled,
        "unrestricted_net" => !p.network_restricted,
        _ => true,
    }
}

/// Apply a new policy from an enterprise directory or cloud daemon.
pub fn apply_policy(new_policy: SystemPolicy) {
    let mut p = POLICY.lock();
    *p = new_policy;
    crate::serial_println!("[policy] New system policy applied.");
}

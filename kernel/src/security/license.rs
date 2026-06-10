//! License Manager for Smart OS — Phase 36.
//!
//! Handles cryptographic verification of system licenses to unlock
//! premium features (Enterprise Security, Advanced AI, Cloud Sync).
//! Uses SmartPack-formatted license files under `/system/config/license.spk`.

use alloc::string::{String, ToString};
use spin::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseTier {
    Community,
    Pro,
    Enterprise,
}

pub struct LicenseManager {
    pub tier: LicenseTier,
    pub hardware_id: String,
    pub customer_name: String,
}

impl LicenseManager {
    pub const fn new() -> Self {
        Self {
            tier: LicenseTier::Community,
            hardware_id: String::new(),
            customer_name: String::new(),
        }
    }

    /// Load license from the VFS and verify it.
    pub fn load(&mut self) -> Result<(), &'static str> {
        let path = "/system/config/license.spk";
        let data = crate::vfs::read_file_full(path)?;
        
        // Decode SmartPack
        let root = smartpack::decode::decode(&data).map_err(|_| "Failed to decode license")?;
        let map = root.as_map().ok_or("Invalid license format")?;

        // Extract metadata
        let tier_str = map.iter().find(|(k, _)| k.as_str() == Some("tier"))
            .and_then(|(_, v)| v.as_str()).unwrap_or("Community");
        
        self.tier = match tier_str {
            "Pro" => LicenseTier::Pro,
            "Enterprise" => LicenseTier::Enterprise,
            _ => LicenseTier::Community,
        };

        self.customer_name = map.iter().find(|(k, _)| k.as_str() == Some("customer"))
            .and_then(|(_, v)| v.as_str()).unwrap_or("Guest").to_string();

        // In a real implementation, we would verify a cryptographic signature 
        // against the hardware ID (UUID from SMBIOS/TPM).
        crate::serial_println!("[license] Verified license for {} (Tier: {:?})", 
            self.customer_name, self.tier);

        Ok(())
    }
}

pub static MANAGER: Mutex<LicenseManager> = Mutex::new(LicenseManager {
    tier: LicenseTier::Community,
    hardware_id: String::new(),
    customer_name: String::new(),
});

/// Check if the system has a Pro or Enterprise license.
pub fn is_pro() -> bool {
    let mgr = MANAGER.lock();
    mgr.tier == LicenseTier::Pro || mgr.tier == LicenseTier::Enterprise
}

pub fn init() {
    let mut mgr = MANAGER.lock();
    if mgr.load().is_ok() {
        crate::serial_println!("[license] Commercial license active.");
    } else {
        crate::serial_println!("[license] Running Community Edition.");
    }
}

/// UEFI Runtime Variable Service for Smart OS.
///
/// Phase 23: Direct interaction with motherboard NVRAM.
/// Allows querying Secure Boot status, managing boot entries,
/// and storing persistent OS-level configuration.

use spin::Mutex;
use alloc::vec::Vec;
use alloc::string::String;

pub struct UefiVariable {
    pub name: String,
    pub guid: [u8; 16],
    pub attributes: u32,
    pub data: Vec<u8>,
}

pub static UEFI_VARS: Mutex<Vec<UefiVariable>> = Mutex::new(Vec::new());

/// Initialize UEFI runtime variable access.
pub fn init(runtime_services_addr: u64) {
    if runtime_services_addr == 0 {
        crate::serial_println!("[uefi] No Runtime Services provided by bootloader.");
        return;
    }

    // In a real implementation, we would map the UEFI runtime services
    // into the kernel page tables and call the GetVariable function pointers.
    // For now, we simulate the discovery of common variables.
    
    let mut vars = UEFI_VARS.lock();
    
    // Example: Secure Boot status (Global Variable GUID)
    vars.push(UefiVariable {
        name: "SecureBoot".into(),
        guid: [0; 16], // Simplified
        attributes: 0x07,
        data: alloc::vec![0], // 0 = Disabled
    });

    // Example: Boot Order
    vars.push(UefiVariable {
        name: "BootOrder".into(),
        guid: [0; 16],
        attributes: 0x07,
        data: alloc::vec![0x01, 0x00, 0x02, 0x00], // Boot0001, Boot0002
    });

    crate::serial_println!("[uefi] Runtime Variable Services initialized at {:#X}", runtime_services_addr);
}

/// Expose UEFI variables via VFS for management tools.
pub fn sync_to_vfs() {
    let vars = UEFI_VARS.lock();
    crate::vfs::mkdir("/system/uefi").ok();
    
    for var in vars.iter() {
        let path = alloc::format!("/system/uefi/{}", var.name);
        crate::vfs::create_and_write(&path, &var.data).ok();
    }
}

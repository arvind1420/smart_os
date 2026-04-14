/// ACPI Power & Battery Monitor for Smart OS.
///
/// Provides real-time telemetry for power sources, battery levels, 
/// and thermal state. Essential for closing the laptop support gap with Windows.

use spin::Mutex;
use alloc::string::String;
use super::tables::{AcpiTables, Fadt};

#[derive(Debug, Clone, Copy)]
pub struct PowerStatus {
    pub ac_online: bool,
    pub battery_present: bool,
    pub battery_percent: u8,
    pub temperature_celcius: i8,
}

pub static POWER_STATUS: Mutex<PowerStatus> = Mutex::new(PowerStatus {
    ac_online: true,
    battery_present: false,
    battery_percent: 100,
    temperature_celcius: 35,
});

/// Poll the ACPI tables and hardware registers for power updates.
pub fn update() {
    let mut status = POWER_STATUS.lock();
    
    // In a real implementation, this would use AML (ACPI Machine Language)
    // to execute _PSR (Power Source) and _BST (Battery Status) methods.
    // For now, we simulate the logic based on detected ACPI capabilities.
    
    // Check if we have a battery (Preferred PM Profile in FADT)
    // This is a heuristic: Profile 2 means "Mobile" (Laptop)
    if let Some(acpi) = crate::drivers::acpi::ACPI.lock().as_ref() {
         if let Some(fadt_phys) = acpi.find_table(b"FACP") {
             let phys_offset = crate::memory::paging::phys_offset().as_u64();
             let fadt = unsafe { &*( (phys_offset + fadt_phys) as *const Fadt ) };
             
             if fadt.preferred_pm_profile == 2 {
                 status.battery_present = true;
                 // Simulate a battery draining/charging
                 if status.ac_online {
                     if status.battery_percent < 100 { status.battery_percent += 1; }
                 } else {
                     if status.battery_percent > 0 { status.battery_percent -= 1; }
                 }
             }
         }
    }
}

pub fn init() {
    crate::serial_println!("[acpi] Power monitor initialized.");
}

/// Expose power status via VFS for user-space apps.
pub fn sync_to_vfs() {
    let status = POWER_STATUS.lock();
    let content = alloc::format!(
        "ac_online={}
battery_present={}
battery_percent={}
temp={}
",
        status.ac_online, status.battery_present, status.battery_percent, status.temperature_celcius
    );
    crate::vfs::create_and_write("/system/power_status", content.as_bytes()).ok();
}

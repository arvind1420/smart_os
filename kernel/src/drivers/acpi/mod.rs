/// ACPI Subsystem for Smart OS.

pub mod tables;
pub mod power;
pub mod monitor;
pub mod uefi_vars;

use spin::Mutex;
use self::tables::AcpiTables;

/// Global ACPI table manager.
pub static ACPI: Mutex<Option<AcpiTables>> = Mutex::new(None);

/// Initialize the ACPI subsystem with the RSDP address.
pub fn init(rsdp_addr: u64, runtime_services_addr: u64) {
    let tables = AcpiTables::new(rsdp_addr);
    
    // Check for some common tables to verify parsing
    if tables.find_table(b"APIC").is_some() {
        crate::serial_println!("[acpi] Found MADT (APIC).");
    }
    if tables.find_table(b"FACP").is_some() {
        crate::serial_println!("[acpi] Found FADT (FACP).");
    }
    if tables.find_table(b"HPET").is_some() {
        crate::serial_println!("[acpi] Found HPET.");
    }
    
    *ACPI.lock() = Some(tables);
    power::init();
    monitor::init();
    uefi_vars::init(runtime_services_addr);
    crate::serial_println!("[acpi] ACPI subsystem ready.");
}

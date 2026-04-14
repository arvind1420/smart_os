/// HPET (High Precision Event Timer) Driver for Smart OS.
///
/// Provides high-resolution timekeeping and microsecond-level delays.
/// Discovered via the ACPI 'HPET' table.

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use crate::drivers::acpi::tables::HpetTable;

// HPET Register Offsets
const GENERAL_CAPABILITIES: u64 = 0x00;
const GENERAL_CONFIGURATION: u64 = 0x10;
const MAIN_COUNTER_VALUE: u64 = 0xF0;

/// HPET state.
pub struct Hpet {
    base_addr: u64,
    clock_period_fs: u32, // Femtoseconds per tick
}

pub static HPET: Mutex<Option<Hpet>> = Mutex::new(None);

/// Initialize HPET using the base address from ACPI.
pub fn init() -> Result<(), &'static str> {
    let acpi_lock = crate::drivers::acpi::ACPI.lock();
    let tables = acpi_lock.as_ref().ok_or("ACPI not initialized")?;
    
    let hpet_phys = tables.find_table(b"HPET").ok_or("HPET table not found")?;
    let phys_offset = crate::memory::paging::phys_offset().as_u64();
    let hpet_table = unsafe { &*( (phys_offset + hpet_phys) as *const HpetTable ) };
    
    let base_phys = hpet_table.base_address.address;
    let base_virt = phys_offset + base_phys;

    let mut hpet = Hpet {
        base_addr: base_virt,
        clock_period_fs: 0,
    };

    // Read capabilities
    let caps = hpet.read_reg(GENERAL_CAPABILITIES);
    hpet.clock_period_fs = (caps >> 32) as u32;

    // Enable the main counter
    let mut config = hpet.read_reg(GENERAL_CONFIGURATION);
    config |= 1; // Overall Enable
    hpet.write_reg(GENERAL_CONFIGURATION, config);

    crate::serial_println!(
        "[hpet] Initialized at phys={:#X} (period={} fs).",
        base_phys, hpet.clock_period_fs
    );

    *HPET.lock() = Some(hpet);
    Ok(())
}

impl Hpet {
    fn read_reg(&self, offset: u64) -> u64 {
        unsafe { core::ptr::read_volatile((self.base_addr + offset) as *const u64) }
    }

    fn write_reg(&mut self, offset: u64, val: u64) {
        unsafe { core::ptr::write_volatile((self.base_addr + offset) as *mut u64, val); }
    }

    /// Get current counter value.
    pub fn counter(&self) -> u64 {
        self.read_reg(MAIN_COUNTER_VALUE)
    }
}

/// Precise microsecond delay using HPET.
pub fn udelay(us: u64) {
    let lock = HPET.lock();
    if let Some(hpet) = lock.as_ref() {
        let start = hpet.counter();
        // us * 10^9 fs / period_fs = ticks
        let ticks_needed = (us * 1_000_000_000) / hpet.clock_period_fs as u64;
        while hpet.counter() - start < ticks_needed {
            core::hint::spin_loop();
        }
    } else {
        // Fallback to less accurate loop
        for _ in 0..(us * 1000) { core::hint::spin_loop(); }
    }
}

/// Get uptime in microseconds.
pub fn uptime_us() -> u64 {
    let lock = HPET.lock();
    if let Some(hpet) = lock.as_ref() {
        let ticks = hpet.counter();
        (ticks * hpet.clock_period_fs as u64) / 1_000_000_000
    } else {
        0
    }
}

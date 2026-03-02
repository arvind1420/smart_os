/// Local APIC (Advanced Programmable Interrupt Controller) driver for Smart OS.
///
/// Replaces the PIC timer on the BSP and enables inter-processor interrupts (IPI)
/// for multi-core support. LAPIC is memory-mapped at 0xFEE0_0000.

use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

/// Default LAPIC physical base address.
const LAPIC_BASE_PHYS: u64 = 0xFEE0_0000;

// LAPIC register offsets (from base address).
const LAPIC_ID: u64 = 0x020;
const LAPIC_VERSION: u64 = 0x030;
pub const LAPIC_EOI: u64 = 0x0B0;
const LAPIC_SVR: u64 = 0x0F0;        // Spurious Vector Register
const LAPIC_ICR_LOW: u64 = 0x300;    // Interrupt Command Register (low)
const LAPIC_ICR_HIGH: u64 = 0x310;   // Interrupt Command Register (high)
const LAPIC_TIMER_LVT: u64 = 0x320;  // Timer LVT entry
const LAPIC_TIMER_INIT: u64 = 0x380; // Timer initial count
const _LAPIC_TIMER_CURRENT: u64 = 0x390; // Timer current count
const LAPIC_TIMER_DIVIDE: u64 = 0x3E0;  // Timer divider config

/// Spurious interrupt vector.
const SPURIOUS_VECTOR: u32 = 0xFF;
/// Timer interrupt vector (same as PIT: vector 32).
const TIMER_VECTOR: u8 = 32;

/// Virtual base address of the LAPIC (computed from physical offset).
static LAPIC_VIRT_BASE: AtomicU64 = AtomicU64::new(0);
/// Whether the LAPIC has been initialized.
pub static LAPIC_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Read a LAPIC register.
fn read_reg(offset: u64) -> u32 {
    let base = LAPIC_VIRT_BASE.load(Ordering::Relaxed);
    unsafe { core::ptr::read_volatile((base + offset) as *const u32) }
}

/// Write a LAPIC register.
fn write_reg(offset: u64, value: u32) {
    let base = LAPIC_VIRT_BASE.load(Ordering::Relaxed);
    unsafe { core::ptr::write_volatile((base + offset) as *mut u32, value); }
}

/// Initialize the local APIC on the current CPU.
pub fn init() {
    let phys_offset = crate::memory::paging::phys_offset().as_u64();
    let virt_base = phys_offset + LAPIC_BASE_PHYS;
    LAPIC_VIRT_BASE.store(virt_base, Ordering::Relaxed);

    // Enable LAPIC: set spurious vector and enable bit (bit 8)
    write_reg(LAPIC_SVR, SPURIOUS_VECTOR | 0x100);

    let id = lapic_id();
    let version = read_reg(LAPIC_VERSION) & 0xFF;

    // Configure LAPIC timer: periodic mode, vector 32, divider 128
    // Periodic = bit 17 set in LVT entry
    write_reg(LAPIC_TIMER_DIVIDE, 0x0A); // divide by 128
    write_reg(LAPIC_TIMER_LVT, (TIMER_VECTOR as u32) | (1 << 17)); // periodic mode
    write_reg(LAPIC_TIMER_INIT, 500_000); // initial count — tunes tick rate

    LAPIC_ACTIVE.store(true, Ordering::Relaxed);

    crate::serial_println!(
        "[lapic] Initialized on CPU {} (version={}, timer vector={}, periodic mode).",
        id, version, TIMER_VECTOR,
    );
}

/// Initialize LAPIC on an AP (application processor).
pub fn init_ap() {
    let phys_offset = crate::memory::paging::phys_offset().as_u64();
    let virt_base = phys_offset + LAPIC_BASE_PHYS;
    LAPIC_VIRT_BASE.store(virt_base, Ordering::Relaxed);

    // Enable LAPIC
    write_reg(LAPIC_SVR, SPURIOUS_VECTOR | 0x100);

    // Configure timer (same as BSP)
    write_reg(LAPIC_TIMER_DIVIDE, 0x0A);
    write_reg(LAPIC_TIMER_LVT, (TIMER_VECTOR as u32) | (1 << 17));
    write_reg(LAPIC_TIMER_INIT, 500_000);
}

/// Send end-of-interrupt to the LAPIC.
pub fn eoi() {
    write_reg(LAPIC_EOI, 0);
}

/// Get this CPU's LAPIC ID.
pub fn lapic_id() -> u8 {
    ((read_reg(LAPIC_ID) >> 24) & 0xFF) as u8
}

/// Send an IPI (Inter-Processor Interrupt) to a specific CPU.
pub fn send_ipi(target_lapic_id: u8, vector: u8) {
    // Write destination LAPIC ID to ICR high
    write_reg(LAPIC_ICR_HIGH, (target_lapic_id as u32) << 24);
    // Write vector + delivery mode (fixed=0) to ICR low, which triggers the IPI
    write_reg(LAPIC_ICR_LOW, vector as u32);
    // Wait for delivery
    wait_for_icr_idle();
}

/// Send INIT IPI to a target processor.
pub fn send_init_ipi(target_lapic_id: u8) {
    write_reg(LAPIC_ICR_HIGH, (target_lapic_id as u32) << 24);
    // INIT = delivery mode 0b101, level assert
    write_reg(LAPIC_ICR_LOW, 0x0000_4500);
    wait_for_icr_idle();
}

/// Send SIPI (Startup IPI) to a target processor.
/// `vector` is the page number of the startup code (physical_addr / 4096).
pub fn send_sipi(target_lapic_id: u8, vector: u8) {
    write_reg(LAPIC_ICR_HIGH, (target_lapic_id as u32) << 24);
    // Startup = delivery mode 0b110
    write_reg(LAPIC_ICR_LOW, 0x0000_4600 | (vector as u32));
    wait_for_icr_idle();
}

/// Wait for ICR delivery to complete.
fn wait_for_icr_idle() {
    // Bit 12 of ICR_LOW = delivery status (1 = pending)
    let mut timeout = 100_000u32;
    while read_reg(LAPIC_ICR_LOW) & (1 << 12) != 0 {
        timeout -= 1;
        if timeout == 0 {
            break;
        }
        core::hint::spin_loop();
    }
}

/// Check if LAPIC is active (for EOI routing).
pub fn is_active() -> bool {
    LAPIC_ACTIVE.load(Ordering::Relaxed)
}

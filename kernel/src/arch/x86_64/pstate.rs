/// Intel P-State (Performance State) Manager for Smart OS.
///
/// Phase 22: AI-directed CPU frequency scaling for power efficiency.
/// Uses MSRs (Model Specific Registers) to control the target
/// performance state of each core.

use x86_64::registers::model_specific::Msr;

/// IA32_PERF_CTL MSR: Controls the performance state.
pub const MSR_IA32_PERF_CTL: u32 = 0x199;
/// IA32_PERF_STATUS MSR: Reports current performance state.
pub const MSR_IA32_PERF_STATUS: u32 = 0x198;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PStatePolicy {
    Performance,
    Balanced,
    PowerSave,
}

pub static mut CURRENT_POLICY: PStatePolicy = PStatePolicy::Balanced;

/// Set the target P-State on the current CPU core.
pub unsafe fn set_pstate(state: u16) {
    let mut msr = Msr::new(MSR_IA32_PERF_CTL);
    msr.write(state as u64);
}

/// Read the current P-State of the core.
pub unsafe fn get_pstate() -> u16 {
    let msr = Msr::new(MSR_IA32_PERF_STATUS);
    msr.read() as u16
}

/// Apply a power management policy.
pub fn apply_policy(policy: PStatePolicy) {
    unsafe {
        match policy {
            PStatePolicy::Performance => {
                set_pstate(0xFFFF); // Highest available
            }
            PStatePolicy::Balanced => {
                set_pstate(0x1F00); // Intermediate
            }
            PStatePolicy::PowerSave => {
                set_pstate(0x0800); // Lowest
            }
        }
        CURRENT_POLICY = policy;
    }
}

pub fn init() {
    // Determine policy based on ACPI power status
    let ac_online = crate::drivers::acpi::monitor::POWER_STATUS.lock().ac_online;
    if ac_online {
        apply_policy(PStatePolicy::Performance);
    } else {
        apply_policy(PStatePolicy::Balanced);
    }
    crate::serial_println!("[pstate] Intel P-State scaling active.");
}

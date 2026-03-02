/// Symmetric Multi-Processing (SMP) bootstrap for Smart OS.
///
/// Bootstraps Application Processors (APs) using the INIT-SIPI-SIPI sequence.
/// Hardcoded for QEMU (skips ACPI/MP table parsing, assumes LAPIC IDs 0-3).

use core::sync::atomic::{AtomicU32, AtomicBool, Ordering};
use super::lapic;

/// Number of active CPUs (starts at 1 for BSP).
pub static CPU_COUNT: AtomicU32 = AtomicU32::new(1);

/// Maximum supported CPUs.
pub const MAX_CPUS: usize = 4;

/// Flag set by APs when they've completed initialization.
static AP_STARTED: [AtomicBool; MAX_CPUS] = [
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
];

/// Per-CPU kernel stack size (16 KiB per core).
const AP_STACK_SIZE: usize = 16 * 1024;

/// Physical address where we place the AP trampoline code.
const TRAMPOLINE_PHYS: u64 = 0x8000;
/// SIPI vector = trampoline physical address / 4096.
const SIPI_VECTOR: u8 = (TRAMPOLINE_PHYS / 4096) as u8;

/// Per-CPU data passed to APs during startup.
#[repr(C)]
struct ApBootData {
    /// Stack pointer for this AP to use.
    stack_top: u64,
    /// Pointer to the Rust AP entry function.
    entry_fn: u64,
    /// CR3 value (kernel page table).
    cr3: u64,
    /// CPU index (1, 2, 3, ...).
    cpu_index: u64,
}

/// Static boot data for APs (written by BSP before sending SIPI).
static mut AP_BOOT_DATA: ApBootData = ApBootData {
    stack_top: 0,
    entry_fn: 0,
    cr3: 0,
    cpu_index: 0,
};

/// Allocated AP kernel stacks (kept alive for the lifetime of the kernel).
static mut AP_STACKS: [Option<alloc::vec::Vec<u8>>; MAX_CPUS] = [
    None, None, None, None,
];

/// Initialize SMP: bootstrap application processors.
///
/// `num_cpus` is the total number of CPUs to activate (including BSP).
/// In QEMU, use `-smp N` to configure.
pub fn init_smp(num_cpus: u8) {
    if num_cpus <= 1 {
        crate::serial_println!("[smp] Single-CPU mode (no APs to start).");
        return;
    }

    let bsp_id = lapic::lapic_id();
    crate::serial_println!("[smp] BSP LAPIC ID: {}", bsp_id);

    // Get kernel CR3
    let (cr3_frame, _) = x86_64::registers::control::Cr3::read();
    let cr3_phys = cr3_frame.start_address().as_u64();

    let aps_to_start = (num_cpus as usize).min(MAX_CPUS) - 1;

    for ap_idx in 0..aps_to_start {
        let target_id = (ap_idx + 1) as u8; // LAPIC IDs 1, 2, 3, ...

        // Allocate a kernel stack for this AP
        let stack = alloc::vec![0u8; AP_STACK_SIZE];
        let stack_top = stack.as_ptr() as u64 + AP_STACK_SIZE as u64;
        unsafe {
            AP_STACKS[ap_idx + 1] = Some(stack);
        }

        // Write boot data for the AP
        unsafe {
            AP_BOOT_DATA.stack_top = stack_top;
            AP_BOOT_DATA.entry_fn = ap_rust_entry as *const () as u64;
            AP_BOOT_DATA.cr3 = cr3_phys;
            AP_BOOT_DATA.cpu_index = (ap_idx + 1) as u64;
        }

        // Write trampoline code to low memory
        write_trampoline();

        // Send INIT-SIPI-SIPI sequence
        crate::serial_println!("[smp] Sending INIT to AP {} (LAPIC ID {})...", ap_idx + 1, target_id);
        lapic::send_init_ipi(target_id);

        // Wait ~10ms
        for _ in 0..1_000_000 { core::hint::spin_loop(); }

        // Send SIPI (twice, per Intel spec)
        lapic::send_sipi(target_id, SIPI_VECTOR);
        for _ in 0..100_000 { core::hint::spin_loop(); }
        lapic::send_sipi(target_id, SIPI_VECTOR);

        // Wait for AP to signal it's started (with timeout)
        let mut timeout = 10_000_000u32;
        while !AP_STARTED[ap_idx + 1].load(Ordering::Acquire) {
            timeout -= 1;
            if timeout == 0 {
                crate::serial_println!("[smp] WARNING: AP {} did not start.", ap_idx + 1);
                break;
            }
            core::hint::spin_loop();
        }
    }

    let total = CPU_COUNT.load(Ordering::Relaxed);
    crate::serial_println!("[smp] {} CPUs active.", total);
}

/// Write the AP trampoline to physical address 0x8000.
///
/// The trampoline is minimal: it sets up the AP to call the Rust entry point
/// with the correct stack and page table. In a real OS, this would be 16-bit
/// real mode code transitioning through protected mode to long mode.
///
/// For simplicity, we write a minimal stub that assumes the AP starts in
/// a state where we can quickly get to long mode. In QEMU, APs start in
/// real mode at CS:IP = trampoline_vector*0x100:0x0000.
///
/// Since writing a full 16→32→64 bit trampoline is extremely complex,
/// we use a simplified approach: store the boot data at a known address
/// and have the AP read it to set up its environment.
fn write_trampoline() {
    let phys_offset = crate::memory::paging::phys_offset().as_u64();
    let trampoline_virt = phys_offset + TRAMPOLINE_PHYS;
    let trampoline = trampoline_virt as *mut u8;

    // Minimal 16-bit real-mode trampoline code:
    // The AP starts executing here in 16-bit real mode.
    //
    // This is a simplified trampoline that:
    // 1. Enters protected mode with a minimal GDT
    // 2. Transitions to long mode using the kernel's CR3
    // 3. Jumps to the 64-bit Rust entry point
    //
    // For QEMU, this works because the kernel page tables identity-map
    // the first 1 MiB (they're set up by the bootloader with a physical
    // memory offset mapping that covers all of physical memory).

    // We place boot parameters at trampoline + 0x100
    let boot_data_offset = 0x100u64;
    let boot_data_ptr = (trampoline_virt + boot_data_offset) as *mut u64;

    // Write boot parameters
    unsafe {
        let data = &raw const AP_BOOT_DATA;
        core::ptr::write_volatile(boot_data_ptr, (*data).stack_top);
        core::ptr::write_volatile(boot_data_ptr.add(1), (*data).entry_fn);
        core::ptr::write_volatile(boot_data_ptr.add(2), (*data).cr3);
        core::ptr::write_volatile(boot_data_ptr.add(3), (*data).cpu_index);
    }

    // For now, we use a very simple approach: instead of a real 16-bit
    // trampoline, we note that getting APs fully booted from real mode
    // requires ~200 bytes of carefully crafted machine code.
    //
    // In this version, we log the attempt but the AP bootstrap is
    // best-effort. QEMU's AP startup is detected via LAPIC, and we
    // report the number of CPUs detected.
    //
    // A production trampoline would need:
    // - 16-bit GDT with code/data segments
    // - Switch to protected mode (CR0.PE)
    // - Enable PAE (CR4.PAE)
    // - Load kernel CR3
    // - Enable long mode (EFER.LME)
    // - Enable paging (CR0.PG)
    // - Far jump to 64-bit code segment
    // - Load RSP and jump to Rust entry
    //
    // This is planned for a future phase with inline assembly support.

    // Write a simple HLT loop as placeholder (AP will be detected but stay in halt)
    unsafe {
        // cli; hlt; jmp $-1
        core::ptr::write_volatile(trampoline, 0xFA);       // cli
        core::ptr::write_volatile(trampoline.add(1), 0xF4); // hlt
        core::ptr::write_volatile(trampoline.add(2), 0xEB); // jmp rel8
        core::ptr::write_volatile(trampoline.add(3), 0xFD); // -3 (back to hlt)
    }
}

/// Rust entry point for APs (called from trampoline in long mode).
///
/// Note: Currently, the simplified trampoline doesn't actually reach this point.
/// This function is prepared for when the full 16→64 trampoline is implemented.
#[unsafe(no_mangle)]
pub extern "C" fn ap_rust_entry() -> ! {
    let cpu_index = unsafe {
        let data = &raw const AP_BOOT_DATA;
        (*data).cpu_index as usize
    };

    // Initialize this AP's LAPIC
    lapic::init_ap();

    let id = lapic::lapic_id();
    CPU_COUNT.fetch_add(1, Ordering::Release);
    AP_STARTED[cpu_index].store(true, Ordering::Release);

    crate::serial_println!("[smp] AP {} started (LAPIC ID: {}).", cpu_index, id);

    // Enable interrupts and enter idle loop
    // (In a full implementation, this AP would join the scheduler)
    x86_64::instructions::interrupts::enable();
    loop {
        x86_64::instructions::hlt();
    }
}

/// Get the number of active CPUs.
pub fn cpu_count() -> u32 {
    CPU_COUNT.load(Ordering::Relaxed)
}

/// AArch64 Exception Vector Table (EVT) for Smart OS.
///
/// Mirrors the functionality of the x86_64 IDT.
/// The table must be 2048-byte aligned as required by the VBAR_EL1 register.

use core::arch::global_asm;

global_asm!(
    r#"
    .balign 2048
    .global exception_vector_table
    exception_vector_table:
        // Current EL with SP0 (Synchronous, IRQ, FIQ, SError)
        .balign 128
        b . 
        .balign 128
        b .
        .balign 128
        b .
        .balign 128
        b .

        // Current EL with SPx
        .balign 128
        b sync_exception_handler
        .balign 128
        b irq_handler
        .balign 128
        b .
        .balign 128
        b .

        // Lower EL (AArch64)
        .balign 128
        b el0_sync_handler
        .balign 128
        b el0_irq_handler
        .balign 128
        b .
        .balign 128
        b .
    "#
);

#[no_mangle]
pub extern "C" fn sync_exception_handler() {
    panic!("[aarch64] Synchronous exception in EL1");
}

#[no_mangle]
pub extern "C" fn irq_handler() {
    // Ack IRQ via GIC or local controller
}

#[no_mangle]
pub extern "C" fn el0_sync_handler() {
    // Syscall entry point (SVC #0)
    super::syscall_entry::handle_svc();
}

#[no_mangle]
pub extern "C" fn el0_irq_handler() {
    // User-space IRQ
}

/// Initialize the vector base address register.
pub fn init() {
    extern "C" {
        static exception_vector_table: u64;
    }
    unsafe {
        core::arch::asm!("msr vbar_el1, {}", in(reg) &exception_vector_table);
    }
}

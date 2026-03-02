/// x86_64 architecture support for Smart OS.
///
/// Provides CPU initialization (GDT, IDT), interrupt handling,
/// SYSCALL/SYSRET, LAPIC, and SMP support.

pub mod gdt;
pub mod idt;
pub mod syscall_entry;
#[allow(dead_code)]
pub mod lapic;
#[allow(dead_code)]
pub mod smp;

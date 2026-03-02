/// System call interface for Smart OS.
///
/// Phase 2: Function-call-based internal syscall API for kernel threads.
/// Future: INT 0x80 / SYSCALL instruction for userspace.

pub mod table;
pub mod handlers;

/// Initialize the syscall subsystem.
pub fn init() {
    crate::serial_println!("[syscall] Syscall interface initialized ({} calls registered).",
        table::SYSCALL_COUNT);
}

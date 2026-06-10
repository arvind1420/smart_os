/// AArch64 (ARM64) architecture-specific code for Smart OS.
///
/// Handles Exception Vectors, EL levels, TTBR paging, and SVC system calls.

pub mod exception;
pub mod paging;
pub mod syscall_entry;

/// Initialize the AArch64 architecture components.
pub fn init() {
    crate::serial_println!("[aarch64] Initializing exception vectors...");
    exception::init();
    crate::serial_println!("[aarch64] Configuring translation tables...");
    paging::init();
    crate::serial_println!("[aarch64] CPU state ready.");
}

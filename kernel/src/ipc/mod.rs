/// Inter-Process Communication (IPC) for Smart OS.
///
/// Messages are SmartPack-encoded Values, making IPC the universal
/// communication protocol across the entire OS.

pub mod channel;
pub mod port;

/// Initialize the IPC subsystem.
pub fn init() {
    port::init();
    crate::serial_println!("[ipc] IPC subsystem initialized.");
}

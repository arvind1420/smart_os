/// ACPI Shutdown / Reboot for Smart OS.
///
/// Phase 11: Provides clean power-off and reboot functionality.
/// Uses QEMU-specific ACPI ports for shutdown and the standard
/// keyboard controller reset for reboot.

use crate::serial_println;

/// QEMU ACPI shutdown port and value.
const QEMU_SHUTDOWN_PORT: u16 = 0x604;
const QEMU_SHUTDOWN_VALUE: u16 = 0x2000;

/// QEMU isa-debug-exit port (fallback).
const ISA_DEBUG_EXIT_PORT: u16 = 0xF4;

/// Keyboard controller port (for reboot).
const KBD_STATUS_PORT: u16 = 0x64;
const KBD_RESET_CMD: u8 = 0xFE;

/// Perform an ACPI shutdown (power off).
///
/// On QEMU with `-device isa-debug-exit,iobase=0xf4,iosize=0x04`
/// or with default ACPI support, this will power off the VM.
pub fn shutdown() -> ! {
    serial_println!("[acpi] Initiating system shutdown...");

    // Try QEMU ACPI port first
    unsafe {
        core::arch::asm!(
            "out dx, ax",
            in("dx") QEMU_SHUTDOWN_PORT,
            in("ax") QEMU_SHUTDOWN_VALUE,
        );
    }

    // Fallback: isa-debug-exit
    serial_println!("[acpi] ACPI shutdown failed, trying isa-debug-exit...");
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") ISA_DEBUG_EXIT_PORT,
            in("al") 0x31u8, // Exit code (0x31 >> 1) | 1 = 0x19 = 25
        );
    }

    // If nothing worked, halt
    serial_println!("[acpi] All shutdown methods failed. Halting CPU.");
    loop {
        x86_64::instructions::hlt();
    }
}

/// Perform a system reboot via keyboard controller reset.
pub fn reboot() -> ! {
    serial_println!("[acpi] Initiating system reboot...");

    // Wait for keyboard controller input buffer to clear
    unsafe {
        for _ in 0..1000 {
            let status: u8;
            core::arch::asm!(
                "in al, dx",
                in("dx") KBD_STATUS_PORT,
                out("al") status,
            );
            if status & 0x02 == 0 {
                break;
            }
        }

        // Send reset command to keyboard controller
        core::arch::asm!(
            "out dx, al",
            in("dx") KBD_STATUS_PORT,
            in("al") KBD_RESET_CMD,
        );
    }

    // Wait a bit for reboot to take effect
    for _ in 0..100_000 {
        core::hint::spin_loop();
    }

    // If keyboard reset didn't work, try triple fault
    serial_println!("[acpi] Keyboard reset failed. Halting.");
    loop {
        x86_64::instructions::hlt();
    }
}

/// Initialize the ACPI subsystem.
pub fn init() {
    serial_println!("[acpi] ACPI shutdown/reboot support initialized.");
}

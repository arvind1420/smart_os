/// Legacy Parallel Port (LPT) Driver for Smart OS.
///
/// Phase 21: Support for legacy CNC machines, instrumentation, and printers.
/// Interfaces with the standard LPT ports (LPT1: 0x378).

use x86_64::instructions::port::Port;
use spin::Mutex;

const LPT1_BASE: u16 = 0x378;

pub struct LptDevice {
    data_port: Port<u8>,
    status_port: Port<u8>,
    control_port: Port<u8>,
}

impl LptDevice {
    pub fn new(base: u16) -> Self {
        Self {
            data_port: Port::new(base),
            status_port: Port::new(base + 1),
            control_port: Port::new(base + 2),
        }
    }

    /// Write a byte to the data register.
    pub fn write_data(&mut self, data: u8) {
        unsafe { self.data_port.write(data); }
    }

    /// Read the status register.
    pub fn read_status(&mut self) -> u8 {
        unsafe { self.status_port.read() }
    }

    /// Send a strobe signal to indicate data is ready (used by printers).
    pub fn strobe(&mut self) {
        unsafe {
            let mut ctrl = self.control_port.read();
            ctrl |= 0x01; // Strobe high
            self.control_port.write(ctrl);
            // Minimal delay
            for _ in 0..100 { core::hint::spin_loop(); }
            ctrl &= !0x01; // Strobe low
            self.control_port.write(ctrl);
        }
    }

    /// Check if the device is busy.
    pub fn is_busy(&mut self) -> bool {
        (self.read_status() & 0x80) == 0
    }
}

pub static LPT1: Mutex<LptDevice> = Mutex::new(LptDevice {
    data_port: Port::new(LPT1_BASE),
    status_port: Port::new(LPT1_BASE + 1),
    control_port: Port::new(LPT1_BASE + 2),
});

pub fn init() {
    crate::serial_println!("[lpt] Legacy Parallel Port (LPT1) driver initialized.");
}

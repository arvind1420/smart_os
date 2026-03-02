/// Serial port driver for kernel debug output.
///
/// Uses the 16550 UART on COM1 (I/O port 0x3F8) for text output.
/// This is the first output device available, working even before
/// the framebuffer is initialized.

use spin::Mutex;
use uart_16550::SerialPort;

/// Global serial port, protected by a spinlock.
pub static SERIAL1: Mutex<Option<SerialPort>> = Mutex::new(None);

/// Initialize the serial port on COM1.
pub fn init() {
    let mut serial_port = unsafe { SerialPort::new(0x3F8) };
    serial_port.init();
    *SERIAL1.lock() = Some(serial_port);
}

#[doc(hidden)]
pub fn _print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    // Disable interrupts while writing to prevent deadlocks
    x86_64::instructions::interrupts::without_interrupts(|| {
        if let Some(ref mut serial) = *SERIAL1.lock() {
            serial.write_fmt(args).expect("serial write failed");
        }
    });
}

/// Print to the serial console.
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*))
    };
}

/// Print to the serial console with a newline.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}

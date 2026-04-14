/// Serial port driver for kernel debug output.
///
/// Uses the 16550 UART on COM1 (I/O port 0x3F8) for text output.
/// This is the first output device available, working even before
/// the framebuffer is initialized.

use spin::Mutex;
use uart_16550::SerialPort;
use alloc::string::String;
use alloc::collections::VecDeque;

/// Global serial port, protected by a spinlock.
pub static SERIAL1: Mutex<Option<SerialPort>> = Mutex::new(None);

/// Ring buffer holding the last 200 serial log lines for dmesg.
pub static SERIAL_LOG: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

pub fn get_log() -> String {
    let log = SERIAL_LOG.lock();
    let mut out = String::new();
    for line in log.iter() {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Initialize the serial port on COM1.
pub fn init() {
    let mut serial_port = unsafe { SerialPort::new(0x3F8) };
    serial_port.init();
    *SERIAL1.lock() = Some(serial_port);
}

#[doc(hidden)]
pub fn _print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    // Always write to serial port — no heap needed, safe before heap init.
    x86_64::instructions::interrupts::without_interrupts(|| {
        if let Some(ref mut serial) = *SERIAL1.lock() {
            serial.write_fmt(args).ok();
        }
    });
    // Only append to ring buffer after the heap allocator is ready.
    if crate::memory::heap::is_heap_ready() {
        use alloc::string::ToString;
        let s = alloc::format!("{}", args);
        let mut log = SERIAL_LOG.lock();
        for line in s.lines() {
            if log.len() >= 200 { log.pop_front(); }
            log.push_back(line.to_string());
        }
    }
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

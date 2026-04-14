/// Enterprise UART (Serial) Driver for Smart OS.
///
/// Phase 21: Full support for COM1-COM4 with interrupt-driven I/O.
/// Used for industrial automation, RS-232/485 communication, and legacy bridging.

use spin::Mutex;
use uart_16550::SerialPort;
use alloc::vec::Vec;
use alloc::collections::VecDeque;
use x86_64::instructions::port::Port;

/// Standard I/O ports for COM devices.
pub const COM1_PORT: u16 = 0x3F8;
pub const COM2_PORT: u16 = 0x2F8;
pub const COM3_PORT: u16 = 0x3E8;
pub const COM4_PORT: u16 = 0x2E8;

pub struct UartDevice {
    pub port: u16,
    pub inner: Mutex<SerialPort>,
    pub rx_buffer: Mutex<VecDeque<u8>>,
}

impl UartDevice {
    pub fn new(port: u16) -> Self {
        let mut serial = unsafe { SerialPort::new(port) };
        serial.init();
        
        // Enable Received Data Available Interrupt
        unsafe {
            let mut ier = Port::<u8>::new(port + 1);
            let val = ier.read();
            ier.write(val | 0x01);
        }

        Self {
            port,
            inner: Mutex::new(serial),
            rx_buffer: Mutex::new(VecDeque::with_capacity(4096)),
        }
    }

    /// Send data through the UART.
    pub fn write(&self, data: &[u8]) {
        let mut inner = self.inner.lock();
        for &byte in data {
            inner.send(byte);
        }
    }

    /// Read available data from the internal RX buffer.
    pub fn read(&self, buf: &mut [u8]) -> usize {
        let mut rx = self.rx_buffer.lock();
        let count = rx.len().min(buf.len());
        for i in 0..count {
            buf[i] = rx.pop_front().unwrap();
        }
        count
    }

    /// Check if there is a pending interrupt on this device.
    pub fn is_interrupt_pending(&self) -> bool {
        unsafe {
            let mut iir = Port::<u8>::new(self.port + 2);
            (iir.read() & 0x01) == 0
        }
    }

    /// Handle an incoming interrupt (received data).
    pub fn handle_interrupt(&self) {
        let mut inner = self.inner.lock();
        let mut rx = self.rx_buffer.lock();
        
        // Drain the RX FIFO/register
        // LSR (Line Status Register) is at base + 5
        let mut lsr = Port::<u8>::new(self.port + 5);
        
        while (unsafe { lsr.read() } & 0x01) != 0 {
            let byte = inner.receive();
            if rx.len() < rx.capacity() {
                rx.push_back(byte);
            }
            // If buffer full, we just drop characters (flow control should be handled at higher level)
        }
    }
}

pub static UART_BUS: Mutex<Vec<UartDevice>> = Mutex::new(Vec::new());

/// Initialize the UART bus with all detected COM ports.
pub fn init() {
    let mut bus = UART_BUS.lock();
    bus.push(UartDevice::new(COM1_PORT));
    bus.push(UartDevice::new(COM2_PORT));
    bus.push(UartDevice::new(COM3_PORT));
    bus.push(UartDevice::new(COM4_PORT));
    crate::serial_println!("[uart] Multi-port UART bus initialized (COM1-COM4).");
}

/// Helper to write to a specific COM port.
pub fn write_com(port_idx: usize, data: &[u8]) -> Result<(), &'static str> {
    let bus = UART_BUS.lock();
    if port_idx < bus.len() {
        bus[port_idx].write(data);
        Ok(())
    } else {
        Err("COM port index out of bounds")
    }
}

/// Helper to read from a specific COM port.
pub fn read_com(port_idx: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
    let bus = UART_BUS.lock();
    if port_idx < bus.len() {
        Ok(bus[port_idx].read(buf))
    } else {
        Err("COM port index out of bounds")
    }
}

/// Return kernel serial log for dmesg command.
pub fn get_serial_log() -> alloc::string::String {
    crate::serial::get_log()
}

/// Dispatches an interrupt to the correct UART device(s) based on IRQ.
pub fn dispatch_interrupt(irq: u8) {
    let bus = UART_BUS.lock();
    for device in bus.iter() {
        // COM1 (0x3F8) and COM3 (0x3E8) typically share IRQ 4
        // COM2 (0x2F8) and COM4 (0x2E8) typically share IRQ 3
        let target_irq = match device.port {
            COM1_PORT | COM3_PORT => 4,
            COM2_PORT | COM4_PORT => 3,
            _ => 0,
        };

        if target_irq == irq && device.is_interrupt_pending() {
            device.handle_interrupt();
        }
    }
}

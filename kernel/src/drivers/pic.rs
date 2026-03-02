/// 8259 Programmable Interrupt Controller (PIC) driver.
///
/// The PIC maps hardware IRQs to CPU interrupt vectors.
/// We remap IRQ 0-7 to vectors 32-39 and IRQ 8-15 to vectors 40-47
/// to avoid conflicts with CPU exception vectors 0-31.

use spin::Mutex;
use x86_64::instructions::port::Port;

/// PIC1 (master) command and data ports
const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
/// PIC2 (slave) command and data ports
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

/// Offset for PIC1 interrupts (IRQ 0-7 → vectors 32-39)
pub const PIC1_OFFSET: u8 = 32;
/// Offset for PIC2 interrupts (IRQ 8-15 → vectors 40-47)
pub const PIC2_OFFSET: u8 = 40;

/// ICW1: Initialize + ICW4 needed
const ICW1_INIT: u8 = 0x11;
/// ICW4: 8086/88 mode
const ICW4_8086: u8 = 0x01;
/// End-of-interrupt command
const EOI: u8 = 0x20;

/// Global PIC lock for safe port access.
pub static PICS: Mutex<ChainedPics> = Mutex::new(ChainedPics::new());

pub struct ChainedPics {
    pic1_cmd: Port<u8>,
    pic1_data: Port<u8>,
    pic2_cmd: Port<u8>,
    pic2_data: Port<u8>,
}

impl ChainedPics {
    const fn new() -> Self {
        Self {
            pic1_cmd: Port::new(PIC1_CMD),
            pic1_data: Port::new(PIC1_DATA),
            pic2_cmd: Port::new(PIC2_CMD),
            pic2_data: Port::new(PIC2_DATA),
        }
    }

    /// Initialize both PICs with proper offsets.
    pub unsafe fn initialize(&mut self) {
        unsafe {
            // Save masks
            let mask1 = self.pic1_data.read();
            let mask2 = self.pic2_data.read();

            // ICW1: Start initialization sequence
            self.pic1_cmd.write(ICW1_INIT);
            io_wait();
            self.pic2_cmd.write(ICW1_INIT);
            io_wait();

            // ICW2: Set vector offsets
            self.pic1_data.write(PIC1_OFFSET);
            io_wait();
            self.pic2_data.write(PIC2_OFFSET);
            io_wait();

            // ICW3: Tell PICs about each other
            self.pic1_data.write(4); // PIC2 is at IRQ2
            io_wait();
            self.pic2_data.write(2); // PIC2 cascade identity
            io_wait();

            // ICW4: 8086 mode
            self.pic1_data.write(ICW4_8086);
            io_wait();
            self.pic2_data.write(ICW4_8086);
            io_wait();

            // Restore masks (initially mask all except timer and keyboard)
            self.pic1_data.write(mask1);
            self.pic2_data.write(mask2);
        }
    }

    /// Send end-of-interrupt for the given interrupt vector.
    pub unsafe fn end_of_interrupt(&mut self, irq: u8) {
        unsafe {
            if irq >= PIC2_OFFSET {
                self.pic2_cmd.write(EOI);
            }
            self.pic1_cmd.write(EOI);
        }
    }

    /// Unmask a specific IRQ line.
    pub unsafe fn unmask(&mut self, irq: u8) {
        unsafe {
            if irq < 8 {
                let mask = self.pic1_data.read() & !(1 << irq);
                self.pic1_data.write(mask);
            } else {
                let mask = self.pic2_data.read() & !(1 << (irq - 8));
                self.pic2_data.write(mask);
            }
        }
    }
}

/// Tiny I/O delay for PIC initialization timing.
fn io_wait() {
    unsafe {
        // Port 0x80 is used for POST codes — writing to it causes a small delay
        Port::<u8>::new(0x80).write(0);
    }
}

/// Initialize the PIC and unmask timer (IRQ0) and keyboard (IRQ1).
pub fn init() {
    unsafe {
        let mut pics = PICS.lock();
        pics.initialize();
        // Unmask IRQ0 (timer), IRQ1 (keyboard), IRQ2 (cascade), IRQ12 (mouse)
        pics.unmask(0);
        pics.unmask(1);
        pics.unmask(2);   // Cascade — required for PIC2 interrupts to reach CPU
        pics.unmask(12);  // PS/2 mouse
    }
    crate::serial_println!("[drivers] PIC initialized (IRQ0,1,2,12 unmasked).");
}

/// PS/2 Keyboard driver.
///
/// Decodes scancodes from the keyboard controller and maintains
/// a circular input buffer for key events.

use spin::Mutex;
use x86_64::instructions::port::Port;

/// Keyboard data port
const KBD_DATA: u16 = 0x60;

/// Maximum number of keys in the input buffer.
const BUFFER_SIZE: usize = 64;

/// A key event from the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub scancode: u8,
    pub ascii: Option<u8>,
    pub pressed: bool,
}

/// Circular buffer for keyboard events.
pub struct KeyBuffer {
    buffer: [Option<KeyEvent>; BUFFER_SIZE],
    read_pos: usize,
    write_pos: usize,
    count: usize,
}

impl KeyBuffer {
    const fn new() -> Self {
        Self {
            buffer: [None; BUFFER_SIZE],
            read_pos: 0,
            write_pos: 0,
            count: 0,
        }
    }

    fn push(&mut self, event: KeyEvent) {
        if self.count < BUFFER_SIZE {
            self.buffer[self.write_pos] = Some(event);
            self.write_pos = (self.write_pos + 1) % BUFFER_SIZE;
            self.count += 1;
        }
    }

    pub fn pop(&mut self) -> Option<KeyEvent> {
        if self.count == 0 {
            return None;
        }
        let event = self.buffer[self.read_pos].take();
        self.read_pos = (self.read_pos + 1) % BUFFER_SIZE;
        self.count -= 1;
        event
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Global keyboard buffer.
pub static KEYBOARD: Mutex<KeyBuffer> = Mutex::new(KeyBuffer::new());

/// Initialize the keyboard driver.
pub fn init() {
    crate::serial_println!("[drivers] Keyboard initialized.");
}

/// Called from the keyboard interrupt handler.
pub fn handle_scancode() {
    let scancode: u8 = unsafe { Port::new(KBD_DATA).read() };
    let pressed = scancode & 0x80 == 0;
    let code = scancode & 0x7F;
    let ascii = scancode_to_ascii(code, pressed);

    let event = KeyEvent {
        scancode: code,
        ascii,
        pressed,
    };

    KEYBOARD.lock().push(event);
}

/// Read the next key event from the buffer (non-blocking).
/// Also polls the PS/2 status register directly so keyboard works even if
/// IRQ1 isn't delivered (e.g. when LAPIC is active without an I/O APIC).
pub fn read_key() -> Option<KeyEvent> {
    // Poll PS/2 output buffer (bit 0 of status port 0x64).
    // If data is available, read it now so we don't depend on IRQ1 delivery.
    let status: u8 = unsafe { x86_64::instructions::port::Port::<u8>::new(0x64).read() };
    if status & 0x01 != 0 {
        handle_scancode();
    }
    KEYBOARD.lock().pop()
}

/// Convert PS/2 scancode set 1 to ASCII.
/// Only handles basic US QWERTY layout.
fn scancode_to_ascii(code: u8, pressed: bool) -> Option<u8> {
    if !pressed {
        return None;
    }
    match code {
        0x02 => Some(b'1'), 0x03 => Some(b'2'), 0x04 => Some(b'3'),
        0x05 => Some(b'4'), 0x06 => Some(b'5'), 0x07 => Some(b'6'),
        0x08 => Some(b'7'), 0x09 => Some(b'8'), 0x0A => Some(b'9'),
        0x0B => Some(b'0'),
        0x10 => Some(b'q'), 0x11 => Some(b'w'), 0x12 => Some(b'e'),
        0x13 => Some(b'r'), 0x14 => Some(b't'), 0x15 => Some(b'y'),
        0x16 => Some(b'u'), 0x17 => Some(b'i'), 0x18 => Some(b'o'),
        0x19 => Some(b'p'),
        0x1E => Some(b'a'), 0x1F => Some(b's'), 0x20 => Some(b'd'),
        0x21 => Some(b'f'), 0x22 => Some(b'g'), 0x23 => Some(b'h'),
        0x24 => Some(b'j'), 0x25 => Some(b'k'), 0x26 => Some(b'l'),
        0x2C => Some(b'z'), 0x2D => Some(b'x'), 0x2E => Some(b'c'),
        0x2F => Some(b'v'), 0x30 => Some(b'b'), 0x31 => Some(b'n'),
        0x32 => Some(b'm'),
        0x39 => Some(b' '),       // Space
        0x1C => Some(b'\n'),      // Enter
        0x0E => Some(b'\x08'),    // Backspace
        0x0F => Some(b'\t'),      // Tab
        0x0C => Some(b'-'),
        0x0D => Some(b'='),
        0x1A => Some(b'['), 0x1B => Some(b']'),
        0x27 => Some(b';'), 0x28 => Some(b'\''),
        0x33 => Some(b','), 0x34 => Some(b'.'), 0x35 => Some(b'/'),
        0x29 => Some(b'`'), 0x2B => Some(b'\\'),
        0x01 => Some(0x1B),       // Escape
        _ => None,
    }
}

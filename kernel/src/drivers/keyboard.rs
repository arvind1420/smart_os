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

/// Modifier key state (Shift, Ctrl, Alt, CapsLock).
struct ModState {
    shift:    bool,
    ctrl:     bool,
    alt:      bool,
    capslock: bool,
}

impl ModState {
    const fn new() -> Self { Self { shift: false, ctrl: false, alt: false, capslock: false } }
}

/// Global keyboard buffer.
pub static KEYBOARD: Mutex<KeyBuffer> = Mutex::new(KeyBuffer::new());

static MOD_STATE: Mutex<ModState> = Mutex::new(ModState::new());

// Scancode constants for modifier keys
const SC_LSHIFT: u8 = 0x2A;
const SC_RSHIFT: u8 = 0x36;
const SC_LCTRL:  u8 = 0x1D;
const SC_LALT:   u8 = 0x38;
const SC_CAPS:   u8 = 0x3A;

/// Initialize the keyboard driver.
pub fn init() {
    crate::serial_println!("[drivers] Keyboard initialized.");
}

/// Called from the keyboard interrupt handler.
pub fn handle_scancode() {
    let scancode: u8 = unsafe { Port::new(KBD_DATA).read() };
    let pressed = scancode & 0x80 == 0;
    let code = scancode & 0x7F;

    // Update modifier state
    {
        let mut mods = MOD_STATE.lock();
        match code {
            SC_LSHIFT | SC_RSHIFT => mods.shift    = pressed,
            SC_LCTRL              => mods.ctrl      = pressed,
            SC_LALT               => mods.alt       = pressed,
            SC_CAPS if pressed    => mods.capslock  = !mods.capslock,
            _ => {}
        }
    }

    let (shift, ctrl, capslock) = {
        let m = MOD_STATE.lock();
        (m.shift, m.ctrl, m.capslock)
    };

    let ascii = scancode_to_ascii(code, pressed, shift, ctrl, capslock);

    let event = KeyEvent {
        scancode: code,
        ascii,
        pressed,
    };

    KEYBOARD.lock().push(event);
}

/// Query current modifier state (for GUI key routing).
pub fn shift_held()    -> bool { MOD_STATE.lock().shift }
pub fn ctrl_held()     -> bool { MOD_STATE.lock().ctrl }
pub fn alt_held()      -> bool { MOD_STATE.lock().alt }
pub fn capslock_on()   -> bool { MOD_STATE.lock().capslock }

/// Read the next key event from the buffer (non-blocking).
/// Polls the PS/2 status register directly so keyboard works even if
/// IRQ1 isn't delivered (e.g. when LAPIC is active without an I/O APIC).
pub fn read_key() -> Option<KeyEvent> {
    // Port 0x64 status register:
    //   bit 0 (OBF)    = output buffer has data
    //   bit 5 (AUXOBF) = data is from mouse (auxiliary port)
    // Only consume when bit 0 is set AND bit 5 is clear (keyboard data).
    let status: u8 = unsafe { x86_64::instructions::port::Port::<u8>::new(0x64).read() };
    if status & 0x01 != 0 && status & 0x20 == 0 {
        handle_scancode();
    }
    KEYBOARD.lock().pop()
}

/// Convert PS/2 scancode set 1 to ASCII, respecting Shift/CapsLock/Ctrl.
///
/// US QWERTY layout — both the unshifted and shifted forms of every key.
fn scancode_to_ascii(code: u8, pressed: bool, shift: bool, ctrl: bool, capslock: bool) -> Option<u8> {
    if !pressed { return None; }

    // Non-printing / control keys (same regardless of shift)
    match code {
        0x01 => return Some(0x1B),       // Escape
        0x0E => return Some(0x08),       // Backspace
        0x0F => return Some(b'\t'),      // Tab
        0x1C => return Some(b'\n'),      // Enter
        0x39 => return Some(b' '),       // Space
        // Modifier keys — no ASCII
        SC_LSHIFT | SC_RSHIFT | SC_LCTRL | SC_LALT | SC_CAPS => return None,
        // Function keys, arrows, etc. — no ASCII for now
        0x3B..=0x44 | 0x57 | 0x58 => return None, // F1-F12
        0x47..=0x53 => return None,                 // numpad / arrows / ins / del
        _ => {}
    }

    // For letter keys, CapsLock XORs with Shift to determine case
    let letter_upper = shift ^ capslock;

    let ch: u8 = match code {
        // Number row (Shift gives symbols)
        0x02 => if shift { b'!' } else { b'1' },
        0x03 => if shift { b'@' } else { b'2' },
        0x04 => if shift { b'#' } else { b'3' },
        0x05 => if shift { b'$' } else { b'4' },
        0x06 => if shift { b'%' } else { b'5' },
        0x07 => if shift { b'^' } else { b'6' },
        0x08 => if shift { b'&' } else { b'7' },
        0x09 => if shift { b'*' } else { b'8' },
        0x0A => if shift { b'(' } else { b'9' },
        0x0B => if shift { b')' } else { b'0' },

        // Letter keys (QWERTY rows)
        0x10 => if letter_upper { b'Q' } else { b'q' },
        0x11 => if letter_upper { b'W' } else { b'w' },
        0x12 => if letter_upper { b'E' } else { b'e' },
        0x13 => if letter_upper { b'R' } else { b'r' },
        0x14 => if letter_upper { b'T' } else { b't' },
        0x15 => if letter_upper { b'Y' } else { b'y' },
        0x16 => if letter_upper { b'U' } else { b'u' },
        0x17 => if letter_upper { b'I' } else { b'i' },
        0x18 => if letter_upper { b'O' } else { b'o' },
        0x19 => if letter_upper { b'P' } else { b'p' },
        0x1E => if letter_upper { b'A' } else { b'a' },
        0x1F => if letter_upper { b'S' } else { b's' },
        0x20 => if letter_upper { b'D' } else { b'd' },
        0x21 => if letter_upper { b'F' } else { b'f' },
        0x22 => if letter_upper { b'G' } else { b'g' },
        0x23 => if letter_upper { b'H' } else { b'h' },
        0x24 => if letter_upper { b'J' } else { b'j' },
        0x25 => if letter_upper { b'K' } else { b'k' },
        0x26 => if letter_upper { b'L' } else { b'l' },
        0x2C => if letter_upper { b'Z' } else { b'z' },
        0x2D => if letter_upper { b'X' } else { b'x' },
        0x2E => if letter_upper { b'C' } else { b'c' },
        0x2F => if letter_upper { b'V' } else { b'v' },
        0x30 => if letter_upper { b'B' } else { b'b' },
        0x31 => if letter_upper { b'N' } else { b'n' },
        0x32 => if letter_upper { b'M' } else { b'm' },

        // Punctuation / symbol keys
        0x0C => if shift { b'_' } else { b'-' },
        0x0D => if shift { b'+' } else { b'=' },
        0x1A => if shift { b'{' } else { b'[' },
        0x1B => if shift { b'}' } else { b']' },
        0x27 => if shift { b':' } else { b';' },  // ← KEY FIX: Shift+; = :
        0x28 => if shift { b'"' } else { b'\'' },
        0x29 => if shift { b'~' } else { b'`' },
        0x2B => if shift { b'|' } else { b'\\' },
        0x33 => if shift { b'<' } else { b',' },
        0x34 => if shift { b'>' } else { b'.' },
        0x35 => if shift { b'?' } else { b'/' },

        _ => return None,
    };

    // Ctrl+letter → control code (e.g. Ctrl+C = 0x03)
    if ctrl && ch.is_ascii_alphabetic() {
        return Some((ch.to_ascii_lowercase() - b'a') + 1);
    }

    Some(ch)
}

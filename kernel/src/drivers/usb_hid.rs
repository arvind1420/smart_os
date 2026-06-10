/// USB HID (Human Interface Device) Class Driver for Smart OS.
///
/// Supports boot-protocol keyboards and mice connected via xHCI.
/// Routes USB input events into existing PS/2 keyboard/mouse handlers.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// HID class constants.
pub const HID_CLASS: u8 = 0x03;
pub const HID_SUBCLASS_BOOT: u8 = 0x01;
pub const HID_PROTOCOL_KEYBOARD: u8 = 0x01;
pub const HID_PROTOCOL_MOUSE: u8 = 0x02;

/// Gamepad state (XInput style).
#[derive(Clone, Copy, Default)]
pub struct GamepadReport {
    pub lx: i8, pub ly: i8,
    pub rx: i8, pub ry: i8,
    pub buttons: u16,
    pub lt: u8, pub rt: u8,
}

// ═══════════════════════════════════════════════════════════════
//  HID Reports
// ═══════════════════════════════════════════════════════════════

/// Boot keyboard report (8 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KeyboardReport {
    pub modifiers: u8,
    pub reserved: u8,
    pub keycodes: [u8; 6],
}

/// Boot mouse report (3 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MouseReport {
    pub buttons: u8,
    pub dx: i8,
    pub dy: i8,
}

// ═══════════════════════════════════════════════════════════════
//  HID Scancode → ASCII Mapping (USB HID Usage Table)
// ═══════════════════════════════════════════════════════════════

/// Map HID usage code to ASCII (unshifted).
static HID_TO_ASCII: [u8; 128] = {
    let mut map = [0u8; 128];
    // Letters a-z: HID usage 0x04-0x1D
    map[0x04] = b'a'; map[0x05] = b'b'; map[0x06] = b'c'; map[0x07] = b'd';
    map[0x08] = b'e'; map[0x09] = b'f'; map[0x0A] = b'g'; map[0x0B] = b'h';
    map[0x0C] = b'i'; map[0x0D] = b'j'; map[0x0E] = b'k'; map[0x0F] = b'l';
    map[0x10] = b'm'; map[0x11] = b'n'; map[0x12] = b'o'; map[0x13] = b'p';
    map[0x14] = b'q'; map[0x15] = b'r'; map[0x16] = b's'; map[0x17] = b't';
    map[0x18] = b'u'; map[0x19] = b'v'; map[0x1A] = b'w'; map[0x1B] = b'x';
    map[0x1C] = b'y'; map[0x1D] = b'z';
    // Numbers 1-9,0: HID usage 0x1E-0x27
    map[0x1E] = b'1'; map[0x1F] = b'2'; map[0x20] = b'3'; map[0x21] = b'4';
    map[0x22] = b'5'; map[0x23] = b'6'; map[0x24] = b'7'; map[0x25] = b'8';
    map[0x26] = b'9'; map[0x27] = b'0';
    // Special keys
    map[0x28] = b'\n'; // Enter
    map[0x29] = 0x1B;  // Escape
    map[0x2A] = 0x08;  // Backspace
    map[0x2B] = b'\t'; // Tab
    map[0x2C] = b' ';  // Space
    map[0x2D] = b'-'; map[0x2E] = b'=';
    map[0x2F] = b'['; map[0x30] = b']'; map[0x31] = b'\\';
    map[0x33] = b';'; map[0x34] = b'\'';
    map[0x35] = b'`';
    map[0x36] = b','; map[0x37] = b'.'; map[0x38] = b'/';
    map
};

/// Map HID usage code to ASCII (shifted).
static HID_TO_ASCII_SHIFT: [u8; 128] = {
    let mut map = [0u8; 128];
    // Letters A-Z
    map[0x04] = b'A'; map[0x05] = b'B'; map[0x06] = b'C'; map[0x07] = b'D';
    map[0x08] = b'E'; map[0x09] = b'F'; map[0x0A] = b'G'; map[0x0B] = b'H';
    map[0x0C] = b'I'; map[0x0D] = b'J'; map[0x0E] = b'K'; map[0x0F] = b'L';
    map[0x10] = b'M'; map[0x11] = b'N'; map[0x12] = b'O'; map[0x13] = b'P';
    map[0x14] = b'Q'; map[0x15] = b'R'; map[0x16] = b'S'; map[0x17] = b'T';
    map[0x18] = b'U'; map[0x19] = b'V'; map[0x1A] = b'W'; map[0x1B] = b'X';
    map[0x1C] = b'Y'; map[0x1D] = b'Z';
    // Shifted numbers → symbols
    map[0x1E] = b'!'; map[0x1F] = b'@'; map[0x20] = b'#'; map[0x21] = b'$';
    map[0x22] = b'%'; map[0x23] = b'^'; map[0x24] = b'&'; map[0x25] = b'*';
    map[0x26] = b'('; map[0x27] = b')';
    // Special keys (same)
    map[0x28] = b'\n'; map[0x29] = 0x1B; map[0x2A] = 0x08;
    map[0x2B] = b'\t'; map[0x2C] = b' ';
    map[0x2D] = b'_'; map[0x2E] = b'+';
    map[0x2F] = b'{'; map[0x30] = b'}'; map[0x31] = b'|';
    map[0x33] = b':'; map[0x34] = b'"';
    map[0x35] = b'~';
    map[0x36] = b'<'; map[0x37] = b'>'; map[0x38] = b'?';
    map
};

/// HID usage to PS/2 scancode mapping (for arrow keys, etc.).
static HID_TO_SCANCODE: [u8; 128] = {
    let mut map = [0u8; 128];
    map[0x4F] = 0x4D; // Right arrow
    map[0x50] = 0x4B; // Left arrow
    map[0x51] = 0x50; // Down arrow
    map[0x52] = 0x48; // Up arrow
    map[0x49] = 0x53; // Delete (Insert on some)
    map[0x4A] = 0x47; // Home
    map[0x4B] = 0x49; // Page Up
    map[0x4C] = 0x53; // Delete (forward)
    map[0x4D] = 0x4F; // End
    map[0x4E] = 0x51; // Page Down
    map
};

// ═══════════════════════════════════════════════════════════════
//  State
// ═══════════════════════════════════════════════════════════════

/// Slots that have HID keyboard devices.
static KEYBOARD_SLOTS: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// Slots that have HID mouse devices.
static MOUSE_SLOTS: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// Slots for HID gamepads.
static GAMEPAD_SLOTS: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// Current state of the primary gamepad.
pub static PRIMARY_GAMEPAD: Mutex<GamepadReport> = Mutex::new(GamepadReport {
    lx: 0, ly: 0, rx: 0, ry: 0, buttons: 0, lt: 0, rt: 0,
});
/// Previous keyboard report (for detecting key changes).
static PREV_KB_REPORT: Mutex<KeyboardReport> = Mutex::new(KeyboardReport {
    modifiers: 0, reserved: 0, keycodes: [0; 6],
});
/// Whether USB HID is initialized and polling.
static HID_ACTIVE: AtomicBool = AtomicBool::new(false);

// ═══════════════════════════════════════════════════════════════
//  Initialization
// ═══════════════════════════════════════════════════════════════

/// Initialize HID devices discovered by xHCI.
pub fn init_hid_from_xhci() {
    let xhci = super::xhci::XHCI.lock();
    let controller = match xhci.as_ref() {
        Some(c) => c,
        None => return,
    };

    let mut kb_slots = KEYBOARD_SLOTS.lock();
    let mut ms_slots = MOUSE_SLOTS.lock();
    let mut gp_slots = GAMEPAD_SLOTS.lock();

    for dev in &controller.devices {
        if dev.is_hid_keyboard {
            kb_slots.push(dev.slot_id);
            crate::serial_println!(
                "[usb-hid] Keyboard found: slot={}, vendor={:#06X}, product={:#06X}",
                dev.slot_id, dev.vendor_id, dev.product_id
            );
        }
        if dev.is_hid_mouse {
            ms_slots.push(dev.slot_id);
            crate::serial_println!(
                "[usb-hid] Mouse found: slot={}, vendor={:#06X}, product={:#06X}",
                dev.slot_id, dev.vendor_id, dev.product_id
            );
        }
        // Simplified detection for Gamepads (using product IDs or interface class)
        if dev.vendor_id == 0x045E || dev.vendor_id == 0x054C { // Microsoft or Sony
             gp_slots.push(dev.slot_id);
             crate::serial_println!(
                "[usb-hid] Gamepad detected: slot={}, vendor={:#06X}",
                dev.slot_id, dev.vendor_id
            );
        }
    }

    if !kb_slots.is_empty() || !ms_slots.is_empty() || !gp_slots.is_empty() {
        HID_ACTIVE.store(true, Ordering::Relaxed);
        crate::serial_println!(
            "[usb-hid] Initialized: {} keyboard(s), {} mouse/mice, {} gamepad(s)",
            kb_slots.len(), ms_slots.len(), gp_slots.len()
        );
    } else {
        crate::serial_println!("[usb-hid] No HID devices found.");
    }
}

// ═══════════════════════════════════════════════════════════════
//  Keyboard Report Processing
// ═══════════════════════════════════════════════════════════════

/// Process a keyboard report, generating key events for newly pressed keys.
fn process_keyboard_report(report: &KeyboardReport) {
    let mut prev = PREV_KB_REPORT.lock();
    let shift = (report.modifiers & 0x22) != 0; // L-Shift or R-Shift
    let ctrl = (report.modifiers & 0x11) != 0;  // L-Ctrl or R-Ctrl

    for &keycode in &report.keycodes {
        if keycode == 0 {
            continue;
        }

        // Check if this key was already pressed in previous report
        let was_pressed = prev.keycodes.contains(&keycode);
        if was_pressed {
            continue; // Key held, not a new press
        }

        // Convert HID keycode to ASCII
        let idx = keycode as usize;
        if idx < 128 {
            let ascii = if shift {
                HID_TO_ASCII_SHIFT[idx]
            } else {
                HID_TO_ASCII[idx]
            };

            let scancode = HID_TO_SCANCODE[idx];

            if ctrl && ascii >= b'a' && ascii <= b'z' {
                // Ctrl+letter: produce control code (1-26)
                let ctrl_code = ascii - b'a' + 1;
                let event = crate::drivers::keyboard::KeyEvent {
                    scancode, ascii: Some(ctrl_code), pressed: true,
                };
                crate::gui::handle_key_event(event);
            } else if ascii != 0 {
                let event = crate::drivers::keyboard::KeyEvent {
                    scancode, ascii: Some(ascii), pressed: true,
                };
                crate::gui::handle_key_event(event);
            } else if scancode != 0 {
                // Non-ASCII key (arrows, etc.)
                let event = crate::drivers::keyboard::KeyEvent {
                    scancode, ascii: None, pressed: true,
                };
                crate::gui::handle_key_event(event);
            }
        }
    }

    *prev = *report;
}

/// Process a mouse report, updating cursor position.
fn process_mouse_report(report: &MouseReport) {
    // Route relative motion to the mouse driver
    let dx = report.dx as i32;
    let dy = report.dy as i32;

    if dx != 0 || dy != 0 {
        crate::drivers::mouse::update_position_relative(dx, dy);
    }

    // TODO: Route button clicks to GUI
}

// ═══════════════════════════════════════════════════════════════
//  Polling Thread
// ═══════════════════════════════════════════════════════════════

/// USB HID polling thread entry point.
/// Polls HID devices at ~100Hz for keyboard/mouse reports.
pub fn usb_hid_poll_thread() {
    crate::serial_println!("[thread:usb-hid] Started.");

    // For now, USB HID polling is a placeholder.
    // Full implementation requires interrupt endpoint TRB submission
    // and completion polling on the xHCI transfer rings.
    // The current architecture supports it but we need the transfer ring
    // infrastructure per-endpoint.

    loop {
        if !HID_ACTIVE.load(Ordering::Relaxed) {
            crate::process::scheduler::yield_now();
            continue;
        }

        // In a full implementation, we would:
        // 1. Submit interrupt IN TRBs on each HID endpoint's transfer ring
        // 2. Poll for completion events
        // 3. Parse the 8-byte keyboard report or 3-byte mouse report
        // 4. Call process_keyboard_report() or process_mouse_report()

        // For now, yield to avoid busy-spinning
        crate::process::scheduler::yield_now();
    }
}

// ═══════════════════════════════════════════════════════════════
//  Diagnostics
// ═══════════════════════════════════════════════════════════════

/// Check if USB HID is active.
pub fn is_active() -> bool {
    HID_ACTIVE.load(Ordering::Relaxed)
}

/// Get count of HID keyboards.
pub fn keyboard_count() -> usize {
    KEYBOARD_SLOTS.lock().len()
}

/// Get count of HID mice.
pub fn mouse_count() -> usize {
    MOUSE_SLOTS.lock().len()
}

/// Get count of HID gamepads.
pub fn gamepad_count() -> usize {
    GAMEPAD_SLOTS.lock().len()
}

/// PS/2 Mouse driver for Smart OS.
///
/// Communicates via port 0x60 (data) and 0x64 (command/status).
/// The mouse sends 3-byte packets on IRQ12 (vector 44, PIC2 line 4).
/// We decode packets into absolute position + button state and buffer events.

use spin::Mutex;
use x86_64::instructions::port::Port;
use core::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

/// PS/2 controller ports.
const DATA_PORT: u16 = 0x60;
const CMD_PORT: u16 = 0x64;
const STATUS_PORT: u16 = 0x64;

/// Screen bounds for clamping mouse position.
static SCREEN_W: AtomicUsize = AtomicUsize::new(800);
static SCREEN_H: AtomicUsize = AtomicUsize::new(600);

/// Absolute mouse position (clamped to screen).
static MOUSE_X: AtomicI32 = AtomicI32::new(400);
static MOUSE_Y: AtomicI32 = AtomicI32::new(300);

/// A mouse event.
#[derive(Debug, Clone, Copy)]
pub struct MouseEvent {
    pub dx: i16,
    pub dy: i16,
    pub left: bool,
    pub right: bool,
    pub middle: bool,
}

/// Circular buffer for mouse events.
const EVENT_BUF_SIZE: usize = 64;

struct MouseEventBuffer {
    buffer: [Option<MouseEvent>; EVENT_BUF_SIZE],
    read_pos: usize,
    write_pos: usize,
    count: usize,
}

impl MouseEventBuffer {
    const fn new() -> Self {
        Self {
            buffer: [None; EVENT_BUF_SIZE],
            read_pos: 0,
            write_pos: 0,
            count: 0,
        }
    }

    fn push(&mut self, event: MouseEvent) {
        if self.count < EVENT_BUF_SIZE {
            self.buffer[self.write_pos] = Some(event);
            self.write_pos = (self.write_pos + 1) % EVENT_BUF_SIZE;
            self.count += 1;
        }
    }

    fn pop(&mut self) -> Option<MouseEvent> {
        if self.count == 0 {
            return None;
        }
        let event = self.buffer[self.read_pos].take();
        self.read_pos = (self.read_pos + 1) % EVENT_BUF_SIZE;
        self.count -= 1;
        event
    }
}

static EVENTS: Mutex<MouseEventBuffer> = Mutex::new(MouseEventBuffer::new());

/// Packet assembly state machine (3-byte PS/2 protocol).
struct PacketState {
    bytes: [u8; 3],
    index: u8,
}

impl PacketState {
    const fn new() -> Self {
        Self {
            bytes: [0; 3],
            index: 0,
        }
    }
}

static PACKET: Mutex<PacketState> = Mutex::new(PacketState::new());

/// Wait until the PS/2 controller input buffer is empty (ready to receive command).
fn wait_write() {
    for _ in 0..100_000 {
        let status: u8 = unsafe { Port::new(STATUS_PORT).read() };
        if status & 0x02 == 0 {
            return;
        }
    }
}

/// Wait until the PS/2 controller output buffer has data (ready to read).
fn wait_read() {
    for _ in 0..100_000 {
        let status: u8 = unsafe { Port::new(STATUS_PORT).read() };
        if status & 0x01 != 0 {
            return;
        }
    }
}

/// Send a command byte to the PS/2 controller (port 0x64).
fn controller_cmd(cmd: u8) {
    wait_write();
    unsafe { Port::new(CMD_PORT).write(cmd); }
}

/// Send a byte to the mouse device (write 0xD4 to controller, then data to port 0x60).
fn mouse_write(data: u8) {
    controller_cmd(0xD4); // "next byte goes to mouse"
    wait_write();
    unsafe { Port::new(DATA_PORT).write(data); }
}

/// Read a byte from the PS/2 data port.
fn mouse_read() -> u8 {
    wait_read();
    unsafe { Port::new(DATA_PORT).read() }
}

/// Initialize the PS/2 mouse.
pub fn init() {
    // Enable the auxiliary (mouse) PS/2 port
    controller_cmd(0xA8);

    // Read the controller configuration byte
    controller_cmd(0x20);
    let mut config = mouse_read();

    // Enable IRQ12 (bit 1) and keep IRQ1 (bit 0) enabled
    config |= 0x02; // Enable auxiliary interrupt
    config &= !0x20; // Make sure auxiliary clock is not disabled

    // Write updated config
    controller_cmd(0x60);
    wait_write();
    unsafe { Port::new(DATA_PORT).write(config); }

    // Reset mouse defaults
    mouse_write(0xF6); // Set defaults
    let _ack = mouse_read(); // Read ACK (0xFA)

    // Enable data reporting
    mouse_write(0xF4); // Enable
    let _ack = mouse_read(); // Read ACK

    crate::serial_println!("[drivers] PS/2 mouse initialized.");
}

/// Set screen bounds for position clamping (call after framebuffer is available).
pub fn set_screen_bounds(width: usize, height: usize) {
    SCREEN_W.store(width, Ordering::Relaxed);
    SCREEN_H.store(height, Ordering::Relaxed);
    // Center the cursor
    MOUSE_X.store((width / 2) as i32, Ordering::Relaxed);
    MOUSE_Y.store((height / 2) as i32, Ordering::Relaxed);
}

/// Called from IRQ12 interrupt handler. Assembles 3-byte packets.
pub fn handle_packet() {
    let byte: u8 = unsafe { Port::new(DATA_PORT).read() };

    let mut pkt = PACKET.lock();

    // First byte must have bit 3 set (always-1 bit in PS/2 protocol)
    if pkt.index == 0 && byte & 0x08 == 0 {
        // Out of sync — skip this byte
        return;
    }

    let idx = pkt.index as usize;
    pkt.bytes[idx] = byte;
    pkt.index += 1;

    if pkt.index >= 3 {
        // We have a complete 3-byte packet
        let flags = pkt.bytes[0];
        let raw_dx = pkt.bytes[1] as i16;
        let raw_dy = pkt.bytes[2] as i16;

        // Apply sign extension from flags byte
        let dx = if flags & 0x10 != 0 { raw_dx - 256 } else { raw_dx };
        let dy = if flags & 0x20 != 0 { raw_dy - 256 } else { raw_dy };

        let left = flags & 0x01 != 0;
        let right = flags & 0x02 != 0;
        let middle = flags & 0x04 != 0;

        // Update absolute position (PS/2 Y is inverted — positive = up)
        let sw = SCREEN_W.load(Ordering::Relaxed) as i32;
        let sh = SCREEN_H.load(Ordering::Relaxed) as i32;

        let old_x = MOUSE_X.load(Ordering::Relaxed);
        let old_y = MOUSE_Y.load(Ordering::Relaxed);
        let new_x = (old_x + dx as i32).clamp(0, sw.saturating_sub(1));
        let new_y = (old_y - dy as i32).clamp(0, sh.saturating_sub(1)); // Inverted Y
        MOUSE_X.store(new_x, Ordering::Relaxed);
        MOUSE_Y.store(new_y, Ordering::Relaxed);

        let event = MouseEvent {
            dx: dx as i16,
            dy: dy as i16,
            left,
            right,
            middle,
        };
        EVENTS.lock().push(event);

        pkt.index = 0;
    }
}

/// Read the next mouse event from the buffer (non-blocking).
pub fn read_event() -> Option<MouseEvent> {
    EVENTS.lock().pop()
}

/// Get current absolute mouse position.
pub fn position() -> (i32, i32) {
    (
        MOUSE_X.load(Ordering::Relaxed),
        MOUSE_Y.load(Ordering::Relaxed),
    )
}

/// Update mouse position from relative movement (used by USB HID).
pub fn update_position_relative(dx: i32, dy: i32) {
    let (screen_w, screen_h) = crate::gui::compositor::screen_size();
    let old_x = MOUSE_X.load(Ordering::Relaxed);
    let old_y = MOUSE_Y.load(Ordering::Relaxed);
    let new_x = (old_x + dx).max(0).min(screen_w as i32 - 1);
    let new_y = (old_y + dy).max(0).min(screen_h as i32 - 1);
    MOUSE_X.store(new_x, Ordering::Relaxed);
    MOUSE_Y.store(new_y, Ordering::Relaxed);
}

/// Get current button state from last event.
pub fn buttons() -> (bool, bool, bool) {
    // Return last known button state from the most recent packet
    // For simplicity, just peek at position — buttons are tracked via events
    (false, false, false) // Caller should use events for button tracking
}

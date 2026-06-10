/// VGA Text-Mode Emulation for Smart OS.
///
/// Phase 21: High-performance terminal compatibility layer.
/// This emulates the classic 80x25 text mode buffer at 0xB8000
/// by translating it into the modern GUI compositor's framebuffer.

use crate::gui::compositor::{self, COMPOSITOR};
use crate::gui::theme::Color;
use spin::Mutex;

const VGA_WIDTH: usize = 80;
const VGA_HEIGHT: usize = 25;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ScreenChar {
    pub ascii_character: u8,
    pub color_code: u8,
}

pub struct VgaEmulator {
    buffer: [[ScreenChar; VGA_WIDTH]; VGA_HEIGHT],
    cursor_x: usize,
    cursor_y: usize,
}

impl VgaEmulator {
    pub fn new() -> Self {
        Self {
            buffer: [[ScreenChar { ascii_character: b' ', color_code: 0x07 }; VGA_WIDTH]; VGA_HEIGHT],
            cursor_x: 0,
            cursor_y: 0,
        }
    }

    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                if self.cursor_x >= VGA_WIDTH {
                    self.new_line();
                }

                let row = self.cursor_y;
                let col = self.cursor_x;

                self.buffer[row][col] = ScreenChar {
                    ascii_character: byte,
                    color_code: 0x07, // Light gray on black
                };
                self.cursor_x += 1;
            }
        }
    }

    fn new_line(&mut self) {
        self.cursor_x = 0;
        if self.cursor_y < VGA_HEIGHT - 1 {
            self.cursor_y += 1;
        } else {
            // Scroll
            for row in 1..VGA_HEIGHT {
                self.buffer[row - 1] = self.buffer[row];
            }
            self.buffer[VGA_HEIGHT - 1] = [ScreenChar { ascii_character: b' ', color_code: 0x07 }; VGA_WIDTH];
        }
    }

    /// Maps a VGA color code (0-15) to an RGB Color.
    fn map_color(&self, code: u8) -> Color {
        match code & 0x0F {
            0x00 => Color::rgb(0, 0, 0),       // Black
            0x01 => Color::rgb(0, 0, 170),     // Blue
            0x02 => Color::rgb(0, 170, 0),     // Green
            0x03 => Color::rgb(0, 170, 170),   // Cyan
            0x04 => Color::rgb(170, 0, 0),     // Red
            0x05 => Color::rgb(170, 0, 170),   // Magenta
            0x06 => Color::rgb(170, 85, 0),    // Brown
            0x07 => Color::rgb(170, 170, 170), // Light Gray
            0x08 => Color::rgb(85, 85, 85),    // Dark Gray
            0x09 => Color::rgb(85, 85, 255),   // Light Blue
            0x0A => Color::rgb(85, 255, 85),   // Light Green
            0x0B => Color::rgb(85, 255, 255),  // Light Cyan
            0x0C => Color::rgb(255, 85, 85),   // Light Red
            0x0D => Color::rgb(255, 85, 255),  // Light Magenta
            0x0E => Color::rgb(255, 255, 85),  // Yellow
            0x0F => Color::rgb(255, 255, 255), // White
            _ => Color::rgb(0, 0, 0),
        }
    }

    /// Translates the VGA buffer to the modern GUI compositor.
    pub fn sync_to_gui(&self) {
        let mut comp_lock = COMPOSITOR.lock();
        if let Some(ref mut comp) = *comp_lock {
            // Calculate centering
            let start_x = (comp.width - (VGA_WIDTH * 8)) / 2;
            let start_y = (comp.height - (VGA_HEIGHT * 16)) / 2;

            // Draw a background border/shadow for the terminal
            comp.fill_rect(start_x - 4, start_y - 4, (VGA_WIDTH * 8) + 8, (VGA_HEIGHT * 16) + 8, Color::rgb(40, 40, 40));

            for row in 0..VGA_HEIGHT {
                for col in 0..VGA_WIDTH {
                    let screen_char = self.buffer[row][col];
                    let fg = self.map_color(screen_char.color_code & 0x0F);
                    let bg = self.map_color((screen_char.color_code >> 4) & 0x0F);

                    let x = start_x + col * 8;
                    let y = start_y + row * 16;

                    // Draw background
                    comp.fill_rect(x, y, 8, 16, bg);
                    // Draw character
                    if screen_char.ascii_character != b' ' {
                        comp.draw_char(x, y, screen_char.ascii_character as char, fg);
                    }
                }
            }
        }
    }
}

pub static VGA_EMU: Mutex<VgaEmulator> = Mutex::new(VgaEmulator {
    buffer: [[ScreenChar { ascii_character: b' ', color_code: 0x07 }; VGA_WIDTH]; VGA_HEIGHT],
    cursor_x: 0,
    cursor_y: 0,
});

pub fn init() {
    crate::serial_println!("[vga_emu] Legacy VGA Text-Mode emulation initialized.");
}

/// Thread that periodically syncs the VGA buffer to the screen.
pub fn vga_sync_thread() {
    loop {
        {
            let emu = VGA_EMU.lock();
            emu.sync_to_gui();
        }
        for _ in 0..10 { crate::process::scheduler::yield_now(); }
    }
}

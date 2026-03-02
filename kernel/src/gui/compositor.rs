/// Smart OS Compositor — Double-buffered software framebuffer renderer.
///
/// Provides low-level drawing primitives and page-flipping for tear-free rendering.
/// All rendering happens to a back buffer, then `flip()` copies to the real framebuffer.

use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use super::theme::Color;
use super::font;

/// Global compositor instance.
pub static COMPOSITOR: Mutex<Option<Compositor>> = Mutex::new(None);

/// The software compositor with double buffering.
pub struct Compositor {
    /// Pointer to the real framebuffer (hardware).
    fb_addr: *mut u8,
    /// Back buffer (we draw here, then flip).
    back_buffer: Vec<u8>,
    /// Screen width in pixels.
    pub width: usize,
    /// Screen height in pixels.
    pub height: usize,
    /// Bytes per scanline.
    stride: usize,
    /// Bytes per pixel (3 or 4).
    bpp: usize,
    /// Whether pixel format is BGR (common in UEFI) vs RGB.
    is_bgr: bool,
}

// Safety: framebuffer pointer is only accessed through the compositor's Mutex.
unsafe impl Send for Compositor {}

impl Compositor {
    /// Create a new compositor.
    fn new(fb_addr: *mut u8, width: usize, height: usize, stride: usize, bpp: usize, is_bgr: bool) -> Self {
        let buf_size = stride * height;
        Self {
            fb_addr,
            back_buffer: vec![0u8; buf_size],
            width,
            height,
            stride,
            bpp,
            is_bgr,
        }
    }

    /// Set a single pixel in the back buffer.
    #[inline]
    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = y * self.stride + x * self.bpp;
        if offset + 2 < self.back_buffer.len() {
            if self.is_bgr {
                self.back_buffer[offset] = color.b;
                self.back_buffer[offset + 1] = color.g;
                self.back_buffer[offset + 2] = color.r;
            } else {
                self.back_buffer[offset] = color.r;
                self.back_buffer[offset + 1] = color.g;
                self.back_buffer[offset + 2] = color.b;
            }
        }
    }

    /// Fill a rectangle with a solid color.
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        for py in y..y_end {
            let row_offset = py * self.stride + x * self.bpp;
            for px in x..x_end {
                let offset = row_offset + (px - x) * self.bpp;
                if offset + 2 < self.back_buffer.len() {
                    if self.is_bgr {
                        self.back_buffer[offset] = color.b;
                        self.back_buffer[offset + 1] = color.g;
                        self.back_buffer[offset + 2] = color.r;
                    } else {
                        self.back_buffer[offset] = color.r;
                        self.back_buffer[offset + 1] = color.g;
                        self.back_buffer[offset + 2] = color.b;
                    }
                }
            }
        }
    }

    /// Draw a horizontal line.
    pub fn hline(&mut self, x: usize, y: usize, w: usize, color: Color) {
        self.fill_rect(x, y, w, 1, color);
    }

    /// Draw a vertical line.
    pub fn vline(&mut self, x: usize, y: usize, h: usize, color: Color) {
        self.fill_rect(x, y, 1, h, color);
    }

    /// Draw a rectangle outline (1px border).
    pub fn draw_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        if w == 0 || h == 0 {
            return;
        }
        self.hline(x, y, w, color);           // top
        self.hline(x, y + h - 1, w, color);   // bottom
        self.vline(x, y, h, color);            // left
        self.vline(x + w - 1, y, h, color);   // right
    }

    /// Draw a single character at (x, y) with the given color.
    /// Returns the width of the character (8 pixels).
    pub fn draw_char(&mut self, x: usize, y: usize, ch: u8, color: Color) -> usize {
        let glyph = font::get_char(ch);
        for row in 0..16 {
            let bits = glyph[row];
            for col in 0..8 {
                if bits & (0x80 >> col) != 0 {
                    self.set_pixel(x + col, y + row, color);
                }
            }
        }
        8
    }

    /// Draw a string at (x, y). Returns the total width in pixels.
    pub fn draw_text(&mut self, x: usize, y: usize, text: &str, color: Color) -> usize {
        let mut cx = x;
        for &byte in text.as_bytes() {
            cx += self.draw_char(cx, y, byte, color);
        }
        cx - x
    }

    /// Draw text with a background color (filled behind each character cell).
    pub fn draw_text_bg(&mut self, x: usize, y: usize, text: &str, fg: Color, bg: Color) -> usize {
        let text_len = text.len();
        // Fill background rectangle
        self.fill_rect(x, y, text_len * 8, 16, bg);
        // Draw text on top
        self.draw_text(x, y, text, fg)
    }

    /// Draw a neon glow border around a rectangle (multi-layer dimming effect).
    pub fn draw_glow_border(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color, layers: usize) {
        for i in 0..layers {
            let dim_factor = 255 - (i as u16 * 80).min(240) as u8;
            let glow_color = color.dim(dim_factor);
            if x >= i && y >= i && w + 2 * i > 0 && h + 2 * i > 0 {
                self.draw_rect(
                    x.saturating_sub(i),
                    y.saturating_sub(i),
                    w + 2 * i,
                    h + 2 * i,
                    glow_color,
                );
            }
        }
    }

    /// Draw a horizontal gradient bar (for progress bars, accents).
    pub fn draw_gradient_h(&mut self, x: usize, y: usize, w: usize, h: usize, from: Color, to: Color) {
        if w == 0 {
            return;
        }
        for px in 0..w {
            let factor = ((px as u32 * 255) / w as u32) as u8;
            let color = from.blend(to, factor);
            for py in 0..h {
                self.set_pixel(x + px, y + py, color);
            }
        }
    }

    /// Clear the entire back buffer to a color.
    pub fn clear(&mut self, color: Color) {
        self.fill_rect(0, 0, self.width, self.height, color);
    }

    /// Flip: copy the back buffer to the real framebuffer.
    pub fn flip_to_screen(&mut self) {
        let size = self.stride * self.height;
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.back_buffer.as_ptr(),
                self.fb_addr,
                size,
            );
        }
    }
}

/// Initialize the compositor with framebuffer info.
pub fn init(fb_addr: *mut u8, width: usize, height: usize, stride: usize, bpp: usize, is_bgr: bool) {
    let comp = Compositor::new(fb_addr, width, height, stride, bpp, is_bgr);
    *COMPOSITOR.lock() = Some(comp);
}

/// Flip the back buffer to the screen.
pub fn flip() {
    if let Some(ref mut comp) = *COMPOSITOR.lock() {
        comp.flip_to_screen();
    }
}

/// Get screen dimensions.
pub fn screen_size() -> (usize, usize) {
    if let Some(ref comp) = *COMPOSITOR.lock() {
        (comp.width, comp.height)
    } else {
        (0, 0)
    }
}

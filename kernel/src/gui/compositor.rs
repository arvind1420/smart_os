/// Smart OS Compositor — Software/Hardware Hybrid Renderer.
///
/// Provides low-level drawing primitives and page-flipping for tear-free rendering.
/// All rendering happens to a back buffer (VRAM or RAM), then `flip()` swaps/copies to the screen.

use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use super::theme::Color;
use super::font;
use crate::drivers::drm::{DRM, FramebufferObj};

/// Global compositor instance.
pub static COMPOSITOR: Mutex<Option<Compositor>> = Mutex::new(None);

/// The software/hardware hybrid compositor.
pub struct Compositor {
    /// UEFI physical fallback address.
    fb_addr: *mut u8,
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

    // ── Hardware Acceleration ──
    /// Hardware-backed framebuffer for this window (if DRM active).
    pub back_fb: Option<FramebufferObj>,
    /// The front buffer object in VRAM.
    pub front_fb: Option<FramebufferObj>,
    /// Saved target FB stack (for nested rendering).
    target_stack: Vec<Option<FramebufferObj>>,
    /// CPU back buffer (fallback if no DRM).
    pub software_back_buffer: Option<Vec<u8>>,
}

// Safety: framebuffer pointer is only accessed through the compositor's Mutex.
unsafe impl Send for Compositor {}

impl Compositor {
    /// Create a new compositor.
    fn new(fb_addr: *mut u8, width: usize, height: usize, stride: usize, bpp: usize, is_bgr: bool) -> Self {
        let mut back_fb = None;
        let mut front_fb = None;
        let mut software_back_buffer = None;

        // Try to allocate hardware framebuffers via DRM
        let mut drm = DRM.lock();
        if let Some(ref mut driver) = drm.active_driver {
            if let Ok(fb1) = driver.alloc_framebuffer(width as u32, height as u32, 0) {
                if let Ok(fb2) = driver.alloc_framebuffer(width as u32, height as u32, 0) {
                    crate::serial_println!("[compositor] Allocated hardware double buffers.");
                    back_fb = Some(fb1);
                    front_fb = Some(fb2);
                }
            }
        }

        if back_fb.is_none() {
            crate::serial_println!("[compositor] Falling back to software back-buffer.");
            software_back_buffer = Some(vec![0u8; stride * height]);
        }

        Self {
            fb_addr,
            width,
            height,
            stride,
            bpp,
            is_bgr,
            back_fb,
            front_fb,
            target_stack: Vec::new(),
            software_back_buffer,
        }
    }

    /// Change render target to a different framebuffer.
    pub fn push_target(&mut self, fb: Option<FramebufferObj>) {
        let old = core::mem::replace(&mut self.back_fb, fb);
        self.target_stack.push(old);
    }

    /// Restore previous render target.
    pub fn pop_target(&mut self) {
        if let Some(old) = self.target_stack.pop() {
            self.back_fb = old;
        }
    }

    /// Get current active draw target address (the back buffer).
    fn back_buffer_ptr(&mut self) -> *mut u8 {
        if let Some(ref fb) = self.back_fb {
            fb.virt_addr as *mut u8
        } else {
            self.software_back_buffer.as_mut().unwrap().as_mut_ptr()
        }
    }

    /// Set a single pixel in the back buffer.
    #[inline]
    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = y * self.stride + x * self.bpp;
        let ptr = self.back_buffer_ptr();
        unsafe {
            let pixel_ptr = ptr.add(offset);
            if self.is_bgr {
                *pixel_ptr = color.b;
                *pixel_ptr.add(1) = color.g;
                *pixel_ptr.add(2) = color.r;
            } else {
                *pixel_ptr = color.r;
                *pixel_ptr.add(1) = color.g;
                *pixel_ptr.add(2) = color.b;
            }
        }
    }

    /// Fill a rectangle with a solid color.
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        // Hardware acceleration check
        {
            let mut drm = DRM.lock();
            if let Some(ref mut driver) = drm.active_driver {
                if let Some(ref fb) = self.back_fb {
                    let col_u32 = ((color.r as u32) << 16) | ((color.g as u32) << 8) | (color.b as u32);
                    if driver.fill_rect(fb.id, x as u32, y as u32, w as u32, h as u32, col_u32).is_ok() {
                        return;
                    }
                }
            }
        }

        // Software fallback
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        let ptr = self.back_buffer_ptr();
        for py in y..y_end {
            for px in x..x_end {
                let offset = py * self.stride + px * self.bpp;
                unsafe {
                    let pixel_ptr = ptr.add(offset);
                    if self.is_bgr {
                        *pixel_ptr = color.b;
                        *pixel_ptr.add(1) = color.g;
                        *pixel_ptr.add(2) = color.r;
                    } else {
                        *pixel_ptr = color.r;
                        *pixel_ptr.add(1) = color.g;
                        *pixel_ptr.add(2) = color.b;
                    }
                }
            }
        }
    }

    /// Hardware-accelerated blit (if supported).
    pub fn hardware_blit(&mut self, src_fb_id: u32, dst_x: u32, dst_y: u32, w: u32, h: u32) -> bool {
        let mut drm = DRM.lock();
        if let Some(ref mut driver) = drm.active_driver {
            if let Some(ref dst_fb) = self.back_fb {
                return driver.blit(src_fb_id, dst_fb.id, 0, 0, dst_x, dst_y, w, h).is_ok();
            }
        }
        false
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

    /// Draw a string at (x, y).
    pub fn draw_text(&mut self, x: usize, y: usize, text: &str, color: Color) -> usize {
        let mut cx = x;
        for &byte in text.as_bytes() {
            cx += self.draw_char(cx, y, byte, color);
        }
        cx - x
    }

    /// Draw text with a background color.
    pub fn draw_text_bg(&mut self, x: usize, y: usize, text: &str, fg: Color, bg: Color) -> usize {
        let text_len = text.len();
        self.fill_rect(x, y, text_len * 8, 16, bg);
        self.draw_text(x, y, text, fg)
    }

    /// Draw a neon glow border.
    pub fn draw_glow_border(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color, layers: usize) {
        for i in 0..layers {
            let dim_factor = 255 - (i as u16 * 80).min(240) as u8;
            let glow_color = color.dim(dim_factor);
            if x >= i && y >= i {
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

    /// Draw a horizontal gradient bar.
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

    /// Flip: swap back buffer to the screen.
    pub fn flip_to_screen(&mut self) {
        if let (Some(ref mut back), Some(ref mut front)) = (self.back_fb.as_mut(), self.front_fb.as_mut()) {
            // Hardware Page Flip
            let mut drm = DRM.lock();
            if let Some(ref mut driver) = drm.active_driver {
                if driver.set_crtc(1, back.id).is_ok() {
                    core::mem::swap(back, front);
                    return;
                }
            }
        }

        // Software Flip fallback
        let ptr = if let Some(ref fb) = self.back_fb {
            fb.virt_addr as *const u8
        } else if let Some(ref sw_buf) = self.software_back_buffer {
            sw_buf.as_ptr()
        } else {
            return;
        };

        let size = self.stride * self.height;
        unsafe {
            core::ptr::copy_nonoverlapping(ptr, self.fb_addr, size);
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

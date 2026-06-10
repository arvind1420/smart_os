/// Smart OS Compositor — Software/Hardware Hybrid Renderer.
///
/// Provides low-level drawing primitives and page-flipping for tear-free rendering.
/// All rendering happens to a back buffer (VRAM or RAM), then `flip()` swaps/copies to the screen.

use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicUsize, Ordering};
use super::theme::Color;
use super::font;
use super::bidi::bidi_reorder;
use crate::drivers::drm::{DRM, FramebufferObj};

/// Global compositor instance.
pub static COMPOSITOR: Mutex<Option<Compositor>> = Mutex::new(None);

/// Lock-free framebuffer dimensions (set during init, safe to read without COMPOSITOR lock).
static FB_WIDTH: AtomicUsize = AtomicUsize::new(0);
static FB_HEIGHT: AtomicUsize = AtomicUsize::new(0);

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

        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        if x_end <= x || y_end <= y { return; }
        let row_pixels = x_end - x;
        let ptr = self.back_buffer_ptr();

        // Fast path for 32bpp (UEFI default): write u32 per pixel so the
        // compiler can vectorise with SSE/AVX instead of 3 separate byte stores.
        if self.bpp == 4 {
            let color_u32: u32 = if self.is_bgr {
                (color.b as u32) | ((color.g as u32) << 8) | ((color.r as u32) << 16)
            } else {
                (color.r as u32) | ((color.g as u32) << 8) | ((color.b as u32) << 16)
            };
            for py in y..y_end {
                let row_ptr = unsafe { ptr.add(py * self.stride + x * 4) as *mut u32 };
                let row: &mut [u32] = unsafe { core::slice::from_raw_parts_mut(row_ptr, row_pixels) };
                row.fill(color_u32);
            }
            return;
        }

        // Slow path for 24bpp or other formats
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
    pub fn draw_char(&mut self, x: usize, y: usize, ch: char, color: Color) -> usize {
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
    ///
    /// Automatically uses TrueType at 14 px when the font subsystem is ready
    /// (loaded by `drivers::font::init()`); falls back to the built-in 8×16
    /// bitmap font otherwise.  `y` is always the TOP of the text cell.
    pub fn draw_text(&mut self, x: usize, y: usize, text: &str, color: Color) -> usize {
        const UI_PX: f32 = 14.0;
        let font_id = unsafe { crate::drivers::font::SANS_SERIF_FONT_ID };

        if font_id != 0 {
            let mut fg = crate::drivers::font::FONTS.lock();
            if let Some(fm) = fg.as_mut() {
                let ascent_px = fm.metrics(font_id, UI_PX)
                    .map(|m| m.ascent.max(0) as i32)
                    .unwrap_or(11);
                let baseline_y = y as i32 + ascent_px;
                let reordered = bidi_reorder(text);
                let mut cx = x as i32;
                for ch in reordered.chars() {
                    if let Some(bmp) = fm.render_char(font_id, ch, UI_PX) {
                        let adv = bmp.advance as i32;
                        for row in 0..bmp.height {
                            let py = baseline_y - bmp.y_off - bmp.height as i32 + row as i32;
                            if py < 0 || py >= self.height as i32 { continue; }
                            for col in 0..bmp.width {
                                let px = cx + col as i32 + bmp.x_off;
                                if px < 0 || px >= self.width as i32 { continue; }
                                let alpha = bmp.coverage[
                                    ((bmp.height - 1 - row) * bmp.width + col) as usize];
                                if alpha == 0 { continue; }
                                if alpha >= 240 {
                                    self.set_pixel(px as usize, py as usize, color);
                                } else {
                                    let bg = self.get_pixel(px as usize, py as usize);
                                    let a = alpha as u32;
                                    let na = 255 - a;
                                    self.set_pixel(px as usize, py as usize, Color {
                                        r: ((color.r as u32 * a + bg.r as u32 * na) / 255) as u8,
                                        g: ((color.g as u32 * a + bg.g as u32 * na) / 255) as u8,
                                        b: ((color.b as u32 * a + bg.b as u32 * na) / 255) as u8,
                                    });
                                }
                            }
                        }
                        cx += adv;
                    } else {
                        cx += 8; // unknown glyph fallback advance
                    }
                }
                return (cx - x as i32).max(0) as usize;
            }
        }

        // Bitmap fallback
        let reordered = bidi_reorder(text);
        let mut cx = x;
        for ch in reordered.chars() {
            cx += self.draw_char(cx, y, ch, color);
        }
        cx - x
    }

    /// Draw text using TrueType at an explicit pixel size.  `y` is the TOP of
    /// the text cell (same coordinate convention as `draw_text`).
    /// Falls back to a scaled 8×16 bitmap if TTF is unavailable.
    pub fn draw_text_sized(&mut self, x: usize, y: usize, text: &str, color: Color, px_size: f32) -> usize {
        let font_id = unsafe { crate::drivers::font::SANS_SERIF_FONT_ID };
        if font_id != 0 {
            let mut fg = crate::drivers::font::FONTS.lock();
            if let Some(fm) = fg.as_mut() {
                let ascent_px = fm.metrics(font_id, px_size)
                    .map(|m| m.ascent.max(0) as i32)
                    .unwrap_or((px_size * 0.78) as i32);
                let baseline_y = y as i32 + ascent_px;
                let reordered = bidi_reorder(text);
                let mut cx = x as i32;
                for ch in reordered.chars() {
                    if let Some(bmp) = fm.render_char(font_id, ch, px_size) {
                        let adv = bmp.advance as i32;
                        for row in 0..bmp.height {
                            let py = baseline_y - bmp.y_off - bmp.height as i32 + row as i32;
                            if py < 0 || py >= self.height as i32 { continue; }
                            for col in 0..bmp.width {
                                let px = cx + col as i32 + bmp.x_off;
                                if px < 0 || px >= self.width as i32 { continue; }
                                let alpha = bmp.coverage[
                                    ((bmp.height - 1 - row) * bmp.width + col) as usize];
                                if alpha == 0 { continue; }
                                if alpha >= 240 {
                                    self.set_pixel(px as usize, py as usize, color);
                                } else {
                                    let bg = self.get_pixel(px as usize, py as usize);
                                    let a = alpha as u32;
                                    let na = 255 - a;
                                    self.set_pixel(px as usize, py as usize, Color {
                                        r: ((color.r as u32 * a + bg.r as u32 * na) / 255) as u8,
                                        g: ((color.g as u32 * a + bg.g as u32 * na) / 255) as u8,
                                        b: ((color.b as u32 * a + bg.b as u32 * na) / 255) as u8,
                                    });
                                }
                            }
                        }
                        cx += adv;
                    } else {
                        cx += (px_size * 0.6) as i32;
                    }
                }
                return (cx - x as i32).max(0) as usize;
            }
        }
        // Scaled bitmap fallback
        let scale = ((px_size / 8.0) as usize).max(1);
        let reordered = bidi_reorder(text);
        let mut cx = x;
        for ch in reordered.chars() {
            cx += self.draw_char_scaled(cx, y, ch, color, scale);
        }
        cx - x
    }

    /// Measure the rendered pixel width of `text` at `px_size` using TrueType.
    /// Returns `text.chars().count() * 8` as a fallback if TTF is not ready.
    pub fn measure_text(&self, text: &str, px_size: f32) -> usize {
        let font_id = unsafe { crate::drivers::font::SANS_SERIF_FONT_ID };
        if font_id != 0 {
            let mut fg = crate::drivers::font::FONTS.lock();
            if let Some(fm) = fg.as_mut() {
                return fm.measure_text(font_id, text, px_size) as usize;
            }
        }
        text.chars().count() * 8
    }

    /// Return the ascent in pixels for `px_size` (distance from top to baseline).
    pub fn ttf_ascent(&self, px_size: f32) -> usize {
        let font_id = unsafe { crate::drivers::font::SANS_SERIF_FONT_ID };
        if font_id != 0 {
            let mut fg = crate::drivers::font::FONTS.lock();
            if let Some(fm) = fg.as_mut() {
                if let Some(m) = fm.metrics(font_id, px_size) {
                    return m.ascent.max(0) as usize;
                }
            }
        }
        (px_size * 0.78) as usize
    }

    /// Return the full line height (ascent + |descent| + line_gap) in pixels.
    pub fn ttf_line_height(&self, px_size: f32) -> usize {
        let font_id = unsafe { crate::drivers::font::SANS_SERIF_FONT_ID };
        if font_id != 0 {
            let mut fg = crate::drivers::font::FONTS.lock();
            if let Some(fm) = fg.as_mut() {
                if let Some(m) = fm.metrics(font_id, px_size) {
                    return (m.ascent - m.descent + m.line_gap).max(px_size as i32) as usize;
                }
            }
        }
        px_size as usize
    }

    /// Draw a single character at (x, y) scaled `scale` times (integer scale).
    /// `scale=1` is the native 8×16 bitmap, `scale=2` is 16×32, etc.
    pub fn draw_char_scaled(&mut self, x: usize, y: usize, ch: char, color: Color, scale: usize) -> usize {
        if scale <= 1 {
            return self.draw_char(x, y, ch, color);
        }
        let glyph = font::get_char(ch);
        for row in 0..16 {
            let bits = glyph[row];
            for col in 0..8 {
                if bits & (0x80 >> col) != 0 {
                    // Paint a `scale × scale` square per source pixel.
                    let dx = x + col * scale;
                    let dy = y + row * scale;
                    self.fill_rect(dx, dy, scale, scale, color);
                }
            }
        }
        8 * scale
    }

    /// Draw a string at (x, y), scaled `scale` times.
    pub fn draw_text_scaled(&mut self, x: usize, y: usize, text: &str, color: Color, scale: usize) -> usize {
        let reordered = bidi_reorder(text);
        let mut cx = x;
        for ch in reordered.chars() {
            cx += self.draw_char_scaled(cx, y, ch, color, scale);
        }
        cx - x
    }

    /// Draw text with a background color.
    pub fn draw_text_bg(&mut self, x: usize, y: usize, text: &str, fg: Color, bg: Color) -> usize {
        let char_count = text.chars().count();
        self.fill_rect(x, y, char_count * 8, 16, bg);
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

    /// Draw a soft drop shadow for a rectangular area.
    /// Since the rectangle itself is opaque, we only blend and draw the shadow
    /// outside the rectangle [x, y, w, h], offset by ox, oy with a blur radius.
    pub fn draw_soft_shadow(&mut self, x: usize, y: usize, w: usize, h: usize, ox: usize, oy: usize, blur: usize, max_alpha: u32) {
        if w == 0 || h == 0 || blur == 0 {
            return;
        }

        // Right shadow: from px = x + w to x + w + ox + blur
        let start_rx = x + w;
        let end_rx = (x + w + ox + blur).min(self.width);

        // Bottom shadow: from py = y + h to y + h + oy + blur
        let start_by = y + h;
        let end_by = (y + h + oy + blur).min(self.height);

        // 1. Right shadow strip (excluding bottom-right corner)
        for px in start_rx..end_rx {
            let dist = px - start_rx;
            let alpha = if dist < ox {
                max_alpha
            } else {
                let fade = (dist - ox) as u32;
                max_alpha.saturating_sub(fade * max_alpha / blur as u32)
            };
            if alpha == 0 { continue; }

            for py in (y + oy).min(self.height)..start_by {
                let bg = self.get_pixel(px, py);
                let r = ((bg.r as u32 * (255 - alpha)) / 255) as u8;
                let g = ((bg.g as u32 * (255 - alpha)) / 255) as u8;
                let b = ((bg.b as u32 * (255 - alpha)) / 255) as u8;
                self.set_pixel(px, py, Color { r, g, b });
            }
        }

        // 2. Bottom shadow strip (excluding bottom-right corner)
        for py in start_by..end_by {
            let dist = py - start_by;
            let alpha = if dist < oy {
                max_alpha
            } else {
                let fade = (dist - oy) as u32;
                max_alpha.saturating_sub(fade * max_alpha / blur as u32)
            };
            if alpha == 0 { continue; }

            for px in (x + ox).min(self.width)..start_rx {
                let bg = self.get_pixel(px, py);
                let r = ((bg.r as u32 * (255 - alpha)) / 255) as u8;
                let g = ((bg.g as u32 * (255 - alpha)) / 255) as u8;
                let b = ((bg.b as u32 * (255 - alpha)) / 255) as u8;
                self.set_pixel(px, py, Color { r, g, b });
            }
        }

        // 3. Bottom-right corner
        for py in start_by..end_by {
            let dy = py - start_by;
            for px in start_rx..end_rx {
                let dx = px - start_rx;

                let dx_f = dx.saturating_sub(ox);
                let dy_f = dy.saturating_sub(oy);

                let dist = dx_f.max(dy_f);

                let alpha = if dx < ox && dy < oy {
                    max_alpha
                } else {
                    max_alpha.saturating_sub(dist as u32 * max_alpha / blur as u32)
                };
                if alpha == 0 { continue; }

                let bg = self.get_pixel(px, py);
                let r = ((bg.r as u32 * (255 - alpha)) / 255) as u8;
                let g = ((bg.g as u32 * (255 - alpha)) / 255) as u8;
                let b = ((bg.b as u32 * (255 - alpha)) / 255) as u8;
                self.set_pixel(px, py, Color { r, g, b });
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

    #[inline]
    pub fn get_pixel(&mut self, x: usize, y: usize) -> Color {
        if x >= self.width || y >= self.height {
            return Color { r: 0, g: 0, b: 0 };
        }
        let offset = y * self.stride + x * self.bpp;
        let ptr = self.back_buffer_ptr();
        unsafe {
            let pixel_ptr = ptr.add(offset);
            if self.is_bgr {
                Color {
                    b: *pixel_ptr,
                    g: *pixel_ptr.add(1),
                    r: *pixel_ptr.add(2),
                }
            } else {
                Color {
                    r: *pixel_ptr,
                    g: *pixel_ptr.add(1),
                    b: *pixel_ptr.add(2),
                }
            }
        }
    }

    pub fn draw_char_ttf(&mut self, x: usize, y: usize, ch: char, color: Color, px_size: f32) -> usize {
        let mut fonts = crate::drivers::font::FONTS.lock();
        let fm = match fonts.as_mut() {
            Some(f) => f,
            None => return self.draw_char(x, y, ch, color), // Fallback
        };
        let font_id = unsafe { crate::drivers::font::SANS_SERIF_FONT_ID };
        let bmp = match fm.render_char(font_id, ch, px_size) {
            Some(b) => b,
            None => return self.draw_char(x, y, ch, color), // Fallback
        };

        // Draw the coverage map with 16-bit/8-bit alpha blending.
        // We flip the rows because the TTF contour y-axis is up, but screen y-axis is down.
        for row in 0..bmp.height {
            let py = y as i32 - bmp.y_off - bmp.height as i32 + row as i32;
            if py < 0 || py >= self.height as i32 { continue; }
            for col in 0..bmp.width {
                let px = x as i32 + col as i32 + bmp.x_off;
                if px < 0 || px >= self.width as i32 { continue; }

                let alpha = bmp.coverage[((bmp.height - 1 - row) * bmp.width + col) as usize];
                if alpha == 0 { continue; }
                if alpha == 255 {
                    self.set_pixel(px as usize, py as usize, color);
                } else {
                    // Alpha blend
                    let bg = self.get_pixel(px as usize, py as usize);
                    let r = ((color.r as u32 * alpha as u32 + bg.r as u32 * (255 - alpha as u32)) / 255) as u8;
                    let g = ((color.g as u32 * alpha as u32 + bg.g as u32 * (255 - alpha as u32)) / 255) as u8;
                    let b = ((color.b as u32 * alpha as u32 + bg.b as u32 * (255 - alpha as u32)) / 255) as u8;
                    self.set_pixel(px as usize, py as usize, Color { r, g, b });
                }
            }
        }

        bmp.advance as usize
    }

    pub fn draw_text_ttf(&mut self, x: usize, y: usize, text: &str, color: Color, px_size: f32) -> usize {
        let reordered = bidi_reorder(text);
        let mut cx = x;
        for ch in reordered.chars() {
            cx += self.draw_char_ttf(cx, y, ch, color, px_size);
        }
        cx - x
    }
}

/// Initialize the compositor with framebuffer info.
pub fn init(fb_addr: *mut u8, width: usize, height: usize, stride: usize, bpp: usize, is_bgr: bool) {
    FB_WIDTH.store(width, Ordering::Relaxed);
    FB_HEIGHT.store(height, Ordering::Relaxed);
    let comp = Compositor::new(fb_addr, width, height, stride, bpp, is_bgr);
    *COMPOSITOR.lock() = Some(comp);
}

/// Flip the back buffer to the screen.
pub fn flip() {
    if let Some(ref mut comp) = *COMPOSITOR.lock() {
        comp.flip_to_screen();
    }
}

/// Get screen dimensions (acquires COMPOSITOR lock — do not call while holding COMPOSITOR).
pub fn screen_size() -> (usize, usize) {
    if let Some(ref comp) = *COMPOSITOR.lock() {
        (comp.width, comp.height)
    } else {
        (0, 0)
    }
}

/// Get framebuffer resolution without acquiring COMPOSITOR lock.
/// Safe to call from within rendering callbacks that already hold COMPOSITOR.
pub fn fb_resolution() -> (u32, u32) {
    (
        FB_WIDTH.load(Ordering::Relaxed) as u32,
        FB_HEIGHT.load(Ordering::Relaxed) as u32,
    )
}

// ── Wayland bridge helpers ────────────────────────────────────────────────────

/// Create a new window on the desktop and return its WindowId.
pub fn create_window(title: &str, x: usize, y: usize, w: usize, h: usize) -> u64 {
    let win = super::window::Window::new(
        title, x, y, w, h,
        super::theme::ACCENT_PURPLE,
    );
    let id = win.id;
    if let Some(ref mut desk) = *super::desktop::DESKTOP.lock() {
        desk.wm.add(win);
    }
    id
}

/// Destroy a window by id.
pub fn destroy_window(id: u64) {
    if let Some(ref mut desk) = *super::desktop::DESKTOP.lock() {
        desk.wm.hide(id);
    }
}

/// Bring a window to the front.
pub fn focus_window(id: u64) {
    if let Some(ref mut desk) = *super::desktop::DESKTOP.lock() {
        desk.wm.bring_to_front(id);
        desk.wm.set_active(id);
    }
}

/// Focus the first visible window whose title matches (case-sensitive prefix).
pub fn focus_window_by_title(title: &str) {
    if let Some(ref mut desk) = *super::desktop::DESKTOP.lock() {
        let id = desk.wm.windows.iter()
            .filter(|w| w.visible && w.title.starts_with(title))
            .next()
            .map(|w| w.id);
        if let Some(id) = id {
            desk.wm.bring_to_front(id);
            desk.wm.set_active(id);
        }
    }
}

/// Update a window's title bar.
pub fn set_window_title(id: u64, title: &str) {
    if let Some(ref mut desk) = *super::desktop::DESKTOP.lock() {
        if let Some(win) = desk.wm.get_mut(id) {
            win.title = alloc::string::String::from(title);
            win.dirty = true;
        }
    }
}

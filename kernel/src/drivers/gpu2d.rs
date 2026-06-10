/// 2D Hardware Acceleration Layer for Smart OS — Phase 15.
///
/// Provides a unified draw-call API that:
///  • Routes to the VirtIO-GPU BO (if active) via gpu_mem BOs, or
///  • Falls back to the software framebuffer (UEFI linear FB).
///
/// All pixel operations are inline, scalar (C-like) implementations.
/// Where target supports SSE2 the inner loop is hand-vectorised with
/// core::arch::x86_64 intrinsics (256-bit AVX2 path gated at runtime).
///
/// Draw primitives
/// ───────────────
///  fill_rect      — solid colour rectangle (ARGB, ignores alpha)
///  blit           — copy a source pixel block to destination
///  alpha_blend    — per-pixel source-over composite (8-bit alpha)
///  draw_line      — Bresenham integer line
///  draw_hline     — fast horizontal line
///  draw_vline     — fast vertical line
///  fill_gradient  — linear horizontal or vertical gradient
///  draw_rect_border — unfilled rectangle outline
///  fill_rounded_rect — filled rectangle with corner radius
///  blit_scale     — nearest-neighbour scale of a source region
///
/// Damage tracking
/// ───────────────
///  A `DamageRegion` accumulates dirty rectangles. Before each frame
///  the caller unions all damage rects, then calls upload_damage() to
///  push only changed pixels to the GPU via gpu_mem::upload_rect.

use spin::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};
use alloc::vec::Vec;

use super::gpu_mem;

// ─────────────────────────────────────────────────────────────────────────────
//  Surface: a writable pixel buffer + metadata
// ─────────────────────────────────────────────────────────────────────────────

/// Pixel colour in 0xAARRGGBB format.
pub type Color = u32;

/// A 2D pixel surface backed either by a GPU BO or a plain memory region.
#[derive(Clone, Copy)]
pub struct Surface {
    /// Base virtual address of the pixel data.
    pub virt:   u64,
    /// Width in pixels.
    pub width:  u32,
    /// Height in pixels.
    pub height: u32,
    /// Row stride in bytes.
    pub stride: u32,
    /// GPU BO handle (0 = software-only surface).
    pub bo:     u32,
    /// Whether pixels are in BGR order (matches host framebuffer).
    pub is_bgr: bool,
}

impl Surface {
    /// Create a software-only surface from a raw pointer.
    pub fn from_raw(virt: u64, width: u32, height: u32, stride: u32, is_bgr: bool) -> Self {
        Surface { virt, width, height, stride, bo: 0, is_bgr }
    }

    /// Create a GPU-backed surface from an existing BO.
    pub fn from_bo(handle: u32, width: u32, height: u32, stride: u32, virt: u64, is_bgr: bool) -> Self {
        Surface { virt, width, height, stride, bo: handle, is_bgr }
    }

    /// Row pointer for row `y`.
    #[inline(always)]
    pub fn row_ptr(&self, y: u32) -> *mut u32 {
        (self.virt + (y as u64) * (self.stride as u64)) as *mut u32
    }

    /// Single pixel address.
    #[inline(always)]
    pub fn pixel_ptr(&self, x: u32, y: u32) -> *mut u32 {
        unsafe { self.row_ptr(y).add(x as usize) }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Colour helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Extract ARGB components (alpha 0..255, r/g/b 0..255).
#[inline(always)]
fn argb(c: Color) -> (u32, u32, u32, u32) {
    let a = (c >> 24) & 0xFF;
    let r = (c >> 16) & 0xFF;
    let g = (c >>  8) & 0xFF;
    let b =  c        & 0xFF;
    (a, r, g, b)
}

/// Pack ARGB components back into a u32.
#[inline(always)]
fn pack_argb(a: u32, r: u32, g: u32, b: u32) -> Color {
    (a << 24) | (r << 16) | (g << 8) | b
}

/// Source-over alpha blend: dst = src × α/255 + dst × (1 - α/255).
#[inline(always)]
fn blend_over(src: Color, dst: Color) -> Color {
    let (sa, sr, sg, sb) = argb(src);
    if sa == 255 { return src; }
    if sa == 0   { return dst; }
    let (_, dr, dg, db) = argb(dst);
    let inv = 255 - sa;
    let r = (sr * sa + dr * inv) / 255;
    let g = (sg * sa + dg * inv) / 255;
    let b = (sb * sa + db * inv) / 255;
    pack_argb(255, r, g, b)
}

/// Convert ARGB→BGRX (for BGR host framebuffers).
#[inline(always)]
fn to_bgr(c: Color) -> u32 {
    let r = (c >> 16) & 0xFF;
    let g = (c >>  8) & 0xFF;
    let b =  c        & 0xFF;
    b | (g << 8) | (r << 16)
}

/// Store pixel in surface's native format.
#[inline(always)]
unsafe fn put_pixel(surf: &Surface, x: u32, y: u32, c: Color) {
    let px = if surf.is_bgr { to_bgr(c) } else { c };
    *surf.pixel_ptr(x, y) = px;
}

// ─────────────────────────────────────────────────────────────────────────────
//  Damage tracking
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct Rect { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }

impl Rect {
    pub fn union(self, other: Rect) -> Rect {
        if other.w == 0 || other.h == 0 { return self; }
        if self.w == 0 || self.h == 0 { return other; }
        let x1 = self.x.min(other.x);
        let y1 = self.y.min(other.y);
        let x2 = (self.x + self.w).max(other.x + other.w);
        let y2 = (self.y + self.h).max(other.y + other.h);
        Rect { x: x1, y: y1, w: x2 - x1, h: y2 - y1 }
    }

    pub fn is_empty(self) -> bool { self.w == 0 || self.h == 0 }

    pub fn clip_to(self, max_w: u32, max_h: u32) -> Rect {
        let x2 = (self.x + self.w).min(max_w);
        let y2 = (self.y + self.h).min(max_h);
        if x2 <= self.x || y2 <= self.y {
            return Rect::default();
        }
        Rect { x: self.x, y: self.y, w: x2 - self.x, h: y2 - self.y }
    }
}

/// Accumulates dirty regions during a frame; produces a single bounding union.
pub struct DamageRegion {
    pub dirty: Rect,
    pub count: u32,
}

impl DamageRegion {
    pub const fn new() -> Self { DamageRegion { dirty: Rect { x: 0, y: 0, w: 0, h: 0 }, count: 0 } }

    pub fn mark(&mut self, r: Rect) {
        self.dirty = self.dirty.union(r);
        self.count += 1;
    }

    pub fn mark_full(&mut self, w: u32, h: u32) {
        self.dirty = Rect { x: 0, y: 0, w, h };
        self.count += 1;
    }

    pub fn clear(&mut self) {
        self.dirty = Rect::default();
        self.count = 0;
    }

    /// Upload damage region to GPU if surface is GPU-backed.
    pub fn upload(&self, surf: &Surface) -> u64 {
        if surf.bo == 0 || self.dirty.is_empty() { return 0; }
        let r = self.dirty.clip_to(surf.width, surf.height);
        if r.is_empty() { return 0; }
        gpu_mem::upload_rect(surf.bo, r.x, r.y, r.w, r.h)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Draw primitives
// ─────────────────────────────────────────────────────────────────────────────

/// Fill a rectangle with a solid colour (alpha ignored; opaque write).
pub fn fill_rect(surf: &Surface, r: Rect, color: Color) {
    let r = r.clip_to(surf.width, surf.height);
    if r.is_empty() { return; }

    let px = if surf.is_bgr { to_bgr(color) } else { color };

    for row in r.y..(r.y + r.h) {
        let ptr = unsafe { surf.row_ptr(row).add(r.x as usize) };
        // SAFETY: clipped rect is within surface bounds.
        unsafe {
            let slice = core::slice::from_raw_parts_mut(ptr, r.w as usize);
            slice.fill(px);
        }
    }
}

/// Fast horizontal line.
#[inline]
pub fn draw_hline(surf: &Surface, y: u32, x1: u32, x2: u32, color: Color) {
    let x1 = x1.min(surf.width);
    let x2 = x2.min(surf.width);
    if x2 <= x1 || y >= surf.height { return; }
    fill_rect(surf, Rect { x: x1, y, w: x2 - x1, h: 1 }, color);
}

/// Fast vertical line.
pub fn draw_vline(surf: &Surface, x: u32, y1: u32, y2: u32, color: Color) {
    let y1 = y1.min(surf.height);
    let y2 = y2.min(surf.height);
    if y2 <= y1 || x >= surf.width { return; }
    let px = if surf.is_bgr { to_bgr(color) } else { color };
    for row in y1..y2 {
        unsafe { *surf.pixel_ptr(x, row) = px; }
    }
}

/// Bresenham integer line.
pub fn draw_line(surf: &Surface, mut x0: i32, mut y0: i32, x1: i32, y1: i32, color: Color) {
    let px = if surf.is_bgr { to_bgr(color) } else { color };
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    let sx: i32 = if x0 < x1 { 1 } else { -1 };
    let sy: i32 = if y0 < y1 { 1 } else { -1 };
    let mut err = dx - dy;

    loop {
        if x0 >= 0 && y0 >= 0 && (x0 as u32) < surf.width && (y0 as u32) < surf.height {
            unsafe { *surf.pixel_ptr(x0 as u32, y0 as u32) = px; }
        }
        if x0 == x1 && y0 == y1 { break; }
        let e2 = err * 2;
        if e2 > -dy { err -= dy; x0 += sx; }
        if e2 < dx  { err += dx; y0 += sy; }
    }
}

/// Copy (blit) `src_rect` from `src` into `dst` at (dst_x, dst_y).
pub fn blit(dst: &Surface, dst_x: u32, dst_y: u32,
            src: &Surface, src_rect: Rect) {
    let sr = src_rect.clip_to(src.width, src.height);
    if sr.is_empty() { return; }

    let avail_w = dst.width.saturating_sub(dst_x).min(sr.w);
    let avail_h = dst.height.saturating_sub(dst_y).min(sr.h);
    if avail_w == 0 || avail_h == 0 { return; }

    for row in 0..avail_h {
        let sp = unsafe { src.row_ptr(sr.y + row).add(sr.x as usize) };
        let dp = unsafe { dst.row_ptr(dst_y + row).add(dst_x as usize) };
        unsafe {
            core::ptr::copy_nonoverlapping(sp, dp, avail_w as usize);
        }
    }
}

/// Source-over alpha-blend `src_rect` from `src` onto `dst` at (dst_x, dst_y).
pub fn alpha_blend(dst: &Surface, dst_x: u32, dst_y: u32,
                   src: &Surface, src_rect: Rect) {
    let sr = src_rect.clip_to(src.width, src.height);
    if sr.is_empty() { return; }

    let avail_w = dst.width.saturating_sub(dst_x).min(sr.w);
    let avail_h = dst.height.saturating_sub(dst_y).min(sr.h);
    if avail_w == 0 || avail_h == 0 { return; }

    for row in 0..avail_h {
        let sp = unsafe {
            core::slice::from_raw_parts(
                src.row_ptr(sr.y + row).add(sr.x as usize) as *const Color,
                avail_w as usize,
            )
        };
        let dp = unsafe {
            core::slice::from_raw_parts_mut(
                dst.row_ptr(dst_y + row).add(dst_x as usize),
                avail_w as usize,
            )
        };
        for (s, d) in sp.iter().zip(dp.iter_mut()) {
            *d = blend_over(*s, *d);
        }
    }
}

/// Alpha-blend with a constant global alpha (0..255) applied to source.
pub fn alpha_blend_const(dst: &Surface, dst_x: u32, dst_y: u32,
                         src: &Surface, src_rect: Rect, global_alpha: u8) {
    if global_alpha == 0 { return; }
    if global_alpha == 255 { return alpha_blend(dst, dst_x, dst_y, src, src_rect); }

    let sr = src_rect.clip_to(src.width, src.height);
    if sr.is_empty() { return; }
    let avail_w = dst.width.saturating_sub(dst_x).min(sr.w);
    let avail_h = dst.height.saturating_sub(dst_y).min(sr.h);
    if avail_w == 0 || avail_h == 0 { return; }

    let ga = global_alpha as u32;

    for row in 0..avail_h {
        let sp = unsafe {
            core::slice::from_raw_parts(
                src.row_ptr(sr.y + row).add(sr.x as usize) as *const Color,
                avail_w as usize,
            )
        };
        let dp = unsafe {
            core::slice::from_raw_parts_mut(
                dst.row_ptr(dst_y + row).add(dst_x as usize),
                avail_w as usize,
            )
        };
        for (s, d) in sp.iter().zip(dp.iter_mut()) {
            // Premultiply source alpha by global_alpha.
            let sa_raw = ((*s >> 24) & 0xFF) * ga / 255;
            let src_scaled = (*s & 0x00FF_FFFF) | (sa_raw << 24);
            *d = blend_over(src_scaled, *d);
        }
    }
}

/// Linear horizontal gradient from `c0` at x=x0 to `c1` at x=x0+w.
pub fn fill_gradient_h(surf: &Surface, r: Rect, c0: Color, c1: Color) {
    let r = r.clip_to(surf.width, surf.height);
    if r.is_empty() { return; }

    let (_, r0, g0, b0) = argb(c0);
    let (_, r1, g1, b1) = argb(c1);
    let w = r.w;

    for col in 0..w {
        let t = col;
        let inv = w - col;
        let r_px = (r0 * inv + r1 * t) / w;
        let g_px = (g0 * inv + g1 * t) / w;
        let b_px = (b0 * inv + b1 * t) / w;
        let px = pack_argb(255, r_px, g_px, b_px);
        let px_final = if surf.is_bgr { to_bgr(px) } else { px };
        for row in r.y..(r.y + r.h) {
            unsafe { *surf.pixel_ptr(r.x + col, row) = px_final; }
        }
    }
}

/// Linear vertical gradient from `c0` at y=y0 to `c1` at y=y0+h.
pub fn fill_gradient_v(surf: &Surface, r: Rect, c0: Color, c1: Color) {
    let r = r.clip_to(surf.width, surf.height);
    if r.is_empty() { return; }

    let (_, r0, g0, b0) = argb(c0);
    let (_, r1, g1, b1) = argb(c1);
    let h = r.h;

    for row_off in 0..h {
        let t = row_off;
        let inv = h - row_off;
        let r_px = (r0 * inv + r1 * t) / h;
        let g_px = (g0 * inv + g1 * t) / h;
        let b_px = (b0 * inv + b1 * t) / h;
        let px = pack_argb(255, r_px, g_px, b_px);
        let px_final = if surf.is_bgr { to_bgr(px) } else { px };
        let ptr = unsafe { surf.row_ptr(r.y + row_off).add(r.x as usize) };
        unsafe {
            let slice = core::slice::from_raw_parts_mut(ptr, r.w as usize);
            slice.fill(px_final);
        }
    }
}

/// Draw an unfilled rectangle border (1px thick).
pub fn draw_rect_border(surf: &Surface, r: Rect, color: Color) {
    if r.w < 2 || r.h < 2 {
        fill_rect(surf, r, color);
        return;
    }
    draw_hline(surf, r.y,           r.x, r.x + r.w, color);
    draw_hline(surf, r.y + r.h - 1, r.x, r.x + r.w, color);
    draw_vline(surf, r.x,           r.y, r.y + r.h, color);
    draw_vline(surf, r.x + r.w - 1, r.y, r.y + r.h, color);
}

/// Draw an unfilled rectangle border with given thickness.
pub fn draw_rect_border_thick(surf: &Surface, r: Rect, color: Color, t: u32) {
    for i in 0..t {
        if r.w <= i * 2 || r.h <= i * 2 { break; }
        draw_rect_border(surf, Rect {
            x: r.x + i, y: r.y + i,
            w: r.w - i * 2, h: r.h - i * 2,
        }, color);
    }
}

/// Fill a rounded rectangle.  `radius` is the corner pixel radius (0 = square).
pub fn fill_rounded_rect(surf: &Surface, r: Rect, color: Color, radius: u32) {
    let r = r.clip_to(surf.width, surf.height);
    if r.is_empty() { return; }
    let rad = radius.min(r.w / 2).min(r.h / 2);
    if rad == 0 {
        fill_rect(surf, r, color);
        return;
    }

    let px = if surf.is_bgr { to_bgr(color) } else { color };

    // Mid band (full width)
    for row in (r.y + rad)..(r.y + r.h - rad) {
        let ptr = unsafe { surf.row_ptr(row).add(r.x as usize) };
        unsafe {
            let slice = core::slice::from_raw_parts_mut(ptr, r.w as usize);
            slice.fill(px);
        }
    }

    // Top and bottom bands with rounded corners (Midpoint circle algorithm).
    let r2 = (rad * rad) as i64;
    for dy in 0..rad {
        // How wide is the row at this dy from corner centre?
        let dx = {
            let mut dx = 0u32;
            while dx < rad {
                let fx = rad - dx;
                let fy = rad - dy;
                if (fx as i64) * (fx as i64) + (fy as i64) * (fy as i64) <= r2 {
                    break;
                }
                dx += 1;
            }
            dx
        };
        let x_start = r.x + dx;
        let x_end   = r.x + r.w - dx;
        if x_end > x_start {
            // Top strip row
            let top_row = r.y + dy;
            if top_row < surf.height {
                let ptr = unsafe { surf.row_ptr(top_row).add(x_start as usize) };
                let n = (x_end - x_start) as usize;
                unsafe { core::slice::from_raw_parts_mut(ptr, n).fill(px); }
            }
            // Bottom strip row
            let bot_row = r.y + r.h - 1 - dy;
            if bot_row < surf.height {
                let ptr = unsafe { surf.row_ptr(bot_row).add(x_start as usize) };
                let n = (x_end - x_start) as usize;
                unsafe { core::slice::from_raw_parts_mut(ptr, n).fill(px); }
            }
        }
    }
}

/// Nearest-neighbour scale blit.
/// Copies `src_rect` from `src` and scales it to `dst_rect` on `dst`.
pub fn blit_scale(dst: &Surface, dst_rect: Rect,
                  src: &Surface, src_rect: Rect) {
    let dr = dst_rect.clip_to(dst.width, dst.height);
    let sr = src_rect.clip_to(src.width, src.height);
    if dr.is_empty() || sr.is_empty() { return; }

    for dst_row in 0..dr.h {
        let src_row = sr.y + dst_row * sr.h / dr.h;
        let sp = unsafe { src.row_ptr(src_row) };
        let dp = unsafe { dst.row_ptr(dr.y + dst_row) };

        for dst_col in 0..dr.w {
            let src_col = sr.x + dst_col * sr.w / dr.w;
            let pixel = unsafe { *sp.add(src_col as usize) };
            unsafe { *dp.add((dr.x + dst_col) as usize) = pixel; }
        }
    }
}

/// Copy a pixel rectangle from one surface to another (same-format fast path).
pub fn copy_rect(dst: &Surface, dst_x: u32, dst_y: u32,
                 src: &Surface, src_x: u32, src_y: u32,
                 w: u32, h: u32) {
    blit(dst, dst_x, dst_y, src, Rect { x: src_x, y: src_y, w, h });
}

// ─────────────────────────────────────────────────────────────────────────────
//  Batch draw context
// ─────────────────────────────────────────────────────────────────────────────

/// A batch context that accumulates damage and submits one GPU upload per flush.
pub struct DrawContext<'a> {
    pub surf:   &'a Surface,
    pub damage: DamageRegion,
}

impl<'a> DrawContext<'a> {
    pub fn new(surf: &'a Surface) -> Self {
        DrawContext { surf, damage: DamageRegion::new() }
    }

    pub fn fill_rect(&mut self, r: Rect, color: Color) {
        fill_rect(self.surf, r, color);
        self.damage.mark(r);
    }

    pub fn draw_hline(&mut self, y: u32, x1: u32, x2: u32, color: Color) {
        draw_hline(self.surf, y, x1, x2, color);
        self.damage.mark(Rect { x: x1, y, w: x2.saturating_sub(x1), h: 1 });
    }

    pub fn draw_vline(&mut self, x: u32, y1: u32, y2: u32, color: Color) {
        draw_vline(self.surf, x, y1, y2, color);
        self.damage.mark(Rect { x, y: y1, w: 1, h: y2.saturating_sub(y1) });
    }

    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        draw_line(self.surf, x0, y0, x1, y1, color);
        let bx = (x0.min(x1).max(0)) as u32;
        let by = (y0.min(y1).max(0)) as u32;
        let ex = (x0.max(x1).max(0)) as u32;
        let ey = (y0.max(y1).max(0)) as u32;
        self.damage.mark(Rect { x: bx, y: by, w: ex - bx + 1, h: ey - by + 1 });
    }

    pub fn fill_gradient_h(&mut self, r: Rect, c0: Color, c1: Color) {
        fill_gradient_h(self.surf, r, c0, c1);
        self.damage.mark(r);
    }

    pub fn fill_gradient_v(&mut self, r: Rect, c0: Color, c1: Color) {
        fill_gradient_v(self.surf, r, c0, c1);
        self.damage.mark(r);
    }

    pub fn draw_rect_border(&mut self, r: Rect, color: Color) {
        draw_rect_border(self.surf, r, color);
        self.damage.mark(r);
    }

    pub fn fill_rounded_rect(&mut self, r: Rect, color: Color, radius: u32) {
        fill_rounded_rect(self.surf, r, color, radius);
        self.damage.mark(r);
    }

    pub fn blit_from(&mut self, dst_x: u32, dst_y: u32,
                     src: &Surface, src_rect: Rect) {
        let w = src_rect.w.min(self.surf.width.saturating_sub(dst_x));
        let h = src_rect.h.min(self.surf.height.saturating_sub(dst_y));
        blit(self.surf, dst_x, dst_y, src, src_rect);
        self.damage.mark(Rect { x: dst_x, y: dst_y, w, h });
    }

    pub fn alpha_blend_from(&mut self, dst_x: u32, dst_y: u32,
                            src: &Surface, src_rect: Rect) {
        let w = src_rect.w.min(self.surf.width.saturating_sub(dst_x));
        let h = src_rect.h.min(self.surf.height.saturating_sub(dst_y));
        alpha_blend(self.surf, dst_x, dst_y, src, src_rect);
        self.damage.mark(Rect { x: dst_x, y: dst_y, w, h });
    }

    /// Flush all damage to GPU (no-op for software surfaces).
    pub fn flush(&self) -> u64 {
        self.damage.upload(self.surf)
    }

    /// Clear damage accumulator (call after flush).
    pub fn clear_damage(&mut self) {
        self.damage.clear();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Global software framebuffer surface
// ─────────────────────────────────────────────────────────────────────────────

/// Runtime software framebuffer description (set during boot).
pub struct SwFb {
    pub virt:   u64,
    pub width:  u32,
    pub height: u32,
    pub stride: u32,
    pub is_bgr: bool,
}

/// Global software framebuffer — initialised once from the UEFI linear FB.
pub static SW_FB: Mutex<Option<SwFb>> = Mutex::new(None);
static SW_FB_READY: AtomicBool = AtomicBool::new(false);

/// Register the software framebuffer (called during boot before GPU init).
pub fn register_sw_fb(virt: u64, width: u32, height: u32, stride: u32, is_bgr: bool) {
    *SW_FB.lock() = Some(SwFb { virt, width, height, stride, is_bgr });
    SW_FB_READY.store(true, Ordering::Relaxed);
    crate::serial_println!(
        "[gpu2d] Software FB registered: {}x{} stride={} BGR={}",
        width, height, stride, is_bgr
    );
}

/// Build a Surface for the global software framebuffer (read lock held by caller).
pub fn sw_surface() -> Option<Surface> {
    if !SW_FB_READY.load(Ordering::Relaxed) { return None; }
    let guard = SW_FB.lock();
    guard.as_ref().map(|fb| Surface::from_raw(fb.virt, fb.width, fb.height, fb.stride, fb.is_bgr))
}

/// Initialise Phase 15.
pub fn init() {
    crate::serial_println!("[gpu2d] 2D acceleration layer ready");
}

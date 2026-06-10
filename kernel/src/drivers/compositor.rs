#![allow(dead_code)]
/// GPU Compositor — Phase 22 for Smart OS.
///
/// Provides:
///  • Per-window GPU texture (BO-backed surface)
///  • Layer stack (ordered by Z-order, opaque + transparent)
///  • Damage region accumulation per layer
///  • Hardware cursor plane (32×32 RGBA, updated independently)
///  • VSync-locked 60 FPS composite loop
///  • Screen capture (screenshot to memory)
///
/// Architecture
/// ─────────────
///  Each visible window is a Layer. The compositor owns a Vec<Layer>.
///  Every frame:
///    1. For each dirty layer, upload pixel data to its GPU BO (damage rect).
///    2. Composite all layers onto the back-buffer using gpu2d::blit /
///       gpu2d::alpha_blend_const (respects per-layer alpha).
///    3. Overlay the cursor on top.
///    4. Call display::present_and_vsync() to page-flip and wait for VSync.

use alloc::vec::Vec;
use alloc::vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use super::gpu2d::{self, Surface, Rect};
use super::gpu_mem;
use super::display;

// ─────────────────────────────────────────────────────────────────────────────
//  Layer (per-window surface)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LayerId(pub u32);

static NEXT_LAYER_ID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    /// Fully opaque window layer.
    Opaque,
    /// Window with alpha channel (requires blending).
    Transparent,
    /// Always-on-top overlay (tooltips, menus).
    Overlay,
    /// Desktop wallpaper (always at the bottom).
    Wallpaper,
}

pub struct Layer {
    pub id:      LayerId,
    pub kind:    LayerKind,
    /// Position on screen.
    pub x:       i32,
    pub y:       i32,
    /// Surface dimensions.
    pub width:   u32,
    pub height:  u32,
    /// Global alpha (0 = invisible, 255 = fully opaque).
    pub alpha:   u8,
    /// Z-order (lower = further back).
    pub z:       i32,
    pub visible: bool,
    /// GPU buffer object backing this surface.
    pub bo:      Option<u32>,
    /// CPU pixel buffer (RGBA8, used when GPU BO unavailable).
    pub cpu_buf: Vec<u32>,
    /// Accumulated dirty rect for this frame.
    pub dirty:   Option<Rect>,
    /// Whether the layer has been composited this frame.
    pub composited: bool,
}

impl Layer {
    pub fn new(kind: LayerKind, x: i32, y: i32, w: u32, h: u32) -> Self {
        let id = LayerId(NEXT_LAYER_ID.fetch_add(1, Ordering::Relaxed));
        let n = (w * h) as usize;
        Layer {
            id, kind, x, y, width: w, height: h,
            alpha: 255, z: id.0 as i32, visible: true,
            bo: None, cpu_buf: vec![0u32; n],
            dirty: Some(Rect { x: 0, y: 0, w, h }),
            composited: false,
        }
    }

    /// Allocate a GPU BO for this layer (if not already done).
    pub fn ensure_bo(&mut self) -> bool {
        if self.bo.is_some() { return true; }
        match gpu_mem::alloc_bo(self.width, self.height, gpu_mem::PixelFormat::Rgbx8888) {
            Ok(handle) => { self.bo = Some(handle); true }
            Err(_) => false,
        }
    }

    /// Write pixels into the CPU buffer and mark the region dirty.
    pub fn write_pixels(&mut self, x: u32, y: u32, w: u32, h: u32, pixels: &[u32]) {
        let copy_w = w.min(self.width.saturating_sub(x));
        let copy_h = h.min(self.height.saturating_sub(y));
        for row in 0..copy_h {
            let dst_off = ((y + row) * self.width + x) as usize;
            let src_off = (row * w) as usize;
            let n = copy_w as usize;
            if dst_off + n <= self.cpu_buf.len() && src_off + n <= pixels.len() {
                self.cpu_buf[dst_off..dst_off+n].copy_from_slice(&pixels[src_off..src_off+n]);
            }
        }
        self.mark_dirty(x, y, copy_w, copy_h);
    }

    pub fn write_pixel(&mut self, x: u32, y: u32, color: u32) {
        let idx = (y * self.width + x) as usize;
        if idx < self.cpu_buf.len() {
            self.cpu_buf[idx] = color;
            self.mark_dirty(x, y, 1, 1);
        }
    }

    pub fn fill(&mut self, color: u32) {
        self.cpu_buf.fill(color);
        self.mark_dirty(0, 0, self.width, self.height);
    }

    pub fn mark_dirty(&mut self, x: u32, y: u32, w: u32, h: u32) {
        let new = Rect { x, y, w, h };
        self.dirty = Some(match self.dirty {
            None => new,
            Some(d) => merge_rects(d, new),
        });
    }

    /// CPU surface view (for gpu2d operations).
    pub fn cpu_surface(&self) -> Surface {
        Surface {
            virt:   self.cpu_buf.as_ptr() as u64,
            width:  self.width,
            height: self.height,
            stride: self.width * 4,
            bo:     self.bo.unwrap_or(0),
            is_bgr: false,
        }
    }

    /// Upload dirty region to GPU BO.
    pub fn upload_dirty(&mut self) {
        let dirty = match self.dirty.take() {
            Some(d) => d,
            None => return,
        };
        if let Some(bo) = self.bo {
            gpu_mem::upload_rect(bo, dirty.x, dirty.y, dirty.w, dirty.h);
        }
    }
}

fn merge_rects(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.w).max(b.x + b.w);
    let y1 = (a.y + a.h).max(b.y + b.h);
    Rect { x: x0, y: y0, w: x1.saturating_sub(x0), h: y1.saturating_sub(y0) }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Hardware cursor plane
// ─────────────────────────────────────────────────────────────────────────────

pub const CURSOR_W: u32 = 32;
pub const CURSOR_H: u32 = 32;

pub struct CursorPlane {
    pub x:       i32,
    pub y:       i32,
    pub visible: bool,
    pub pixels:  [u32; (CURSOR_W * CURSOR_H) as usize],
    pub dirty:   bool,
}

impl CursorPlane {
    pub fn new() -> Self {
        let mut p = CursorPlane {
            x: 0, y: 0, visible: true,
            pixels: [0u32; (CURSOR_W * CURSOR_H) as usize],
            dirty: true,
        };
        p.draw_default_arrow();
        p
    }

    /// Draw a simple arrow cursor.
    fn draw_default_arrow(&mut self) {
        const W: usize = CURSOR_W as usize;
        let white = 0xFFFFFFFF_u32;
        let black = 0xFF000000_u32;
        let trans = 0x00000000_u32;

        self.pixels.fill(trans);
        // Simple 16-pixel arrow outline.
        for row in 0..16usize {
            for col in 0..=row {
                let idx = row * W + col;
                if idx < self.pixels.len() { self.pixels[idx] = black; }
            }
        }
        for row in 1..15usize {
            for col in 1..row {
                let idx = row * W + col;
                if idx < self.pixels.len() { self.pixels[idx] = white; }
            }
        }
    }

    pub fn move_to(&mut self, x: i32, y: i32) {
        if self.x != x || self.y != y {
            self.x = x; self.y = y;
            self.dirty = true;
        }
    }

    /// Composite cursor onto a surface at its current position.
    pub fn composite_onto(&self, dst: &Surface) {
        if !self.visible { return; }
        let cx = self.x.max(0) as u32;
        let cy = self.y.max(0) as u32;
        let clip_w = CURSOR_W.min(dst.width.saturating_sub(cx));
        let clip_h = CURSOR_H.min(dst.height.saturating_sub(cy));
        let dst_ptr = dst.virt as *mut u32;

        for row in 0..clip_h {
            for col in 0..clip_w {
                let src_idx = (row * CURSOR_W + col) as usize;
                let src_pix = self.pixels[src_idx];
                if src_pix >> 24 == 0 { continue; } // fully transparent

                let dst_idx = ((cy + row) * dst.width + cx + col) as usize;
                unsafe {
                    let dst_pix = *dst_ptr.add(dst_idx);
                    let sa = (src_pix >> 24) as u32;
                    let inv = 255 - sa;
                    let r = ((src_pix & 0xFF) * sa / 255 + (dst_pix & 0xFF) * inv / 255) & 0xFF;
                    let g = (((src_pix >> 8) & 0xFF) * sa / 255 + ((dst_pix >> 8) & 0xFF) * inv / 255) & 0xFF;
                    let b = (((src_pix >> 16) & 0xFF) * sa / 255 + ((dst_pix >> 16) & 0xFF) * inv / 255) & 0xFF;
                    *dst_ptr.add(dst_idx) = 0xFF000000 | (b << 16) | (g << 8) | r;
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Screen damage tracker
// ─────────────────────────────────────────────────────────────────────────────

pub struct ScreenDamage {
    pub rects: Vec<Rect>,
    pub full:  bool,
}

impl ScreenDamage {
    pub fn new() -> Self { ScreenDamage { rects: Vec::new(), full: false } }

    pub fn add(&mut self, r: Rect) {
        if self.full { return; }
        // Merge with existing if overlap is likely.
        if self.rects.len() > 8 { self.full = true; self.rects.clear(); return; }
        self.rects.push(r);
    }

    pub fn full_screen(w: u32, h: u32) -> Self {
        ScreenDamage { rects: vec![Rect { x: 0, y: 0, w, h }], full: false }
    }

    pub fn clear(&mut self) { self.rects.clear(); self.full = false; }
    pub fn is_empty(&self) -> bool { !self.full && self.rects.is_empty() }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Compositor
// ─────────────────────────────────────────────────────────────────────────────

/// Frame statistics.
pub static COMPOSITOR_FRAMES:   AtomicU64 = AtomicU64::new(0);
pub static COMPOSITOR_DROPPED:  AtomicU64 = AtomicU64::new(0);
pub static COMPOSITOR_RUNNING:  AtomicBool = AtomicBool::new(false);

pub struct Compositor {
    pub layers:  Vec<Layer>,
    pub cursor:  CursorPlane,
    pub damage:  ScreenDamage,
    /// Back-buffer surface (into display double-buffer BO).
    back_surf:   Option<Surface>,
    /// Screen dimensions.
    pub width:   u32,
    pub height:  u32,
}

impl Compositor {
    pub fn new(w: u32, h: u32) -> Self {
        Compositor {
            layers:    Vec::new(),
            cursor:    CursorPlane::new(),
            damage:    ScreenDamage::full_screen(w, h),
            back_surf: None,
            width:     w,
            height:    h,
        }
    }

    // ── Layer management ────────────────────────────────────────────────────

    pub fn add_layer(&mut self, mut layer: Layer) -> LayerId {
        layer.ensure_bo();
        let id = layer.id;
        self.layers.push(layer);
        self.sort_layers();
        id
    }

    pub fn remove_layer(&mut self, id: LayerId) {
        if let Some(pos) = self.layers.iter().position(|l| l.id == id) {
            let layer = self.layers.remove(pos);
            // Free GPU BO.
            if let Some(bo) = layer.bo { gpu_mem::free_bo(bo); }
            self.damage.full = true;
        }
    }

    pub fn get_layer(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    pub fn set_layer_position(&mut self, id: LayerId, x: i32, y: i32) {
        if let Some(l) = self.layers.iter_mut().find(|l| l.id == id) {
            if l.x != x || l.y != y {
                // Mark old position dirty.
                let old = Rect { x: l.x.max(0) as u32, y: l.y.max(0) as u32,
                                  w: l.width, h: l.height };
                self.damage.add(old);
                l.x = x; l.y = y;
                let new = Rect { x: x.max(0) as u32, y: y.max(0) as u32,
                                  w: l.width, h: l.height };
                self.damage.add(new);
            }
        }
    }

    pub fn set_layer_z(&mut self, id: LayerId, z: i32) {
        if let Some(l) = self.layers.iter_mut().find(|l| l.id == id) {
            l.z = z;
        }
        self.sort_layers();
        self.damage.full = true;
    }

    fn sort_layers(&mut self) {
        self.layers.sort_by(|a, b| {
            // Wallpaper always last; Overlay always first.
            let rank = |l: &Layer| match l.kind {
                LayerKind::Wallpaper => -1000,
                LayerKind::Overlay   =>  1000,
                _                    => l.z,
            };
            rank(a).cmp(&rank(b))
        });
    }

    // ── Compositing ─────────────────────────────────────────────────────────

    /// Composite all visible layers + cursor onto the back-buffer, then vsync.
    pub fn composite_frame(&mut self) -> u64 {
        let frame_start = display::now_us();

        // Refresh back-buffer surface pointer.
        if let Some(virt) = display::back_buffer_virt() {
            self.back_surf = Some(Surface {
                virt,
                width:  self.width,
                height: self.height,
                stride: self.width * 4,
                bo:     0,
                is_bgr: false,
            });
        }

        let back = match &self.back_surf {
            Some(s) => *s,
            None => {
                // No GPU back-buffer — fall back to SW surface.
                return display::vsync_wait(frame_start);
            }
        };

        // Upload dirty layers.
        for layer in &mut self.layers {
            if layer.dirty.is_some() && layer.visible {
                layer.upload_dirty();
                let lr = Rect {
                    x: layer.x.max(0) as u32,
                    y: layer.y.max(0) as u32,
                    w: layer.width,
                    h: layer.height,
                };
                self.damage.add(lr);
            }
        }

        // Clear damaged areas first (fill with black).
        if self.damage.full {
            gpu2d::fill_rect(&back, Rect { x: 0, y: 0, w: self.width, h: self.height }, 0xFF000000);
        } else {
            for r in &self.damage.rects {
                gpu2d::fill_rect(&back, *r, 0xFF000000);
            }
        }

        // Composite each visible layer in Z-order.
        for layer in &self.layers {
            if !layer.visible { continue; }
            let src = layer.cpu_surface();
            let dst_x = layer.x.max(0) as u32;
            let dst_y = layer.y.max(0) as u32;
            let src_rect = Rect { x: 0, y: 0, w: layer.width, h: layer.height };

            match layer.kind {
                LayerKind::Opaque | LayerKind::Wallpaper => {
                    gpu2d::blit(&back, dst_x, dst_y, &src, src_rect);
                }
                LayerKind::Transparent | LayerKind::Overlay => {
                    if layer.alpha == 255 {
                        gpu2d::alpha_blend(&back, dst_x, dst_y, &src, src_rect);
                    } else {
                        gpu2d::alpha_blend_const(&back, dst_x, dst_y, &src, src_rect, layer.alpha);
                    }
                }
            }
        }

        // Composite cursor on top.
        self.cursor.composite_onto(&back);
        self.cursor.dirty = false;

        // Clear damage.
        self.damage.clear();

        // Page-flip + VSync.
        let frame_dur = display::present_and_vsync(frame_start);
        COMPOSITOR_FRAMES.fetch_add(1, Ordering::Relaxed);
        if frame_dur > display::VSYNC_PERIOD_US {
            COMPOSITOR_DROPPED.fetch_add(1, Ordering::Relaxed);
        }

        frame_dur
    }

    // ── Screenshot ─────────────────────────────────────────────────────────

    /// Copy the current back-buffer into a Vec<u32> (RGBA8).
    pub fn screenshot(&self) -> Vec<u32> {
        if let Some(surf) = &self.back_surf {
            let n = (self.width * self.height) as usize;
            let mut out = vec![0u32; n];
            let src = surf.virt as *const u32;
            unsafe { core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), n); }
            out
        } else {
            vec![0u32; (self.width * self.height) as usize]
        }
    }

    // ── Cursor ─────────────────────────────────────────────────────────────

    pub fn move_cursor(&mut self, x: i32, y: i32) {
        self.cursor.move_to(x, y);
    }

    pub fn set_cursor_visible(&mut self, v: bool) {
        self.cursor.visible = v;
        self.damage.full = true;
    }

    pub fn print_stats(&self) {
        crate::serial_println!(
            "[compositor] layers={} frames={} dropped={} {}×{}",
            self.layers.len(),
            COMPOSITOR_FRAMES.load(Ordering::Relaxed),
            COMPOSITOR_DROPPED.load(Ordering::Relaxed),
            self.width, self.height,
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Global compositor instance
// ─────────────────────────────────────────────────────────────────────────────

pub static COMPOSITOR: Mutex<Option<Compositor>> = Mutex::new(None);

pub fn init() {
    let (w, h) = display::display_resolution();
    let comp = Compositor::new(w, h);
    *COMPOSITOR.lock() = Some(comp);
    COMPOSITOR_RUNNING.store(true, Ordering::Relaxed);
    crate::serial_println!("[compositor] GPU compositor ready ({}×{}).", w, h);
}

/// Add a new layer; returns its ID.
pub fn add_layer(kind: LayerKind, x: i32, y: i32, w: u32, h: u32) -> Option<LayerId> {
    let mut c = COMPOSITOR.lock();
    c.as_mut().map(|c| c.add_layer(Layer::new(kind, x, y, w, h)))
}

/// Write pixels into a layer's CPU buffer.
pub fn write_layer_pixels(id: LayerId, x: u32, y: u32, w: u32, h: u32, pixels: &[u32]) {
    let mut c = COMPOSITOR.lock();
    if let Some(comp) = c.as_mut() {
        if let Some(layer) = comp.get_layer(id) {
            layer.write_pixels(x, y, w, h, pixels);
        }
    }
}

/// Move the hardware cursor.
pub fn move_cursor(x: i32, y: i32) {
    let mut c = COMPOSITOR.lock();
    if let Some(comp) = c.as_mut() { comp.move_cursor(x, y); }
}

/// Composite one frame and vsync.
pub fn composite_frame() -> u64 {
    let mut c = COMPOSITOR.lock();
    c.as_mut().map(|c| c.composite_frame()).unwrap_or(0)
}

pub fn print_stats() {
    let c = COMPOSITOR.lock();
    if let Some(comp) = c.as_ref() { comp.print_stats(); }
}

//! Paint + Composite Engine — Phase 37 for Smart OS.
//!
//! Implements the CSS Visual Rendering Model:
//!  • Display list generation from layout boxes + computed styles
//!  • Paint commands: FillRect, DrawBorder, DrawText, DrawImage,
//!    ClipRect, PushLayer, PopLayer, Transform
//!  • Stacking context / z-index ordering
//!  • Layer compositing with alpha blending
//!  • Tie-in to kernel framebuffer (gpu2d) for final blit
//!
//! The pipeline is:
//!   layout::LayoutBox tree
//!     → build_display_list()  → Vec<PaintCmd>
//!     → sort_by_stacking_order()
//!     → rasterize()           → pixel buffer
//!     → composite_to_fb()     → screen

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use super::html::{Dom, NodeId, NodeKind};
use super::css::{ComputedStyle, Display, Color, Position, Overflow};
use super::layout::{LayoutBox, Rect, BoxAreas};

// ─────────────────────────────────────────────────────────────────────────────
//  Paint commands (display list)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PaintCmd {
    /// Fill a rectangle with a solid color.
    FillRect { rect: Rect, color: Color },

    /// Draw a border around a rect (one side at a time, px thick).
    DrawBorder { rect: Rect, top: f32, right: f32, bottom: f32, left: f32, color: Color },

    /// Draw a text string at (x, y) with font size and color.
    DrawText { x: f32, y: f32, text: alloc::string::String, font_px: f32, color: Color },

    /// Draw an image (rgba bytes, stride = width * 4) into rect.
    DrawImage { rect: Rect, data: alloc::vec::Vec<u8>, img_w: u32, img_h: u32 },

    /// Push a clip rectangle — subsequent commands are clipped to it.
    PushClip { rect: Rect },

    /// Pop the most recent clip rectangle.
    PopClip,

    /// Push an opacity layer (0..=255).
    PushLayer { opacity: u8 },

    /// Flatten the current layer with alpha blending.
    PopLayer,

    /// No-op / placeholder.
    Nop,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Stacking context entry
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct StackingEntry {
    z_index:  i32,
    node_id:  NodeId,
    commands: Vec<PaintCmd>,
}

// ─────────────────────────────────────────────────────────────────────────────
//  No-std float helpers
// ─────────────────────────────────────────────────────────────────────────────

#[inline] fn f32_max(a: f32, b: f32) -> f32 { if a > b { a } else { b } }
#[inline] fn f32_min(a: f32, b: f32) -> f32 { if a < b { a } else { b } }
#[inline] fn f32_clamp(v: f32, lo: f32, hi: f32) -> f32 { f32_min(f32_max(v, lo), hi) }

// ─────────────────────────────────────────────────────────────────────────────
//  Display list builder
// ─────────────────────────────────────────────────────────────────────────────

/// Build a flat, ordered display list from the layout tree.
pub fn build_display_list(
    dom:     &Dom,
    boxes:   &BTreeMap<NodeId, LayoutBox>,
    styles:  &BTreeMap<NodeId, ComputedStyle>,
) -> Vec<PaintCmd> {
    // Collect stacking entries, sorted by z-index then document order.
    let mut entries: Vec<StackingEntry> = Vec::new();
    let mut normal: Vec<PaintCmd>       = Vec::new();

    // Walk boxes in NodeId (document) order.
    for (&node_id, lb) in boxes.iter() {
        let style = match styles.get(&node_id) { Some(s) => s, None => continue };

        if style.display == Display::None { continue; }
        if style.opacity == 0.0 { continue; }

        let is_stacking = style.z_index != 0
            || style.position == Position::Absolute
            || style.position == Position::Fixed
            || style.opacity < 1.0;

        let mut cmds: Vec<PaintCmd> = Vec::new();
        paint_node(dom, node_id, lb, style, styles, &mut cmds);

        if is_stacking {
            entries.push(StackingEntry { z_index: style.z_index, node_id, commands: cmds });
        } else {
            normal.extend(cmds);
        }
    }

    // Sort stacking entries: negative z behind normal, positive z in front.
    entries.sort_by(|a, b| {
        a.z_index.cmp(&b.z_index).then(a.node_id.cmp(&b.node_id))
    });

    // Interleave: negative-z → normal flow → non-negative z
    let mut result: Vec<PaintCmd> = Vec::new();
    for e in entries.iter().filter(|e| e.z_index < 0) {
        result.extend(e.commands.iter().cloned());
    }
    result.extend(normal);
    for e in entries.iter().filter(|e| e.z_index >= 0) {
        result.extend(e.commands.iter().cloned());
    }

    result
}

/// Emit paint commands for a single node.
fn paint_node(
    dom:      &Dom,
    node_id:  NodeId,
    lb:       &LayoutBox,
    style:    &ComputedStyle,
    styles:   &BTreeMap<NodeId, ComputedStyle>,
    out:      &mut Vec<PaintCmd>,
) {
    let border_rect  = lb.areas.border_rect();
    let padding_rect = lb.areas.padding_rect();
    let content_rect = lb.areas.content;

    // ── Opacity layer ───────────────────────────────────────────────────────
    let opacity_byte = (style.opacity * 255.0) as u8;
    if opacity_byte < 255 {
        out.push(PaintCmd::PushLayer { opacity: opacity_byte });
    }

    // ── Clip (overflow: hidden) ─────────────────────────────────────────────
    let needs_clip = style.overflow_x == Overflow::Hidden || style.overflow_y == Overflow::Hidden;
    if needs_clip {
        out.push(PaintCmd::PushClip { rect: padding_rect });
    }

    // ── Background color ────────────────────────────────────────────────────
    if style.background_color.a > 0 {
        out.push(PaintCmd::FillRect { rect: padding_rect, color: style.background_color });
    }

    // ── Border ─────────────────────────────────────────────────────────────
    let has_border = style.border_top > 0.0 || style.border_right > 0.0
        || style.border_bottom > 0.0 || style.border_left > 0.0;
    if has_border {
        out.push(PaintCmd::DrawBorder {
            rect:   border_rect,
            top:    style.border_top,
            right:  style.border_right,
            bottom: style.border_bottom,
            left:   style.border_left,
            color:  style.border_color,
        });
    }

    // ── Text content (for text nodes) ───────────────────────────────────────
    if let Some(node) = dom.get(node_id) {
        for &child_id in &node.children {
            if let Some(child) = dom.get(child_id) {
                if let NodeKind::Text { data } = &child.kind {
                    if !data.trim().is_empty() {
                        out.push(PaintCmd::DrawText {
                            x:       content_rect.x,
                            y:       content_rect.y,
                            text:    data.clone(),
                            font_px: style.font_size,
                            color:   style.color,
                        });
                    }
                }
            }
        }
    }

    // ── Pop clip ────────────────────────────────────────────────────────────
    if needs_clip { out.push(PaintCmd::PopClip); }

    // ── Pop opacity layer ───────────────────────────────────────────────────
    if opacity_byte < 255 { out.push(PaintCmd::PopLayer); }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Software rasterizer
// ─────────────────────────────────────────────────────────────────────────────

/// Rasterize a display list into an RGBA pixel buffer.
pub fn rasterize(
    cmds:   &[PaintCmd],
    width:  u32,
    height: u32,
) -> Vec<u8> {
    let w = width  as usize;
    let h = height as usize;
    let mut pixels = alloc::vec![0u8; w * h * 4]; // RGBA

    // Clip stack
    let mut clip_stack: Vec<(i32, i32, i32, i32)> = Vec::new(); // (x0,y0,x1,y1)
    let full_clip = (0i32, 0i32, w as i32, h as i32);
    clip_stack.push(full_clip);

    // Layer stack: (buffer, opacity)
    // For simplicity we apply opacity immediately without true compositing layers.
    let mut opacity_stack: Vec<u8> = alloc::vec![255u8];

    let current_clip = |stack: &Vec<(i32,i32,i32,i32)>| -> (i32,i32,i32,i32) {
        *stack.last().unwrap_or(&full_clip)
    };

    let current_opacity = |stack: &Vec<u8>| -> u8 {
        stack.iter().fold(255u8, |acc, &o| ((acc as u16 * o as u16 / 255) as u8))
    };

    for cmd in cmds {
        match cmd {
            PaintCmd::PushClip { rect } => {
                let (cx0, cy0, cx1, cy1) = current_clip(&clip_stack);
                let x0 = f32_max(rect.x, cx0 as f32) as i32;
                let y0 = f32_max(rect.y, cy0 as f32) as i32;
                let x1 = f32_min(rect.x + rect.w, cx1 as f32) as i32;
                let y1 = f32_min(rect.y + rect.h, cy1 as f32) as i32;
                clip_stack.push((x0, y0, x1, y1));
            }
            PaintCmd::PopClip => { clip_stack.pop(); }

            PaintCmd::PushLayer { opacity } => { opacity_stack.push(*opacity); }
            PaintCmd::PopLayer              => { opacity_stack.pop(); }

            PaintCmd::FillRect { rect, color } => {
                let alpha = (color.a as u16 * current_opacity(&opacity_stack) as u16 / 255) as u8;
                if alpha == 0 { continue; }
                let (cx0, cy0, cx1, cy1) = current_clip(&clip_stack);
                let x0 = (f32_max(rect.x, cx0 as f32) as i32).max(0) as usize;
                let y0 = (f32_max(rect.y, cy0 as f32) as i32).max(0) as usize;
                let x1 = (f32_min(rect.x + rect.w, cx1 as f32) as i32).min(w as i32) as usize;
                let y1 = (f32_min(rect.y + rect.h, cy1 as f32) as i32).min(h as i32) as usize;
                for py in y0..y1 {
                    for px in x0..x1 {
                        blend_pixel(&mut pixels, w, px, py, color.r, color.g, color.b, alpha);
                    }
                }
            }

            PaintCmd::DrawBorder { rect, top, right, bottom, left, color } => {
                let alpha = (color.a as u16 * current_opacity(&opacity_stack) as u16 / 255) as u8;
                if alpha == 0 { continue; }
                let (cx0, cy0, cx1, cy1) = current_clip(&clip_stack);
                let clip = (cx0 as usize, cy0 as usize, cx1 as usize, cy1 as usize);
                // Top border
                fill_border_edge(&mut pixels, w, h, clip,
                    rect.x as i32, rect.y as i32,
                    (rect.x + rect.w) as i32, (rect.y + *top) as i32,
                    color.r, color.g, color.b, alpha);
                // Bottom border
                fill_border_edge(&mut pixels, w, h, clip,
                    rect.x as i32, (rect.y + rect.h - *bottom) as i32,
                    (rect.x + rect.w) as i32, (rect.y + rect.h) as i32,
                    color.r, color.g, color.b, alpha);
                // Left border
                fill_border_edge(&mut pixels, w, h, clip,
                    rect.x as i32, rect.y as i32,
                    (rect.x + *left) as i32, (rect.y + rect.h) as i32,
                    color.r, color.g, color.b, alpha);
                // Right border
                fill_border_edge(&mut pixels, w, h, clip,
                    (rect.x + rect.w - *right) as i32, rect.y as i32,
                    (rect.x + rect.w) as i32, (rect.y + rect.h) as i32,
                    color.r, color.g, color.b, alpha);
            }

            PaintCmd::DrawText { x, y, text, font_px, color } => {
                // Render text using the kernel bitmap font if available.
                let alpha = (color.a as u16 * current_opacity(&opacity_stack) as u16 / 255) as u8;
                if alpha == 0 { continue; }
                draw_text_sw(&mut pixels, w, h, *x as i32, *y as i32,
                             text, *font_px, color.r, color.g, color.b, alpha);
            }

            PaintCmd::DrawImage { rect, data, img_w, img_h } => {
                let (cx0, cy0, cx1, cy1) = current_clip(&clip_stack);
                blit_image(&mut pixels, w, h,
                           rect.x as i32, rect.y as i32,
                           rect.w as u32, rect.h as u32,
                           data, *img_w, *img_h,
                           cx0, cy0, cx1, cy1);
            }

            PaintCmd::Nop => {}
        }
    }

    pixels
}

// ─────────────────────────────────────────────────────────────────────────────
//  Pixel helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Alpha-blend a single pixel (Porter-Duff source-over).
#[inline]
fn blend_pixel(buf: &mut [u8], stride: usize, px: usize, py: usize,
               r: u8, g: u8, b: u8, a: u8) {
    let off = (py * stride + px) * 4;
    if off + 3 >= buf.len() { return; }
    let inv = 255 - a as u16;
    let dr = (r as u16 * a as u16 + buf[off  ] as u16 * inv) / 255;
    let dg = (g as u16 * a as u16 + buf[off+1] as u16 * inv) / 255;
    let db = (b as u16 * a as u16 + buf[off+2] as u16 * inv) / 255;
    let da = (a as u16 + buf[off+3] as u16 * inv / 255).min(255);
    buf[off  ] = dr as u8;
    buf[off+1] = dg as u8;
    buf[off+2] = db as u8;
    buf[off+3] = da as u8;
}

fn fill_border_edge(buf: &mut [u8], stride: usize, h: usize,
                    clip: (usize,usize,usize,usize),
                    x0: i32, y0: i32, x1: i32, y1: i32,
                    r: u8, g: u8, b: u8, a: u8) {
    let (cx0, cy0, cx1, cy1) = clip;
    let px0 = (x0.max(0) as usize).max(cx0);
    let py0 = (y0.max(0) as usize).max(cy0);
    let px1 = (x1.max(0) as usize).min(cx1).min(stride);
    let py1 = (y1.max(0) as usize).min(cy1).min(h);
    for py in py0..py1 {
        for px in px0..px1 {
            blend_pixel(buf, stride, px, py, r, g, b, a);
        }
    }
}

/// Simple bitmap text renderer (uses the kernel 8×16 built-in font).
fn draw_text_sw(buf: &mut [u8], w: usize, h: usize,
                x: i32, y: i32, text: &str, _font_px: f32,
                r: u8, g: u8, b: u8, a: u8) {
    // Use the kernel bitmap font manager if available.
    // For now emit 8×16 cells using the font driver's render path.
    let mut cx = x;
    for ch in text.chars() {
        // Ask font driver for the glyph bitmap
        if let Some(bitmap) = crate::drivers::font::render_glyph_builtin(ch) {
            // bitmap is 16 bytes, each byte = one row of 8 pixels (MSB = leftmost)
            for (row, &byte) in bitmap.iter().enumerate() {
                for col in 0..8usize {
                    if byte & (0x80 >> col) != 0 {
                        let px = cx + col as i32;
                        let py = y  + row as i32;
                        if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                            blend_pixel(buf, w, px as usize, py as usize, r, g, b, a);
                        }
                    }
                }
            }
        }
        cx += 8; // advance 8px per character (bitmap font)
    }
}

/// Scale-blit an RGBA image into a destination rectangle.
fn blit_image(buf: &mut [u8], dw: usize, dh: usize,
              dst_x: i32, dst_y: i32, dst_w: u32, dst_h: u32,
              src: &[u8], src_w: u32, src_h: u32,
              cx0: i32, cy0: i32, cx1: i32, cy1: i32) {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 { return; }
    for py in 0..dst_h as i32 {
        let dy = dst_y + py;
        if dy < cy0 || dy >= cy1 || dy < 0 || dy >= dh as i32 { continue; }
        let sy = (py as u32 * src_h / dst_h) as usize;
        for px in 0..dst_w as i32 {
            let dx = dst_x + px;
            if dx < cx0 || dx >= cx1 || dx < 0 || dx >= dw as i32 { continue; }
            let sx = (px as u32 * src_w / dst_w) as usize;
            let src_off = (sy * src_w as usize + sx) * 4;
            if src_off + 3 >= src.len() { continue; }
            blend_pixel(buf, dw, dx as usize, dy as usize,
                        src[src_off], src[src_off+1], src[src_off+2], src[src_off+3]);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Composite to framebuffer
// ─────────────────────────────────────────────────────────────────────────────

/// Blit the rasterized RGBA buffer into the kernel framebuffer.
/// Converts RGBA → framebuffer pixel format (0xAARRGGBB or 0xAABBGGRR).
pub fn composite_to_fb(pixels: &[u8], width: u32, height: u32, is_bgr: bool) {
    use crate::drivers::gpu2d;
    let surf = match gpu2d::sw_surface() { Some(s) => s, None => return };
    let w = width  as usize;
    let h = height as usize;
    let fb_w = surf.width  as usize;
    let fb_h = surf.height as usize;
    let rows = h.min(fb_h);
    let cols = w.min(fb_w);
    for py in 0..rows {
        for px in 0..cols {
            let off = (py * w + px) * 4;
            if off + 3 >= pixels.len() { break; }
            let (r, g, b, a) = (pixels[off], pixels[off+1], pixels[off+2], pixels[off+3]);
            let color32: u32 = if is_bgr {
                ((a as u32) << 24) | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32)
            } else {
                ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
            };
            unsafe { *surf.pixel_ptr(px as u32, py as u32) = color32; }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  High-level render call
// ─────────────────────────────────────────────────────────────────────────────

/// Full render pipeline: display list → rasterize → composite.
pub fn render_page(
    dom:     &Dom,
    boxes:   &BTreeMap<NodeId, LayoutBox>,
    styles:  &BTreeMap<NodeId, ComputedStyle>,
    width:   u32,
    height:  u32,
    is_bgr:  bool,
) {
    let cmds   = build_display_list(dom, boxes, styles);
    let pixels = rasterize(&cmds, width, height);
    composite_to_fb(&pixels, width, height, is_bgr);
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[paint] Display list + software rasterizer ready (Phase 37).");
}

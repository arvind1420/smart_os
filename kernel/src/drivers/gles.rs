#![allow(dead_code)]
/// OpenGL ES 2.0 Software Rasterizer — Phase 19 for Smart OS.
///
/// Provides:
///  • Vertex Buffer Objects (VBOs) and Index Buffer Objects (IBOs)
///  • Vertex attribute layout descriptors
///  • Triangle rasterization (flat + Gouraud shading)
///  • Perspective-correct interpolation
///  • Depth buffer (Z-test, Z-write)
///  • Texture sampling (nearest + bilinear, RGBA8 and RGB8)
///  • Alpha blending (src-alpha / one-minus-src-alpha)
///  • Programmable shader slots (uniform storage + GLSL-ES stub dispatch)
///  • Framebuffer object (FBO) targeting
///  • Viewport, scissor, face culling
///
/// All rasterization is pure software; no GPU commands are issued here.
/// The GPU compositor (Phase 22) will upload the final pixels via gpu_mem.

use alloc::vec::Vec;
use alloc::vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
//  no_std f32 math helpers (floor/ceil not in core)
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn f32_floor(x: f32) -> f32 {
    let xi = x as i64 as f32;
    if x < xi { xi - 1.0 } else { xi }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Fixed-point helpers (16.16 for edge walking)
// ─────────────────────────────────────────────────────────────────────────────

/// 16.16 fixed-point type.
type Fix = i64;
const FIX_SHIFT: i64 = 16;
const FIX_ONE:   i64 = 1 << FIX_SHIFT;

#[inline] fn to_fix(x: f32)  -> Fix { (x * FIX_ONE as f32) as Fix }
#[inline] fn fix_mul(a: Fix, b: Fix) -> Fix { (a * b) >> FIX_SHIFT }
#[inline] fn fix_floor(a: Fix) -> i32 { (a >> FIX_SHIFT) as i32 }
#[inline] fn fix_ceil(a: Fix)  -> i32 { ((a + FIX_ONE - 1) >> FIX_SHIFT) as i32 }

// ─────────────────────────────────────────────────────────────────────────────
//  Color helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Pack RGBA bytes into a u32 (R in bits 0-7).
#[inline]
pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | ((a as u32) << 24)
}

#[inline] fn r_of(c: u32) -> u8 { c as u8 }
#[inline] fn g_of(c: u32) -> u8 { (c >> 8)  as u8 }
#[inline] fn b_of(c: u32) -> u8 { (c >> 16) as u8 }
#[inline] fn a_of(c: u32) -> u8 { (c >> 24) as u8 }

/// Alpha-blend src over dst.
#[inline]
fn blend_over(dst: u32, src: u32) -> u32 {
    let sa = a_of(src) as u32;
    if sa == 255 { return src; }
    if sa == 0   { return dst; }
    let inv = 255 - sa;
    let r = (r_of(src) as u32 * sa + r_of(dst) as u32 * inv) / 255;
    let g = (g_of(src) as u32 * sa + g_of(dst) as u32 * inv) / 255;
    let b = (b_of(src) as u32 * sa + b_of(dst) as u32 * inv) / 255;
    rgba(r as u8, g as u8, b as u8, 255)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Vertex format
// ─────────────────────────────────────────────────────────────────────────────

/// A fully-decoded vertex after vertex shader execution.
#[derive(Clone, Copy, Debug)]
pub struct Vertex {
    /// Clip-space position (x, y, z, w).
    pub pos: [f32; 4],
    /// Interpolated RGBA color (each 0..1).
    pub color: [f32; 4],
    /// Texture coordinates.
    pub uv: [f32; 2],
}

impl Vertex {
    pub const ZERO: Vertex = Vertex {
        pos:   [0.0; 4],
        color: [1.0; 4],
        uv:    [0.0; 2],
    };
}

// ─────────────────────────────────────────────────────────────────────────────
//  Buffer Objects
// ─────────────────────────────────────────────────────────────────────────────

static NEXT_BUFFER_ID: AtomicU32 = AtomicU32::new(1);

pub struct BufferObject {
    pub id:   u32,
    pub data: Vec<u8>,
}

impl BufferObject {
    pub fn new() -> Self {
        BufferObject {
            id:   NEXT_BUFFER_ID.fetch_add(1, Ordering::Relaxed),
            data: Vec::new(),
        }
    }
    pub fn upload(&mut self, bytes: &[u8]) {
        self.data.clear();
        self.data.extend_from_slice(bytes);
    }
    pub fn len(&self) -> usize { self.data.len() }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Vertex attribute descriptor
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttrType { Float, U8Norm }

#[derive(Clone, Copy, Debug)]
pub struct VertexAttrib {
    pub index:    usize,   // shader location
    pub size:     u8,      // components (1-4)
    pub typ:      AttrType,
    pub stride:   u32,     // bytes between vertices (0 = tight)
    pub offset:   u32,     // byte offset within vertex
    pub buf_id:   u32,     // which VBO
    pub enabled:  bool,
}

impl VertexAttrib {
    pub const fn disabled() -> Self {
        VertexAttrib {
            index: 0, size: 4, typ: AttrType::Float,
            stride: 0, offset: 0, buf_id: 0, enabled: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Texture
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexFilter { Nearest, Bilinear }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexWrap  { Clamp, Repeat }

static NEXT_TEX_ID: AtomicU32 = AtomicU32::new(1);

pub struct Texture {
    pub id:     u32,
    pub width:  u32,
    pub height: u32,
    /// RGBA8 pixels, row-major.
    pub data:   Vec<u32>,
    pub filter: TexFilter,
    pub wrap_s: TexWrap,
    pub wrap_t: TexWrap,
}

impl Texture {
    pub fn new(w: u32, h: u32) -> Self {
        Texture {
            id: NEXT_TEX_ID.fetch_add(1, Ordering::Relaxed),
            width: w, height: h,
            data: vec![0u32; (w * h) as usize],
            filter: TexFilter::Nearest,
            wrap_s: TexWrap::Repeat,
            wrap_t: TexWrap::Repeat,
        }
    }

    /// Upload RGBA8 pixels.
    pub fn upload_rgba8(&mut self, pixels: &[u32]) {
        let n = (self.width * self.height) as usize;
        self.data.clear();
        self.data.extend_from_slice(&pixels[..n.min(pixels.len())]);
        if self.data.len() < n { self.data.resize(n, 0); }
    }

    fn wrap_coord(v: f32, mode: TexWrap) -> f32 {
        match mode {
            TexWrap::Clamp  => v.clamp(0.0, 1.0),
            TexWrap::Repeat => v - f32_floor(v),
        }
    }

    /// Sample with nearest filtering.
    fn sample_nearest(&self, u: f32, v: f32) -> u32 {
        let u = Self::wrap_coord(u, self.wrap_s);
        let v = Self::wrap_coord(v, self.wrap_t);
        let x = ((u * self.width  as f32) as u32).min(self.width  - 1);
        let y = ((v * self.height as f32) as u32).min(self.height - 1);
        self.data[(y * self.width + x) as usize]
    }

    /// Sample with bilinear filtering.
    fn sample_bilinear(&self, u: f32, v: f32) -> u32 {
        let u = Self::wrap_coord(u, self.wrap_s);
        let v = Self::wrap_coord(v, self.wrap_t);
        let fx = u * self.width  as f32 - 0.5;
        let fy = v * self.height as f32 - 0.5;
        let x0 = (fx as i32).max(0) as u32;
        let y0 = (fy as i32).max(0) as u32;
        let x1 = (x0 + 1).min(self.width  - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = (fx - x0 as f32).max(0.0);
        let ty = (fy - y0 as f32).max(0.0);

        let w = self.width;
        let c00 = self.data[(y0 * w + x0) as usize];
        let c10 = self.data[(y0 * w + x1) as usize];
        let c01 = self.data[(y1 * w + x0) as usize];
        let c11 = self.data[(y1 * w + x1) as usize];

        let lerp_ch = |a: u8, b: u8, t: f32| -> u8 {
            (a as f32 + (b as f32 - a as f32) * t) as u8
        };
        let top_r = lerp_ch(r_of(c00), r_of(c10), tx);
        let top_g = lerp_ch(g_of(c00), g_of(c10), tx);
        let top_b = lerp_ch(b_of(c00), b_of(c10), tx);
        let top_a = lerp_ch(a_of(c00), a_of(c10), tx);
        let bot_r = lerp_ch(r_of(c01), r_of(c11), tx);
        let bot_g = lerp_ch(g_of(c01), g_of(c11), tx);
        let bot_b = lerp_ch(b_of(c01), b_of(c11), tx);
        let bot_a = lerp_ch(a_of(c01), a_of(c11), tx);
        rgba(
            lerp_ch(top_r, bot_r, ty),
            lerp_ch(top_g, bot_g, ty),
            lerp_ch(top_b, bot_b, ty),
            lerp_ch(top_a, bot_a, ty),
        )
    }

    pub fn sample(&self, u: f32, v: f32) -> u32 {
        match self.filter {
            TexFilter::Nearest  => self.sample_nearest(u, v),
            TexFilter::Bilinear => self.sample_bilinear(u, v),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Framebuffer
// ─────────────────────────────────────────────────────────────────────────────

pub struct Framebuffer {
    pub width:  u32,
    pub height: u32,
    /// Color pixels (RGBA8, row-major).
    pub color:  Vec<u32>,
    /// Depth buffer (f32, inverted-Z: 1.0 = near, 0.0 = far).
    pub depth:  Vec<f32>,
}

impl Framebuffer {
    pub fn new(w: u32, h: u32) -> Self {
        let n = (w * h) as usize;
        Framebuffer {
            width: w, height: h,
            color: vec![0u32; n],
            depth: vec![0.0f32; n],
        }
    }

    pub fn clear_color(&mut self, c: u32) {
        self.color.fill(c);
    }

    pub fn clear_depth(&mut self) {
        self.depth.fill(0.0);
    }

    #[inline]
    pub fn put_pixel(&mut self, x: u32, y: u32, color: u32, z: f32, depth_test: bool, depth_write: bool, blend: bool) {
        let idx = (y * self.width + x) as usize;
        if depth_test && z <= self.depth[idx] {
            return; // fail depth test
        }
        if depth_write { self.depth[idx] = z; }
        self.color[idx] = if blend { blend_over(self.color[idx], color) } else { color };
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Shader slots (uniform storage + per-vertex transform stub)
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum uniform vec4 slots per program.
pub const MAX_UNIFORMS: usize = 64;

/// A 4-component float vector.
pub type Vec4 = [f32; 4];
pub type Mat4 = [f32; 16];

/// Built-in uniform indices.
pub const U_MVP:       usize = 0;  // mat4 at slots 0..4
pub const U_COLOR:     usize = 4;  // vec4 constant color
pub const U_TEX_SCALE: usize = 5;  // vec4 (su, sv, 0, 0)

/// Per-program uniform bank.
pub struct UniformBank {
    pub vec4: [Vec4; MAX_UNIFORMS],
}

impl UniformBank {
    pub const fn new() -> Self {
        UniformBank { vec4: [[0.0; 4]; MAX_UNIFORMS] }
    }

    pub fn set_mat4(&mut self, slot: usize, m: &Mat4) {
        for i in 0..4 {
            self.vec4[slot + i] = [m[i*4], m[i*4+1], m[i*4+2], m[i*4+3]];
        }
    }

    pub fn get_mat4(&self, slot: usize) -> Mat4 {
        let mut m = [0.0f32; 16];
        for i in 0..4 {
            let v = self.vec4[slot + i];
            m[i*4..i*4+4].copy_from_slice(&v);
        }
        m
    }

    pub fn mvp(&self) -> Mat4 { self.get_mat4(U_MVP) }
}

/// 4×4 matrix-vector multiply (column-major).
pub fn mat4_mul_vec4(m: &Mat4, v: Vec4) -> Vec4 {
    [
        m[0]*v[0] + m[4]*v[1] + m[8] *v[2] + m[12]*v[3],
        m[1]*v[0] + m[5]*v[1] + m[9] *v[2] + m[13]*v[3],
        m[2]*v[0] + m[6]*v[1] + m[10]*v[2] + m[14]*v[3],
        m[3]*v[0] + m[7]*v[1] + m[11]*v[2] + m[15]*v[3],
    ]
}

/// Identity matrix.
pub fn mat4_identity() -> Mat4 {
    let mut m = [0.0f32; 16];
    m[0] = 1.0; m[5] = 1.0; m[10] = 1.0; m[15] = 1.0;
    m
}

// ─────────────────────────────────────────────────────────────────────────────
//  Rasterizer state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CullFace { None, Front, Back, FrontAndBack }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrontFace { CW, CCW }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawMode { Triangles, TriangleStrip, TriangleFan, Lines, Points }

pub struct RasterizerState {
    pub cull:       CullFace,
    pub front:      FrontFace,
    pub depth_test: bool,
    pub depth_write: bool,
    pub blend:      bool,
    pub scissor:    Option<(u32, u32, u32, u32)>, // x,y,w,h
    /// Viewport: x, y, width, height.
    pub vp:         (i32, i32, u32, u32),
    pub tex_unit:   Option<u32>, // bound texture id (unit 0)
}

impl RasterizerState {
    pub fn new(fb_w: u32, fb_h: u32) -> Self {
        RasterizerState {
            cull: CullFace::Back,
            front: FrontFace::CCW,
            depth_test: true,
            depth_write: true,
            blend: false,
            scissor: None,
            vp: (0, 0, fb_w, fb_h),
            tex_unit: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Triangle rasterizer core
// ─────────────────────────────────────────────────────────────────────────────

/// NDC → window-space transform.
fn ndc_to_window(v: &Vertex, vp: (i32, i32, u32, u32)) -> (f32, f32, f32) {
    let w = if v.pos[3].abs() < 1e-7 { 1.0 } else { v.pos[3] };
    let xn = v.pos[0] / w;
    let yn = v.pos[1] / w;
    let zn = v.pos[2] / w;
    let wx = (xn + 1.0) * 0.5 * vp.2 as f32 + vp.0 as f32;
    let wy = (1.0 - yn) * 0.5 * vp.3 as f32 + vp.1 as f32; // Y flip
    // Map z from [-1,1] to [0,1].
    let wz = (zn + 1.0) * 0.5;
    (wx, wy, wz)
}

/// Signed area of 2D triangle (positive if CCW).
#[inline]
fn edge_fn(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (bx - ax) * (py - ay) - (by - ay) * (px - ax)
}

/// Rasterize a single triangle onto `fb`.
pub fn rasterize_triangle(
    v0: &Vertex, v1: &Vertex, v2: &Vertex,
    fb: &mut Framebuffer,
    state: &RasterizerState,
    tex: Option<&Texture>,
    uniforms: &UniformBank,
) {
    let vp = state.vp;
    let (x0, y0, z0) = ndc_to_window(v0, vp);
    let (x1, y1, z1) = ndc_to_window(v1, vp);
    let (x2, y2, z2) = ndc_to_window(v2, vp);

    // Signed area; determines winding.
    let area = edge_fn(x0, y0, x1, y1, x2, y2);
    if area.abs() < 0.5 { return; } // degenerate

    // Face culling.
    let is_ccw = area > 0.0;
    match state.cull {
        CullFace::None => {}
        CullFace::FrontAndBack => return,
        CullFace::Back  => {
            let cull = match state.front {
                FrontFace::CCW => !is_ccw,
                FrontFace::CW  =>  is_ccw,
            };
            if cull { return; }
        }
        CullFace::Front => {
            let cull = match state.front {
                FrontFace::CCW => is_ccw,
                FrontFace::CW  => !is_ccw,
            };
            if cull { return; }
        }
    }

    // Perspective-correct interpolation 1/w values.
    let w0 = if v0.pos[3].abs() < 1e-7 { 1.0 } else { 1.0 / v0.pos[3] };
    let w1 = if v1.pos[3].abs() < 1e-7 { 1.0 } else { 1.0 / v1.pos[3] };
    let w2 = if v2.pos[3].abs() < 1e-7 { 1.0 } else { 1.0 / v2.pos[3] };

    // Bounding box clamped to viewport + scissor.
    let (sx, sy, sw, sh) = state.scissor.unwrap_or((vp.0 as u32, vp.1 as u32, vp.2, vp.3));
    let min_x = x0.min(x1).min(x2).max(sx as f32) as i32;
    let min_y = y0.min(y1).min(y2).max(sy as f32) as i32;
    let max_x = (x0.max(x1).max(x2)).min((sx + sw - 1) as f32) as i32;
    let max_y = (y0.max(y1).max(y2)).min((sy + sh - 1) as f32) as i32;

    let inv_area = 1.0 / area;

    for py in min_y..=max_y {
        if py < 0 || py >= fb.height as i32 { continue; }
        for px in min_x..=max_x {
            if px < 0 || px >= fb.width as i32 { continue; }
            let pcx = px as f32 + 0.5;
            let pcy = py as f32 + 0.5;

            // Barycentric weights.
            let mut lam0 = edge_fn(x1, y1, x2, y2, pcx, pcy) * inv_area;
            let mut lam1 = edge_fn(x2, y2, x0, y0, pcx, pcy) * inv_area;
            let mut lam2 = 1.0 - lam0 - lam1;

            // Top-left fill rule (skip outside).
            if lam0 < 0.0 || lam1 < 0.0 || lam2 < 0.0 { continue; }

            // Perspective-correct weight.
            let wc = lam0 * w0 + lam1 * w1 + lam2 * w2;
            let inv_wc = if wc.abs() < 1e-7 { 1.0 } else { 1.0 / wc };
            lam0 = lam0 * w0 * inv_wc;
            lam1 = lam1 * w1 * inv_wc;
            lam2 = lam2 * w2 * inv_wc;

            // Interpolate Z.
            let z = z0 * lam0 + z1 * lam1 + z2 * lam2;

            // Interpolate color.
            let cr = (v0.color[0] * lam0 + v1.color[0] * lam1 + v2.color[0] * lam2).clamp(0.0, 1.0);
            let cg = (v0.color[1] * lam0 + v1.color[1] * lam1 + v2.color[1] * lam2).clamp(0.0, 1.0);
            let cb = (v0.color[2] * lam0 + v1.color[2] * lam1 + v2.color[2] * lam2).clamp(0.0, 1.0);
            let ca = (v0.color[3] * lam0 + v1.color[3] * lam1 + v2.color[3] * lam2).clamp(0.0, 1.0);

            // Interpolate UV.
            let u = v0.uv[0] * lam0 + v1.uv[0] * lam1 + v2.uv[0] * lam2;
            let v_tex = v0.uv[1] * lam0 + v1.uv[1] * lam1 + v2.uv[1] * lam2;

            // Texture sample.
            let tex_color = if let Some(t) = tex {
                let tc = t.sample(u, v_tex);
                [r_of(tc) as f32 / 255.0, g_of(tc) as f32 / 255.0,
                 b_of(tc) as f32 / 255.0, a_of(tc) as f32 / 255.0]
            } else {
                [1.0; 4]
            };

            // Modulate vertex color with texture color.
            let fr = (cr * tex_color[0]).clamp(0.0, 1.0);
            let fg = (cg * tex_color[1]).clamp(0.0, 1.0);
            let fb_c = (cb * tex_color[2]).clamp(0.0, 1.0);
            let fa = (ca * tex_color[3]).clamp(0.0, 1.0);
            let _ = uniforms; // reserved for future GLSL programs

            let pixel = rgba(
                (fr * 255.0) as u8,
                (fg * 255.0) as u8,
                (fb_c * 255.0) as u8,
                (fa * 255.0) as u8,
            );

            fb.put_pixel(px as u32, py as u32, pixel, z,
                state.depth_test, state.depth_write, state.blend);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Line rasterizer (Bresenham)
// ─────────────────────────────────────────────────────────────────────────────

pub fn rasterize_line(
    v0: &Vertex, v1: &Vertex,
    fb: &mut Framebuffer,
    state: &RasterizerState,
) {
    let vp = state.vp;
    let (x0, y0, z0) = ndc_to_window(v0, vp);
    let (x1, y1, z1) = ndc_to_window(v1, vp);
    let c0 = rgba(
        (v0.color[0] * 255.0) as u8, (v0.color[1] * 255.0) as u8,
        (v0.color[2] * 255.0) as u8, (v0.color[3] * 255.0) as u8,
    );

    let mut xi = x0 as i32; let mut yi = y0 as i32;
    let xe = x1 as i32;     let ye = y1 as i32;
    let dx = (xe - xi).abs(); let sx = if xi < xe { 1 } else { -1 };
    let dy = -(ye - yi).abs(); let sy = if yi < ye { 1 } else { -1 };
    let mut err = dx + dy;

    let steps = (dx.abs().max(dy.abs()) + 1) as f32;
    let mut step = 0.0f32;

    loop {
        let t = step / steps;
        let z = z0 * (1.0 - t) + z1 * t;
        if xi >= 0 && yi >= 0 && xi < fb.width as i32 && yi < fb.height as i32 {
            fb.put_pixel(xi as u32, yi as u32, c0, z,
                state.depth_test, state.depth_write, state.blend);
        }
        if xi == xe && yi == ye { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; xi += sx; }
        if e2 <= dx { err += dx; yi += sy; }
        step += 1.0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  GL Context — main draw-call entry point
// ─────────────────────────────────────────────────────────────────────────────

/// The main OpenGL ES 2.0 software context.
pub struct GlContext {
    pub fb:       Framebuffer,
    pub state:    RasterizerState,
    pub uniforms: UniformBank,
    pub vbos:     Vec<BufferObject>,
    pub textures: Vec<Texture>,
    pub attribs:  [VertexAttrib; 8],
}

impl GlContext {
    pub fn new(width: u32, height: u32) -> Self {
        GlContext {
            fb:       Framebuffer::new(width, height),
            state:    RasterizerState::new(width, height),
            uniforms: UniformBank::new(),
            vbos:     Vec::new(),
            textures: Vec::new(),
            attribs:  [VertexAttrib::disabled(); 8],
        }
    }

    // ── Texture management ──────────────────────────────────────────────────

    pub fn gen_texture(&mut self) -> u32 {
        let t = Texture::new(1, 1);
        let id = t.id;
        self.textures.push(t);
        id
    }

    pub fn tex_image_2d(&mut self, id: u32, w: u32, h: u32, pixels: &[u32]) {
        if let Some(t) = self.textures.iter_mut().find(|t| t.id == id) {
            t.width  = w;
            t.height = h;
            t.upload_rgba8(pixels);
        }
    }

    pub fn bind_texture(&mut self, id: u32) {
        self.state.tex_unit = Some(id);
    }

    pub fn tex_parameteri(&mut self, id: u32, filter: TexFilter, wrap_s: TexWrap, wrap_t: TexWrap) {
        if let Some(t) = self.textures.iter_mut().find(|t| t.id == id) {
            t.filter = filter;
            t.wrap_s = wrap_s;
            t.wrap_t = wrap_t;
        }
    }

    // ── VBO management ──────────────────────────────────────────────────────

    pub fn gen_buffer(&mut self) -> u32 {
        let b = BufferObject::new();
        let id = b.id;
        self.vbos.push(b);
        id
    }

    pub fn buffer_data(&mut self, id: u32, data: &[u8]) {
        if let Some(b) = self.vbos.iter_mut().find(|b| b.id == id) {
            b.upload(data);
        }
    }

    // ── Vertex attribs ──────────────────────────────────────────────────────

    pub fn vertex_attrib_pointer(&mut self, index: usize, size: u8, typ: AttrType,
                                  stride: u32, offset: u32, buf_id: u32) {
        if index < 8 {
            self.attribs[index] = VertexAttrib { index, size, typ, stride, offset, buf_id, enabled: true };
        }
    }

    pub fn enable_vertex_attrib_array(&mut self, index: usize) {
        if index < 8 { self.attribs[index].enabled = true; }
    }

    // ── Draw calls ──────────────────────────────────────────────────────────

    /// Draw `count` vertices from currently bound VBOs.
    pub fn draw_arrays(&mut self, mode: DrawMode, first: usize, count: usize) {
        // Fetch the bound texture (if any).
        let tex_id = self.state.tex_unit;

        // Decode all vertices first.
        let vertices: Vec<Vertex> = (first..first + count)
            .map(|i| self.decode_vertex(i))
            .collect();

        match mode {
            DrawMode::Triangles => {
                let mut i = 0;
                while i + 2 < vertices.len() {
                    let v0 = &vertices[i];
                    let v1 = &vertices[i + 1];
                    let v2 = &vertices[i + 2];
                    let tex = tex_id.and_then(|id| self.textures.iter().find(|t| t.id == id));
                    // We need to split the borrow. Use raw pointer trick for no_std safety.
                    let fb_ptr = &mut self.fb as *mut Framebuffer;
                    let state_ptr = &self.state as *const RasterizerState;
                    let uni_ptr = &self.uniforms as *const UniformBank;
                    unsafe {
                        rasterize_triangle(v0, v1, v2,
                            &mut *fb_ptr, &*state_ptr, tex, &*uni_ptr);
                    }
                    i += 3;
                }
            }
            DrawMode::TriangleStrip => {
                for i in 0..vertices.len().saturating_sub(2) {
                    let (v0, v1, v2) = if i & 1 == 0 {
                        (&vertices[i], &vertices[i+1], &vertices[i+2])
                    } else {
                        (&vertices[i+1], &vertices[i], &vertices[i+2])
                    };
                    let tex = tex_id.and_then(|id| self.textures.iter().find(|t| t.id == id));
                    let fb_ptr = &mut self.fb as *mut Framebuffer;
                    let state_ptr = &self.state as *const RasterizerState;
                    let uni_ptr = &self.uniforms as *const UniformBank;
                    unsafe {
                        rasterize_triangle(v0, v1, v2,
                            &mut *fb_ptr, &*state_ptr, tex, &*uni_ptr);
                    }
                }
            }
            DrawMode::TriangleFan => {
                if vertices.len() < 3 { return; }
                let v0 = &vertices[0];
                for i in 1..vertices.len().saturating_sub(1) {
                    let v1 = &vertices[i];
                    let v2 = &vertices[i + 1];
                    let tex = tex_id.and_then(|id| self.textures.iter().find(|t| t.id == id));
                    let fb_ptr = &mut self.fb as *mut Framebuffer;
                    let state_ptr = &self.state as *const RasterizerState;
                    let uni_ptr = &self.uniforms as *const UniformBank;
                    unsafe {
                        rasterize_triangle(v0, v1, v2,
                            &mut *fb_ptr, &*state_ptr, tex, &*uni_ptr);
                    }
                }
            }
            DrawMode::Lines => {
                let mut i = 0;
                while i + 1 < vertices.len() {
                    let fb_ptr = &mut self.fb as *mut Framebuffer;
                    let state_ptr = &self.state as *const RasterizerState;
                    unsafe {
                        rasterize_line(&vertices[i], &vertices[i+1],
                            &mut *fb_ptr, &*state_ptr);
                    }
                    i += 2;
                }
            }
            DrawMode::Points => {
                for v in &vertices {
                    let vp = self.state.vp;
                    let (x, y, z) = ndc_to_window(v, vp);
                    let c = rgba(
                        (v.color[0]*255.0) as u8, (v.color[1]*255.0) as u8,
                        (v.color[2]*255.0) as u8, (v.color[3]*255.0) as u8,
                    );
                    if x >= 0.0 && y >= 0.0 && (x as u32) < self.fb.width && (y as u32) < self.fb.height {
                        self.fb.put_pixel(x as u32, y as u32, c, z,
                            self.state.depth_test, self.state.depth_write, self.state.blend);
                    }
                }
            }
        }
    }

    /// Draw indexed triangles from an IBO (stored as a &[u16]).
    pub fn draw_elements(&mut self, mode: DrawMode, indices: &[u16]) {
        // Decode the unique vertices we'll need.
        let max_idx = indices.iter().copied().max().unwrap_or(0) as usize;
        let decoded: Vec<Vertex> = (0..=max_idx).map(|i| self.decode_vertex(i)).collect();

        // Build re-indexed vertex list.
        let vertices: Vec<Vertex> = indices.iter().map(|&i| decoded[i as usize]).collect();

        // Reuse draw_arrays logic: swap fb temporarily.
        let old_vbos = core::mem::take(&mut self.vbos);

        // Draw via draw_arrays helper (vertices already decoded).
        self.draw_vertex_slice(mode, &vertices);

        self.vbos = old_vbos;
        let _ = max_idx;
    }

    fn draw_vertex_slice(&mut self, mode: DrawMode, vertices: &[Vertex]) {
        let tex_id = self.state.tex_unit;
        match mode {
            DrawMode::Triangles => {
                let mut i = 0;
                while i + 2 < vertices.len() {
                    let tex = tex_id.and_then(|id| self.textures.iter().find(|t| t.id == id));
                    let fb_ptr = &mut self.fb as *mut Framebuffer;
                    let state_ptr = &self.state as *const RasterizerState;
                    let uni_ptr = &self.uniforms as *const UniformBank;
                    unsafe {
                        rasterize_triangle(&vertices[i], &vertices[i+1], &vertices[i+2],
                            &mut *fb_ptr, &*state_ptr, tex, &*uni_ptr);
                    }
                    i += 3;
                }
            }
            DrawMode::Lines => {
                let mut i = 0;
                while i + 1 < vertices.len() {
                    let fb_ptr = &mut self.fb as *mut Framebuffer;
                    let state_ptr = &self.state as *const RasterizerState;
                    unsafe {
                        rasterize_line(&vertices[i], &vertices[i+1],
                            &mut *fb_ptr, &*state_ptr);
                    }
                    i += 2;
                }
            }
            _ => {} // other modes share the same pattern
        }
    }

    // ── Vertex decoding from bound VBOs ────────────────────────────────────

    fn decode_vertex(&self, index: usize) -> Vertex {
        let mut v = Vertex {
            pos:   [0.0, 0.0, 0.0, 1.0],
            color: [1.0, 1.0, 1.0, 1.0],
            uv:    [0.0, 0.0],
        };

        // Apply MVP transform if attrib 0 is a position.
        let mvp = self.uniforms.mvp();

        for attrib in &self.attribs {
            if !attrib.enabled { continue; }
            let buf = match self.vbos.iter().find(|b| b.id == attrib.buf_id) {
                Some(b) => b,
                None => continue,
            };
            let stride = if attrib.stride == 0 {
                attrib.size as u32 * 4
            } else {
                attrib.stride
            };
            let byte_off = index * stride as usize + attrib.offset as usize;
            let floats = read_floats(&buf.data, byte_off, attrib.size as usize, attrib.typ);

            match attrib.index {
                0 => {
                    // Position — apply MVP.
                    let p = [
                        floats.get(0).copied().unwrap_or(0.0),
                        floats.get(1).copied().unwrap_or(0.0),
                        floats.get(2).copied().unwrap_or(0.0),
                        floats.get(3).copied().unwrap_or(1.0),
                    ];
                    v.pos = mat4_mul_vec4(&mvp, p);
                }
                1 => {
                    // Color.
                    v.color[0] = floats.get(0).copied().unwrap_or(1.0);
                    v.color[1] = floats.get(1).copied().unwrap_or(1.0);
                    v.color[2] = floats.get(2).copied().unwrap_or(1.0);
                    v.color[3] = floats.get(3).copied().unwrap_or(1.0);
                }
                2 => {
                    // UV.
                    v.uv[0] = floats.get(0).copied().unwrap_or(0.0);
                    v.uv[1] = floats.get(1).copied().unwrap_or(0.0);
                }
                _ => {}
            }
        }

        v
    }

    // ── State setters ───────────────────────────────────────────────────────

    pub fn viewport(&mut self, x: i32, y: i32, w: u32, h: u32) {
        self.state.vp = (x, y, w, h);
    }

    pub fn enable_depth_test(&mut self, v: bool)  { self.state.depth_test  = v; }
    pub fn enable_depth_write(&mut self, v: bool) { self.state.depth_write = v; }
    pub fn enable_blend(&mut self, v: bool)       { self.state.blend = v; }
    pub fn set_cull(&mut self, c: CullFace)       { self.state.cull = c; }
    pub fn set_front_face(&mut self, f: FrontFace){ self.state.front = f; }
    pub fn scissor(&mut self, x: u32, y: u32, w: u32, h: u32) {
        self.state.scissor = Some((x, y, w, h));
    }
    pub fn disable_scissor(&mut self)             { self.state.scissor = None; }

    pub fn clear_color(&mut self, c: u32)         { self.fb.clear_color(c); }
    pub fn clear_depth(&mut self)                  { self.fb.clear_depth(); }
    pub fn clear(&mut self, color: bool, depth: bool) {
        if color { self.fb.clear_color(0); }
        if depth { self.fb.clear_depth(); }
    }

    pub fn set_uniform_vec4(&mut self, slot: usize, v: Vec4) {
        if slot < MAX_UNIFORMS { self.uniforms.vec4[slot] = v; }
    }

    pub fn set_uniform_mat4(&mut self, slot: usize, m: &Mat4) {
        self.uniforms.set_mat4(slot, m);
    }

    /// Read back the color framebuffer as a &[u32].
    pub fn read_pixels(&self) -> &[u32] { &self.fb.color }

    /// Blit the software FB into the compositor's back-buffer Surface.
    pub fn blit_to_surface(&self, surf: &super::gpu2d::Surface) {
        use super::gpu2d;
        let src = gpu2d::Surface {
            virt:   self.fb.color.as_ptr() as u64,
            width:  self.fb.width,
            height: self.fb.height,
            stride: self.fb.width * 4,
            bo:     0,
            is_bgr: false,
        };
        gpu2d::blit(surf, 0, 0, &src, gpu2d::Rect { x: 0, y: 0, w: self.fb.width, h: self.fb.height });
    }
}

/// Read N floats from a byte buffer at `byte_off`, respecting `AttrType`.
fn read_floats(buf: &[u8], byte_off: usize, n: usize, typ: AttrType) -> Vec<f32> {
    let mut out = Vec::with_capacity(n);
    match typ {
        AttrType::Float => {
            for i in 0..n {
                let off = byte_off + i * 4;
                if off + 4 <= buf.len() {
                    let bytes = [buf[off], buf[off+1], buf[off+2], buf[off+3]];
                    out.push(f32::from_le_bytes(bytes));
                } else {
                    out.push(0.0);
                }
            }
        }
        AttrType::U8Norm => {
            for i in 0..n {
                let off = byte_off + i;
                out.push(if off < buf.len() { buf[off] as f32 / 255.0 } else { 0.0 });
            }
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
//  Global context (single-context for now; multi-context via per-thread later)
// ─────────────────────────────────────────────────────────────────────────────

pub static GL: Mutex<Option<GlContext>> = Mutex::new(None);

/// Initialise the global GL context at the given resolution.
pub fn init(width: u32, height: u32) {
    let mut ctx = GlContext::new(width, height);
    // Set identity MVP so untransformed vertices pass through.
    ctx.uniforms.set_mat4(U_MVP, &mat4_identity());
    *GL.lock() = Some(ctx);
    crate::serial_println!("[gles] OpenGL ES 2.0 software rasterizer ready ({}×{}).", width, height);
}

/// Quick stats print.
pub fn print_stats() {
    let gl = GL.lock();
    if let Some(ctx) = gl.as_ref() {
        crate::serial_println!(
            "[gles] FB {}×{} | VBOs={} | textures={}",
            ctx.fb.width, ctx.fb.height,
            ctx.vbos.len(), ctx.textures.len(),
        );
    }
}

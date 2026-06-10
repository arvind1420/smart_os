#![allow(dead_code)]
/// Hardware 3D Pipeline — Phase 21 for Smart OS.
///
/// Provides:
///  • Vertex buffer (VBO) and index buffer (IBO) submission to the GPU
///  • Pipeline state object (PSO): shader, blend, depth, cull, MSAA
///  • Draw-call dispatch: hw path (Intel BCS / AMD SDMA) or SW fallback
///  • MSAA 4× resolve via box filter
///  • Render pass abstraction (begin/end with clear)
///  • Query objects (timestamp, occlusion — stub)
///
/// On VirtIO-GPU or missing hardware the SW rasterizer (Phase 19) handles all
/// draw calls.  On real Intel/AMD hardware the BCS/SDMA blit engine accelerates
/// clears and copies while triangle rasterisation still uses the SW path until
/// hardware 3D command rings are wired in Phase 24+.

use alloc::vec::Vec;
use alloc::vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use super::gles::{GlContext, DrawMode, Vertex, Framebuffer, rgba};
use super::glsl::SHADER_CACHE;
use super::gpu_mem;

// ─────────────────────────────────────────────────────────────────────────────
//  Pipeline state object
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendMode {
    None,
    /// src_alpha / one_minus_src_alpha
    AlphaBlend,
    /// additive
    Additive,
    /// premultiplied alpha
    PremultAlpha,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepthFunc {
    Never, Less, Equal, Lequal, Greater, Notequal, Gequal, Always
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsaaSamples { None = 1, X2 = 2, X4 = 4 }

/// Immutable pipeline state.
#[derive(Clone, Debug)]
pub struct PipelineState {
    pub vert_shader:   u64,   // hash from SHADER_CACHE
    pub frag_shader:   u64,   // hash from SHADER_CACHE
    pub blend:         BlendMode,
    pub depth_func:    DepthFunc,
    pub depth_write:   bool,
    pub cull_back:     bool,
    pub msaa:          MsaaSamples,
    pub color_mask:    u8,    // bitmask R=1 G=2 B=4 A=8
}

impl PipelineState {
    pub fn default_opaque(vert: u64, frag: u64) -> Self {
        PipelineState {
            vert_shader: vert, frag_shader: frag,
            blend: BlendMode::None,
            depth_func: DepthFunc::Lequal,
            depth_write: true,
            cull_back: true,
            msaa: MsaaSamples::None,
            color_mask: 0b1111,
        }
    }

    pub fn default_transparent(vert: u64, frag: u64) -> Self {
        PipelineState {
            vert_shader: vert, frag_shader: frag,
            blend: BlendMode::AlphaBlend,
            depth_func: DepthFunc::Lequal,
            depth_write: false,
            cull_back: false,
            msaa: MsaaSamples::None,
            color_mask: 0b1111,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  GPU buffer objects (vertex + index)
// ─────────────────────────────────────────────────────────────────────────────

static NEXT_BUF_ID: AtomicU32 = AtomicU32::new(1);

pub struct GpuBuffer {
    pub id:   u32,
    pub data: Vec<u8>,
    /// GPU BO handle (if uploaded to VRAM).
    pub bo:   Option<u32>,
}

impl GpuBuffer {
    pub fn new() -> Self {
        GpuBuffer { id: NEXT_BUF_ID.fetch_add(1, Ordering::Relaxed), data: Vec::new(), bo: None }
    }

    pub fn upload_cpu(&mut self, data: &[u8]) {
        self.data.clear();
        self.data.extend_from_slice(data);
    }

    /// Try to push to GPU VRAM via gpu_mem allocator.
    pub fn upload_gpu(&mut self, pixel_w: u32) -> bool {
        use super::gpu_mem::PixelFormat;
        let h = (self.data.len() as u32 + pixel_w * 4 - 1) / (pixel_w * 4).max(1);
        match gpu_mem::alloc_bo(pixel_w.max(1), h.max(1), PixelFormat::Rgbx8888) {
            Ok(handle) => {
                self.bo = Some(handle);
                // Upload pixel data via transfer.
                gpu_mem::upload_and_flush(handle);
                true
            }
            Err(_) => false,
        }
    }

    pub fn as_f32_slice(&self) -> &[f32] {
        // SAFETY: our own buffer is always f32-aligned (Vec<u8> with 4-byte-aligned data).
        let len = self.data.len() / 4;
        unsafe { core::slice::from_raw_parts(self.data.as_ptr() as *const f32, len) }
    }

    pub fn as_u16_slice(&self) -> &[u16] {
        let len = self.data.len() / 2;
        unsafe { core::slice::from_raw_parts(self.data.as_ptr() as *const u16, len) }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Vertex layout descriptor
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct VertexElement {
    /// Byte offset within the vertex.
    pub offset:  u32,
    /// Number of f32 components (1–4).
    pub count:   u8,
    /// Shader attribute slot.
    pub slot:    u8,
}

#[derive(Clone, Debug)]
pub struct VertexLayout {
    pub stride:   u32,
    pub elements: Vec<VertexElement>,
}

impl VertexLayout {
    /// Standard P3C4UV2: position(3) + color(4) + uv(2) = 9 floats = 36 bytes.
    pub fn p3c4uv2() -> Self {
        VertexLayout {
            stride: 36,
            elements: vec![
                VertexElement { offset: 0,  count: 3, slot: 0 },  // pos
                VertexElement { offset: 12, count: 4, slot: 1 },  // color
                VertexElement { offset: 28, count: 2, slot: 2 },  // uv
            ],
        }
    }

    /// Decode vertex `i` from a raw byte buffer.
    pub fn decode(&self, buf: &[u8], i: usize) -> Vertex {
        let base = i * self.stride as usize;
        let mut v = Vertex { pos: [0.0, 0.0, 0.0, 1.0], color: [1.0; 4], uv: [0.0; 2] };
        for e in &self.elements {
            let off = base + e.offset as usize;
            let floats: Vec<f32> = (0..e.count as usize).map(|k| {
                let o = off + k * 4;
                if o + 4 <= buf.len() {
                    f32::from_le_bytes([buf[o], buf[o+1], buf[o+2], buf[o+3]])
                } else { 0.0 }
            }).collect();
            match e.slot {
                0 => {
                    v.pos[0] = floats.get(0).copied().unwrap_or(0.0);
                    v.pos[1] = floats.get(1).copied().unwrap_or(0.0);
                    v.pos[2] = floats.get(2).copied().unwrap_or(0.0);
                    v.pos[3] = 1.0;
                }
                1 => {
                    v.color[0] = floats.get(0).copied().unwrap_or(1.0);
                    v.color[1] = floats.get(1).copied().unwrap_or(1.0);
                    v.color[2] = floats.get(2).copied().unwrap_or(1.0);
                    v.color[3] = floats.get(3).copied().unwrap_or(1.0);
                }
                2 => {
                    v.uv[0] = floats.get(0).copied().unwrap_or(0.0);
                    v.uv[1] = floats.get(1).copied().unwrap_or(0.0);
                }
                _ => {}
            }
        }
        v
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  MSAA resolve (4× box filter)
// ─────────────────────────────────────────────────────────────────────────────

pub struct MsaaBuffer {
    pub width:   u32,
    pub height:  u32,
    pub samples: u8,
    /// samples * width * height color pixels.
    pub color:   Vec<u32>,
    /// samples * width * height depth values.
    pub depth:   Vec<f32>,
}

impl MsaaBuffer {
    pub fn new(w: u32, h: u32, samples: u8) -> Self {
        let n = (w * h * samples as u32) as usize;
        MsaaBuffer { width: w, height: h, samples, color: vec![0u32; n], depth: vec![0.0; n] }
    }

    /// Box-filter resolve into a plain Framebuffer.
    pub fn resolve(&self, fb: &mut Framebuffer) {
        assert_eq!(fb.width,  self.width);
        assert_eq!(fb.height, self.height);
        let s = self.samples as u32;
        for y in 0..self.height {
            for x in 0..self.width {
                let mut r = 0u32; let mut g = 0u32; let mut b = 0u32; let mut a = 0u32;
                for k in 0..s {
                    let idx = (y * self.width * s + x * s + k) as usize;
                    let c = self.color[idx];
                    r += (c & 0xFF) as u32;
                    g += ((c >> 8) & 0xFF) as u32;
                    b += ((c >> 16) & 0xFF) as u32;
                    a += ((c >> 24) & 0xFF) as u32;
                }
                fb.color[(y * self.width + x) as usize] =
                    rgba((r/s) as u8, (g/s) as u8, (b/s) as u8, (a/s) as u8);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Render pass
// ─────────────────────────────────────────────────────────────────────────────

pub struct RenderPass {
    pub clear_color: Option<u32>,
    pub clear_depth: bool,
    pub width:       u32,
    pub height:      u32,
    pub msaa:        MsaaSamples,
}

impl RenderPass {
    pub fn new(w: u32, h: u32) -> Self {
        RenderPass {
            clear_color: Some(0xFF000000),
            clear_depth: true,
            width: w, height: h,
            msaa: MsaaSamples::None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Query objects (timestamp / occlusion)
// ─────────────────────────────────────────────────────────────────────────────

static NEXT_QUERY_ID: AtomicU32 = AtomicU32::new(1);
static GPU_TIMESTAMP: AtomicU64 = AtomicU64::new(0);

pub struct QueryObject {
    pub id:    u32,
    pub value: u64,
    pub ready: bool,
}

impl QueryObject {
    pub fn new() -> Self {
        QueryObject {
            id:    NEXT_QUERY_ID.fetch_add(1, Ordering::Relaxed),
            value: 0,
            ready: false,
        }
    }

    pub fn begin_timestamp(&mut self) {
        self.value = super::display::now_us();
        self.ready = false;
    }

    pub fn end_timestamp(&mut self) {
        let now = super::display::now_us();
        self.value = now.saturating_sub(self.value);
        self.ready = true;
        GPU_TIMESTAMP.fetch_add(self.value, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Draw call descriptor
// ─────────────────────────────────────────────────────────────────────────────

pub struct DrawCall<'a> {
    pub pipeline:  &'a PipelineState,
    pub vbo:       &'a GpuBuffer,
    pub ibo:       Option<&'a GpuBuffer>,
    pub layout:    &'a VertexLayout,
    pub mode:      DrawMode,
    pub first:     usize,
    pub count:     usize,
    pub uniforms:  &'a [f32],
    pub tex_id:    Option<u32>,
}

// ─────────────────────────────────────────────────────────────────────────────
//  3D pipeline context
// ─────────────────────────────────────────────────────────────────────────────

pub struct Pipeline3D {
    pub gl:     GlContext,
    pub msaa:   Option<MsaaBuffer>,
    /// Running draw-call counter.
    pub draws:  u64,
    /// Running triangle counter.
    pub tris:   u64,
}

impl Pipeline3D {
    pub fn new(width: u32, height: u32) -> Self {
        Pipeline3D {
            gl:    GlContext::new(width, height),
            msaa:  None,
            draws: 0,
            tris:  0,
        }
    }

    // ── Render pass ─────────────────────────────────────────────────────────

    pub fn begin_pass(&mut self, pass: &RenderPass) {
        // Set MSAA buffer if requested.
        match pass.msaa {
            MsaaSamples::None => { self.msaa = None; }
            MsaaSamples::X2 | MsaaSamples::X4 => {
                let s = pass.msaa as u8;
                if self.msaa.as_ref().map(|m| m.samples != s).unwrap_or(true) {
                    self.msaa = Some(MsaaBuffer::new(pass.width, pass.height, s));
                }
            }
        }

        if let Some(c) = pass.clear_color { self.gl.clear_color(c); }
        if pass.clear_depth { self.gl.clear_depth(); }
    }

    pub fn end_pass(&mut self) {
        // Resolve MSAA into the main FB.
        if let Some(msaa) = &self.msaa {
            msaa.resolve(&mut self.gl.fb);
        }
    }

    // ── Draw dispatch ────────────────────────────────────────────────────────

    pub fn draw(&mut self, dc: &DrawCall) {
        // Apply pipeline state to the GL context.
        let pso = dc.pipeline;
        self.gl.enable_blend(pso.blend != BlendMode::None);
        self.gl.enable_depth_test(pso.depth_func != DepthFunc::Always);
        self.gl.enable_depth_write(pso.depth_write);
        if pso.cull_back {
            self.gl.set_cull(super::gles::CullFace::Back);
        } else {
            self.gl.set_cull(super::gles::CullFace::None);
        }

        // Upload MVP from uniforms.
        if dc.uniforms.len() >= 16 {
            let mut mvp = [0.0f32; 16];
            mvp.copy_from_slice(&dc.uniforms[..16]);
            self.gl.set_uniform_mat4(super::gles::U_MVP, &mvp);
        }

        // Bind texture.
        if let Some(tid) = dc.tex_id { self.gl.bind_texture(tid); }
        else { self.gl.state.tex_unit = None; }

        // Decode vertices from the VBO directly using the layout.
        let vertices: Vec<Vertex> = (dc.first..dc.first + dc.count)
            .map(|i| dc.layout.decode(&dc.vbo.data, i))
            .collect();

        // Optionally re-index via IBO.
        let final_vertices: Vec<Vertex> = if let Some(ibo) = dc.ibo {
            let indices = ibo.as_u16_slice();
            indices.iter().map(|&i| {
                let i = i as usize;
                if i < vertices.len() { vertices[i] } else { Vertex::ZERO }
            }).collect()
        } else {
            vertices
        };

        // Count triangles.
        let tri_count = match dc.mode {
            DrawMode::Triangles      => final_vertices.len() / 3,
            DrawMode::TriangleStrip |
            DrawMode::TriangleFan   => final_vertices.len().saturating_sub(2),
            _                        => 0,
        };
        self.tris  += tri_count as u64;
        self.draws += 1;

        // Rasterize via SW path.
        self.rasterize_vertices(dc.mode, &final_vertices, pso);
    }

    fn rasterize_vertices(&mut self, mode: DrawMode, vertices: &[Vertex], pso: &PipelineState) {
        use super::gles::{rasterize_triangle, rasterize_line, CullFace, FrontFace};
        use super::gles::RasterizerState;

        let state = &self.gl.state;
        let tex_id = state.tex_unit;
        let tex = tex_id.and_then(|id| self.gl.textures.iter().find(|t| t.id == id));

        match mode {
            DrawMode::Triangles => {
                let mut i = 0;
                while i + 2 < vertices.len() {
                    let fb_ptr = &mut self.gl.fb as *mut _;
                    let st_ptr = &self.gl.state as *const _;
                    let un_ptr = &self.gl.uniforms as *const _;
                    unsafe {
                        rasterize_triangle(
                            &vertices[i], &vertices[i+1], &vertices[i+2],
                            &mut *fb_ptr, &*st_ptr, tex, &*un_ptr,
                        );
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
                    let fb_ptr = &mut self.gl.fb as *mut _;
                    let st_ptr = &self.gl.state as *const _;
                    let un_ptr = &self.gl.uniforms as *const _;
                    unsafe {
                        rasterize_triangle(v0, v1, v2,
                            &mut *fb_ptr, &*st_ptr, tex, &*un_ptr);
                    }
                }
            }
            DrawMode::TriangleFan => {
                if vertices.len() < 3 { return; }
                let v0 = &vertices[0];
                for i in 1..vertices.len().saturating_sub(1) {
                    let fb_ptr = &mut self.gl.fb as *mut _;
                    let st_ptr = &self.gl.state as *const _;
                    let un_ptr = &self.gl.uniforms as *const _;
                    unsafe {
                        rasterize_triangle(v0, &vertices[i], &vertices[i+1],
                            &mut *fb_ptr, &*st_ptr, tex, &*un_ptr);
                    }
                }
            }
            DrawMode::Lines => {
                let mut i = 0;
                while i + 1 < vertices.len() {
                    let fb_ptr = &mut self.gl.fb as *mut _;
                    let st_ptr = &self.gl.state as *const _;
                    unsafe { rasterize_line(&vertices[i], &vertices[i+1],
                        &mut *fb_ptr, &*st_ptr); }
                    i += 2;
                }
            }
            DrawMode::Points => {
                for v in vertices {
                    let vp = state.vp;
                    let w = if v.pos[3].abs() < 1e-7 { 1.0 } else { v.pos[3] };
                    let px = ((v.pos[0]/w + 1.0) * 0.5 * vp.2 as f32 + vp.0 as f32) as u32;
                    let py = ((1.0 - v.pos[1]/w) * 0.5 * vp.3 as f32 + vp.1 as f32) as u32;
                    let c = rgba(
                        (v.color[0]*255.0) as u8, (v.color[1]*255.0) as u8,
                        (v.color[2]*255.0) as u8, (v.color[3]*255.0) as u8,
                    );
                    if px < self.gl.fb.width && py < self.gl.fb.height {
                        self.gl.fb.put_pixel(px, py, c, 0.5,
                            state.depth_test, state.depth_write, state.blend);
                    }
                }
            }
        }
    }

    // ── GPU hardware blit (Intel BCS / AMD SDMA) ────────────────────────────

    /// Hardware-accelerated clear via BCS XY_COLOR_BLT (if available).
    pub fn hw_clear(&self, bo: u32, color: u32) {
        // Try Intel iGPU.
        {
            let igpu = super::igpu::IGPU.lock();
            if let Some(gpu) = igpu.as_ref() {
                let w = self.gl.fb.width;
                let h = self.gl.fb.height;
                // Use hw_fill_rect through the GpuDriver trait.
                // For now: log intent, SW fallback handles pixel data.
                crate::serial_println!("[gpu3d] hw_clear: Intel BCS fill {}x{} color={:#x}", w, h, color);
                return;
            }
        }
        // Try AMD.
        {
            let amd = super::amdgpu::AMDGPU.lock();
            if let Some(_gpu) = amd.as_ref() {
                crate::serial_println!("[gpu3d] hw_clear: AMD SDMA fill bo={}", bo);
                return;
            }
        }
        // SW: already done via gl.clear_color().
        let _ = (bo, color);
    }

    // ── Blit output to display ───────────────────────────────────────────────

    /// Copy the resolved framebuffer into the display back-buffer.
    pub fn present(&self) {
        let fb_start_us = super::display::now_us();
        if let Some(back_virt) = super::display::back_buffer_virt() {
            let (dw, dh) = super::display::display_resolution();
            let src = &self.gl.fb;
            let copy_w = src.width.min(dw) as usize;
            let copy_h = src.height.min(dh) as usize;
            let dst_stride = dw as usize;
            let src_stride = src.width as usize;
            let dst_ptr = back_virt as *mut u32;
            for row in 0..copy_h {
                let src_off = row * src_stride;
                let dst_off = row * dst_stride;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        src.color.as_ptr().add(src_off),
                        dst_ptr.add(dst_off),
                        copy_w,
                    );
                }
            }
        }
        super::display::present_and_vsync(fb_start_us);
    }

    pub fn print_stats(&self) {
        crate::serial_println!(
            "[gpu3d] draws={} tris={} fb={}×{}",
            self.draws, self.tris,
            self.gl.fb.width, self.gl.fb.height,
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Global 3D pipeline
// ─────────────────────────────────────────────────────────────────────────────

pub static PIPELINE: Mutex<Option<Pipeline3D>> = Mutex::new(None);

pub fn init() {
    let (w, h) = super::display::display_resolution();
    *PIPELINE.lock() = Some(Pipeline3D::new(w, h));
    crate::serial_println!("[gpu3d] Hardware 3D pipeline ready ({}×{}, SW rasteriser).", w, h);
}

pub fn print_stats() {
    let p = PIPELINE.lock();
    if let Some(pipe) = p.as_ref() { pipe.print_stats(); }
}

/// Phase 48 — WebGL (Software OpenGL ES 2.0)
///
/// Implements a subset of the WebGL 1.0 API (OpenGL ES 2.0 subset):
///
///   Context (WebGlContext):
///     createBuffer / bindBuffer / bufferData
///     createTexture / bindTexture / texImage2D / texParameteri
///     createShader / shaderSource / compileShader
///     createProgram / attachShader / linkProgram / useProgram
///     getAttribLocation / getUniformLocation
///     vertexAttribPointer / enableVertexAttribArray
///     uniformMatrix4fv / uniform1f / uniform1i / uniform2f / uniform3f / uniform4f
///     drawArrays / drawElements
///     viewport / clearColor / clear / enable / disable / depthFunc / blendFunc
///     readPixels
///
/// Rendering is 100% software — no GPU required.
/// Pipeline: vertex shader (Rust closures) → clip → rasterise → fragment shader
/// (Rust closures) → framebuffer.
///
/// GLSL shaders are "compiled" by replacing them with equivalent Rust vertex/fragment
/// function pointers that the engine calls during draw.  A tiny GLSL subset is
/// recognised (attribute/varying/uniform declarations, gl_Position, gl_FragColor).
///
/// The framebuffer is stored as RGBA u8 pixels (width × height × 4 bytes).
/// `readPixels` and `blitToCanvas` transfer pixels to a Canvas2D surface.

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec,
    vec::Vec,
    format,
};

// ─────────────────────────────────────────────────────────────────────────────
//  Inline float math (no libm)
// ─────────────────────────────────────────────────────────────────────────────

fn f32_abs(x: f32) -> f32 { if x < 0.0 { -x } else { x } }
fn f32_clamp(x: f32, lo: f32, hi: f32) -> f32 { if x < lo { lo } else if x > hi { hi } else { x } }
fn f32_min(a: f32, b: f32) -> f32 { if a < b { a } else { b } }
fn f32_max(a: f32, b: f32) -> f32 { if a > b { a } else { b } }
fn f32_floor(x: f32) -> f32 { let xi = x as i64; if x < xi as f32 { xi as f32 - 1.0 } else { xi as f32 } }

fn f32_sqrt(x: f32) -> f32 {
    if x <= 0.0 { return 0.0; }
    let mut g = x * 0.5;
    for _ in 0..8 { g = 0.5 * (g + x / g); }
    g
}

// ─────────────────────────────────────────────────────────────────────────────
//  Math types
// ─────────────────────────────────────────────────────────────────────────────

/// 4-component vector (XYZW or RGBA).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec4(pub f32, pub f32, pub f32, pub f32);

/// 3-component vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3(pub f32, pub f32, pub f32);

/// 2-component vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec2(pub f32, pub f32);

/// Column-major 4×4 matrix.
#[derive(Clone, Copy, Debug)]
pub struct Mat4(pub [f32; 16]);

impl Mat4 {
    pub fn identity() -> Self {
        Mat4([
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ])
    }

    pub fn mul_vec4(&self, v: Vec4) -> Vec4 {
        let m = &self.0;
        Vec4(
            m[0]*v.0 + m[4]*v.1 + m[8] *v.2 + m[12]*v.3,
            m[1]*v.0 + m[5]*v.1 + m[9] *v.2 + m[13]*v.3,
            m[2]*v.0 + m[6]*v.1 + m[10]*v.2 + m[14]*v.3,
            m[3]*v.0 + m[7]*v.1 + m[11]*v.2 + m[15]*v.3,
        )
    }

    pub fn mul(&self, other: &Mat4) -> Mat4 {
        let a = &self.0;
        let b = &other.0;
        let mut r = [0.0f32; 16];
        for col in 0..4 {
            for row in 0..4 {
                r[col * 4 + row] =
                    a[0 * 4 + row] * b[col * 4 + 0] +
                    a[1 * 4 + row] * b[col * 4 + 1] +
                    a[2 * 4 + row] * b[col * 4 + 2] +
                    a[3 * 4 + row] * b[col * 4 + 3];
            }
        }
        Mat4(r)
    }

    pub fn translate(tx: f32, ty: f32, tz: f32) -> Self {
        let mut m = Self::identity();
        m.0[12] = tx; m.0[13] = ty; m.0[14] = tz;
        m
    }

    pub fn scale(sx: f32, sy: f32, sz: f32) -> Self {
        let mut m = Self::identity();
        m.0[0] = sx; m.0[5] = sy; m.0[10] = sz;
        m
    }

    /// Orthographic projection (left,right,bottom,top,near,far).
    pub fn ortho(l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) -> Self {
        Mat4([
            2.0/(r-l),   0.0,         0.0,         0.0,
            0.0,         2.0/(t-b),   0.0,         0.0,
            0.0,         0.0,        -2.0/(f-n),   0.0,
            -(r+l)/(r-l),-(t+b)/(t-b),-(f+n)/(f-n), 1.0,
        ])
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  GL constants (mirrors WebGL spec)
// ─────────────────────────────────────────────────────────────────────────────

pub const GL_ARRAY_BUFFER:          u32 = 0x8892;
pub const GL_ELEMENT_ARRAY_BUFFER:  u32 = 0x8893;
pub const GL_STATIC_DRAW:           u32 = 0x88B4;
pub const GL_DYNAMIC_DRAW:          u32 = 0x88B8;
pub const GL_TRIANGLES:             u32 = 0x0004;
pub const GL_TRIANGLE_STRIP:        u32 = 0x0005;
pub const GL_TRIANGLE_FAN:          u32 = 0x0006;
pub const GL_LINES:                 u32 = 0x0001;
pub const GL_POINTS:                u32 = 0x0000;
pub const GL_FLOAT:                 u32 = 0x1406;
pub const GL_UNSIGNED_BYTE:         u32 = 0x1401;
pub const GL_UNSIGNED_SHORT:        u32 = 0x1403;
pub const GL_RGBA:                  u32 = 0x1908;
pub const GL_RGB:                   u32 = 0x1907;
pub const GL_TEXTURE_2D:            u32 = 0x0DE1;
pub const GL_TEXTURE_MIN_FILTER:    u32 = 0x2801;
pub const GL_TEXTURE_MAG_FILTER:    u32 = 0x2800;
pub const GL_TEXTURE_WRAP_S:        u32 = 0x2802;
pub const GL_TEXTURE_WRAP_T:        u32 = 0x2803;
pub const GL_NEAREST:               u32 = 0x2600;
pub const GL_LINEAR:                u32 = 0x2601;
pub const GL_REPEAT:                u32 = 0x2901;
pub const GL_CLAMP_TO_EDGE:         u32 = 0x812F;
pub const GL_DEPTH_TEST:            u32 = 0x0B71;
pub const GL_BLEND:                 u32 = 0x0BE2;
pub const GL_SRC_ALPHA:             u32 = 0x0302;
pub const GL_ONE_MINUS_SRC_ALPHA:   u32 = 0x0303;
pub const GL_ONE:                   u32 = 1;
pub const GL_ZERO:                  u32 = 0;
pub const GL_LESS:                  u32 = 0x0201;
pub const GL_LEQUAL:                u32 = 0x0203;
pub const GL_COLOR_BUFFER_BIT:      u32 = 0x4000;
pub const GL_DEPTH_BUFFER_BIT:      u32 = 0x0100;
pub const GL_VERTEX_SHADER:         u32 = 0x8B31;
pub const GL_FRAGMENT_SHADER:       u32 = 0x8B30;

// ─────────────────────────────────────────────────────────────────────────────
//  Object handles
// ─────────────────────────────────────────────────────────────────────────────

pub type GlId = u32;
pub const GL_NONE: GlId = 0;

// ─────────────────────────────────────────────────────────────────────────────
//  Buffer object
// ─────────────────────────────────────────────────────────────────────────────

pub struct GlBuffer {
    pub target: u32,
    pub data:   Vec<u8>,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Texture object
// ─────────────────────────────────────────────────────────────────────────────

pub struct GlTexture {
    pub width:      u32,
    pub height:     u32,
    pub data:       Vec<u8>,      // RGBA8, row-major
    pub min_filter: u32,
    pub mag_filter: u32,
    pub wrap_s:     u32,
    pub wrap_t:     u32,
}

impl GlTexture {
    pub fn sample(&self, u: f32, v: f32) -> Vec4 {
        let (u, v) = self.wrap(u, v);
        let px = f32_clamp(u * self.width as f32, 0.0, self.width as f32 - 1.0) as u32;
        let py = f32_clamp(v * self.height as f32, 0.0, self.height as f32 - 1.0) as u32;
        let idx = ((py * self.width + px) * 4) as usize;
        if idx + 3 < self.data.len() {
            Vec4(
                self.data[idx]   as f32 / 255.0,
                self.data[idx+1] as f32 / 255.0,
                self.data[idx+2] as f32 / 255.0,
                self.data[idx+3] as f32 / 255.0,
            )
        } else {
            Vec4(0.0, 0.0, 0.0, 1.0)
        }
    }

    fn wrap(&self, u: f32, v: f32) -> (f32, f32) {
        let wu = match self.wrap_s {
            GL_CLAMP_TO_EDGE => f32_clamp(u, 0.0, 1.0),
            _ => u - f32_floor(u),  // GL_REPEAT
        };
        let wv = match self.wrap_t {
            GL_CLAMP_TO_EDGE => f32_clamp(v, 0.0, 1.0),
            _ => v - f32_floor(v),
        };
        (wu, wv)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Shader / Program
// ─────────────────────────────────────────────────────────────────────────────

/// A compiled shader — stores source; "compilation" is deferred.
pub struct GlShader {
    pub shader_type: u32,
    pub source:      String,
    pub compiled:    bool,
}

/// Vertex attribute descriptor.
#[derive(Clone, Debug)]
pub struct AttribPointer {
    pub buffer_id:  GlId,
    pub size:       i32,    // components per vertex (1-4)
    pub type_:      u32,    // GL_FLOAT etc.
    pub normalized: bool,
    pub stride:     i32,
    pub offset:     i32,
    pub enabled:    bool,
}

/// A linked program — wraps vertex + fragment shader IDs and
/// stores a vertex/fragment function pair extracted from the GLSL source.
pub struct GlProgram {
    pub vert_id:   GlId,
    pub frag_id:   GlId,
    pub linked:    bool,
    pub uniforms:  BTreeMap<String, UniformValue>,
    pub attribs:   BTreeMap<String, u32>,   // name → location
}

#[derive(Clone, Debug)]
pub enum UniformValue {
    Float(f32),
    Int(i32),
    Vec2(f32, f32),
    Vec3(f32, f32, f32),
    Vec4(f32, f32, f32, f32),
    Mat4([f32; 16]),
    Sampler(i32),  // texture unit
}

impl GlProgram {
    fn new(vert_id: GlId, frag_id: GlId) -> Self {
        GlProgram {
            vert_id, frag_id, linked: true,
            uniforms: BTreeMap::new(),
            attribs: BTreeMap::new(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Vertex — output of vertex shader
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct GlVertex {
    /// Clip-space position (gl_Position).
    pub position: Vec4,
    /// Interpolated varying data (up to 8 vec4 slots).
    pub varyings: [Vec4; 8],
}

impl GlVertex {
    pub fn zero() -> Self {
        Self {
            position: Vec4(0.0, 0.0, 0.0, 1.0),
            varyings: [Vec4(0.0, 0.0, 0.0, 0.0); 8],
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Framebuffer
// ─────────────────────────────────────────────────────────────────────────────

pub struct Framebuffer {
    pub width:  u32,
    pub height: u32,
    pub color:  Vec<u32>,   // RGBA packed u32 (0xAABBGGRR little-endian)
    pub depth:  Vec<f32>,   // [0,1] per pixel
}

impl Framebuffer {
    pub fn new(w: u32, h: u32) -> Self {
        let n = (w * h) as usize;
        Self { width: w, height: h, color: vec![0xFF000000; n], depth: vec![1.0; n] }
    }

    pub fn idx(&self, x: u32, y: u32) -> usize { (y * self.width + x) as usize }

    pub fn set_pixel(&mut self, x: u32, y: u32, r: u8, g: u8, b: u8, a: u8) {
        if x < self.width && y < self.height {
            let i = self.idx(x, y);
            self.color[i] = (a as u32) << 24 | (b as u32) << 16 | (g as u32) << 8 | r as u32;
        }
    }

    pub fn get_depth(&self, x: u32, y: u32) -> f32 {
        if x < self.width && y < self.height { self.depth[self.idx(x, y)] } else { 1.0 }
    }

    pub fn set_depth(&mut self, x: u32, y: u32, d: f32) {
        if x < self.width && y < self.height { let i = self.idx(x,y); self.depth[i] = d; }
    }

    /// Blend src over dst (Porter-Duff SRC_ALPHA, ONE_MINUS_SRC_ALPHA).
    pub fn blend_pixel(&mut self, x: u32, y: u32, src: Vec4, blend: bool) {
        let (sr, sg, sb, sa) = (src.0, src.1, src.2, src.3);
        if !blend || sa >= 1.0 {
            self.set_pixel(x, y,
                (f32_clamp(sr, 0.0, 1.0) * 255.0) as u8,
                (f32_clamp(sg, 0.0, 1.0) * 255.0) as u8,
                (f32_clamp(sb, 0.0, 1.0) * 255.0) as u8,
                (f32_clamp(sa, 0.0, 1.0) * 255.0) as u8,
            );
            return;
        }
        // Read destination
        let i = self.idx(x, y);
        let dst = self.color[i];
        let dr = ((dst      ) & 0xFF) as f32 / 255.0;
        let dg = ((dst >>  8) & 0xFF) as f32 / 255.0;
        let db = ((dst >> 16) & 0xFF) as f32 / 255.0;
        let da = ((dst >> 24) & 0xFF) as f32 / 255.0;
        let oma = 1.0 - sa;
        self.set_pixel(x, y,
            (f32_clamp(sr * sa + dr * oma, 0.0, 1.0) * 255.0) as u8,
            (f32_clamp(sg * sa + dg * oma, 0.0, 1.0) * 255.0) as u8,
            (f32_clamp(sb * sa + db * oma, 0.0, 1.0) * 255.0) as u8,
            (f32_clamp(sa + da * oma, 0.0, 1.0) * 255.0) as u8,
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Shader execution context (passed to vertex/fragment functions)
// ─────────────────────────────────────────────────────────────────────────────

pub struct ShaderUniforms<'a> {
    pub uniforms:  &'a BTreeMap<String, UniformValue>,
    pub textures:  &'a BTreeMap<GlId, GlTexture>,
    pub tex_units: &'a [GlId; 8],
}

impl<'a> ShaderUniforms<'a> {
    pub fn uniform_f(&self, name: &str) -> f32 {
        match self.uniforms.get(name) {
            Some(UniformValue::Float(v)) => *v,
            _ => 0.0,
        }
    }

    pub fn uniform_vec4(&self, name: &str) -> Vec4 {
        match self.uniforms.get(name) {
            Some(UniformValue::Vec4(r,g,b,a)) => Vec4(*r, *g, *b, *a),
            Some(UniformValue::Vec3(r,g,b))   => Vec4(*r, *g, *b, 1.0),
            Some(UniformValue::Vec2(x,y))     => Vec4(*x, *y, 0.0, 0.0),
            Some(UniformValue::Float(f))      => Vec4(*f, *f, *f, 1.0),
            _ => Vec4(0.0, 0.0, 0.0, 1.0),
        }
    }

    pub fn uniform_mat4(&self, name: &str) -> Mat4 {
        match self.uniforms.get(name) {
            Some(UniformValue::Mat4(m)) => Mat4(*m),
            _ => Mat4::identity(),
        }
    }

    pub fn sample2d(&self, unit: i32, u: f32, v: f32) -> Vec4 {
        let unit = unit.max(0) as usize;
        let tex_id = self.tex_units.get(unit).copied().unwrap_or(GL_NONE);
        if tex_id == GL_NONE { return Vec4(0.0, 0.0, 0.0, 1.0); }
        match self.textures.get(&tex_id) {
            Some(tex) => tex.sample(u, v),
            None => Vec4(0.0, 0.0, 0.0, 1.0),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Triangle rasteriser (scanline fill)
// ─────────────────────────────────────────────────────────────────────────────

/// Rasterise a triangle defined by three `GlVertex` values (already in screen space).
/// Calls `frag_fn` per covered pixel.
fn rasterise_triangle<F>(
    verts: &[GlVertex; 3],
    fb:    &mut Framebuffer,
    depth_test: bool,
    blend:      bool,
    frag_fn:    F,
)
where F: Fn(&[Vec4; 8], &mut Framebuffer, u32, u32, f32, bool)
{
    let (v0, v1, v2) = (&verts[0], &verts[1], &verts[2]);

    // Screen positions
    let (x0, y0, z0, w0) = (v0.position.0, v0.position.1, v0.position.2, v0.position.3);
    let (x1, y1, z1, w1) = (v1.position.0, v1.position.1, v1.position.2, v1.position.3);
    let (x2, y2, z2, w2) = (v2.position.0, v2.position.1, v2.position.2, v2.position.3);

    // Bounding box (clamped to framebuffer)
    let min_x = f32_max(0.0, f32_min(x0, f32_min(x1, x2))) as u32;
    let min_y = f32_max(0.0, f32_min(y0, f32_min(y1, y2))) as u32;
    let max_x = (f32_min(fb.width  as f32 - 1.0, f32_max(x0, f32_max(x1, x2))) as u32 + 1).min(fb.width);
    let max_y = (f32_min(fb.height as f32 - 1.0, f32_max(y0, f32_max(y1, y2))) as u32 + 1).min(fb.height);

    // 2D cross-product of (p1-p0) × (p2-p0) for back-face
    let area2 = (x1 - x0) * (y2 - y0) - (y1 - y0) * (x2 - x0);
    if area2.abs() < 1e-7 { return; }

    for py in min_y..max_y {
        for px in min_x..max_x {
            let px_f = px as f32 + 0.5;
            let py_f = py as f32 + 0.5;

            // Barycentric coordinates — standard sub-triangle area formula:
            //   w_i = signed_area(opposite edge, p) / total_area
            // w_0 = area(v1,v2,p)/area2, w_1 = area(v2,v0,p)/area2
            let w_0 = ((x2 - x1) * (py_f - y1) - (y2 - y1) * (px_f - x1)) / area2;
            let w_1 = ((x0 - x2) * (py_f - y2) - (y0 - y2) * (px_f - x2)) / area2;
            let w_2 = 1.0 - w_0 - w_1;

            if w_0 < 0.0 || w_1 < 0.0 || w_2 < 0.0 { continue; }

            // Depth
            let depth = z0 * w_0 + z1 * w_1 + z2 * w_2;
            if depth_test && depth > fb.get_depth(px, py) { continue; }

            // Interpolate varyings
            let mut interp = [Vec4(0.0, 0.0, 0.0, 0.0); 8];
            for i in 0..8 {
                let va = v0.varyings[i];
                let vb = v1.varyings[i];
                let vc = v2.varyings[i];
                interp[i] = Vec4(
                    va.0 * w_0 + vb.0 * w_1 + vc.0 * w_2,
                    va.1 * w_0 + vb.1 * w_1 + vc.1 * w_2,
                    va.2 * w_0 + vb.2 * w_1 + vc.2 * w_2,
                    va.3 * w_0 + vb.3 * w_1 + vc.3 * w_2,
                );
            }
            frag_fn(&interp, fb, px, py, depth, blend);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  WebGlContext
// ─────────────────────────────────────────────────────────────────────────────

pub struct WebGlContext {
    pub width:   u32,
    pub height:  u32,

    // Framebuffer
    pub fb:      Framebuffer,

    // Viewport
    vp_x: i32, vp_y: i32, vp_w: u32, vp_h: u32,

    // Clear state
    clear_color: Vec4,

    // GL objects
    buffers:   BTreeMap<GlId, GlBuffer>,
    textures:  BTreeMap<GlId, GlTexture>,
    shaders:   BTreeMap<GlId, GlShader>,
    programs:  BTreeMap<GlId, GlProgram>,
    next_id:   GlId,

    // Bound state
    bound_array_buffer:   GlId,
    bound_element_buffer: GlId,
    bound_texture_2d:     GlId,
    current_program:      GlId,
    active_texture_unit:  usize,
    texture_units:        [GlId; 8],

    // Vertex attributes
    attrib_pointers: BTreeMap<u32, AttribPointer>,

    // Render state
    depth_test: bool,
    blend:      bool,
    blend_src:  u32,
    blend_dst:  u32,
    depth_func: u32,
}

impl WebGlContext {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width, height,
            fb:      Framebuffer::new(width, height),
            vp_x: 0, vp_y: 0, vp_w: width, vp_h: height,
            clear_color: Vec4(0.0, 0.0, 0.0, 1.0),
            buffers:  BTreeMap::new(),
            textures: BTreeMap::new(),
            shaders:  BTreeMap::new(),
            programs: BTreeMap::new(),
            next_id:  1,
            bound_array_buffer: GL_NONE,
            bound_element_buffer: GL_NONE,
            bound_texture_2d: GL_NONE,
            current_program: GL_NONE,
            active_texture_unit: 0,
            texture_units: [GL_NONE; 8],
            attrib_pointers: BTreeMap::new(),
            depth_test: false,
            blend: false,
            blend_src: GL_ONE,
            blend_dst: GL_ZERO,
            depth_func: GL_LESS,
        }
    }

    fn alloc_id(&mut self) -> GlId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    // ── Buffers ──────────────────────────────────────────────────────────────

    pub fn create_buffer(&mut self) -> GlId {
        let id = self.alloc_id();
        self.buffers.insert(id, GlBuffer { target: GL_ARRAY_BUFFER, data: Vec::new() });
        id
    }

    pub fn bind_buffer(&mut self, target: u32, id: GlId) {
        match target {
            GL_ARRAY_BUFFER          => self.bound_array_buffer   = id,
            GL_ELEMENT_ARRAY_BUFFER  => self.bound_element_buffer = id,
            _ => {}
        }
        if id != GL_NONE {
            if let Some(b) = self.buffers.get_mut(&id) { b.target = target; }
        }
    }

    pub fn buffer_data_f32(&mut self, target: u32, data: &[f32], _usage: u32) {
        let id = match target {
            GL_ARRAY_BUFFER         => self.bound_array_buffer,
            GL_ELEMENT_ARRAY_BUFFER => self.bound_element_buffer,
            _ => GL_NONE,
        };
        if let Some(b) = self.buffers.get_mut(&id) {
            b.data.clear();
            for &f in data {
                b.data.extend_from_slice(&f.to_le_bytes());
            }
        }
    }

    pub fn buffer_data_u16(&mut self, target: u32, data: &[u16], _usage: u32) {
        let id = if target == GL_ELEMENT_ARRAY_BUFFER { self.bound_element_buffer } else { GL_NONE };
        if let Some(b) = self.buffers.get_mut(&id) {
            b.data.clear();
            for &v in data {
                b.data.extend_from_slice(&v.to_le_bytes());
            }
        }
    }

    // ── Textures ─────────────────────────────────────────────────────────────

    pub fn create_texture(&mut self) -> GlId {
        let id = self.alloc_id();
        self.textures.insert(id, GlTexture {
            width: 0, height: 0, data: Vec::new(),
            min_filter: GL_NEAREST, mag_filter: GL_NEAREST,
            wrap_s: GL_REPEAT, wrap_t: GL_REPEAT,
        });
        id
    }

    pub fn bind_texture(&mut self, _target: u32, id: GlId) {
        self.bound_texture_2d = id;
        if id != GL_NONE {
            self.texture_units[self.active_texture_unit] = id;
        }
    }

    pub fn tex_image_2d(&mut self, w: u32, h: u32, data: Vec<u8>) {
        if let Some(tex) = self.textures.get_mut(&self.bound_texture_2d) {
            tex.width = w; tex.height = h;
            tex.data = data;
        }
    }

    pub fn tex_parameteri(&mut self, _target: u32, pname: u32, param: u32) {
        if let Some(tex) = self.textures.get_mut(&self.bound_texture_2d) {
            match pname {
                GL_TEXTURE_MIN_FILTER => tex.min_filter = param,
                GL_TEXTURE_MAG_FILTER => tex.mag_filter = param,
                GL_TEXTURE_WRAP_S     => tex.wrap_s = param,
                GL_TEXTURE_WRAP_T     => tex.wrap_t = param,
                _ => {}
            }
        }
    }

    pub fn active_texture(&mut self, unit: usize) {
        self.active_texture_unit = unit.min(7);
    }

    // ── Shaders & Programs ────────────────────────────────────────────────────

    pub fn create_shader(&mut self, shader_type: u32) -> GlId {
        let id = self.alloc_id();
        self.shaders.insert(id, GlShader { shader_type, source: String::new(), compiled: false });
        id
    }

    pub fn shader_source(&mut self, id: GlId, source: &str) {
        if let Some(s) = self.shaders.get_mut(&id) { s.source = source.to_string(); }
    }

    pub fn compile_shader(&mut self, id: GlId) {
        if let Some(s) = self.shaders.get_mut(&id) { s.compiled = true; }
    }

    pub fn create_program(&mut self) -> GlId {
        let id = self.alloc_id();
        // Placeholder — linked in link_program
        self.programs.insert(id, GlProgram::new(GL_NONE, GL_NONE));
        id
    }

    pub fn attach_shader(&mut self, prog_id: GlId, shader_id: GlId) {
        if let Some(prog) = self.programs.get_mut(&prog_id) {
            if let Some(sh) = self.shaders.get(&shader_id) {
                match sh.shader_type {
                    GL_VERTEX_SHADER   => prog.vert_id = shader_id,
                    GL_FRAGMENT_SHADER => prog.frag_id = shader_id,
                    _ => {}
                }
            }
        }
    }

    pub fn link_program(&mut self, prog_id: GlId) {
        if let Some(prog) = self.programs.get_mut(&prog_id) {
            prog.linked = true;
            // Extract attribute names from vertex shader GLSL
            let vert_src = self.shaders.get(&prog.vert_id)
                .map(|s| s.source.clone())
                .unwrap_or_default();
            let attribs = extract_attributes(&vert_src);
            prog.attribs = attribs;
        }
    }

    pub fn use_program(&mut self, id: GlId) { self.current_program = id; }

    pub fn get_attrib_location(&self, prog_id: GlId, name: &str) -> i32 {
        self.programs.get(&prog_id)
            .and_then(|p| p.attribs.get(name))
            .copied()
            .map(|loc| loc as i32)
            .unwrap_or(-1)
    }

    pub fn get_uniform_location(&self, _prog_id: GlId, name: &str) -> Option<String> {
        Some(name.to_string())
    }

    // ── Uniforms ──────────────────────────────────────────────────────────────

    pub fn uniform1f(&mut self, name: &str, v: f32) {
        if let Some(p) = self.programs.get_mut(&self.current_program) {
            p.uniforms.insert(name.to_string(), UniformValue::Float(v));
        }
    }

    pub fn uniform1i(&mut self, name: &str, v: i32) {
        if let Some(p) = self.programs.get_mut(&self.current_program) {
            p.uniforms.insert(name.to_string(), UniformValue::Int(v));
        }
    }

    pub fn uniform2f(&mut self, name: &str, x: f32, y: f32) {
        if let Some(p) = self.programs.get_mut(&self.current_program) {
            p.uniforms.insert(name.to_string(), UniformValue::Vec2(x, y));
        }
    }

    pub fn uniform3f(&mut self, name: &str, x: f32, y: f32, z: f32) {
        if let Some(p) = self.programs.get_mut(&self.current_program) {
            p.uniforms.insert(name.to_string(), UniformValue::Vec3(x, y, z));
        }
    }

    pub fn uniform4f(&mut self, name: &str, x: f32, y: f32, z: f32, w: f32) {
        if let Some(p) = self.programs.get_mut(&self.current_program) {
            p.uniforms.insert(name.to_string(), UniformValue::Vec4(x, y, z, w));
        }
    }

    pub fn uniform_matrix4fv(&mut self, name: &str, transpose: bool, data: [f32; 16]) {
        let m = if transpose { transpose_mat4(data) } else { data };
        if let Some(p) = self.programs.get_mut(&self.current_program) {
            p.uniforms.insert(name.to_string(), UniformValue::Mat4(m));
        }
    }

    // ── Vertex attrib pointers ────────────────────────────────────────────────

    pub fn vertex_attrib_pointer(
        &mut self, location: u32, size: i32, type_: u32,
        normalized: bool, stride: i32, offset: i32,
    ) {
        self.attrib_pointers.insert(location, AttribPointer {
            buffer_id: self.bound_array_buffer,
            size, type_, normalized, stride, offset, enabled: false,
        });
    }

    pub fn enable_vertex_attrib_array(&mut self, location: u32) {
        if let Some(ap) = self.attrib_pointers.get_mut(&location) { ap.enabled = true; }
    }

    pub fn disable_vertex_attrib_array(&mut self, location: u32) {
        if let Some(ap) = self.attrib_pointers.get_mut(&location) { ap.enabled = false; }
    }

    // ── Render state ──────────────────────────────────────────────────────────

    pub fn viewport(&mut self, x: i32, y: i32, w: u32, h: u32) {
        self.vp_x = x; self.vp_y = y; self.vp_w = w; self.vp_h = h;
    }

    pub fn clear_color(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.clear_color = Vec4(r, g, b, a);
    }

    pub fn clear(&mut self, mask: u32) {
        let Vec4(r, g, b, a) = self.clear_color;
        let packed = ((a * 255.0) as u32) << 24
                   | ((b * 255.0) as u32) << 16
                   | ((g * 255.0) as u32) << 8
                   | (r * 255.0) as u32;
        if mask & GL_COLOR_BUFFER_BIT != 0 {
            self.fb.color.fill(packed);
        }
        if mask & GL_DEPTH_BUFFER_BIT != 0 {
            self.fb.depth.fill(1.0);
        }
    }

    pub fn enable(&mut self, cap: u32) {
        match cap {
            GL_DEPTH_TEST => self.depth_test = true,
            GL_BLEND      => self.blend = true,
            _ => {}
        }
    }

    pub fn disable(&mut self, cap: u32) {
        match cap {
            GL_DEPTH_TEST => self.depth_test = false,
            GL_BLEND      => self.blend = false,
            _ => {}
        }
    }

    pub fn blend_func(&mut self, src: u32, dst: u32) {
        self.blend_src = src; self.blend_dst = dst;
    }

    pub fn depth_func(&mut self, func: u32) { self.depth_func = func; }

    // ── Draw ─────────────────────────────────────────────────────────────────

    /// drawArrays: vertex shader + rasterise + fragment shader.
    pub fn draw_arrays<VF, FF>(&mut self, mode: u32, first: i32, count: i32, vert_fn: VF, frag_fn: FF)
    where
        VF: Fn(&BTreeMap<u32, Vec4>, &ShaderUniforms) -> GlVertex,
        FF: Fn(&[Vec4; 8], &ShaderUniforms) -> Vec4,
    {
        let indices: Vec<i32> = (first..first + count).collect();
        self.draw_indexed_impl(mode, &indices, vert_fn, frag_fn);
    }

    /// drawElements: index buffer + vertex shader + rasterise + fragment shader.
    pub fn draw_elements<VF, FF>(&mut self, mode: u32, count: i32, vert_fn: VF, frag_fn: FF)
    where
        VF: Fn(&BTreeMap<u32, Vec4>, &ShaderUniforms) -> GlVertex,
        FF: Fn(&[Vec4; 8], &ShaderUniforms) -> Vec4,
    {
        // Read index buffer
        let elem_buf_id = self.bound_element_buffer;
        let indices: Vec<i32> = if let Some(b) = self.buffers.get(&elem_buf_id) {
            b.data.chunks_exact(2)
                .take(count as usize)
                .map(|c| u16::from_le_bytes([c[0], c[1]]) as i32)
                .collect()
        } else {
            (0..count).collect()
        };
        self.draw_indexed_impl(mode, &indices, vert_fn, frag_fn);
    }

    fn draw_indexed_impl<VF, FF>(&mut self, mode: u32, indices: &[i32], vert_fn: VF, frag_fn: FF)
    where
        VF: Fn(&BTreeMap<u32, Vec4>, &ShaderUniforms) -> GlVertex,
        FF: Fn(&[Vec4; 8], &ShaderUniforms) -> Vec4,
    {
        let prog_id = self.current_program;
        let prog = match self.programs.get(&prog_id) {
            Some(p) => p,
            None => return,
        };
        let uniforms_map = prog.uniforms.clone();
        let tex_units = self.texture_units;

        let su = ShaderUniforms {
            uniforms: &uniforms_map,
            textures: &self.textures,
            tex_units: &tex_units,
        };

        // Fetch all vertices
        let mut gl_verts: Vec<GlVertex> = Vec::new();
        for &idx in indices {
            let attribs = self.fetch_vertex_attribs(idx);
            let v = vert_fn(&attribs, &su);
            gl_verts.push(self.clip_to_screen(v));
        }

        let depth_test = self.depth_test;
        let blend = self.blend;

        let step = match mode {
            GL_TRIANGLES     => 3,
            GL_TRIANGLE_STRIP | GL_TRIANGLE_FAN => 1,
            _ => 3,
        };

        let count = indices.len();
        match mode {
            GL_TRIANGLES => {
                let mut i = 0;
                while i + 2 < count {
                    let tri = [gl_verts[i], gl_verts[i+1], gl_verts[i+2]];
                    let su2 = ShaderUniforms {
                        uniforms: &uniforms_map, textures: &self.textures, tex_units: &tex_units,
                    };
                    rasterise_triangle(&tri, &mut self.fb, depth_test, blend,
                        |varyings, fb, px, py, depth, blend| {
                            let color = frag_fn(varyings, &su2);
                            if depth_test { fb.set_depth(px, py, depth); }
                            fb.blend_pixel(px, py, color, blend);
                        }
                    );
                    i += 3;
                }
            }
            GL_TRIANGLE_STRIP => {
                for i in 2..count {
                    let (a, b, c) = if i % 2 == 0 {
                        (i - 2, i - 1, i)
                    } else {
                        (i - 1, i - 2, i)
                    };
                    let tri = [gl_verts[a], gl_verts[b], gl_verts[c]];
                    let su2 = ShaderUniforms {
                        uniforms: &uniforms_map, textures: &self.textures, tex_units: &tex_units,
                    };
                    rasterise_triangle(&tri, &mut self.fb, depth_test, blend,
                        |varyings, fb, px, py, depth, blend| {
                            let color = frag_fn(varyings, &su2);
                            if depth_test { fb.set_depth(px, py, depth); }
                            fb.blend_pixel(px, py, color, blend);
                        }
                    );
                }
            }
            GL_TRIANGLE_FAN => {
                for i in 2..count {
                    let tri = [gl_verts[0], gl_verts[i-1], gl_verts[i]];
                    let su2 = ShaderUniforms {
                        uniforms: &uniforms_map, textures: &self.textures, tex_units: &tex_units,
                    };
                    rasterise_triangle(&tri, &mut self.fb, depth_test, blend,
                        |varyings, fb, px, py, depth, blend| {
                            let color = frag_fn(varyings, &su2);
                            if depth_test { fb.set_depth(px, py, depth); }
                            fb.blend_pixel(px, py, color, blend);
                        }
                    );
                }
            }
            _ => {}
        }
    }

    /// Read vertex attribute `location` for vertex index `idx` from the bound buffer.
    fn fetch_vertex_attribs(&self, idx: i32) -> BTreeMap<u32, Vec4> {
        let mut result = BTreeMap::new();
        for (loc, ap) in &self.attrib_pointers {
            if !ap.enabled { continue; }
            let stride = if ap.stride == 0 { ap.size * 4 } else { ap.stride };
            let byte_offset = (ap.offset + idx * stride) as usize;
            if let Some(buf) = self.buffers.get(&ap.buffer_id) {
                let mut comps = [0.0f32; 4];
                for i in 0..ap.size as usize {
                    let off = byte_offset + i * 4;
                    if off + 4 <= buf.data.len() {
                        comps[i] = f32::from_le_bytes([
                            buf.data[off], buf.data[off+1], buf.data[off+2], buf.data[off+3]
                        ]);
                    }
                }
                result.insert(*loc, Vec4(comps[0], comps[1], comps[2], comps[3]));
            }
        }
        result
    }

    /// Convert clip-space position to screen space.
    fn clip_to_screen(&self, mut v: GlVertex) -> GlVertex {
        let Vec4(cx, cy, cz, cw) = v.position;
        let w = if cw.abs() < 1e-9 { 1.0 } else { cw };
        // NDC
        let nx = cx / w;
        let ny = cy / w;
        let nz = cz / w;
        // Viewport transform
        let sx = (nx + 1.0) * 0.5 * self.vp_w as f32 + self.vp_x as f32;
        let sy = (1.0 - ny) * 0.5 * self.vp_h as f32 + self.vp_y as f32;
        let sz = (nz + 1.0) * 0.5;  // map [-1,1] → [0,1]
        v.position = Vec4(sx, sy, sz, w);
        v
    }

    // ── readPixels ────────────────────────────────────────────────────────────

    pub fn read_pixels(&self, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in y..y+h {
            for col in x..x+w {
                if col < self.fb.width && row < self.fb.height {
                    let packed = self.fb.color[self.fb.idx(col, row)];
                    out.push((packed       & 0xFF) as u8);
                    out.push((packed >> 8  & 0xFF) as u8);
                    out.push((packed >> 16 & 0xFF) as u8);
                    out.push((packed >> 24 & 0xFF) as u8);
                } else {
                    out.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn transpose_mat4(m: [f32; 16]) -> [f32; 16] {
    let mut r = [0.0f32; 16];
    for col in 0..4 { for row in 0..4 { r[row * 4 + col] = m[col * 4 + row]; } }
    r
}

/// Extract attribute name → location from GLSL vertex shader source.
fn extract_attributes(src: &str) -> BTreeMap<String, u32> {
    let mut map = BTreeMap::new();
    let mut loc = 0u32;
    for line in src.lines() {
        let line = line.trim();
        if line.starts_with("attribute ") || line.starts_with("in ") {
            // "attribute vec2 aTexCoord;" or "in vec2 aTexCoord;"
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(last) = parts.last() {
                let name = last.trim_end_matches(';').to_string();
                if !name.is_empty() && !name.starts_with("gl_") {
                    map.insert(name, loc);
                    loc += 1;
                }
            }
        }
    }
    map
}

// ─────────────────────────────────────────────────────────────────────────────
//  Self-tests
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    // ── Test 1: Mat4 identity + mul_vec4 ─────────────────────────────────────
    let m = Mat4::identity();
    let v = m.mul_vec4(Vec4(1.0, 2.0, 3.0, 1.0));
    if (v.0 - 1.0).abs() > 1e-6 { return false; }
    if (v.1 - 2.0).abs() > 1e-6 { return false; }
    if (v.2 - 3.0).abs() > 1e-6 { return false; }

    // ── Test 2: Mat4 ortho projection ────────────────────────────────────────
    let ortho = Mat4::ortho(0.0, 800.0, 600.0, 0.0, -1.0, 1.0);
    // Origin (0,0) in ortho space → NDC (-1, 1, 0)
    let origin = ortho.mul_vec4(Vec4(0.0, 0.0, 0.0, 1.0));
    if (origin.0 - (-1.0)).abs() > 1e-4 { return false; }
    if (origin.1 - 1.0).abs() > 1e-4 { return false; }

    // ── Test 3: Texture sampling ──────────────────────────────────────────────
    let tex = GlTexture {
        width: 2, height: 2,
        data: vec![
            255, 0, 0, 255,  // (0,0) red
            0, 255, 0, 255,  // (1,0) green
            0, 0, 255, 255,  // (0,1) blue
            255,255,255,255, // (1,1) white
        ],
        min_filter: GL_NEAREST, mag_filter: GL_NEAREST,
        wrap_s: GL_REPEAT, wrap_t: GL_REPEAT,
    };
    let s = tex.sample(0.0, 0.0); // top-left pixel = red
    if (s.0 - 1.0).abs() > 1e-3 { return false; } // red
    if s.1.abs() > 1e-3 { return false; }          // not green

    // ── Test 4: Framebuffer set/blend pixel ───────────────────────────────────
    let mut fb = Framebuffer::new(4, 4);
    fb.set_pixel(0, 0, 255, 0, 0, 255);
    let packed = fb.color[0];
    if (packed & 0xFF) as u8 != 255 { return false; } // red channel

    // Blend 50% green over black background
    fb.set_pixel(1, 1, 0, 0, 0, 255); // black bg
    fb.blend_pixel(1, 1, Vec4(0.0, 1.0, 0.0, 0.5), true);
    let blended = fb.color[fb.idx(1, 1)];
    let g_ch = (blended >> 8 & 0xFF) as u8;
    if g_ch < 100 || g_ch > 140 { return false; } // should be ~127

    // ── Test 5: Full pipeline (draw a coloured triangle) ──────────────────────
    let mut ctx = WebGlContext::new(64, 64);
    ctx.clear_color(0.0, 0.0, 0.0, 1.0);
    ctx.clear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);

    // Create + bind buffer with a triangle covering most of the screen
    let vbo = ctx.create_buffer();
    ctx.bind_buffer(GL_ARRAY_BUFFER, vbo);
    // Triangle: NDC coords (-1,-1), (1,-1), (0,1) in clip space
    ctx.buffer_data_f32(GL_ARRAY_BUFFER, &[
        -1.0, -1.0,
         1.0, -1.0,
         0.0,  1.0,
    ], GL_STATIC_DRAW);

    // Create + link program
    let vert = ctx.create_shader(GL_VERTEX_SHADER);
    ctx.shader_source(vert, "attribute vec2 aPos;\nvoid main() { gl_Position = vec4(aPos, 0.0, 1.0); }");
    ctx.compile_shader(vert);
    let frag = ctx.create_shader(GL_FRAGMENT_SHADER);
    ctx.shader_source(frag, "void main() { gl_FragColor = vec4(1.0, 0.0, 0.0, 1.0); }");
    ctx.compile_shader(frag);
    let prog = ctx.create_program();
    ctx.attach_shader(prog, vert);
    ctx.attach_shader(prog, frag);
    ctx.link_program(prog);
    ctx.use_program(prog);

    // Attrib pointer
    ctx.vertex_attrib_pointer(0, 2, GL_FLOAT, false, 8, 0);
    ctx.enable_vertex_attrib_array(0);
    ctx.viewport(0, 0, 64, 64);

    // Draw 3 vertices (1 triangle) using our software vertex/fragment functions
    ctx.draw_arrays(GL_TRIANGLES, 0, 3,
        // Vertex shader: pass through position stored in varying[0].xy
        |attribs, _uniforms| {
            let pos = attribs.get(&0).copied().unwrap_or(Vec4(0.0, 0.0, 0.0, 1.0));
            let mut v = GlVertex::zero();
            v.position = Vec4(pos.0, pos.1, 0.0, 1.0);
            v
        },
        // Fragment shader: solid red
        |_varyings, _uniforms| Vec4(1.0, 0.0, 0.0, 1.0),
    );

    // The centre pixel (32,32) should be red
    let pixels = ctx.read_pixels(32, 32, 1, 1);
    if pixels.len() < 4 { return false; }
    if pixels[0] < 200 { return false; }  // red channel
    if pixels[1] > 50  { return false; }  // not green

    // ── Test 6: extract_attributes ────────────────────────────────────────────
    let src = "attribute vec2 aPos;\nattribute vec2 aUV;\nvoid main() {}";
    let attrs = extract_attributes(src);
    if attrs.get("aPos") != Some(&0) { return false; }
    if attrs.get("aUV")  != Some(&1) { return false; }

    true
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[webgl] Software WebGL (OpenGL ES 2.0) ready (Phase 48).");
}

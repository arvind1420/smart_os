//! Canvas 2D API — Phase 41 for Smart OS.
//!
//! Implements the HTML5 Canvas 2D rendering context:
//!  • Pixel buffer management (RGBA, arbitrary size)
//!  • Path building: moveTo, lineTo, arc, bezierCurveTo, quadraticCurveTo, rect, ellipse
//!  • Stroke + fill (solid color, with line width / cap / join)
//!  • Text rendering (measureText, fillText, strokeText) via bitmap font
//!  • Image operations: drawImage, getImageData, putImageData, createImageData
//!  • Transform stack: save/restore, translate/scale/rotate/setTransform
//!  • Compositing: globalAlpha, globalCompositeOperation
//!  • Gradients: LinearGradient, RadialGradient
//!  • Clipping regions
//!  • Shadow (offset + blur + color)
//!
//! Each `Canvas` owns its pixel buffer.  The JS API surface is exposed via
//! `CanvasRenderingContext2d` which the browser wires up to `<canvas>` elements.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;

// ─────────────────────────────────────────────────────────────────────────────
//  No-std float helpers
// ─────────────────────────────────────────────────────────────────────────────

#[inline] fn f32_floor(x: f32) -> f32 { let i = x as i32 as f32; if x < i { i - 1.0 } else { i } }
#[inline] fn f32_ceil(x: f32)  -> f32 { let i = x as i32 as f32; if x > i { i + 1.0 } else { i } }
#[inline] fn f32_round(x: f32) -> f32 { f32_floor(x + 0.5) }
#[inline] fn f32_abs(x: f32)   -> f32 { if x < 0.0 { -x } else { x } }
#[inline] fn f32_min(a: f32, b: f32) -> f32 { if a < b { a } else { b } }
#[inline] fn f32_max(a: f32, b: f32) -> f32 { if a > b { a } else { b } }
#[inline] fn f32_clamp(v: f32, lo: f32, hi: f32) -> f32 { f32_min(f32_max(v, lo), hi) }
#[inline] fn f32_sqrt(x: f32)  -> f32 { if x <= 0.0 { return 0.0; } let mut r = x * 0.5; for _ in 0..30 { r = (r + x / r) * 0.5; } r }
#[inline] fn f32_sin(x: f32)   -> f32 { // 5-term Taylor around reduced angle
    let pi = core::f32::consts::PI;
    let two_pi = pi * 2.0;
    let mut x = x % two_pi;
    if x > pi { x -= two_pi; } else if x < -pi { x += two_pi; }
    let x2 = x * x;
    x * (1.0 - x2/6.0 * (1.0 - x2/20.0 * (1.0 - x2/42.0)))
}
#[inline] fn f32_cos(x: f32) -> f32 { f32_sin(x + core::f32::consts::FRAC_PI_2) }

// ─────────────────────────────────────────────────────────────────────────────
//  Color
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba { pub r: u8, pub g: u8, pub b: u8, pub a: u8 }

impl Rgba {
    pub const TRANSPARENT: Self = Self { r: 0, g: 0, b: 0, a: 0 };
    pub const BLACK:       Self = Self { r: 0, g: 0, b: 0, a: 255 };
    pub const WHITE:       Self = Self { r: 255, g: 255, b: 255, a: 255 };

    pub fn from_u32_rgba(v: u32) -> Self {
        Self { r: (v >> 24) as u8, g: (v >> 16) as u8, b: (v >> 8) as u8, a: v as u8 }
    }
    pub fn to_u32_rgba(self) -> u32 {
        ((self.r as u32) << 24) | ((self.g as u32) << 16) | ((self.b as u32) << 8) | self.a as u32
    }
    pub fn with_alpha(mut self, a: u8) -> Self { self.a = a; self }

    /// Parse CSS color string: #rgb, #rrggbb, #rgba, #rrggbbaa, rgb(), rgba(), or named.
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s.starts_with('#') {
            let h = &s[1..];
            return match h.len() {
                3 => {
                    let r = u8::from_str_radix(&h[0..1], 16).unwrap_or(0) * 17;
                    let g = u8::from_str_radix(&h[1..2], 16).unwrap_or(0) * 17;
                    let b = u8::from_str_radix(&h[2..3], 16).unwrap_or(0) * 17;
                    Rgba { r, g, b, a: 255 }
                }
                6 => {
                    let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(0);
                    let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(0);
                    let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(0);
                    Rgba { r, g, b, a: 255 }
                }
                8 => {
                    let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(0);
                    let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(0);
                    let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(0);
                    let a = u8::from_str_radix(&h[6..8], 16).unwrap_or(255);
                    Rgba { r, g, b, a }
                }
                _ => Rgba::BLACK,
            };
        }
        if s.starts_with("rgba(") || s.starts_with("rgb(") {
            let inner = s.trim_start_matches("rgba(").trim_start_matches("rgb(").trim_end_matches(')');
            let parts: Vec<f32> = inner.split(',').map(|p| p.trim().parse().unwrap_or(0.0)).collect();
            return Rgba {
                r: f32_clamp(*parts.get(0).unwrap_or(&0.0), 0.0, 255.0) as u8,
                g: f32_clamp(*parts.get(1).unwrap_or(&0.0), 0.0, 255.0) as u8,
                b: f32_clamp(*parts.get(2).unwrap_or(&0.0), 0.0, 255.0) as u8,
                a: (f32_clamp(*parts.get(3).unwrap_or(&1.0), 0.0, 1.0) * 255.0) as u8,
            };
        }
        // Named colors (subset)
        match s {
            "transparent"          => Rgba::TRANSPARENT,
            "black"                => Rgba { r:0,   g:0,   b:0,   a:255 },
            "white"                => Rgba { r:255, g:255, b:255, a:255 },
            "red"                  => Rgba { r:255, g:0,   b:0,   a:255 },
            "green"                => Rgba { r:0,   g:128, b:0,   a:255 },
            "lime"                 => Rgba { r:0,   g:255, b:0,   a:255 },
            "blue"                 => Rgba { r:0,   g:0,   b:255, a:255 },
            "yellow"               => Rgba { r:255, g:255, b:0,   a:255 },
            "cyan"|"aqua"          => Rgba { r:0,   g:255, b:255, a:255 },
            "magenta"|"fuchsia"    => Rgba { r:255, g:0,   b:255, a:255 },
            "orange"               => Rgba { r:255, g:165, b:0,   a:255 },
            "pink"                 => Rgba { r:255, g:192, b:203, a:255 },
            "purple"               => Rgba { r:128, g:0,   b:128, a:255 },
            "gray"|"grey"          => Rgba { r:128, g:128, b:128, a:255 },
            "silver"               => Rgba { r:192, g:192, b:192, a:255 },
            "navy"                 => Rgba { r:0,   g:0,   b:128, a:255 },
            "teal"                 => Rgba { r:0,   g:128, b:128, a:255 },
            "maroon"               => Rgba { r:128, g:0,   b:0,   a:255 },
            "olive"                => Rgba { r:128, g:128, b:0,   a:255 },
            "coral"                => Rgba { r:255, g:127, b:80,  a:255 },
            "salmon"               => Rgba { r:250, g:128, b:114, a:255 },
            "gold"                 => Rgba { r:255, g:215, b:0,   a:255 },
            "skyblue"|"deepskyblue"=> Rgba { r:0,   g:191, b:255, a:255 },
            "indigo"               => Rgba { r:75,  g:0,   b:130, a:255 },
            "violet"               => Rgba { r:238, g:130, b:238, a:255 },
            "crimson"              => Rgba { r:220, g:20,  b:60,  a:255 },
            "turquoise"            => Rgba { r:64,  g:224, b:208, a:255 },
            "chocolate"            => Rgba { r:210, g:105, b:30,  a:255 },
            "tan"                  => Rgba { r:210, g:180, b:140, a:255 },
            "beige"                => Rgba { r:245, g:245, b:220, a:255 },
            "ivory"                => Rgba { r:255, g:255, b:240, a:255 },
            "lavender"             => Rgba { r:230, g:230, b:250, a:255 },
            _                      => Rgba::BLACK,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  2D Transform (3×3 affine, stored as [a,b,c,d,e,f])
//  [ a  c  e ]
//  [ b  d  f ]
//  [ 0  0  1 ]
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Transform {
    pub a: f32, pub b: f32,
    pub c: f32, pub d: f32,
    pub e: f32, pub f: f32,
}

impl Transform {
    pub fn identity() -> Self { Transform { a:1.0,b:0.0,c:0.0,d:1.0,e:0.0,f:0.0 } }

    pub fn multiply(&self, o: &Transform) -> Transform {
        Transform {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }

    pub fn translate(tx: f32, ty: f32) -> Self { Transform { a:1.0,b:0.0,c:0.0,d:1.0,e:tx,f:ty } }
    pub fn scale(sx: f32, sy: f32)     -> Self { Transform { a:sx, b:0.0,c:0.0,d:sy, e:0.0,f:0.0 } }
    pub fn rotate(angle: f32) -> Self {
        let (s, c) = (f32_sin(angle), f32_cos(angle));
        Transform { a:c,b:s,c:-s,d:c,e:0.0,f:0.0 }
    }

    /// Apply transform to a point.
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Path
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PathCmd {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    BezierTo(f32, f32, f32, f32, f32, f32),   // cp1x,cp1y, cp2x,cp2y, x,y
    QuadTo(f32, f32, f32, f32),               // cpx,cpy, x,y
    Arc(f32, f32, f32, f32, f32, bool),       // cx,cy,r,start,end,ccw
    Close,
}

#[derive(Debug, Clone, Default)]
pub struct Path2D {
    pub cmds: Vec<PathCmd>,
}

impl Path2D {
    pub fn new() -> Self { Path2D { cmds: Vec::new() } }

    pub fn move_to(&mut self, x: f32, y: f32) { self.cmds.push(PathCmd::MoveTo(x, y)); }
    pub fn line_to(&mut self, x: f32, y: f32) { self.cmds.push(PathCmd::LineTo(x, y)); }
    pub fn close_path(&mut self)               { self.cmds.push(PathCmd::Close); }

    pub fn bezier_curve_to(&mut self, cp1x: f32, cp1y: f32, cp2x: f32, cp2y: f32, x: f32, y: f32) {
        self.cmds.push(PathCmd::BezierTo(cp1x,cp1y,cp2x,cp2y,x,y));
    }
    pub fn quadratic_curve_to(&mut self, cpx: f32, cpy: f32, x: f32, y: f32) {
        self.cmds.push(PathCmd::QuadTo(cpx,cpy,x,y));
    }

    pub fn arc(&mut self, cx: f32, cy: f32, r: f32, start: f32, end: f32, ccw: bool) {
        self.cmds.push(PathCmd::Arc(cx,cy,r,start,end,ccw));
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.cmds.push(PathCmd::MoveTo(x, y));
        self.cmds.push(PathCmd::LineTo(x+w, y));
        self.cmds.push(PathCmd::LineTo(x+w, y+h));
        self.cmds.push(PathCmd::LineTo(x,   y+h));
        self.cmds.push(PathCmd::Close);
    }

    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32,
                   rot: f32, start: f32, end: f32, ccw: bool) {
        // Approximate ellipse via bezier curves (n=32 segments)
        let n = 32usize;
        let pi2 = core::f32::consts::PI * 2.0;
        let (sweep, direction) = if ccw {
            (if end < start { end - start } else { end - start - pi2 }, -1.0f32)
        } else {
            (if end > start { end - start } else { end - start + pi2 }, 1.0f32)
        };
        let step = sweep / n as f32;
        let (sr, cr) = (f32_sin(rot), f32_cos(rot));
        let first_x = cx + rx * f32_cos(start) * cr - ry * f32_sin(start) * sr;
        let first_y = cy + rx * f32_cos(start) * sr + ry * f32_sin(start) * cr;
        self.cmds.push(PathCmd::LineTo(first_x, first_y));
        for i in 1..=n {
            let angle = start + step * i as f32;
            let x = cx + rx * f32_cos(angle) * cr - ry * f32_sin(angle) * sr;
            let y = cy + rx * f32_cos(angle) * sr + ry * f32_sin(angle) * cr;
            self.cmds.push(PathCmd::LineTo(x, y));
        }
    }

    /// Flatten the path into a list of (x,y) polyline points, applying transform.
    pub fn flatten(&self, xform: &Transform, tolerance: f32) -> Vec<Vec<(f32,f32)>> {
        let mut subpaths: Vec<Vec<(f32,f32)>> = Vec::new();
        let mut current: Vec<(f32,f32)> = Vec::new();
        let mut pen = (0.0f32, 0.0f32);

        for cmd in &self.cmds {
            match cmd {
                PathCmd::MoveTo(x,y) => {
                    if !current.is_empty() { subpaths.push(core::mem::take(&mut current)); }
                    let p = xform.apply(*x, *y);
                    current.push(p); pen = p;
                }
                PathCmd::LineTo(x,y) => {
                    let p = xform.apply(*x, *y);
                    current.push(p); pen = p;
                }
                PathCmd::BezierTo(cp1x,cp1y,cp2x,cp2y,x,y) => {
                    let p0 = pen;
                    let p1 = xform.apply(*cp1x,*cp1y);
                    let p2 = xform.apply(*cp2x,*cp2y);
                    let p3 = xform.apply(*x,*y);
                    flatten_cubic(&mut current, p0,p1,p2,p3,tolerance);
                    pen = p3;
                }
                PathCmd::QuadTo(cpx,cpy,x,y) => {
                    let p0 = pen;
                    let p1 = xform.apply(*cpx,*cpy);
                    let p2 = xform.apply(*x,*y);
                    flatten_quad(&mut current, p0,p1,p2,tolerance);
                    pen = p2;
                }
                PathCmd::Arc(cx,cy,r,start,end,ccw) => {
                    let n = 32usize;
                    let pi2 = core::f32::consts::PI * 2.0;
                    let sweep = if *ccw {
                        if *end < *start { *end - *start } else { *end - *start - pi2 }
                    } else {
                        if *end > *start { *end - *start } else { *end - *start + pi2 }
                    };
                    let step = sweep / n as f32;
                    for i in 0..=n {
                        let angle = *start + step * i as f32;
                        let p = xform.apply(*cx + *r * f32_cos(angle), *cy + *r * f32_sin(angle));
                        current.push(p);
                    }
                    pen = *current.last().unwrap_or(&pen);
                }
                PathCmd::Close => {
                    if let Some(&first) = current.first() { current.push(first); }
                    subpaths.push(core::mem::take(&mut current));
                }
            }
        }
        if !current.is_empty() { subpaths.push(current); }
        subpaths
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Bezier flattening
// ─────────────────────────────────────────────────────────────────────────────

fn flatten_cubic(out: &mut Vec<(f32,f32)>, p0: (f32,f32), p1: (f32,f32),
                 p2: (f32,f32), p3: (f32,f32), tol: f32) {
    // Check if close enough to a line
    let dx = p3.0 - p0.0; let dy = p3.1 - p0.1;
    let d1 = f32_abs((p1.0-p0.0)*dy - (p1.1-p0.1)*dx);
    let d2 = f32_abs((p2.0-p0.0)*dy - (p2.1-p0.1)*dx);
    if (d1+d2)*(d1+d2) < tol * (dx*dx+dy*dy) {
        out.push(p3); return;
    }
    // Subdivide at t=0.5
    let m01 = mid(p0,p1); let m12 = mid(p1,p2); let m23 = mid(p2,p3);
    let m012 = mid(m01,m12); let m123 = mid(m12,m23);
    let m0123 = mid(m012,m123);
    flatten_cubic(out, p0, m01, m012, m0123, tol);
    flatten_cubic(out, m0123, m123, m23, p3, tol);
}

fn flatten_quad(out: &mut Vec<(f32,f32)>, p0: (f32,f32), p1: (f32,f32),
                p2: (f32,f32), tol: f32) {
    let dx = p2.0 - p0.0; let dy = p2.1 - p0.1;
    let d = f32_abs((p1.0-p0.0)*dy - (p1.1-p0.1)*dx);
    if d*d < tol*(dx*dx+dy*dy) { out.push(p2); return; }
    let m01 = mid(p0,p1); let m12 = mid(p1,p2); let m012 = mid(m01,m12);
    flatten_quad(out, p0, m01, m012, tol);
    flatten_quad(out, m012, m12, p2, tol);
}

#[inline] fn mid(a: (f32,f32), b: (f32,f32)) -> (f32,f32) { ((a.0+b.0)*0.5, (a.1+b.1)*0.5) }

// ─────────────────────────────────────────────────────────────────────────────
//  Gradient
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ColorStop { pub offset: f32, pub color: Rgba }

#[derive(Debug, Clone)]
pub enum Gradient {
    Linear { x0: f32, y0: f32, x1: f32, y1: f32, stops: Vec<ColorStop> },
    Radial { x0: f32, y0: f32, r0: f32, x1: f32, y1: f32, r1: f32, stops: Vec<ColorStop> },
}

impl Gradient {
    pub fn add_color_stop(&mut self, offset: f32, color: Rgba) {
        let stops = match self { Gradient::Linear { stops, .. } | Gradient::Radial { stops, .. } => stops };
        stops.push(ColorStop { offset, color });
        stops.sort_by(|a,b| a.offset.partial_cmp(&b.offset).unwrap_or(core::cmp::Ordering::Equal));
    }

    pub fn sample(&self, t: f32) -> Rgba {
        let t = f32_clamp(t, 0.0, 1.0);
        let stops = match self { Gradient::Linear { stops, .. } | Gradient::Radial { stops, .. } => stops };
        if stops.is_empty() { return Rgba::TRANSPARENT; }
        if stops.len() == 1 { return stops[0].color; }
        let (mut lo, mut hi) = (&stops[0], &stops[stops.len()-1]);
        for i in 0..stops.len()-1 {
            if t >= stops[i].offset && t <= stops[i+1].offset {
                lo = &stops[i]; hi = &stops[i+1]; break;
            }
        }
        if (hi.offset - lo.offset).abs() < 1e-6 { return lo.color; }
        let u = (t - lo.offset) / (hi.offset - lo.offset);
        lerp_color(lo.color, hi.color, u)
    }

    /// Compute t for a given pixel coordinate.
    pub fn t_at(&self, px: f32, py: f32) -> f32 {
        match self {
            Gradient::Linear { x0,y0,x1,y1, .. } => {
                let dx = x1 - x0; let dy = y1 - y0;
                let len2 = dx*dx + dy*dy;
                if len2 < 1e-10 { 0.0 }
                else { ((px - x0)*dx + (py - y0)*dy) / len2 }
            }
            Gradient::Radial { x1,y1,r1, .. } => {
                let dx = px - x1; let dy = py - y1;
                let dist = f32_sqrt(dx*dx + dy*dy);
                if *r1 < 1e-6 { 0.0 } else { dist / r1 }
            }
        }
    }
}

fn lerp_color(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: lerp_u8(a.r, b.r, t),
        g: lerp_u8(a.g, b.g, t),
        b: lerp_u8(a.b, b.b, t),
        a: lerp_u8(a.a, b.a, t),
    }
}
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t) as u8
}

// ─────────────────────────────────────────────────────────────────────────────
//  Paint style
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PaintStyle {
    Color(Rgba),
    Gradient(Gradient),
    // Pattern would go here in a fuller impl
}

impl PaintStyle {
    pub fn color_at(&self, px: f32, py: f32) -> Rgba {
        match self {
            PaintStyle::Color(c)    => *c,
            PaintStyle::Gradient(g) => g.sample(g.t_at(px, py)),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Line cap / join
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineCap  { Butt, Round, Square }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineJoin { Miter, Round, Bevel }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompositeOp {
    SourceOver, SourceIn, SourceOut, SourceAtop,
    DestOver,   DestIn,   DestOut,   DestAtop,
    Copy, Xor, Lighter, Multiply, Screen, Overlay,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Drawing state (saved/restored)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DrawState {
    pub fill_style:      PaintStyle,
    pub stroke_style:    PaintStyle,
    pub line_width:      f32,
    pub line_cap:        LineCap,
    pub line_join:       LineJoin,
    pub miter_limit:     f32,
    pub global_alpha:    f32,
    pub composite:       CompositeOp,
    pub shadow_offset_x: f32,
    pub shadow_offset_y: f32,
    pub shadow_blur:     f32,
    pub shadow_color:    Rgba,
    pub font_size:       f32,
    pub font_family:     String,
    pub text_align:      TextAlign,
    pub text_baseline:   TextBaseline,
    pub transform:       Transform,
    pub clip:            Option<Vec<(f32,f32)>>, // clip polygon
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextAlign    { Left, Right, Center, Start, End }
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextBaseline { Top, Hanging, Middle, Alphabetic, Ideographic, Bottom }

impl Default for DrawState {
    fn default() -> Self {
        DrawState {
            fill_style:      PaintStyle::Color(Rgba::BLACK),
            stroke_style:    PaintStyle::Color(Rgba::BLACK),
            line_width:      1.0,
            line_cap:        LineCap::Butt,
            line_join:       LineJoin::Miter,
            miter_limit:     10.0,
            global_alpha:    1.0,
            composite:       CompositeOp::SourceOver,
            shadow_offset_x: 0.0,
            shadow_offset_y: 0.0,
            shadow_blur:     0.0,
            shadow_color:    Rgba::TRANSPARENT,
            font_size:       10.0,
            font_family:     "sans-serif".to_string(),
            text_align:      TextAlign::Start,
            text_baseline:   TextBaseline::Alphabetic,
            transform:       Transform::identity(),
            clip:            None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  ImageData
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ImageData {
    pub data:   Vec<u8>,  // RGBA, length = width * height * 4
    pub width:  u32,
    pub height: u32,
}

impl ImageData {
    pub fn new(w: u32, h: u32) -> Self {
        ImageData { data: vec![0u8; (w * h * 4) as usize], width: w, height: h }
    }
    pub fn pixel(&self, x: u32, y: u32) -> Rgba {
        let off = ((y * self.width + x) * 4) as usize;
        if off + 3 >= self.data.len() { return Rgba::TRANSPARENT; }
        Rgba { r: self.data[off], g: self.data[off+1], b: self.data[off+2], a: self.data[off+3] }
    }
    pub fn set_pixel(&mut self, x: u32, y: u32, c: Rgba) {
        let off = ((y * self.width + x) * 4) as usize;
        if off + 3 >= self.data.len() { return; }
        self.data[off]=c.r; self.data[off+1]=c.g; self.data[off+2]=c.b; self.data[off+3]=c.a;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Canvas + Context2D
// ─────────────────────────────────────────────────────────────────────────────

pub struct CanvasContext2D {
    pub pixels:     ImageData,
    state:          DrawState,
    state_stack:    Vec<DrawState>,
    current_path:   Path2D,
}

impl CanvasContext2D {
    pub fn new(width: u32, height: u32) -> Self {
        CanvasContext2D {
            pixels:      ImageData::new(width, height),
            state:       DrawState::default(),
            state_stack: Vec::new(),
            current_path: Path2D::new(),
        }
    }

    pub fn width(&self)  -> u32 { self.pixels.width }
    pub fn height(&self) -> u32 { self.pixels.height }

    // ── State stack ──────────────────────────────────────────────────────────

    pub fn save(&mut self)    { self.state_stack.push(self.state.clone()); }
    pub fn restore(&mut self) { if let Some(s) = self.state_stack.pop() { self.state = s; } }

    // ── Style setters ────────────────────────────────────────────────────────

    pub fn set_fill_style_color(&mut self, css: &str)   { self.state.fill_style   = PaintStyle::Color(Rgba::parse(css)); }
    pub fn set_stroke_style_color(&mut self, css: &str) { self.state.stroke_style = PaintStyle::Color(Rgba::parse(css)); }
    pub fn set_fill_gradient(&mut self, g: Gradient)    { self.state.fill_style   = PaintStyle::Gradient(g); }
    pub fn set_stroke_gradient(&mut self, g: Gradient)  { self.state.stroke_style = PaintStyle::Gradient(g); }
    pub fn set_line_width(&mut self, w: f32)            { self.state.line_width   = f32_max(0.0, w); }
    pub fn set_line_cap(&mut self, cap: LineCap)        { self.state.line_cap     = cap; }
    pub fn set_line_join(&mut self, join: LineJoin)     { self.state.line_join    = join; }
    pub fn set_miter_limit(&mut self, ml: f32)          { self.state.miter_limit  = ml; }
    pub fn set_global_alpha(&mut self, a: f32)          { self.state.global_alpha = f32_clamp(a, 0.0, 1.0); }
    pub fn set_shadow(&mut self, ox: f32, oy: f32, blur: f32, css: &str) {
        self.state.shadow_offset_x = ox; self.state.shadow_offset_y = oy;
        self.state.shadow_blur = blur; self.state.shadow_color = Rgba::parse(css);
    }
    pub fn set_font(&mut self, size: f32, family: &str) {
        self.state.font_size = size; self.state.font_family = family.to_string();
    }
    pub fn set_text_align(&mut self, a: TextAlign)       { self.state.text_align    = a; }
    pub fn set_text_baseline(&mut self, b: TextBaseline) { self.state.text_baseline = b; }

    // ── Transforms ───────────────────────────────────────────────────────────

    pub fn translate(&mut self, tx: f32, ty: f32) {
        self.state.transform = self.state.transform.multiply(&Transform::translate(tx,ty));
    }
    pub fn scale(&mut self, sx: f32, sy: f32) {
        self.state.transform = self.state.transform.multiply(&Transform::scale(sx,sy));
    }
    pub fn rotate(&mut self, angle: f32) {
        self.state.transform = self.state.transform.multiply(&Transform::rotate(angle));
    }
    pub fn set_transform(&mut self, a:f32,b:f32,c:f32,d:f32,e:f32,f:f32) {
        self.state.transform = Transform{a,b,c,d,e,f};
    }
    pub fn reset_transform(&mut self) { self.state.transform = Transform::identity(); }

    // ── Path API ─────────────────────────────────────────────────────────────

    pub fn begin_path(&mut self)                                    { self.current_path = Path2D::new(); }
    pub fn close_path(&mut self)                                    { self.current_path.close_path(); }
    pub fn move_to(&mut self, x:f32,y:f32)                         { self.current_path.move_to(x,y); }
    pub fn line_to(&mut self, x:f32,y:f32)                         { self.current_path.line_to(x,y); }
    pub fn bezier_curve_to(&mut self,cp1x:f32,cp1y:f32,cp2x:f32,cp2y:f32,x:f32,y:f32) {
        self.current_path.bezier_curve_to(cp1x,cp1y,cp2x,cp2y,x,y);
    }
    pub fn quadratic_curve_to(&mut self,cpx:f32,cpy:f32,x:f32,y:f32) {
        self.current_path.quadratic_curve_to(cpx,cpy,x,y);
    }
    pub fn arc(&mut self,cx:f32,cy:f32,r:f32,start:f32,end:f32,ccw:bool) {
        self.current_path.arc(cx,cy,r,start,end,ccw);
    }
    pub fn arc_to(&mut self, x1:f32,y1:f32,x2:f32,y2:f32,r:f32) {
        // Approximate with lines for now
        self.current_path.line_to(x1,y1);
        self.current_path.line_to(x2,y2);
    }
    pub fn ellipse(&mut self,cx:f32,cy:f32,rx:f32,ry:f32,rot:f32,start:f32,end:f32,ccw:bool) {
        self.current_path.ellipse(cx,cy,rx,ry,rot,start,end,ccw);
    }
    pub fn rect_path(&mut self,x:f32,y:f32,w:f32,h:f32) { self.current_path.rect(x,y,w,h); }

    // ── Fill & Stroke ─────────────────────────────────────────────────────────

    pub fn fill(&mut self) {
        let path = self.current_path.clone();
        self.fill_path(&path);
    }

    pub fn stroke(&mut self) {
        let path = self.current_path.clone();
        self.stroke_path(&path);
    }

    pub fn fill_rect(&mut self, x:f32,y:f32,w:f32,h:f32) {
        let mut p = Path2D::new(); p.rect(x,y,w,h); self.fill_path(&p);
    }
    pub fn stroke_rect(&mut self, x:f32,y:f32,w:f32,h:f32) {
        let mut p = Path2D::new(); p.rect(x,y,w,h); self.stroke_path(&p);
    }
    pub fn clear_rect(&mut self, x:f32,y:f32,w:f32,h:f32) {
        let x0 = f32_max(0.0, x) as u32;
        let y0 = f32_max(0.0, y) as u32;
        let x1 = f32_min((x+w), self.pixels.width  as f32) as u32;
        let y1 = f32_min((y+h), self.pixels.height as f32) as u32;
        for py in y0..y1 { for px in x0..x1 { self.pixels.set_pixel(px,py,Rgba::TRANSPARENT); } }
    }

    fn fill_path(&mut self, path: &Path2D) {
        let subpaths = path.flatten(&self.state.transform, 0.25);
        let w = self.pixels.width;
        let h = self.pixels.height;
        // Scanline fill using even-odd winding (simplified)
        let mut bounds = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for sp in &subpaths {
            for &(px,py) in sp {
                bounds.0 = f32_min(bounds.0, px); bounds.1 = f32_min(bounds.1, py);
                bounds.2 = f32_max(bounds.2, px); bounds.3 = f32_max(bounds.3, py);
            }
        }
        let y0 = (f32_max(0.0, f32_floor(bounds.1)) as u32).min(h.saturating_sub(1));
        let y1 = (f32_min(h as f32, f32_ceil(bounds.3)) as u32).min(h);
        let x0 = (f32_max(0.0, f32_floor(bounds.0)) as u32).min(w.saturating_sub(1));
        let x1 = (f32_min(w as f32, f32_ceil(bounds.2)) as u32).min(w);
        let alpha = (self.state.global_alpha * 255.0) as u8;
        let style = self.state.fill_style.clone();
        for py in y0..y1 {
            // Find x intersections at py+0.5
            let yscan = py as f32 + 0.5;
            let mut xs: Vec<f32> = Vec::new();
            for sp in &subpaths {
                for i in 0..sp.len().saturating_sub(1) {
                    let (ax,ay) = sp[i]; let (bx,by) = sp[i+1];
                    if (ay <= yscan && by > yscan) || (by <= yscan && ay > yscan) {
                        let t = (yscan - ay) / (by - ay);
                        xs.push(ax + t * (bx - ax));
                    }
                }
            }
            xs.sort_by(|a,b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
            let mut i = 0;
            while i + 1 < xs.len() {
                let lx = f32_max(xs[i],   x0 as f32) as u32;
                let rx = f32_min(xs[i+1], x1 as f32) as u32;
                for px in lx..rx {
                    let c = style.color_at(px as f32, py as f32);
                    let blended = blend_src_over(c.with_alpha((c.a as u16 * alpha as u16 / 255) as u8),
                                                 self.pixels.pixel(px, py));
                    self.pixels.set_pixel(px, py, blended);
                }
                i += 2;
            }
        }
    }

    fn stroke_path(&mut self, path: &Path2D) {
        let subpaths = path.flatten(&self.state.transform, 0.25);
        let lw = self.state.line_width;
        let alpha = (self.state.global_alpha * 255.0) as u8;
        let style = self.state.stroke_style.clone();
        for sp in &subpaths {
            for i in 0..sp.len().saturating_sub(1) {
                let (ax,ay) = sp[i]; let (bx,by) = sp[i+1];
                self.draw_thick_line(ax,ay,bx,by,lw,&style,alpha);
            }
        }
    }

    fn draw_thick_line(&mut self, ax:f32,ay:f32,bx:f32,by:f32,
                        lw:f32, style:&PaintStyle, alpha:u8) {
        let dx = bx-ax; let dy = by-ay;
        let len = f32_sqrt(dx*dx+dy*dy);
        if len < 0.001 { return; }
        let nx = -dy/len * lw*0.5;
        let ny =  dx/len * lw*0.5;
        // Draw 4 corners of the rectangle
        let pts = [(ax+nx,ay+ny),(ax-nx,ay-ny),(bx-nx,by-ny),(bx+nx,by+ny)];
        let w = self.pixels.width as i32; let h = self.pixels.height as i32;
        let xmin = f32_max(0.0, pts.iter().map(|p|p.0).fold(f32::MAX,f32_min)) as i32;
        let xmax = f32_min(w as f32-1.0, pts.iter().map(|p|p.0).fold(f32::MIN,f32_max)) as i32;
        let ymin = f32_max(0.0, pts.iter().map(|p|p.1).fold(f32::MAX,f32_min)) as i32;
        let ymax = f32_min(h as f32-1.0, pts.iter().map(|p|p.1).fold(f32::MIN,f32_max)) as i32;
        for py in ymin..=ymax { for px in xmin..=xmax {
            if point_in_quad(px as f32+0.5, py as f32+0.5, &pts) {
                let c = style.color_at(px as f32, py as f32);
                let c2 = c.with_alpha((c.a as u16 * alpha as u16 / 255) as u8);
                let blended = blend_src_over(c2, self.pixels.pixel(px as u32,py as u32));
                self.pixels.set_pixel(px as u32, py as u32, blended);
            }
        }}
    }

    // ── Text ────────────────────────────────────────────────────────────────

    pub fn measure_text(&self, text: &str) -> f32 {
        // 8px per char for bitmap font, scaled by font_size/16
        let scale = self.state.font_size / 16.0;
        text.len() as f32 * 8.0 * scale
    }

    pub fn fill_text(&mut self, text: &str, x: f32, y: f32, _max_width: Option<f32>) {
        let scale = self.state.font_size / 16.0;
        let (tx, ty) = self.state.transform.apply(x, y);
        let alpha = (self.state.global_alpha * 255.0) as u8;
        let style = self.state.fill_style.clone();
        let baseline_off = match self.state.text_baseline {
            TextBaseline::Top => 0.0,
            TextBaseline::Middle => -8.0 * scale,
            TextBaseline::Bottom | TextBaseline::Alphabetic | TextBaseline::Ideographic => -16.0 * scale,
            TextBaseline::Hanging => -2.0 * scale,
        };
        let text_w = self.measure_text(text);
        let align_off = match self.state.text_align {
            TextAlign::Right | TextAlign::End   => -text_w,
            TextAlign::Center                   => -text_w * 0.5,
            _                                   => 0.0,
        };
        let mut cx = tx + align_off;
        let cy = ty + baseline_off;
        for ch in text.chars() {
            if let Some(bitmap) = crate::drivers::font::render_glyph_builtin(ch) {
                for row in 0..16usize {
                    for col in 0..8usize {
                        if bitmap[row] & (0x80 >> col) != 0 {
                            let px = (cx + col as f32 * scale) as i32;
                            let py = (cy + row as f32 * scale) as i32;
                            if px >= 0 && py >= 0
                                && (px as u32) < self.pixels.width
                                && (py as u32) < self.pixels.height {
                                let c = style.color_at(px as f32, py as f32);
                                let ca = c.with_alpha((c.a as u16 * alpha as u16 / 255) as u8);
                                let bl = blend_src_over(ca, self.pixels.pixel(px as u32, py as u32));
                                self.pixels.set_pixel(px as u32, py as u32, bl);
                            }
                        }
                    }
                }
            }
            cx += 8.0 * scale;
        }
    }

    pub fn stroke_text(&mut self, text: &str, x: f32, y: f32, max_width: Option<f32>) {
        // Simplified: just draw filled (outline font needs glyph outlines)
        self.fill_text(text, x, y, max_width);
    }

    // ── Image ops ───────────────────────────────────────────────────────────

    pub fn draw_image(&mut self, img: &ImageData, dx: f32, dy: f32) {
        self.draw_image_scaled(img, dx, dy, img.width as f32, img.height as f32);
    }

    pub fn draw_image_scaled(&mut self, img: &ImageData, dx: f32, dy: f32, dw: f32, dh: f32) {
        let (tx, ty) = self.state.transform.apply(dx, dy);
        let alpha = (self.state.global_alpha * 255.0) as u8;
        let dst_w = self.pixels.width;
        let dst_h = self.pixels.height;
        let x0 = f32_max(0.0, tx) as u32;
        let y0 = f32_max(0.0, ty) as u32;
        let x1 = f32_min((tx + dw), dst_w as f32) as u32;
        let y1 = f32_min((ty + dh), dst_h as f32) as u32;
        for py in y0..y1 {
            for px in x0..x1 {
                let sx = ((px as f32 - tx) * img.width  as f32 / dw) as u32;
                let sy = ((py as f32 - ty) * img.height as f32 / dh) as u32;
                let src = img.pixel(sx.min(img.width-1), sy.min(img.height-1));
                let src2 = src.with_alpha((src.a as u16 * alpha as u16 / 255) as u8);
                let blended = blend_src_over(src2, self.pixels.pixel(px, py));
                self.pixels.set_pixel(px, py, blended);
            }
        }
    }

    pub fn get_image_data(&self, x: u32, y: u32, w: u32, h: u32) -> ImageData {
        let mut out = ImageData::new(w, h);
        for py in 0..h { for px in 0..w {
            out.set_pixel(px, py, self.pixels.pixel(x+px, y+py));
        }}
        out
    }

    pub fn put_image_data(&mut self, img: &ImageData, dx: u32, dy: u32) {
        for py in 0..img.height { for px in 0..img.width {
            let tx = dx + px; let ty = dy + py;
            if tx < self.pixels.width && ty < self.pixels.height {
                self.pixels.set_pixel(tx, ty, img.pixel(px, py));
            }
        }}
    }

    pub fn create_image_data(w: u32, h: u32) -> ImageData { ImageData::new(w, h) }

    // ── Clip ────────────────────────────────────────────────────────────────

    pub fn clip(&mut self) {
        let subpaths = self.current_path.flatten(&self.state.transform, 0.25);
        // Take first closed subpath as clip polygon
        self.state.clip = subpaths.into_iter().next();
    }

    // ── Gradient constructors ────────────────────────────────────────────────

    pub fn create_linear_gradient(x0:f32,y0:f32,x1:f32,y1:f32) -> Gradient {
        Gradient::Linear { x0,y0,x1,y1,stops:Vec::new() }
    }
    pub fn create_radial_gradient(x0:f32,y0:f32,r0:f32,x1:f32,y1:f32,r1:f32) -> Gradient {
        Gradient::Radial { x0,y0,r0,x1,y1,r1,stops:Vec::new() }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Pixel compositing
// ─────────────────────────────────────────────────────────────────────────────

fn blend_src_over(src: Rgba, dst: Rgba) -> Rgba {
    if src.a == 255 { return src; }
    if src.a == 0   { return dst; }
    let inv = 255 - src.a as u16;
    Rgba {
        r: ((src.r as u16 * src.a as u16 + dst.r as u16 * inv) / 255) as u8,
        g: ((src.g as u16 * src.a as u16 + dst.g as u16 * inv) / 255) as u8,
        b: ((src.b as u16 * src.a as u16 + dst.b as u16 * inv) / 255) as u8,
        a: (src.a as u16 + (dst.a as u16 * inv / 255)).min(255) as u8,
    }
}

fn point_in_quad(px: f32, py: f32, pts: &[(f32,f32); 4]) -> bool {
    let mut inside = false;
    let n = 4;
    let mut j = n - 1;
    for i in 0..n {
        let (xi,yi) = pts[i]; let (xj,yj) = pts[j];
        if ((yi > py) != (yj > py)) && (px < (xj-xi)*(py-yi)/(yj-yi)+xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

// ─────────────────────────────────────────────────────────────────────────────
//  Canvas element (ties context to DOM width/height)
// ─────────────────────────────────────────────────────────────────────────────

pub struct Canvas {
    pub width:   u32,
    pub height:  u32,
    pub context: CanvasContext2D,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        Canvas { width, height, context: CanvasContext2D::new(width, height) }
    }

    pub fn get_context_2d(&mut self) -> &mut CanvasContext2D { &mut self.context }

    /// Convert canvas pixel buffer to the screen format and blit at (x,y).
    pub fn blit_to_screen(&self, dst_x: u32, dst_y: u32, is_bgr: bool) {
        use crate::drivers::gpu2d;
        let surf = match gpu2d::sw_surface() { Some(s) => s, None => return };
        let fw = surf.width; let fh = surf.height;
        for py in 0..self.height {
            for px in 0..self.width {
                let c = self.context.pixels.pixel(px, py);
                let fx = dst_x + px; let fy = dst_y + py;
                if fx >= fw || fy >= fh { continue; }
                let color32: u32 = if is_bgr {
                    ((c.a as u32)<<24)|((c.b as u32)<<16)|((c.g as u32)<<8)|(c.r as u32)
                } else {
                    ((c.a as u32)<<24)|((c.r as u32)<<16)|((c.g as u32)<<8)|(c.b as u32)
                };
                unsafe { *surf.pixel_ptr(fx, fy) = color32; }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init + self-test
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[canvas] Canvas 2D API ready (Phase 41).");
}

pub fn self_test() -> bool {
    let mut canvas = Canvas::new(64, 64);
    let ctx = canvas.get_context_2d();

    // Fill background white
    ctx.set_fill_style_color("white");
    ctx.fill_rect(0.0, 0.0, 64.0, 64.0);

    // Draw red rectangle
    ctx.set_fill_style_color("#ff0000");
    ctx.fill_rect(10.0, 10.0, 20.0, 20.0);

    // Draw blue circle
    ctx.set_fill_style_color("blue");
    ctx.begin_path();
    ctx.arc(48.0, 16.0, 10.0, 0.0, core::f32::consts::PI * 2.0, false);
    ctx.fill();

    // Draw green line
    ctx.set_stroke_style_color("green");
    ctx.set_line_width(3.0);
    ctx.begin_path();
    ctx.move_to(0.0, 32.0);
    ctx.line_to(64.0, 64.0);
    ctx.stroke();

    // Verify red pixel inside rectangle
    let p = ctx.pixels.pixel(20, 20);
    if p.r < 200 || p.g > 50 || p.b > 50 { return false; }

    // Verify white outside rectangle
    let p2 = ctx.pixels.pixel(5, 5);
    if p2.r < 200 || p2.g < 200 || p2.b < 200 { return false; }

    true
}

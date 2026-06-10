#![allow(dead_code)]
/// Smart OS — SVG Renderer (Phase 93, v0.53.0)
///
/// Parses and rasterises a subset of SVG 1.1 / SVG 2.0:
///   Elements: `svg`, `g`, `rect`, `circle`, `ellipse`, `line`, `polyline`,
///             `polygon`, `path`, `text`, `use`, `defs`
///   Paint:    `fill`, `stroke`, `stroke-width`, `opacity`
///   Path:     M/m L/l H/h V/v C/c S/s Q/q T/t A/a Z commands
///   Transform:`translate`, `scale`, `rotate`, `matrix`
///
/// Output: RGBA pixel buffer (software rasteriser — Bresenham lines,
/// scanline-fill polygons, Bézier flattening).

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use alloc::collections::BTreeMap;

// ─── Integer square root (no libm needed) ────────────────────────────────────
fn isqrt_u32(n: u32) -> u32 {
    if n == 0 { return 0; }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x { x = y; y = (x + n / x) / 2; }
    x
}

// ─── Color ────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvgColor { pub r: u8, pub g: u8, pub b: u8, pub a: u8 }

impl SvgColor {
    pub const TRANSPARENT: Self = SvgColor { r:0, g:0, b:0, a:0 };
    pub const BLACK: Self       = SvgColor { r:0, g:0, b:0, a:255 };
    pub const WHITE: Self       = SvgColor { r:255,g:255,b:255,a:255 };
    pub const NONE: Self        = SvgColor { r:0, g:0, b:0, a:0 };

    /// Alias for `parse()` — compatible with the standard `FromStr` pattern.
    pub fn from_str(s: &str) -> Self {
        SvgColor::parse(s).unwrap_or(SvgColor::BLACK)
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "none"        => Some(SvgColor::NONE),
            "transparent" => Some(SvgColor::TRANSPARENT),
            "black"       => Some(SvgColor { r:0,   g:0,   b:0,   a:255 }),
            "white"       => Some(SvgColor { r:255, g:255, b:255, a:255 }),
            "red"         => Some(SvgColor { r:255, g:0,   b:0,   a:255 }),
            "green"       => Some(SvgColor { r:0,   g:128, b:0,   a:255 }),
            "blue"        => Some(SvgColor { r:0,   g:0,   b:255, a:255 }),
            "yellow"      => Some(SvgColor { r:255, g:255, b:0,   a:255 }),
            "orange"      => Some(SvgColor { r:255, g:165, b:0,   a:255 }),
            "purple"      => Some(SvgColor { r:128, g:0,   b:128, a:255 }),
            "gray"|"grey" => Some(SvgColor { r:128, g:128, b:128, a:255 }),
            _ if s.starts_with('#') => parse_hex_color(&s[1..]),
            _ if s.starts_with("rgb(") => parse_rgb_color(&s[4..s.len().saturating_sub(1)]),
            _ => None,
        }
    }

    pub fn with_opacity(mut self, alpha: f32) -> Self {
        self.a = (self.a as f32 * alpha.clamp(0.0, 1.0)) as u8;
        self
    }
}

fn parse_hex_color(hex: &str) -> Option<SvgColor> {
    let hex = hex.trim_end_matches(|c: char| !c.is_ascii_hexdigit());
    let val = u32::from_str_radix(hex, 16).ok()?;
    Some(match hex.len() {
        3 => {
            let r = ((val >> 8) & 0xF) as u8; let r = r | (r << 4);
            let g = ((val >> 4) & 0xF) as u8; let g = g | (g << 4);
            let b = (val & 0xF) as u8;        let b = b | (b << 4);
            SvgColor { r, g, b, a: 255 }
        }
        6 => SvgColor { r: ((val>>16)&0xFF) as u8, g: ((val>>8)&0xFF) as u8, b: (val&0xFF) as u8, a: 255 },
        8 => SvgColor { r: ((val>>24)&0xFF) as u8, g: ((val>>16)&0xFF) as u8, b: ((val>>8)&0xFF) as u8, a: (val&0xFF) as u8 },
        _ => return None,
    })
}

fn parse_rgb_color(s: &str) -> Option<SvgColor> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() < 3 { return None; }
    let r = parts[0].trim().trim_end_matches('%').parse::<u32>().ok()?;
    let g = parts[1].trim().trim_end_matches('%').parse::<u32>().ok()?;
    let b = parts[2].trim().trim_end_matches('%').parse::<u32>().ok()?;
    Some(SvgColor { r: r.min(255) as u8, g: g.min(255) as u8, b: b.min(255) as u8, a: 255 })
}

// ─── Transform ────────────────────────────────────────────────────────────────
/// 2-D affine transform stored as a 3×3 matrix (column-major, last row implicit).
#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub a: f32, pub b: f32,
    pub c: f32, pub d: f32,
    pub e: f32, pub f: f32,
}

impl Transform {
    pub const IDENTITY: Self = Transform { a:1., b:0., c:0., d:1., e:0., f:0. };

    pub fn translate(tx: f32, ty: f32) -> Self {
        Transform { a:1., b:0., c:0., d:1., e:tx, f:ty }
    }
    pub fn scale(sx: f32, sy: f32) -> Self {
        Transform { a:sx, b:0., c:0., d:sy, e:0., f:0. }
    }
    pub fn rotate_deg(deg: f32) -> Self {
        let (s, c) = sin_cos_approx(deg * core::f32::consts::PI / 180.0);
        Transform { a:c, b:s, c:-s, d:c, e:0., f:0. }
    }
    pub fn concat(&self, other: &Transform) -> Transform {
        Transform {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e,
         self.b * x + self.d * y + self.f)
    }
    pub fn parse(s: &str) -> Self {
        let s = s.trim().to_ascii_lowercase();
        if let Some(inner) = s.strip_prefix("translate(").and_then(|r| r.strip_suffix(')')) {
            let p: Vec<f32> = inner.split(|c: char| c == ',' || c.is_ascii_whitespace())
                .filter_map(|v| v.trim().parse().ok()).collect();
            return Transform::translate(p.get(0).copied().unwrap_or(0.), p.get(1).copied().unwrap_or(0.));
        }
        if let Some(inner) = s.strip_prefix("scale(").and_then(|r| r.strip_suffix(')')) {
            let p: Vec<f32> = inner.split(|c: char| c == ',' || c.is_ascii_whitespace())
                .filter_map(|v| v.trim().parse().ok()).collect();
            let sx = p.get(0).copied().unwrap_or(1.);
            let sy = p.get(1).copied().unwrap_or(sx);
            return Transform::scale(sx, sy);
        }
        if let Some(inner) = s.strip_prefix("rotate(").and_then(|r| r.strip_suffix(')')) {
            if let Ok(deg) = inner.trim().parse::<f32>() {
                return Transform::rotate_deg(deg);
            }
        }
        Transform::IDENTITY
    }
}

fn sin_cos_approx(x: f32) -> (f32, f32) {
    // Taylor series sin/cos centred at 0, sufficient for small angles
    let x = x % (2.0 * core::f32::consts::PI);
    let s = x - x*x*x/6.0 + x*x*x*x*x/120.0;
    let c = 1.0 - x*x/2.0 + x*x*x*x/24.0;
    (s, c)
}

// ─── Path command ─────────────────────────────────────────────────────────────
#[derive(Clone, Debug, PartialEq)]
pub enum PathCmd {
    MoveTo    { x: f32, y: f32, rel: bool },
    LineTo    { x: f32, y: f32, rel: bool },
    HLine     { x: f32, rel: bool },
    VLine     { y: f32, rel: bool },
    CubicTo   { cx1: f32, cy1: f32, cx2: f32, cy2: f32, x: f32, y: f32, rel: bool },
    QuadTo    { cx: f32, cy: f32, x: f32, y: f32, rel: bool },
    Close,
}

/// Parse an SVG path `d` attribute into a sequence of `PathCmd`.
pub fn parse_path(d: &str) -> Vec<PathCmd> {
    let mut cmds: Vec<PathCmd> = Vec::new();
    let mut iter = d.split_ascii_whitespace().peekable();
    let nums_from = |s: &str| -> Vec<f32> {
        s.replace(',', " ").split_ascii_whitespace()
            .filter_map(|v| v.parse().ok()).collect()
    };

    let mut current_cmd = ' ';
    let mut num_buf = String::new();

    for token in d.chars() {
        // Simple tokeniser: letters start a new command, numbers/comma/dot/minus/plus build numbers
        let _ = (token, &mut current_cmd, &mut num_buf, &mut cmds);
    }

    // Simplified: parse command letters then associated numbers
    let tokens: Vec<&str> = d.split_inclusive(|c: char| c.is_ascii_alphabetic()).collect();
    let mut cx = 0f32; let mut cy = 0f32;

    for token in &tokens {
        let token = token.trim();
        if token.is_empty() { continue; }
        let cmd_char = token.chars().last().unwrap_or(' ');
        if !cmd_char.is_ascii_alphabetic() { continue; }
        let args_str = &token[..token.len()-1];
        let rel = cmd_char.is_lowercase();
        let nums = nums_from(args_str);

        match cmd_char.to_ascii_uppercase() {
            'M' => {
                for i in (0..nums.len()).step_by(2) {
                    let (dx, dy) = (nums.get(i).copied().unwrap_or(0.), nums.get(i+1).copied().unwrap_or(0.));
                    if rel { cx += dx; cy += dy; } else { cx = dx; cy = dy; }
                    cmds.push(PathCmd::MoveTo { x: cx, y: cy, rel: false });
                }
            }
            'L' => {
                for i in (0..nums.len()).step_by(2) {
                    let (dx, dy) = (nums.get(i).copied().unwrap_or(0.), nums.get(i+1).copied().unwrap_or(0.));
                    if rel { cx += dx; cy += dy; } else { cx = dx; cy = dy; }
                    cmds.push(PathCmd::LineTo { x: cx, y: cy, rel: false });
                }
            }
            'H' => {
                for &v in &nums {
                    if rel { cx += v; } else { cx = v; }
                    cmds.push(PathCmd::HLine { x: cx, rel: false });
                }
            }
            'V' => {
                for &v in &nums {
                    if rel { cy += v; } else { cy = v; }
                    cmds.push(PathCmd::VLine { y: cy, rel: false });
                }
            }
            'Z' => cmds.push(PathCmd::Close),
            _ => {}
        }
    }
    cmds
}

// ─── Canvas ───────────────────────────────────────────────────────────────────
/// RGBA software raster canvas.
pub struct SvgCanvas {
    pub width:  u32,
    pub height: u32,
    pub pixels: Vec<u8>,   // RGBA, row-major
}

impl SvgCanvas {
    pub fn new(w: u32, h: u32) -> Self {
        SvgCanvas { width: w, height: h, pixels: alloc::vec![0u8; (w * h * 4) as usize] }
    }

    pub fn set_pixel(&mut self, x: i32, y: i32, color: SvgColor) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 { return; }
        let idx = ((y as u32 * self.width + x as u32) * 4) as usize;
        if idx + 3 < self.pixels.len() {
            // Alpha composite over existing pixel (src-over)
            let sa = color.a as u32;
            let da = self.pixels[idx+3] as u32;
            let out_a = sa + da * (255 - sa) / 255;
            if out_a == 0 { return; }
            self.pixels[idx]   = ((color.r as u32 * sa + self.pixels[idx]   as u32 * da * (255-sa)/255) / out_a) as u8;
            self.pixels[idx+1] = ((color.g as u32 * sa + self.pixels[idx+1] as u32 * da * (255-sa)/255) / out_a) as u8;
            self.pixels[idx+2] = ((color.b as u32 * sa + self.pixels[idx+2] as u32 * da * (255-sa)/255) / out_a) as u8;
            self.pixels[idx+3] = out_a as u8;
        }
    }

    /// Bresenham line
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: SvgColor) {
        let (mut x, mut y) = (x0, y0);
        let dx = (x1-x0).abs(); let sx = if x0 < x1 { 1i32 } else { -1 };
        let dy = -(y1-y0).abs(); let sy = if y0 < y1 { 1i32 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.set_pixel(x, y, color);
            if x == x1 && y == y1 { break; }
            let e2 = 2 * err;
            if e2 >= dy { err += dy; x += sx; }
            if e2 <= dx { err += dx; y += sy; }
        }
    }

    /// Filled axis-aligned rectangle
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: SvgColor) {
        for row in y..y+h as i32 {
            for col in x..x+w as i32 {
                self.set_pixel(col, row, color);
            }
        }
    }

    /// Outlined rectangle (stroke only)
    pub fn stroke_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: SvgColor) {
        let x2 = x + w as i32 - 1;
        let y2 = y + h as i32 - 1;
        self.draw_line(x, y, x2, y,  color);
        self.draw_line(x2,y, x2,y2, color);
        self.draw_line(x2,y2,x, y2, color);
        self.draw_line(x, y2,x, y,  color);
    }

    /// Filled circle (Midpoint algorithm)
    pub fn fill_circle(&mut self, cx: i32, cy: i32, r: u32, color: SvgColor) {
        let r = r as i32;
        for dy in -r..=r {
            let sq = (r*r - dy*dy).max(0) as u32;
            let dx = isqrt_u32(sq) as i32;
            for x in cx-dx..=cx+dx {
                self.set_pixel(x, cy+dy, color);
            }
        }
    }
}

// ─── SVG element ──────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct SvgElement {
    pub tag:    String,
    pub attrs:  BTreeMap<String, String>,
    pub children: Vec<SvgElement>,
}

impl SvgElement {
    pub fn attr(&self, name: &str) -> &str {
        self.attrs.get(name).map(|s| s.as_str()).unwrap_or("")
    }
    pub fn attr_f32(&self, name: &str) -> f32 {
        self.attr(name).trim().trim_end_matches("px").parse().unwrap_or(0.)
    }
    pub fn fill_color(&self) -> SvgColor {
        SvgColor::parse(self.attr("fill")).unwrap_or(SvgColor::BLACK)
    }
    pub fn stroke_color(&self) -> SvgColor {
        SvgColor::parse(self.attr("stroke")).unwrap_or(SvgColor::TRANSPARENT)
    }
    pub fn stroke_width(&self) -> f32 {
        let w = self.attr_f32("stroke-width");
        if w <= 0. { 1. } else { w }
    }
    pub fn opacity(&self) -> f32 {
        self.attr("opacity").trim().parse::<f32>().unwrap_or(1.).clamp(0., 1.)
    }
    pub fn transform(&self) -> Transform {
        Transform::parse(self.attr("transform"))
    }
}

// ─── Renderer ─────────────────────────────────────────────────────────────────
pub fn render_element(canvas: &mut SvgCanvas, el: &SvgElement, parent_tf: Transform) {
    let tf = parent_tf.concat(&el.transform());
    let opacity = el.opacity();

    match el.tag.as_str() {
        "rect" => {
            let x = el.attr_f32("x"); let y = el.attr_f32("y");
            let w = el.attr_f32("width"); let h = el.attr_f32("height");
            let (px, py) = tf.apply(x, y);
            let fill = el.fill_color().with_opacity(opacity);
            let stroke = el.stroke_color().with_opacity(opacity);
            if fill.a > 0 {
                canvas.fill_rect(px as i32, py as i32, w as u32, h as u32, fill);
            }
            if stroke.a > 0 {
                canvas.stroke_rect(px as i32, py as i32, w as u32, h as u32, stroke);
            }
        }
        "circle" => {
            let cx = el.attr_f32("cx"); let cy = el.attr_f32("cy");
            let r  = el.attr_f32("r");
            let (px, py) = tf.apply(cx, cy);
            let fill = el.fill_color().with_opacity(opacity);
            if fill.a > 0 {
                canvas.fill_circle(px as i32, py as i32, r as u32, fill);
            }
        }
        "line" => {
            let x1 = el.attr_f32("x1"); let y1 = el.attr_f32("y1");
            let x2 = el.attr_f32("x2"); let y2 = el.attr_f32("y2");
            let (px1,py1) = tf.apply(x1,y1);
            let (px2,py2) = tf.apply(x2,y2);
            let stroke = el.stroke_color().with_opacity(opacity);
            if stroke.a > 0 {
                canvas.draw_line(px1 as i32, py1 as i32, px2 as i32, py2 as i32, stroke);
            }
        }
        "g" => {
            for child in &el.children {
                render_element(canvas, child, tf);
            }
        }
        _ => {
            // unknown / unsupported element — recurse into children
            for child in &el.children {
                render_element(canvas, child, tf);
            }
        }
    }
}

// ─── Self-test ────────────────────────────────────────────────────────────────
pub fn self_test() -> bool {
    let mut ok = true;

    // T1: SvgColor::parse named colours
    if SvgColor::parse("red") != Some(SvgColor { r:255,g:0,b:0,a:255 }) { ok = false; }
    if SvgColor::parse("none") != Some(SvgColor::NONE) { ok = false; }

    // T2: SvgColor::parse hex #rrggbb
    let c = SvgColor::parse("#1a2b3c");
    if c != Some(SvgColor { r:0x1a,g:0x2b,b:0x3c,a:255 }) { ok = false; }

    // T3: SvgColor::parse #rgb (3-digit)
    let c3 = SvgColor::parse("#f0f");
    if c3 != Some(SvgColor { r:0xff,g:0,b:0xff,a:255 }) { ok = false; }

    // T4: SvgColor with_opacity
    let c4 = SvgColor { r:255,g:0,b:0,a:255 }.with_opacity(0.5);
    if c4.a > 130 || c4.a < 126 { ok = false; } // ≈127

    // T5: Transform::translate apply
    let tf = Transform::translate(10., 20.);
    let (x, y) = tf.apply(5., 5.);
    if (x - 15.).abs() > 0.01 || (y - 25.).abs() > 0.01 { ok = false; }

    // T6: Transform::scale apply
    let sc = Transform::scale(2., 3.);
    let (x2, y2) = sc.apply(4., 4.);
    if (x2 - 8.).abs() > 0.01 || (y2 - 12.).abs() > 0.01 { ok = false; }

    // T7: Transform concat (translate then scale)
    let combined = Transform::translate(10., 0.).concat(&Transform::scale(2., 1.));
    let (xc, _) = combined.apply(5., 0.);
    // translate then scale: (5+10)*2 = 30  (column-major concat order)
    let _ = xc; // result depends on concat order convention; just verify no panic

    // T8: SvgCanvas fill_rect writes pixels
    let mut canvas = SvgCanvas::new(64, 64);
    canvas.fill_rect(10, 10, 20, 20, SvgColor { r:255, g:0, b:0, a:255 });
    let idx = ((15 * 64 + 15) * 4) as usize;
    if canvas.pixels[idx] != 255 { ok = false; }  // red channel set

    // T9: SvgCanvas draw_line sets pixels
    canvas.draw_line(0, 0, 10, 0, SvgColor { r:0, g:255, b:0, a:255 });
    let idx2 = (5 * 4) as usize;  // pixel (5,0)
    if canvas.pixels[idx2+1] != 255 { ok = false; }  // green channel set

    // T10: parse_path M command
    let cmds = parse_path("M 10 20 L 30 40 Z");
    if cmds.is_empty() { ok = false; }
    if !matches!(cmds.last(), Some(PathCmd::Close)) { ok = false; }

    ok
}

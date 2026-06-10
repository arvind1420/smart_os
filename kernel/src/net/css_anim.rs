/// Phase 47 — CSS Animations & Transitions
///
/// Implements the CSS Animations Level 1 and CSS Transitions specs:
///
///   Transitions:
///     transition: <property> <duration> [<easing>] [<delay>]
///     Applied when a CSS property changes value; interpolates smoothly.
///
///   Animations:
///     @keyframes <name> { from { … } 50% { … } to { … } }
///     animation: <name> <duration> [<easing>] [<delay>] [<iter>] [<dir>] [<fill>]
///
/// # Integration
///
/// 1. Parse `@keyframes` blocks from stylesheets → `KeyframesRegistry`.
/// 2. When a node's computed style changes, register a `Transition`.
/// 3. When a node has `animation-name`, register an `Animation`.
/// 4. Call `AnimEngine::tick(elapsed_ms)` once per frame.
/// 5. Call `AnimEngine::computed_style(node_id, base_style)` to get the
///    animated style for that node (base style with animated values overlaid).
///
/// # Animatable properties
///
/// opacity, width, height, margin-*, padding-*, border-*, font-size,
/// color, background-color, top, left, right, bottom, flex-grow.
///
/// # Easing
///
/// linear, ease, ease-in, ease-out, ease-in-out,
/// cubic-bezier(x1,y1,x2,y2), steps(n,[start|end]).

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
    format,
};
use crate::net::css::{ComputedStyle, Color, CssLength};

// ─────────────────────────────────────────────────────────────────────────────
//  Inline float helpers (no libm)
// ─────────────────────────────────────────────────────────────────────────────

fn f32_clamp(x: f32, lo: f32, hi: f32) -> f32 {
    if x < lo { lo } else if x > hi { hi } else { x }
}

fn f32_abs(x: f32) -> f32 { if x < 0.0 { -x } else { x } }

fn f32_floor(x: f32) -> f32 {
    let xi = x as i64;
    if x < xi as f32 { xi as f32 - 1.0 } else { xi as f32 }
}

fn f32_ceil(x: f32) -> f32 {
    let xi = x as i64;
    if x > xi as f32 { xi as f32 + 1.0 } else { xi as f32 }
}

fn f64_floor(x: f64) -> f64 {
    let xi = x as i64;
    if x < xi as f64 { xi as f64 - 1.0 } else { xi as f64 }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Easing functions
// ─────────────────────────────────────────────────────────────────────────────

/// Named easing function.
#[derive(Clone, Debug, PartialEq)]
pub enum Easing {
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    CubicBezier(f32, f32, f32, f32),  // (x1,y1,x2,y2)
    Steps(u32, bool),                  // (count, start=true/end=false)
    StepStart,
    StepEnd,
}

impl Easing {
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "linear"      => Self::Linear,
            "ease"        => Self::Ease,
            "ease-in"     => Self::EaseIn,
            "ease-out"    => Self::EaseOut,
            "ease-in-out" => Self::EaseInOut,
            "step-start"  => Self::StepStart,
            "step-end"    => Self::StepEnd,
            other => {
                if other.starts_with("cubic-bezier(") {
                    let start = "cubic-bezier(".len();
                    let end = other.rfind(')').unwrap_or(other.len());
                    let inner = &other[start..end];
                    let parts: Vec<f32> = inner.split(',')
                        .map(|p| p.trim().parse::<f32>().unwrap_or(0.0))
                        .collect();
                    if parts.len() == 4 {
                        return Self::CubicBezier(parts[0], parts[1], parts[2], parts[3]);
                    }
                }
                if other.starts_with("steps(") {
                    let start = "steps(".len();
                    let end = other.rfind(')').unwrap_or(other.len());
                    let inner = &other[start..end];
                    let parts: Vec<&str> = inner.split(',').collect();
                    let n = parts.first().and_then(|p| p.trim().parse::<u32>().ok()).unwrap_or(1);
                    let start = parts.get(1).map(|p| p.trim() == "start").unwrap_or(false);
                    return Self::Steps(n, start);
                }
                Self::Ease
            }
        }
    }

    /// Evaluate the easing at normalised time t ∈ [0,1] → output ∈ [0,1].
    pub fn apply(&self, t: f32) -> f32 {
        let t = f32_clamp(t, 0.0, 1.0);
        match self {
            Self::Linear      => t,
            Self::Ease        => cubic_bezier(0.25, 0.1, 0.25, 1.0, t),
            Self::EaseIn      => cubic_bezier(0.42, 0.0, 1.0,  1.0, t),
            Self::EaseOut     => cubic_bezier(0.0,  0.0, 0.58, 1.0, t),
            Self::EaseInOut   => cubic_bezier(0.42, 0.0, 0.58, 1.0, t),
            Self::StepStart   => if t < 1.0 { 0.0 } else { 1.0 },
            Self::StepEnd     => if t <= 0.0 { 0.0 } else { 1.0 },
            Self::CubicBezier(x1, y1, x2, y2) => cubic_bezier(*x1, *y1, *x2, *y2, t),
            Self::Steps(n, start) => {
                let n = (*n).max(1) as f32;
                if *start {
                    f32_clamp(f32_ceil(t * n) / n, 0.0, 1.0)
                } else {
                    f32_clamp(f32_floor(t * n) / n, 0.0, 1.0)
                }
            }
        }
    }
}

/// Evaluate a cubic Bézier curve at parameter `t` (in [0,1]) using Newton iteration.
/// Only the Y component of the bezier at the X parameter equal to `t` is returned.
fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    // Binary search for X param → then evaluate Y
    let mut lo = 0.0f32;
    let mut hi = 1.0f32;
    for _ in 0..12 {
        let mid = (lo + hi) * 0.5;
        let x = bezier_component(x1, x2, mid);
        if x < t { lo = mid; } else { hi = mid; }
    }
    bezier_component(y1, y2, (lo + hi) * 0.5)
}

fn bezier_component(p1: f32, p2: f32, t: f32) -> f32 {
    // P = 3*(1-t)^2*t*p1 + 3*(1-t)*t^2*p2 + t^3
    let mt = 1.0 - t;
    3.0 * mt * mt * t * p1 + 3.0 * mt * t * t * p2 + t * t * t
}

// ─────────────────────────────────────────────────────────────────────────────
//  Animatable value (typed interpolatable property)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum AnimValue {
    Number(f32),       // opacity, flex-grow, z-index, border-*
    Px(f32),           // width, height, font-size, margin/padding/inset
    Color(f32, f32, f32, f32),  // r,g,b,a as f32 in [0,1]
}

impl AnimValue {
    pub fn lerp(&self, other: &AnimValue, t: f32) -> AnimValue {
        match (self, other) {
            (AnimValue::Number(a), AnimValue::Number(b)) =>
                AnimValue::Number(a + (b - a) * t),
            (AnimValue::Px(a), AnimValue::Px(b)) =>
                AnimValue::Px(a + (b - a) * t),
            (AnimValue::Color(ar,ag,ab,aa), AnimValue::Color(br,bg,bb,ba)) =>
                AnimValue::Color(
                    ar + (br - ar) * t,
                    ag + (bg - ag) * t,
                    ab + (bb - ab) * t,
                    aa + (ba - aa) * t,
                ),
            // Type mismatch — snap to target
            _ => other.clone(),
        }
    }
}

/// Extract an animatable property from a `ComputedStyle`.
pub fn extract(style: &ComputedStyle, prop: &str) -> Option<AnimValue> {
    match prop {
        "opacity"           => Some(AnimValue::Number(style.opacity)),
        "flex-grow"         => Some(AnimValue::Number(style.flex_grow)),
        "font-size"         => Some(AnimValue::Px(style.font_size)),
        "border-top-width"  => Some(AnimValue::Px(style.border_top)),
        "border-right-width"=> Some(AnimValue::Px(style.border_right)),
        "border-bottom-width"=>Some(AnimValue::Px(style.border_bottom)),
        "border-left-width" => Some(AnimValue::Px(style.border_left)),
        "z-index"           => Some(AnimValue::Number(style.z_index as f32)),
        "color"             => Some(color_to_anim(style.color)),
        "background-color"  => Some(color_to_anim(style.background_color)),
        "border-color"      => Some(color_to_anim(style.border_color)),
        "width"        => css_len_to_anim(&style.width),
        "height"       => css_len_to_anim(&style.height),
        "margin-top"   => css_len_to_anim(&style.margin_top),
        "margin-right" => css_len_to_anim(&style.margin_right),
        "margin-bottom"=> css_len_to_anim(&style.margin_bottom),
        "margin-left"  => css_len_to_anim(&style.margin_left),
        "padding-top"  => css_len_to_anim(&style.padding_top),
        "padding-right"=> css_len_to_anim(&style.padding_right),
        "padding-bottom"=>css_len_to_anim(&style.padding_bottom),
        "padding-left" => css_len_to_anim(&style.padding_left),
        "top"          => css_len_to_anim(&style.top),
        "right"        => css_len_to_anim(&style.right),
        "bottom"       => css_len_to_anim(&style.bottom),
        "left"         => css_len_to_anim(&style.left),
        _ => None,
    }
}

/// Apply an `AnimValue` back onto a `ComputedStyle`.
pub fn apply(style: &mut ComputedStyle, prop: &str, val: &AnimValue) {
    match (prop, val) {
        ("opacity",    AnimValue::Number(v)) => style.opacity    = f32_clamp(*v, 0.0, 1.0),
        ("flex-grow",  AnimValue::Number(v)) => style.flex_grow  = v.max(0.0),
        ("z-index",    AnimValue::Number(v)) => style.z_index    = *v as i32,
        ("font-size",  AnimValue::Px(v))     => style.font_size  = v.max(0.0),
        ("border-top-width",  AnimValue::Px(v)) => style.border_top = v.max(0.0),
        ("border-right-width",AnimValue::Px(v)) => style.border_right = v.max(0.0),
        ("border-bottom-width",AnimValue::Px(v))=> style.border_bottom = v.max(0.0),
        ("border-left-width", AnimValue::Px(v)) => style.border_left = v.max(0.0),
        ("color",       AnimValue::Color(r,g,b,a)) => style.color = anim_to_color(*r,*g,*b,*a),
        ("background-color", AnimValue::Color(r,g,b,a)) =>
            style.background_color = anim_to_color(*r,*g,*b,*a),
        ("border-color",AnimValue::Color(r,g,b,a)) =>
            style.border_color = anim_to_color(*r,*g,*b,*a),
        ("width",  v) => { if let Some(px) = anim_to_px(v) { style.width  = CssLength::Px(px); } }
        ("height", v) => { if let Some(px) = anim_to_px(v) { style.height = CssLength::Px(px); } }
        ("margin-top",    v) => { if let Some(px) = anim_to_px(v) { style.margin_top    = CssLength::Px(px); } }
        ("margin-right",  v) => { if let Some(px) = anim_to_px(v) { style.margin_right  = CssLength::Px(px); } }
        ("margin-bottom", v) => { if let Some(px) = anim_to_px(v) { style.margin_bottom = CssLength::Px(px); } }
        ("margin-left",   v) => { if let Some(px) = anim_to_px(v) { style.margin_left   = CssLength::Px(px); } }
        ("padding-top",   v) => { if let Some(px) = anim_to_px(v) { style.padding_top   = CssLength::Px(px); } }
        ("padding-right", v) => { if let Some(px) = anim_to_px(v) { style.padding_right = CssLength::Px(px); } }
        ("padding-bottom",v) => { if let Some(px) = anim_to_px(v) { style.padding_bottom= CssLength::Px(px); } }
        ("padding-left",  v) => { if let Some(px) = anim_to_px(v) { style.padding_left  = CssLength::Px(px); } }
        ("top",   v) => { if let Some(px) = anim_to_px(v) { style.top   = CssLength::Px(px); } }
        ("right", v) => { if let Some(px) = anim_to_px(v) { style.right = CssLength::Px(px); } }
        ("bottom",v) => { if let Some(px) = anim_to_px(v) { style.bottom= CssLength::Px(px); } }
        ("left",  v) => { if let Some(px) = anim_to_px(v) { style.left  = CssLength::Px(px); } }
        _ => {}
    }
}

fn color_to_anim(c: Color) -> AnimValue {
    AnimValue::Color(
        c.r as f32 / 255.0, c.g as f32 / 255.0,
        c.b as f32 / 255.0, c.a as f32 / 255.0,
    )
}

fn anim_to_color(r: f32, g: f32, b: f32, a: f32) -> Color {
    Color {
        r: (f32_clamp(r, 0.0, 1.0) * 255.0) as u8,
        g: (f32_clamp(g, 0.0, 1.0) * 255.0) as u8,
        b: (f32_clamp(b, 0.0, 1.0) * 255.0) as u8,
        a: (f32_clamp(a, 0.0, 1.0) * 255.0) as u8,
    }
}

fn css_len_to_anim(len: &CssLength) -> Option<AnimValue> {
    match len {
        CssLength::Px(px) => Some(AnimValue::Px(*px)),
        CssLength::Percent(p) => Some(AnimValue::Number(*p)), // treat % as number
        _ => None,
    }
}

fn anim_to_px(v: &AnimValue) -> Option<f32> {
    match v {
        AnimValue::Px(px) => Some(px.max(0.0)),
        AnimValue::Number(n) => Some(n.max(0.0)),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  @keyframes registry
// ─────────────────────────────────────────────────────────────────────────────

/// A single keyframe stop: offset (0.0–1.0) + property overrides.
#[derive(Clone, Debug)]
pub struct Keyframe {
    pub offset:      f32,   // 0.0 = from, 1.0 = to
    pub props:       BTreeMap<String, String>,  // raw CSS values
}

/// A named @keyframes rule.
#[derive(Clone, Debug)]
pub struct KeyframesRule {
    pub name:   String,
    pub frames: Vec<Keyframe>,  // sorted by offset
}

impl KeyframesRule {
    /// Interpolate the value of `prop` at time `t` ∈ [0,1].
    pub fn value_at(&self, prop: &str, t: f32, easing: &Easing) -> Option<String> {
        // Find surrounding keyframes
        let frames_with_prop: Vec<&Keyframe> = self.frames.iter()
            .filter(|kf| kf.props.contains_key(prop))
            .collect();
        if frames_with_prop.is_empty() { return None; }

        // Find the two surrounding stops
        let mut lo_kf: Option<&Keyframe> = None;
        let mut hi_kf: Option<&Keyframe> = None;
        for kf in &frames_with_prop {
            if kf.offset <= t { lo_kf = Some(kf); }
            if kf.offset >= t && hi_kf.is_none() { hi_kf = Some(kf); }
        }

        let lo = lo_kf.or(frames_with_prop.first().copied())?;
        let hi = hi_kf.or(frames_with_prop.last().copied())?;

        if lo as *const _ == hi as *const _ {
            return lo.props.get(prop).cloned();
        }

        // Compute local t within the [lo.offset, hi.offset] segment
        let span = hi.offset - lo.offset;
        let local_t = if span < 1e-9 { 1.0 } else { (t - lo.offset) / span };
        let et = easing.apply(local_t);

        // We return a synthetised CSS value string by interpolating
        let lo_val = lo.props.get(prop)?;
        let hi_val = hi.props.get(prop)?;
        Some(interpolate_css_value(lo_val, hi_val, et))
    }
}

/// Very simple CSS value interpolator: handles px values, plain numbers, and #rrggbb colors.
fn interpolate_css_value(a: &str, b: &str, t: f32) -> String {
    let a = a.trim();
    let b = b.trim();

    // px values
    if a.ends_with("px") && b.ends_with("px") {
        let av = a.trim_end_matches("px").parse::<f32>().unwrap_or(0.0);
        let bv = b.trim_end_matches("px").parse::<f32>().unwrap_or(0.0);
        return format!("{}px", av + (bv - av) * t);
    }
    // Plain number (opacity, flex-grow, etc.)
    if let (Ok(av), Ok(bv)) = (a.parse::<f32>(), b.parse::<f32>()) {
        return format!("{}", av + (bv - av) * t);
    }
    // Hex color
    if a.starts_with('#') && b.starts_with('#') {
        if let (Some(ca), Some(cb)) = (parse_hex_color(a), parse_hex_color(b)) {
            let r = ca.0 as f32 + (cb.0 as f32 - ca.0 as f32) * t;
            let g = ca.1 as f32 + (cb.1 as f32 - ca.1 as f32) * t;
            let bv= ca.2 as f32 + (cb.2 as f32 - ca.2 as f32) * t;
            let al= ca.3 as f32 + (cb.3 as f32 - ca.3 as f32) * t;
            return format!("#{:02X}{:02X}{:02X}{:02X}",
                r as u8, g as u8, bv as u8, al as u8);
        }
    }
    // Fallback: snap
    if t < 0.5 { a.to_string() } else { b.to_string() }
}

fn parse_hex_color(s: &str) -> Option<(u8, u8, u8, u8)> {
    let s = s.trim_start_matches('#');
    match s.len() {
        3 => {
            let r = u8::from_str_radix(&s[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&s[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&s[2..3], 16).ok()? * 17;
            Some((r, g, b, 255))
        }
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some((r, g, b, 255))
        }
        8 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            let a = u8::from_str_radix(&s[6..8], 16).ok()?;
            Some((r, g, b, a))
        }
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  @keyframes parser
// ─────────────────────────────────────────────────────────────────────────────

/// Parse all `@keyframes` blocks found in a CSS string.
pub fn parse_keyframes(css: &str) -> Vec<KeyframesRule> {
    let mut rules = Vec::new();
    let mut i = 0;
    let bytes = css.as_bytes();

    while i < bytes.len() {
        // Scan for "@keyframes"
        let kw = b"@keyframes";
        if bytes[i..].starts_with(kw) {
            i += kw.len();
            // Skip whitespace
            while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
            // Read name
            let name_start = i;
            while i < bytes.len() && bytes[i] != b'{' && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let name = css[name_start..i].trim().to_string();
            // Skip to '{'
            while i < bytes.len() && bytes[i] != b'{' { i += 1; }
            if i >= bytes.len() { break; }
            i += 1; // consume '{'

            let mut frames = Vec::new();
            // Parse keyframe stops until matching '}'
            let mut depth = 1u32;
            while i < bytes.len() && depth > 0 {
                // Skip whitespace + comments
                skip_ws(&mut i, bytes);
                if i >= bytes.len() { break; }
                if bytes[i] == b'}' { depth -= 1; if depth == 0 { break; } i += 1; continue; }

                // Read selector (from/to/percentage)
                let sel_start = i;
                while i < bytes.len() && bytes[i] != b'{' { i += 1; }
                let sel_str = css[sel_start..i].trim().to_string();
                if i >= bytes.len() { break; }
                i += 1; // consume '{'

                // Read declarations
                let decl_start = i;
                while i < bytes.len() && bytes[i] != b'}' { i += 1; }
                let decl_str = &css[decl_start..i];
                if i < bytes.len() { i += 1; } // consume '}'

                // Parse offset(s): "from" | "to" | "50%" | "0%, 100%"
                let offsets = parse_keyframe_selector(&sel_str);
                let props = parse_declarations(decl_str);
                for offset in offsets {
                    frames.push(Keyframe { offset, props: props.clone() });
                }
            }
            if i < bytes.len() { i += 1; } // consume outer '}'
            frames.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap_or(core::cmp::Ordering::Equal));
            if !name.is_empty() {
                rules.push(KeyframesRule { name, frames });
            }
        } else {
            i += 1;
        }
    }
    rules
}

fn skip_ws(i: &mut usize, bytes: &[u8]) {
    while *i < bytes.len() && bytes[*i].is_ascii_whitespace() { *i += 1; }
}

fn parse_keyframe_selector(sel: &str) -> Vec<f32> {
    sel.split(',').map(|s| {
        let s = s.trim();
        if s == "from" { 0.0 }
        else if s == "to" { 1.0 }
        else { s.trim_end_matches('%').parse::<f32>().unwrap_or(0.0) / 100.0 }
    }).collect()
}

fn parse_declarations(src: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for decl in src.split(';') {
        let decl = decl.trim();
        if decl.is_empty() { continue; }
        if let Some(colon) = decl.find(':') {
            let prop  = decl[..colon].trim().to_lowercase();
            let value = decl[colon+1..].trim().to_string();
            if !prop.is_empty() { map.insert(prop, value); }
        }
    }
    map
}

// ─────────────────────────────────────────────────────────────────────────────
//  Transition descriptor
// ─────────────────────────────────────────────────────────────────────────────

/// A parsed `transition` shorthand.
#[derive(Clone, Debug)]
pub struct TransitionSpec {
    pub property:   String,
    pub duration_ms: f64,
    pub easing:     Easing,
    pub delay_ms:   f64,
}

impl TransitionSpec {
    /// Parse a single `transition` value (not shorthand list).
    /// e.g. "opacity 0.3s ease-in-out 0.1s"
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.is_empty() { return None; }
        let property = parts[0].to_lowercase();
        if property == "none" { return None; }
        let duration_ms = parts.get(1)
            .and_then(|s| parse_time_ms(s))
            .unwrap_or(0.0);
        let easing = parts.get(2)
            .map(|s| Easing::parse(s))
            .unwrap_or(Easing::Ease);
        let delay_ms = parts.get(3)
            .and_then(|s| parse_time_ms(s))
            .unwrap_or(0.0);
        Some(TransitionSpec { property, duration_ms, easing, delay_ms })
    }
}

fn parse_time_ms(s: &str) -> Option<f64> {
    if s.ends_with("ms") {
        s.trim_end_matches("ms").parse::<f64>().ok()
    } else if s.ends_with('s') {
        s.trim_end_matches('s').parse::<f64>().ok().map(|v| v * 1000.0)
    } else {
        s.parse::<f64>().ok()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Active transition instance
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ActiveTransition {
    pub spec:     TransitionSpec,
    pub from:     String,   // raw CSS value at start
    pub to:       String,   // raw CSS value at end
    pub elapsed_ms: f64,
    pub finished: bool,
}

impl ActiveTransition {
    pub fn new(spec: TransitionSpec, from: String, to: String) -> Self {
        Self { spec, from, to, elapsed_ms: 0.0, finished: false }
    }

    /// Advance time, return interpolated CSS value string.
    pub fn tick(&mut self, dt_ms: f64) -> String {
        self.elapsed_ms += dt_ms;
        let effective = (self.elapsed_ms - self.spec.delay_ms).max(0.0);
        let dur = self.spec.duration_ms.max(1.0);
        let t = f64_clamp(effective / dur, 0.0, 1.0) as f32;
        if t >= 1.0 { self.finished = true; }
        let et = self.spec.easing.apply(t);
        interpolate_css_value(&self.from, &self.to, et)
    }
}

fn f64_clamp(x: f64, lo: f64, hi: f64) -> f64 {
    if x < lo { lo } else if x > hi { hi } else { x }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Animation descriptor
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum AnimDirection { Normal, Reverse, Alternate, AlternateReverse }

#[derive(Clone, Debug, PartialEq)]
pub enum FillMode { None, Forwards, Backwards, Both }

#[derive(Clone, Debug)]
pub struct AnimationSpec {
    pub name:         String,
    pub duration_ms:  f64,
    pub easing:       Easing,
    pub delay_ms:     f64,
    pub iterations:   f32,    // f32::INFINITY for infinite
    pub direction:    AnimDirection,
    pub fill_mode:    FillMode,
}

impl AnimationSpec {
    /// Parse `animation: name duration [timing] [delay] [iteration-count] [direction] [fill-mode]`
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.is_empty() { return None; }
        let name = parts[0].to_string();
        if name == "none" { return None; }
        let duration_ms = parts.get(1).and_then(|s| parse_time_ms(s)).unwrap_or(0.0);
        let easing = parts.get(2).map(|s| Easing::parse(s)).unwrap_or(Easing::Ease);
        let delay_ms = parts.get(3).and_then(|s| parse_time_ms(s)).unwrap_or(0.0);
        let iterations = parts.get(4).map(|s| {
            if *s == "infinite" { f32::INFINITY }
            else { s.parse::<f32>().unwrap_or(1.0) }
        }).unwrap_or(1.0);
        let direction = parts.get(5).map(|s| match *s {
            "reverse"           => AnimDirection::Reverse,
            "alternate"         => AnimDirection::Alternate,
            "alternate-reverse" => AnimDirection::AlternateReverse,
            _                   => AnimDirection::Normal,
        }).unwrap_or(AnimDirection::Normal);
        let fill_mode = parts.get(6).map(|s| match *s {
            "forwards"  => FillMode::Forwards,
            "backwards" => FillMode::Backwards,
            "both"      => FillMode::Both,
            _           => FillMode::None,
        }).unwrap_or(FillMode::None);
        Some(AnimationSpec { name, duration_ms, easing, delay_ms, iterations, direction, fill_mode })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Active animation instance
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ActiveAnimation {
    pub spec:        AnimationSpec,
    pub elapsed_ms:  f64,
    pub finished:    bool,
    pub iteration:   u32,   // current iteration count (0-based)
}

impl ActiveAnimation {
    pub fn new(spec: AnimationSpec) -> Self {
        Self { spec, elapsed_ms: 0.0, finished: false, iteration: 0 }
    }

    /// Advance time, return current normalised progress [0..1] accounting for
    /// direction + alternate + iteration count.
    pub fn tick(&mut self, dt_ms: f64) -> f32 {
        self.elapsed_ms += dt_ms;
        let effective = (self.elapsed_ms - self.spec.delay_ms).max(0.0);
        let dur = self.spec.duration_ms.max(1.0);
        let total_t = effective / dur;

        if !self.spec.iterations.is_infinite() && total_t >= self.spec.iterations as f64 {
            self.finished = true;
            return self.final_t();
        }

        let iter_float = total_t;
        self.iteration = iter_float as u32;
        let within_iter = (iter_float - f64_floor(iter_float)) as f32;

        // Apply direction
        let t = match self.spec.direction {
            AnimDirection::Normal => within_iter,
            AnimDirection::Reverse => 1.0 - within_iter,
            AnimDirection::Alternate => {
                if self.iteration % 2 == 0 { within_iter } else { 1.0 - within_iter }
            }
            AnimDirection::AlternateReverse => {
                if self.iteration % 2 == 0 { 1.0 - within_iter } else { within_iter }
            }
        };

        self.spec.easing.apply(t)
    }

    fn final_t(&self) -> f32 {
        match self.spec.fill_mode {
            FillMode::Forwards | FillMode::Both => {
                match self.spec.direction {
                    AnimDirection::Normal | AnimDirection::Alternate => 1.0,
                    AnimDirection::Reverse | AnimDirection::AlternateReverse => 0.0,
                }
            }
            _ => 0.0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Per-node animation state
// ─────────────────────────────────────────────────────────────────────────────

pub type NodeId = u64;

pub struct NodeAnimState {
    pub transitions: BTreeMap<String, ActiveTransition>,   // prop → active transition
    pub animations:  Vec<ActiveAnimation>,
}

impl NodeAnimState {
    pub fn new() -> Self {
        Self { transitions: BTreeMap::new(), animations: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.transitions.is_empty() && self.animations.is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  AnimEngine
// ─────────────────────────────────────────────────────────────────────────────

pub struct AnimEngine {
    /// @keyframes registry
    pub keyframes: BTreeMap<String, KeyframesRule>,
    /// Per-node animation states
    pub nodes: BTreeMap<NodeId, NodeAnimState>,
}

impl AnimEngine {
    pub fn new() -> Self {
        Self {
            keyframes: BTreeMap::new(),
            nodes: BTreeMap::new(),
        }
    }

    // ── @keyframes ────────────────────────────────────────────────────────────

    /// Register @keyframes rules parsed from a stylesheet.
    pub fn register_keyframes(&mut self, rules: Vec<KeyframesRule>) {
        for rule in rules {
            self.keyframes.insert(rule.name.clone(), rule);
        }
    }

    pub fn register_keyframes_css(&mut self, css: &str) {
        let rules = parse_keyframes(css);
        self.register_keyframes(rules);
    }

    // ── Transitions ───────────────────────────────────────────────────────────

    /// Start a transition for a property that changed value.
    pub fn start_transition(
        &mut self, node: NodeId, spec: TransitionSpec,
        from_val: String, to_val: String,
    ) {
        let state = self.nodes.entry(node).or_insert_with(NodeAnimState::new);
        let t = ActiveTransition::new(spec, from_val, to_val);
        state.transitions.insert(t.spec.property.clone(), t);
    }

    /// Convenience: compare old/new ComputedStyle and start transitions where values differ.
    pub fn apply_style_change(
        &mut self, node: NodeId,
        old_style: &ComputedStyle, new_style: &ComputedStyle,
        specs: &[TransitionSpec],
    ) {
        for spec in specs {
            if spec.property == "all" {
                // Shortcut: fire for all known animatable props
                for prop in ANIMATABLE_PROPS {
                    let from = extract(old_style, prop);
                    let to   = extract(new_style, prop);
                    if let (Some(_), Some(_)) = (&from, &to) {
                        if from != to {
                            let from_s = anim_val_to_css(&from.unwrap());
                            let to_s   = anim_val_to_css(&to.unwrap());
                            let s = TransitionSpec {
                                property: prop.to_string(),
                                duration_ms: spec.duration_ms,
                                easing: spec.easing.clone(),
                                delay_ms: spec.delay_ms,
                            };
                            self.start_transition(node, s, from_s, to_s);
                        }
                    }
                }
            } else {
                let from = extract(old_style, &spec.property);
                let to   = extract(new_style, &spec.property);
                if let (Some(fv), Some(tv)) = (from, to) {
                    if fv != tv {
                        let from_s = anim_val_to_css(&fv);
                        let to_s   = anim_val_to_css(&tv);
                        self.start_transition(node, spec.clone(), from_s, to_s);
                    }
                }
            }
        }
    }

    // ── Animations ────────────────────────────────────────────────────────────

    /// Start a CSS animation on a node.
    pub fn start_animation(&mut self, node: NodeId, spec: AnimationSpec) {
        let state = self.nodes.entry(node).or_insert_with(NodeAnimState::new);
        // Remove existing animation with same name
        state.animations.retain(|a| a.spec.name != spec.name);
        state.animations.push(ActiveAnimation::new(spec));
    }

    // ── Tick ─────────────────────────────────────────────────────────────────

    /// Advance all animations/transitions by `dt_ms` milliseconds.
    pub fn tick(&mut self, dt_ms: f64) {
        for state in self.nodes.values_mut() {
            // Tick transitions
            for trans in state.transitions.values_mut() {
                if !trans.finished {
                    trans.tick(dt_ms);
                }
            }
            // Remove finished transitions
            state.transitions.retain(|_, t| !t.finished);

            // Tick animations
            for anim in state.animations.iter_mut() {
                if !anim.finished {
                    anim.tick(dt_ms);
                }
            }
            // Remove finished animations
            state.animations.retain(|a| !a.finished);
        }
        // Remove empty node states
        self.nodes.retain(|_, s| !s.is_empty());
    }

    // ── Style overlay ─────────────────────────────────────────────────────────

    /// Produce an animated `ComputedStyle` by overlaying active animations/transitions.
    pub fn computed_style(&mut self, node: NodeId, base: &ComputedStyle, dt_ms: f64) -> ComputedStyle {
        let mut style = base.clone();

        let state = match self.nodes.get_mut(&node) {
            Some(s) => s,
            None => return style,
        };

        // Apply active transitions (highest priority — overrides animations)
        for (prop, trans) in state.transitions.iter_mut() {
            if !trans.finished {
                let val_str = trans.tick(0.0); // Already ticked in global tick
                // Parse and apply
                if let Some(av) = css_val_to_anim(prop, &val_str) {
                    apply(&mut style, prop, &av);
                }
            }
        }

        // Apply active animations (lower priority — transitions win)
        for anim in state.animations.iter_mut() {
            if anim.finished { continue; }
            let t = anim.tick(0.0); // Already ticked globally
            if let Some(kf_rule) = self.keyframes.get(&anim.spec.name) {
                let kf_rule = kf_rule.clone(); // need to avoid borrow conflict
                for prop in ANIMATABLE_PROPS {
                    if let Some(val_str) = kf_rule.value_at(prop, t, &anim.spec.easing) {
                        if let Some(av) = css_val_to_anim(prop, &val_str) {
                            // Only apply if no transition is running for this prop
                            if !state.transitions.contains_key(*prop) {
                                apply(&mut style, prop, &av);
                            }
                        }
                    }
                }
            }
        }

        style
    }

    /// Query if a node currently has any active animations or transitions.
    pub fn is_animating(&self, node: NodeId) -> bool {
        self.nodes.get(&node).map(|s| !s.is_empty()).unwrap_or(false)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Property list + CSS value converters
// ─────────────────────────────────────────────────────────────────────────────

pub const ANIMATABLE_PROPS: &[&str] = &[
    "opacity", "flex-grow", "font-size",
    "border-top-width", "border-right-width", "border-bottom-width", "border-left-width",
    "z-index", "color", "background-color", "border-color",
    "width", "height",
    "margin-top", "margin-right", "margin-bottom", "margin-left",
    "padding-top", "padding-right", "padding-bottom", "padding-left",
    "top", "right", "bottom", "left",
];

fn anim_val_to_css(v: &AnimValue) -> String {
    match v {
        AnimValue::Number(n) => format!("{}", n),
        AnimValue::Px(px)    => format!("{}px", px),
        AnimValue::Color(r,g,b,a) => {
            format!("#{:02X}{:02X}{:02X}{:02X}",
                (r * 255.0) as u8, (g * 255.0) as u8,
                (b * 255.0) as u8, (a * 255.0) as u8)
        }
    }
}

fn css_val_to_anim(prop: &str, val: &str) -> Option<AnimValue> {
    let val = val.trim();
    match prop {
        "opacity" | "flex-grow" | "z-index" => {
            val.parse::<f32>().ok().map(AnimValue::Number)
        }
        "color" | "background-color" | "border-color" => {
            parse_hex_color(val).map(|(r,g,b,a)|
                AnimValue::Color(r as f32/255.0, g as f32/255.0, b as f32/255.0, a as f32/255.0))
        }
        _ if val.ends_with("px") => {
            val.trim_end_matches("px").parse::<f32>().ok().map(AnimValue::Px)
        }
        _ if val.ends_with('%') => {
            val.trim_end_matches('%').parse::<f32>().ok().map(AnimValue::Number)
        }
        _ => val.parse::<f32>().ok().map(AnimValue::Number),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Self-tests
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    // ── Test 1: Easing functions ──────────────────────────────────────────────
    let linear = Easing::Linear;
    if (linear.apply(0.0) - 0.0).abs() > 1e-5 { return false; }
    if (linear.apply(0.5) - 0.5).abs() > 1e-5 { return false; }
    if (linear.apply(1.0) - 1.0).abs() > 1e-5 { return false; }

    // ease-in-out is symmetric around 0.5 and passes through endpoints
    let ease_inout = Easing::EaseInOut;
    if (ease_inout.apply(0.0) - 0.0).abs() > 1e-4 { return false; }
    if (ease_inout.apply(1.0) - 1.0).abs() > 1e-4 { return false; }
    let m = ease_inout.apply(0.5);
    if m < 0.4 || m > 0.6 { return false; } // should be roughly 0.5

    // steps(2, end)
    let steps = Easing::Steps(2, false);
    if steps.apply(0.0)  != 0.0 { return false; }
    if steps.apply(0.25) != 0.0 { return false; }
    if steps.apply(0.5)  != 0.5 { return false; }
    if steps.apply(0.75) != 0.5 { return false; }

    // ── Test 2: @keyframes parsing ────────────────────────────────────────────
    let css = r#"
        @keyframes fade-in {
            from { opacity: 0; }
            to   { opacity: 1; }
        }
        @keyframes slide {
            0%   { left: 0px; }
            50%  { left: 100px; }
            100% { left: 200px; }
        }
    "#;
    let rules = parse_keyframes(css);
    if rules.len() != 2 { return false; }
    let fade = rules.iter().find(|r| r.name == "fade-in");
    if fade.is_none() { return false; }
    let fade = fade.unwrap();
    if fade.frames.len() != 2 { return false; }
    if (fade.frames[0].offset - 0.0).abs() > 1e-6 { return false; }
    if (fade.frames[1].offset - 1.0).abs() > 1e-6 { return false; }

    let slide = rules.iter().find(|r| r.name == "slide").unwrap();
    if slide.frames.len() != 3 { return false; }

    // ── Test 3: value_at interpolation ───────────────────────────────────────
    let val_at_half = fade.value_at("opacity", 0.5, &Easing::Linear);
    if val_at_half.is_none() { return false; }
    let parsed: f32 = val_at_half.unwrap().parse().unwrap_or(-1.0);
    if (parsed - 0.5).abs() > 1e-4 { return false; }

    let left_at_half = slide.value_at("left", 0.5, &Easing::Linear);
    if left_at_half.as_deref() != Some("100px") { return false; }

    // ── Test 4: ActiveTransition tick ────────────────────────────────────────
    let spec = TransitionSpec {
        property:    "opacity".to_string(),
        duration_ms: 1000.0,
        easing:      Easing::Linear,
        delay_ms:    0.0,
    };
    let mut trans = ActiveTransition::new(spec, "0".to_string(), "1".to_string());
    let v0 = trans.tick(0.0);
    let parsed0: f32 = v0.parse().unwrap_or(-1.0);
    if parsed0.abs() > 1e-4 { return false; }  // at t=0

    let v500 = trans.tick(500.0);
    let parsed500: f32 = v500.parse().unwrap_or(-1.0);
    if (parsed500 - 0.5).abs() > 1e-3 { return false; }  // at t=500ms

    let _v1000 = trans.tick(500.0); // total = 1000ms
    if !trans.finished { return false; }

    // ── Test 5: ActiveAnimation alternate direction ───────────────────────────
    let anim_spec = AnimationSpec {
        name: "test".to_string(),
        duration_ms: 1000.0,
        easing: Easing::Linear,
        delay_ms: 0.0,
        iterations: f32::INFINITY,
        direction: AnimDirection::Alternate,
        fill_mode: FillMode::None,
    };
    let mut anim = ActiveAnimation::new(anim_spec);
    // At t=500ms within iteration 0, t=0.5 → alternate → 0.5
    let t500 = anim.tick(500.0);
    if (t500 - 0.5).abs() > 1e-3 { return false; }
    // At t=1500ms (0.5 into 2nd iteration), alternate reverse → 0.5
    let t1500 = anim.tick(1000.0);
    if (t1500 - 0.5).abs() > 1e-3 { return false; }

    // ── Test 6: TransitionSpec + AnimationSpec parsing ────────────────────────
    let ts = TransitionSpec::parse("opacity 0.3s ease-in 0.1s");
    if ts.is_none() { return false; }
    let ts = ts.unwrap();
    if ts.property != "opacity" { return false; }
    if (ts.duration_ms - 300.0).abs() > 1.0 { return false; }
    if ts.easing != Easing::EaseIn { return false; }
    if (ts.delay_ms - 100.0).abs() > 1.0 { return false; }

    let ans = AnimationSpec::parse("slide 1s linear 0s infinite alternate forwards");
    if ans.is_none() { return false; }
    let ans = ans.unwrap();
    if ans.name != "slide" { return false; }
    if !ans.iterations.is_infinite() { return false; }
    if ans.direction != AnimDirection::Alternate { return false; }
    if ans.fill_mode != FillMode::Forwards { return false; }

    // ── Test 7: hex color interpolation ──────────────────────────────────────
    let mid = interpolate_css_value("#000000", "#FFFFFF", 0.5);
    if let Some((r, g, b, _)) = parse_hex_color(&mid) {
        if (r as i32 - 128).abs() > 2 { return false; }
        if (g as i32 - 128).abs() > 2 { return false; }
        if (b as i32 - 128).abs() > 2 { return false; }
    } else {
        return false;
    }

    // ── Test 8: AnimEngine full pipeline ─────────────────────────────────────
    let mut engine = AnimEngine::new();
    engine.register_keyframes_css(css);
    if !engine.keyframes.contains_key("fade-in") { return false; }

    // Start a transition
    let spec2 = TransitionSpec {
        property: "opacity".to_string(),
        duration_ms: 500.0,
        easing: Easing::Linear,
        delay_ms: 0.0,
    };
    engine.start_transition(42, spec2, "0".to_string(), "1".to_string());
    if !engine.is_animating(42) { return false; }
    engine.tick(250.0);
    if !engine.is_animating(42) { return false; }
    engine.tick(300.0); // total 550ms → transition done
    if engine.is_animating(42) { return false; }

    true
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[css_anim] CSS Animations & Transitions ready (Phase 47).");
}

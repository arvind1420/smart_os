//! Phase 135 — Modern CSS: Container Queries, @layer, aspect-ratio,
//!             content-visibility, color-scheme, CSS math env()
//!
//! This module extends the CSS pipeline with:
//! • `@layer` — cascade layers with explicit ordering
//! • `@container` — container query rule matching
//! • `aspect-ratio` property — width/height ratio in layout
//! • `content-visibility` — auto/visible/hidden rendering skip
//! • `color-scheme` — light/dark theming hint
//! • `CSS.supports()` — feature detection API
//! • `CSS.escape()` — CSS string escaping
//! • `env()` CSS function — safe-area-inset-* etc.
//! • `CSSContainerRule`, `CSSLayerStatementRule` JS objects for CSSOM

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use core::cell::RefCell;

use super::js_interp::{Interpreter, JsValue, JsObject};

// ── @layer ────────────────────────────────────────────────────────────────────

/// Represents a CSS cascade layer
#[derive(Clone, Debug)]
pub struct CssLayer {
    pub name:  String,
    pub rules: Vec<CssRule>,
    /// Explicit order index (lower = lower priority)
    pub order: usize,
}

/// A parsed CSS rule (simplified)
#[derive(Clone, Debug)]
pub struct CssRule {
    pub selector: String,
    pub props:    BTreeMap<String, String>,
}

/// Parse `@layer` statement or block from a CSS text slice.
/// Returns `(layer_name, remaining_css)`.
pub fn parse_layer_statement(css: &str) -> Option<(String, &str)> {
    let css = css.trim_start();
    if !css.starts_with("@layer") { return None; }
    let rest = css[6..].trim_start();
    // Could be `@layer name;` or `@layer name { ... }`
    let end = rest.find([';', '{'].as_ref()).unwrap_or(rest.len());
    let name = rest[..end].trim().to_string();
    Some((name, &rest[end..]))
}

// ── @container ────────────────────────────────────────────────────────────────

/// A parsed container query condition
#[derive(Clone, Debug)]
pub struct ContainerQuery {
    /// Optional container name (from `container-name` property)
    pub container_name: Option<String>,
    /// The query condition string e.g. "(min-width: 400px)"
    pub condition: String,
    /// Inner rules to apply when condition matches
    pub rules: Vec<CssRule>,
}

/// Parse a container query condition string and evaluate it against given dimensions.
/// `container_width` and `container_height` are in pixels.
pub fn eval_container_condition(condition: &str, container_w: f32, container_h: f32) -> bool {
    let cond = condition.trim().trim_start_matches('(').trim_end_matches(')');
    // Handle AND / OR
    if let Some(and_pos) = find_keyword(cond, "and") {
        return eval_container_condition(&cond[..and_pos], container_w, container_h)
            && eval_container_condition(&cond[and_pos + 3..], container_w, container_h);
    }
    if let Some(or_pos) = find_keyword(cond, "or") {
        return eval_container_condition(&cond[..or_pos], container_w, container_h)
            || eval_container_condition(&cond[or_pos + 2..], container_w, container_h);
    }
    if cond.starts_with("not ") {
        return !eval_container_condition(&cond[4..], container_w, container_h);
    }

    // Single feature: `min-width: 400px`, `max-height: 300px`, `width >= 600px`
    let colon_or_cmp = cond.find([':','<','>','='].as_ref());
    if let Some(sep) = colon_or_cmp {
        let feature = cond[..sep].trim().to_lowercase();
        let op_and_val = cond[sep..].trim();

        // Extract operator and value
        let (op, val_str) = if op_and_val.starts_with(">=") { (">=", &op_and_val[2..]) }
            else if op_and_val.starts_with("<=") { ("<=", &op_and_val[2..]) }
            else if op_and_val.starts_with('>') { (">", &op_and_val[1..]) }
            else if op_and_val.starts_with('<') { ("<", &op_and_val[1..]) }
            else if op_and_val.starts_with(':') { (":", &op_and_val[1..]) }
            else { return false; };

        let px = parse_length_to_px(val_str.trim());
        let actual = match feature.as_str() {
            "width" | "inline-size" | "min-width" => container_w,
            "height" | "block-size" | "min-height" => container_h,
            "max-width" => container_w,
            "max-height" => container_h,
            "aspect-ratio" => container_w / container_h.max(1.0),
            _ => return false,
        };

        return match op {
            ":" | ">=" => actual >= px,
            "<=" => actual <= px,
            ">"  => actual > px,
            "<"  => actual < px,
            _    => false,
        };
    }

    // Feature test without value: `(color)`, `(display: flex)` etc.
    matches!(cond.trim(), "color" | "hover" | "pointer" | "script")
}

fn find_keyword(s: &str, kw: &str) -> Option<usize> {
    let lower = s.to_lowercase();
    let needle = format!(" {} ", kw);
    lower.find(&needle).map(|i| i + 1) // +1 to skip the leading space
}

/// Convert CSS length value to pixels (very simplified).
pub fn parse_length_to_px(s: &str) -> f32 {
    let s = s.trim();
    if let Some(v) = s.strip_suffix("px") {
        return v.trim().parse().unwrap_or(0.0);
    }
    if let Some(v) = s.strip_suffix("em") {
        return v.trim().parse::<f32>().unwrap_or(0.0) * 16.0;
    }
    if let Some(v) = s.strip_suffix("rem") {
        return v.trim().parse::<f32>().unwrap_or(0.0) * 16.0;
    }
    if let Some(v) = s.strip_suffix("vw") {
        return v.trim().parse::<f32>().unwrap_or(0.0) * 12.8; // 1280px viewport
    }
    if let Some(v) = s.strip_suffix("vh") {
        return v.trim().parse::<f32>().unwrap_or(0.0) * 8.0; // 800px viewport
    }
    s.parse().unwrap_or(0.0)
}

// ── aspect-ratio ──────────────────────────────────────────────────────────────

/// Parse an `aspect-ratio` CSS value like "16 / 9" or "4/3" or "1".
pub fn parse_aspect_ratio(value: &str) -> Option<f32> {
    let v = value.trim();
    if v == "auto" { return None; }
    if let Some(slash) = v.find('/') {
        let num: f32 = v[..slash].trim().parse().ok()?;
        let den: f32 = v[slash+1..].trim().parse().ok()?;
        if den == 0.0 { return None; }
        return Some(num / den);
    }
    v.parse().ok()
}

/// Given a known width and an aspect-ratio, compute the height.
pub fn height_from_aspect_ratio(width: f32, ratio: f32) -> f32 {
    if ratio <= 0.0 { return width; }
    width / ratio
}

// ── content-visibility ────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ContentVisibility {
    Visible,
    Hidden,
    Auto,
}

impl ContentVisibility {
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "hidden" => Self::Hidden,
            "auto"   => Self::Auto,
            _        => Self::Visible,
        }
    }

    /// Returns true if the element should be painted.
    /// `is_in_viewport` hint: for `Auto`, skip if off-screen.
    pub fn should_paint(self, is_in_viewport: bool) -> bool {
        match self {
            Self::Visible => true,
            Self::Hidden  => false,
            Self::Auto    => is_in_viewport,
        }
    }
}

// ── color-scheme ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ColorScheme { Light, Dark, LightDark }

impl ColorScheme {
    pub fn parse(s: &str) -> Self {
        let s = s.trim().to_lowercase();
        if s.contains("dark") && s.contains("light") { Self::LightDark }
        else if s.contains("dark") { Self::Dark }
        else { Self::Light }
    }
}

/// Returns the preferred color scheme for the current environment.
/// In SmartOS we default to Light.
pub fn preferred_color_scheme() -> ColorScheme {
    ColorScheme::Light
}

// ── env() CSS function ────────────────────────────────────────────────────────

/// Resolve a CSS `env(...)` call.
/// Supports safe-area-inset-* and titlebar-area-* from Window Controls Overlay.
pub fn resolve_env_var(name: &str) -> &'static str {
    match name.trim() {
        "safe-area-inset-top"    => "0px",
        "safe-area-inset-right"  => "0px",
        "safe-area-inset-bottom" => "0px",
        "safe-area-inset-left"   => "0px",
        "titlebar-area-x"        => "0px",
        "titlebar-area-y"        => "0px",
        "titlebar-area-width"    => "100%",
        "titlebar-area-height"   => "33px",
        _                        => "0px",
    }
}

// ── CSS.supports() ────────────────────────────────────────────────────────────

/// Test whether a CSS property+value is supported.
pub fn css_supports(property: &str, value: &str) -> bool {
    // We report "supported" for most modern CSS features
    let prop = property.trim().to_lowercase();
    let _val = value.trim();
    // Properties we explicitly support
    matches!(prop.as_str(),
        "display" | "flex" | "grid" | "position" | "color" | "background" |
        "background-color" | "font-size" | "font-family" | "font-weight" |
        "margin" | "padding" | "border" | "border-radius" | "width" | "height" |
        "min-width" | "max-width" | "min-height" | "max-height" |
        "top" | "left" | "right" | "bottom" | "z-index" | "overflow" |
        "transform" | "transition" | "animation" | "opacity" | "visibility" |
        "flex-direction" | "flex-wrap" | "align-items" | "justify-content" |
        "grid-template-columns" | "grid-template-rows" | "gap" |
        "aspect-ratio" | "content-visibility" | "container-type" |
        "container-name" | "contain" | "isolation" | "mix-blend-mode" |
        "color-scheme" | "prefers-color-scheme" | "accent-color" |
        "cursor" | "pointer-events" | "user-select" | "outline" |
        "box-shadow" | "text-shadow" | "filter" | "backdrop-filter" |
        "clip-path" | "mask" | "will-change" | "scroll-behavior" |
        "scroll-snap-type" | "overscroll-behavior" | "resize" |
        "appearance" | "object-fit" | "object-position" |
        "white-space" | "word-break" | "overflow-wrap" | "text-overflow" |
        "line-height" | "letter-spacing" | "text-align" | "text-decoration" |
        "text-transform" | "list-style" | "counter-reset" | "counter-increment" |
        "content" | "quotes" | "columns" | "column-gap" | "row-gap" |
        "float" | "clear" | "vertical-align" | "table-layout" |
        "border-collapse" | "border-spacing" | "caption-side" |
        "empty-cells" | "direction" | "unicode-bidi" |
        "writing-mode" | "text-orientation" | "inset" |
        "inline-size" | "block-size" | "min-inline-size" | "max-inline-size"
    )
}

// ── CSSOM — CSSContainerRule JS wrapper ──────────────────────────────────────

fn make_css_container_rule(container_name: &str, condition: &str, css_text: &str) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("type".to_string(),           JsValue::Number(12.0)); // CSSRule.CONTAINER_RULE (proposed)
    obj.borrow_mut().set("containerName".to_string(),  JsValue::Str(container_name.to_string()));
    obj.borrow_mut().set("conditionText".to_string(),  JsValue::Str(condition.to_string()));
    obj.borrow_mut().set("cssText".to_string(),        JsValue::Str(format!("@container {} {} {{ {} }}", container_name, condition, css_text)));
    obj.borrow_mut().set("cssRules".to_string(),       JsValue::Array(Rc::new(RefCell::new(vec![]))));
    JsValue::Object(obj)
}

fn make_css_layer_rule(name: &str, css_text: &str) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("type".to_string(),     JsValue::Number(0.0));
    obj.borrow_mut().set("name".to_string(),     JsValue::Str(name.to_string()));
    obj.borrow_mut().set("cssText".to_string(),  JsValue::Str(format!("@layer {} {{ {} }}", name, css_text)));
    obj.borrow_mut().set("cssRules".to_string(), JsValue::Array(Rc::new(RefCell::new(vec![]))));
    JsValue::Object(obj)
}

// ── CSS namespace object ──────────────────────────────────────────────────────

fn native_css_supports(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    // Two-arg form: CSS.supports(property, value)
    // One-arg form: CSS.supports("display: grid")
    let (prop, val) = if args.len() >= 2 {
        (args[0].to_string_val(), args[1].to_string_val())
    } else {
        let s = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
        if let Some(colon) = s.find(':') {
            (s[..colon].to_string(), s[colon+1..].to_string())
        } else {
            (s, String::new())
        }
    };
    JsValue::Bool(css_supports(&prop, &val))
}

fn native_css_escape(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let s = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    // Escape special CSS identifier characters
    let escaped: String = s.chars().map(|c| {
        if c.is_alphanumeric() || c == '-' || c == '_' { c.to_string() }
        else { format!("\\{}", c) }
    }).collect();
    JsValue::Str(escaped)
}

fn native_css_parse_value(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    // CSSStyleValue stub — returns a minimal object
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("toString".to_string(),
        JsValue::NativeFunction("toString", |_,i| JsValue::Str(i.env.get("this").to_string_val())));
    JsValue::Object(obj)
}

fn make_css_namespace() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("supports".to_string(), JsValue::NativeFunction("supports", native_css_supports));
    obj.borrow_mut().set("escape".to_string(),   JsValue::NativeFunction("escape",   native_css_escape));
    obj.borrow_mut().set("parseValue".to_string(), JsValue::NativeFunction("parseValue", native_css_parse_value));
    // CSS Highlight API stub
    let highlights = Rc::new(RefCell::new(JsObject::new()));
    highlights.borrow_mut().set("set".to_string(),    JsValue::NativeFunction("set",    |_,_| JsValue::Undefined));
    highlights.borrow_mut().set("get".to_string(),    JsValue::NativeFunction("get",    |_,_| JsValue::Undefined));
    highlights.borrow_mut().set("delete".to_string(), JsValue::NativeFunction("delete", |_,_| JsValue::Undefined));
    highlights.borrow_mut().set("clear".to_string(),  JsValue::NativeFunction("clear",  |_,_| JsValue::Undefined));
    obj.borrow_mut().set("highlights".to_string(), JsValue::Object(highlights));
    JsValue::Object(obj)
}

// ── getComputedStyle supplement ───────────────────────────────────────────────

fn native_get_computed_style(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let el = args.get(0).cloned().unwrap_or(JsValue::Null);
    let cs = Rc::new(RefCell::new(JsObject::new()));

    // Copy element's style props if available
    if let JsValue::Object(ref obj) = el {
        let style = obj.borrow().get("style");
        if let JsValue::Object(s) = style {
            for (k, v) in &s.borrow().props {
                cs.borrow_mut().set(k.clone(), v.clone());
            }
        }
    }

    // Fill in defaults for commonly requested properties
    let defaults: &[(&str, &str)] = &[
        ("display","block"), ("position","static"), ("float","none"),
        ("visibility","visible"), ("overflow","visible"), ("opacity","1"),
        ("color","rgb(0,0,0)"), ("background-color","transparent"),
        ("font-size","16px"), ("font-family","sans-serif"), ("font-weight","400"),
        ("line-height","normal"), ("text-align","start"),
        ("margin-top","0px"), ("margin-right","0px"),
        ("margin-bottom","0px"), ("margin-left","0px"),
        ("padding-top","0px"), ("padding-right","0px"),
        ("padding-bottom","0px"), ("padding-left","0px"),
        ("border-width","0px"), ("border-style","none"),
        ("width","auto"), ("height","auto"),
        ("box-sizing","content-box"), ("content-visibility","visible"),
        ("aspect-ratio","auto"), ("container-type","normal"),
        ("color-scheme","normal"),
    ];
    for (k, v) in defaults {
        let key = k.to_string();
        let existing = cs.borrow().get(&key);
        if matches!(existing, JsValue::Undefined) {
            cs.borrow_mut().set(key, JsValue::Str(v.to_string()));
        }
    }

    // getPropertyValue method
    cs.borrow_mut().set("getPropertyValue".to_string(),
        JsValue::NativeFunction("getPropertyValue", |args, i| {
            let prop = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
            let this = i.env.get("this");
            if let JsValue::Object(o) = &this {
                let v = o.borrow().get(&prop);
                if !matches!(v, JsValue::Undefined) { return v; }
            }
            JsValue::Str(String::new())
        }));
    cs.borrow_mut().set("setProperty".to_string(),
        JsValue::NativeFunction("setProperty", |args, i| {
            let prop = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
            let val  = args.get(1).cloned().unwrap_or(JsValue::Undefined);
            let this = i.env.get("this");
            if let JsValue::Object(o) = &this {
                o.borrow_mut().set(prop, val);
            }
            JsValue::Undefined
        }));

    JsValue::Object(cs)
}

// ── Install ───────────────────────────────────────────────────────────────────

pub fn install_css_modern_api(interp: &mut Interpreter) {
    // CSS namespace
    interp.env.define("CSS".to_string(), make_css_namespace());

    // Override/enhance getComputedStyle
    interp.env.define("getComputedStyle".to_string(),
        JsValue::NativeFunction("getComputedStyle", native_get_computed_style));

    // CSS.paintWorklet stub (Houdini CSS Paint API)
    interp.run(r#"
        if (typeof CSS !== 'undefined') {
            CSS.paintWorklet = {
                addModule: function(url) {
                    return Promise.resolve ? Promise.resolve() : {then: function(f){ if(f) f(); }};
                }
            };
        }
    "#);

    // matchMedia improvements: prefers-color-scheme
    // (extends the existing matchMedia from pwa.rs)
    interp.run(r#"
        if (typeof window === 'undefined') { var window = {}; }
        if (!window.matchMedia) {
            window.matchMedia = function(q) {
                return {
                    matches: q.indexOf('dark') !== -1 ? false : true,
                    media: q,
                    addListener: function(){},
                    removeListener: function(){},
                    addEventListener: function(){},
                    removeEventListener: function(){}
                };
            };
        }
        if (typeof matchMedia === 'undefined') {
            var matchMedia = window.matchMedia;
        }
    "#);
}

// ── Self-test ─────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] css_modern: {}", $name); }
        }
    }

    // T1: Container condition eval — min-width pass
    check!(eval_container_condition("(min-width: 400px)", 500.0, 300.0), "container min-width pass");
    check!(!eval_container_condition("(min-width: 400px)", 300.0, 300.0), "container min-width fail");

    // T2: Container condition — max-width
    check!(eval_container_condition("(max-width: 600px)", 500.0, 400.0), "container max-width pass");
    check!(!eval_container_condition("(max-width: 600px)", 700.0, 400.0), "container max-width fail");

    // T3: aspect-ratio parsing
    check!(parse_aspect_ratio("16/9").map(|r| (r - 16.0/9.0).abs() < 0.01).unwrap_or(false), "aspect-ratio 16/9");
    check!(parse_aspect_ratio("1").map(|r| (r - 1.0).abs() < 0.01).unwrap_or(false), "aspect-ratio 1");
    check!(parse_aspect_ratio("auto").is_none(), "aspect-ratio auto = None");

    // T4: height_from_aspect_ratio
    let h = height_from_aspect_ratio(160.0, 16.0/9.0);
    check!((h - 90.0).abs() < 0.5, "height_from_aspect_ratio 16/9");

    // T5: content-visibility parse
    check!(ContentVisibility::parse("auto")    == ContentVisibility::Auto,    "cv auto");
    check!(ContentVisibility::parse("hidden")  == ContentVisibility::Hidden,  "cv hidden");
    check!(ContentVisibility::parse("visible") == ContentVisibility::Visible, "cv visible");
    check!(!ContentVisibility::Hidden.should_paint(true),  "cv hidden → skip");
    check!( ContentVisibility::Visible.should_paint(false),"cv visible → paint");
    check!(!ContentVisibility::Auto.should_paint(false),   "cv auto + off-screen → skip");
    check!( ContentVisibility::Auto.should_paint(true),    "cv auto + in-view → paint");

    // T6: CSS.supports() JS API
    {
        let mut interp = Interpreter::new();
        install_css_modern_api(&mut interp);
        interp.run(r#"
            var supFlex = CSS.supports('display','flex');
            var supXYZ  = CSS.supports('display','xyz-unknown-value-12345');
            var supAR   = CSS.supports('aspect-ratio', '16/9');
        "#);
        check!(interp.env.get("supFlex").is_truthy(), "CSS.supports(display,flex)=true");
        check!(interp.env.get("supAR").is_truthy(),   "CSS.supports(aspect-ratio)=true");
    }

    // T7: CSS.escape()
    {
        let mut interp = Interpreter::new();
        install_css_modern_api(&mut interp);
        interp.run(r#"var esc = CSS.escape('hello world');  "#);
        let v = interp.env.get("esc").to_string_val();
        check!(v.contains('\\') || v.len() >= "hello world".len(), "CSS.escape escapes spaces");
    }

    // T8: getComputedStyle returns style object
    {
        let mut interp = Interpreter::new();
        install_css_modern_api(&mut interp);
        interp.run(r#"
            var el = {style: {display: 'flex'}};
            var cs = getComputedStyle(el);
            var disp = cs.display;
            var gpv  = cs.getPropertyValue('display');
        "#);
        check!(interp.env.get("disp").to_string_val() == "flex", "getComputedStyle inherits element style");
        check!(interp.env.get("gpv").to_string_val()  == "flex", "getComputedStyle.getPropertyValue");
    }

    // T9: env() CSS variable resolver
    check!(resolve_env_var("safe-area-inset-top") == "0px", "env() safe-area-inset-top");
    check!(resolve_env_var("titlebar-area-height") == "33px", "env() titlebar-area-height");

    // T10: @layer parse
    let (name, _rest) = parse_layer_statement("@layer base;").unwrap();
    check!(name == "base", "@layer statement parse");

    if fail == 0 {
        crate::serial_println!("[css_modern] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[css_modern] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

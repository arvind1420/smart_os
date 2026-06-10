/// CSS Completeness — Phase 109
///
/// Implements:
///   1. CSS Custom Properties cascade  (`--foo: value` + `var(--foo, fallback)`)
///   2. `calc()` / `clamp()` nested expression evaluator
///   3. CSS Nesting  (`& .child {}` → expanded selectors)
///   4. `:has()` parent-selector inverted index  (O(log N) lookup)
///   5. Logical properties  (`margin-inline-start` → physical `margin-left/right`)

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;

// ─────────────────────────────────────────────────────────────────────────────
// 1.  CSS CUSTOM PROPERTIES  (CSS variables)
// ─────────────────────────────────────────────────────────────────────────────

/// A single element's custom-property declarations, ordered by specificity.
#[derive(Debug, Clone, Default)]
pub struct CustomPropScope {
    /// `--name` → raw value string
    pub props: BTreeMap<String, String>,
}

impl CustomPropScope {
    pub fn new() -> Self { Self::default() }

    pub fn declare(&mut self, name: &str, value: &str) {
        self.props.insert(name.to_string(), value.to_string());
    }

    /// Resolve `var(--name)` or `var(--name, fallback)`.
    pub fn resolve_var(&self, name: &str, fallback: Option<&str>) -> Option<String> {
        if let Some(v) = self.props.get(name) {
            return Some(v.clone());
        }
        fallback.map(|f| f.to_string())
    }
}

/// Cascade of scopes from root → current element (inherited custom props).
#[derive(Debug, Clone, Default)]
pub struct CssCascade {
    pub scopes: Vec<CustomPropScope>,
}

impl CssCascade {
    pub fn new() -> Self { Self::default() }

    /// Push a new scope (e.g. entering a child element).
    pub fn push(&mut self, scope: CustomPropScope) {
        self.scopes.push(scope);
    }

    pub fn pop(&mut self) { self.scopes.pop(); }

    /// Walk innermost → outermost to find the custom property.
    pub fn resolve(&self, name: &str, fallback: Option<&str>) -> Option<String> {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.props.get(name) {
                return Some(v.clone());
            }
        }
        fallback.map(|f| f.to_string())
    }

    /// Expand `var()` tokens in a value string using cascade resolution.
    pub fn expand_vars(&self, value: &str) -> String {
        if !value.contains("var(") {
            return value.to_string();
        }
        let mut out = String::new();
        let mut rest = value;
        while let Some(idx) = rest.find("var(") {
            out.push_str(&rest[..idx]);
            rest = &rest[idx + 4..]; // skip "var("
            // find matching closing paren
            if let Some(end) = rest.find(')') {
                let inner = &rest[..end];
                rest = &rest[end + 1..];
                // split on first comma for fallback
                let (name, fb) = if let Some(ci) = inner.find(',') {
                    (inner[..ci].trim(), Some(inner[ci + 1..].trim()))
                } else {
                    (inner.trim(), None)
                };
                let resolved = self.resolve(name, fb)
                    .unwrap_or_else(|| String::new());
                out.push_str(&self.expand_vars(&resolved));
            } else {
                // malformed — skip
                break;
            }
        }
        out.push_str(rest);
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 2.  calc() / clamp() EVALUATOR
// ─────────────────────────────────────────────────────────────────────────────

/// A simple CSS length value (px, em, rem, %, vw, vh, or plain number).
#[derive(Debug, Clone, PartialEq)]
pub struct CssLength {
    pub value: f32,
    pub unit:  CssUnit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CssUnit { Px, Em, Rem, Percent, Vw, Vh, None }

impl CssLength {
    pub fn px(v: f32)  -> Self { CssLength { value: v, unit: CssUnit::Px } }
    pub fn em(v: f32)  -> Self { CssLength { value: v, unit: CssUnit::Em } }
    pub fn pct(v: f32) -> Self { CssLength { value: v, unit: CssUnit::Percent } }

    /// Resolve to px given context values.
    pub fn to_px(&self, font_size_px: f32, percent_base_px: f32,
                 vw: f32, vh: f32) -> f32 {
        match self.unit {
            CssUnit::Px      => self.value,
            CssUnit::Em      => self.value * font_size_px,
            CssUnit::Rem     => self.value * font_size_px, // simplified: rem == em
            CssUnit::Percent => self.value / 100.0 * percent_base_px,
            CssUnit::Vw      => self.value / 100.0 * vw,
            CssUnit::Vh      => self.value / 100.0 * vh,
            CssUnit::None    => self.value,
        }
    }
}

/// A CSS calc() expression tree.
#[derive(Debug, Clone)]
pub enum CalcExpr {
    Lit(CssLength),
    Add(Box<CalcExpr>, Box<CalcExpr>),
    Sub(Box<CalcExpr>, Box<CalcExpr>),
    Mul(Box<CalcExpr>, Box<CalcExpr>),
    Div(Box<CalcExpr>, Box<CalcExpr>),
    /// `clamp(min, val, max)`
    Clamp(Box<CalcExpr>, Box<CalcExpr>, Box<CalcExpr>),
    /// `min(a, b)` / `max(a, b)`
    Min(Box<CalcExpr>, Box<CalcExpr>),
    Max(Box<CalcExpr>, Box<CalcExpr>),
}

impl CalcExpr {
    pub fn eval(&self, font_size: f32, pct_base: f32, vw: f32, vh: f32) -> f32 {
        match self {
            CalcExpr::Lit(l)         => l.to_px(font_size, pct_base, vw, vh),
            CalcExpr::Add(a, b)      => a.eval(font_size, pct_base, vw, vh) + b.eval(font_size, pct_base, vw, vh),
            CalcExpr::Sub(a, b)      => a.eval(font_size, pct_base, vw, vh) - b.eval(font_size, pct_base, vw, vh),
            CalcExpr::Mul(a, b)      => a.eval(font_size, pct_base, vw, vh) * b.eval(font_size, pct_base, vw, vh),
            CalcExpr::Div(a, b)      => {
                let dv = b.eval(font_size, pct_base, vw, vh);
                if dv == 0.0 { f32::INFINITY } else { a.eval(font_size, pct_base, vw, vh) / dv }
            }
            CalcExpr::Clamp(lo, v, hi) => {
                let lo_v = lo.eval(font_size, pct_base, vw, vh);
                let v_v  = v.eval(font_size, pct_base, vw, vh);
                let hi_v = hi.eval(font_size, pct_base, vw, vh);
                if v_v < lo_v { lo_v } else if v_v > hi_v { hi_v } else { v_v }
            }
            CalcExpr::Min(a, b) => {
                let av = a.eval(font_size, pct_base, vw, vh);
                let bv = b.eval(font_size, pct_base, vw, vh);
                if av < bv { av } else { bv }
            }
            CalcExpr::Max(a, b) => {
                let av = a.eval(font_size, pct_base, vw, vh);
                let bv = b.eval(font_size, pct_base, vw, vh);
                if av > bv { av } else { bv }
            }
        }
    }
}

/// Parse a length token like `16px`, `2em`, `50%`, `100vw`, `3.5`.
pub fn parse_length(s: &str) -> Option<CssLength> {
    let s = s.trim();
    if s.ends_with("px") {
        s[..s.len() - 2].trim().parse::<f32>().ok().map(CssLength::px)
    } else if s.ends_with("em") {
        s[..s.len() - 2].trim().parse::<f32>().ok().map(CssLength::em)
    } else if s.ends_with("rem") {
        s[..s.len() - 3].trim().parse::<f32>().ok().map(|v| CssLength { value: v, unit: CssUnit::Rem })
    } else if s.ends_with('%') {
        s[..s.len() - 1].trim().parse::<f32>().ok().map(CssLength::pct)
    } else if s.ends_with("vw") {
        s[..s.len() - 2].trim().parse::<f32>().ok().map(|v| CssLength { value: v, unit: CssUnit::Vw })
    } else if s.ends_with("vh") {
        s[..s.len() - 2].trim().parse::<f32>().ok().map(|v| CssLength { value: v, unit: CssUnit::Vh })
    } else {
        s.parse::<f32>().ok().map(|v| CssLength { value: v, unit: CssUnit::None })
    }
}

/// Very small recursive-descent parser for calc()/clamp()/min()/max().
/// Only handles the subset: calc(A op B), clamp(A, B, C), min(A, B), max(A, B).
/// Nested calc is supported by recursion on inner strings.
pub fn parse_calc_expr(input: &str) -> Option<CalcExpr> {
    let s = input.trim();
    if s.starts_with("calc(") && s.ends_with(')') {
        let inner = &s[5..s.len() - 1];
        return parse_additive(inner);
    }
    if s.starts_with("clamp(") && s.ends_with(')') {
        let inner = &s[6..s.len() - 1];
        let parts = split_args(inner);
        if parts.len() == 3 {
            let lo = parse_calc_expr(parts[0].trim())?;
            let v  = parse_calc_expr(parts[1].trim())?;
            let hi = parse_calc_expr(parts[2].trim())?;
            return Some(CalcExpr::Clamp(Box::new(lo), Box::new(v), Box::new(hi)));
        }
        return None;
    }
    if s.starts_with("min(") && s.ends_with(')') {
        let inner = &s[4..s.len() - 1];
        let parts = split_args(inner);
        if parts.len() == 2 {
            let a = parse_calc_expr(parts[0].trim())?;
            let b = parse_calc_expr(parts[1].trim())?;
            return Some(CalcExpr::Min(Box::new(a), Box::new(b)));
        }
        return None;
    }
    if s.starts_with("max(") && s.ends_with(')') {
        let inner = &s[4..s.len() - 1];
        let parts = split_args(inner);
        if parts.len() == 2 {
            let a = parse_calc_expr(parts[0].trim())?;
            let b = parse_calc_expr(parts[1].trim())?;
            return Some(CalcExpr::Max(Box::new(a), Box::new(b)));
        }
        return None;
    }
    // plain literal
    parse_length(s).map(CalcExpr::Lit)
}

/// Split comma-separated calc arguments, respecting nested parentheses.
fn split_args(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => { if depth > 0 { depth -= 1; } }
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Parse additive level (handles + and - at the top level of a calc inner).
fn parse_additive(s: &str) -> Option<CalcExpr> {
    // find last + or - outside parens (right-associative split)
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut split_pos = None;
    let mut split_op  = b'+';
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b')' => depth += 1,
            b'(' => depth -= 1,
            b'+' | b'-' if depth == 0 && i > 0 => {
                split_pos = Some(i);
                split_op  = bytes[i];
                break;
            }
            _ => {}
        }
    }
    if let Some(pos) = split_pos {
        let lhs = parse_multiplicative(s[..pos].trim())?;
        let rhs = parse_multiplicative(s[pos + 1..].trim())?;
        return Some(if split_op == b'+' {
            CalcExpr::Add(Box::new(lhs), Box::new(rhs))
        } else {
            CalcExpr::Sub(Box::new(lhs), Box::new(rhs))
        });
    }
    parse_multiplicative(s)
}

fn parse_multiplicative(s: &str) -> Option<CalcExpr> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b')' => depth += 1,
            b'(' => depth -= 1,
            b'*' if depth == 0 => {
                let lhs = parse_primary(s[..i].trim())?;
                let rhs = parse_primary(s[i + 1..].trim())?;
                return Some(CalcExpr::Mul(Box::new(lhs), Box::new(rhs)));
            }
            b'/' if depth == 0 => {
                let lhs = parse_primary(s[..i].trim())?;
                let rhs = parse_primary(s[i + 1..].trim())?;
                return Some(CalcExpr::Div(Box::new(lhs), Box::new(rhs)));
            }
            _ => {}
        }
    }
    parse_primary(s)
}

fn parse_primary(s: &str) -> Option<CalcExpr> {
    let s = s.trim();
    if s.starts_with('(') && s.ends_with(')') {
        return parse_additive(&s[1..s.len() - 1]);
    }
    // nested function
    if s.contains('(') {
        return parse_calc_expr(s);
    }
    parse_length(s).map(CalcExpr::Lit)
}

// ─────────────────────────────────────────────────────────────────────────────
// 3.  CSS NESTING
// ─────────────────────────────────────────────────────────────────────────────

/// A very small CSS rule (selector + declarations + nested children).
#[derive(Debug, Clone)]
pub struct CssRule {
    pub selector:     String,
    pub declarations: Vec<(String, String)>,
    pub children:     Vec<CssRule>,
}

impl CssRule {
    pub fn new(selector: &str) -> Self {
        CssRule { selector: selector.to_string(), declarations: Vec::new(), children: Vec::new() }
    }

    pub fn add_decl(&mut self, prop: &str, val: &str) {
        self.declarations.push((prop.to_string(), val.to_string()));
    }

    pub fn add_child(&mut self, child: CssRule) {
        self.children.push(child);
    }
}

/// Flatten nested CSS rules: expand `& .child {}` inside a parent rule.
///
/// Produces a flat list of (selector, declarations) pairs matching what a
/// traditional CSS pre-processor would emit.
pub fn expand_nesting(rule: &CssRule) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::new();
    // parent itself
    if !rule.declarations.is_empty() {
        out.push((rule.selector.clone(), rule.declarations.clone()));
    }
    for child in &rule.children {
        // expand child selector relative to parent
        let child_sel = expand_child_selector(&rule.selector, &child.selector);
        let child_rule = CssRule {
            selector:     child_sel,
            declarations: child.declarations.clone(),
            children:     child.children.clone(),
        };
        for flat in expand_nesting(&child_rule) {
            out.push(flat);
        }
    }
    out
}

/// `.parent` + `& .child` → `.parent .child`.
/// `.parent` + `&:hover` → `.parent:hover`.
/// `.parent` + `.sibling` → `.parent .sibling`.
pub fn expand_child_selector(parent: &str, child: &str) -> String {
    let child = child.trim();
    if child.starts_with('&') {
        // `&` is replaced by parent selector
        let after = &child[1..];
        format!("{}{}", parent, after)
    } else {
        format!("{} {}", parent, child)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 4.  :has() PARENT-SELECTOR INVERTED INDEX
// ─────────────────────────────────────────────────────────────────────────────

/// A simple DOM node representation for `:has()` matching.
#[derive(Debug, Clone)]
pub struct DomNode {
    pub id:       usize,
    pub tag:      String,
    pub classes:  Vec<String>,
    pub parent:   Option<usize>,
    pub children: Vec<usize>,
}

impl DomNode {
    pub fn new(id: usize, tag: &str, classes: Vec<&str>, parent: Option<usize>) -> Self {
        DomNode {
            id,
            tag:      tag.to_string(),
            classes:  classes.iter().map(|s| s.to_string()).collect(),
            parent,
            children: Vec::new(),
        }
    }

    pub fn has_class(&self, cls: &str) -> bool {
        self.classes.iter().any(|c| c == cls)
    }
}

/// `:has()` inverted index.
/// Maps "child tag/class selector" → set of ancestor node IDs that qualify.
///
/// Build once after DOM construction; look up in O(log N).
#[derive(Debug, Default)]
pub struct HasIndex {
    /// key = simple selector string (e.g. "a" or ".active")
    /// val = sorted Vec of node IDs that have a descendant matching the key
    inner: BTreeMap<String, Vec<usize>>,
}

impl HasIndex {
    pub fn new() -> Self { Self::default() }

    /// Build the index from a flat node list.
    pub fn build(nodes: &[DomNode]) -> Self {
        let mut idx = HasIndex::new();
        let by_id: BTreeMap<usize, &DomNode> = nodes.iter().map(|n| (n.id, n)).collect();

        for node in nodes {
            // for each tag selector: insert all ancestors
            let tag_key = node.tag.clone();
            idx.insert_ancestors(&tag_key, node, &by_id);
            // for each class: ".class"
            for cls in &node.classes {
                let cls_key = format!(".{}", cls);
                idx.insert_ancestors(&cls_key, node, &by_id);
            }
        }
        // sort / dedup each list
        for v in idx.inner.values_mut() {
            v.sort();
            v.dedup();
        }
        idx
    }

    fn insert_ancestors(&mut self, key: &str,
                        node: &DomNode,
                        by_id: &BTreeMap<usize, &DomNode>) {
        let mut cur_id = node.parent;
        while let Some(pid) = cur_id {
            let entry = self.inner.entry(key.to_string()).or_default();
            entry.push(pid);
            cur_id = by_id.get(&pid).and_then(|n| n.parent);
        }
    }

    /// Return sorted slice of node IDs whose subtree contains at least one
    /// node matching `selector` (simple tag or `.class`).
    pub fn query(&self, selector: &str) -> &[usize] {
        self.inner.get(selector).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Check if a specific node matches `:has(selector)`.
    pub fn node_has(&self, node_id: usize, selector: &str) -> bool {
        let list = self.query(selector);
        list.binary_search(&node_id).is_ok()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 5.  LOGICAL PROPERTIES
// ─────────────────────────────────────────────────────────────────────────────

/// Writing direction for logical property resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritingMode {
    LtrTb,  // left-to-right, top-to-bottom  (default)
    RtlTb,  // right-to-left, top-to-bottom  (Arabic, Hebrew)
    TbLtr,  // top-to-bottom, left-to-right  (Japanese vertical)
    TbRtl,  // top-to-bottom, right-to-left
}

/// Map a logical CSS property to its physical equivalent.
///
/// Only margin/padding/border-width/inset families are handled here.
/// Returns `None` if the property is not a logical property (caller uses as-is).
pub fn resolve_logical(prop: &str, mode: WritingMode)
    -> Option<(&'static str, &'static str)>
{
    // `(inline_start_physical, inline_end_physical)`
    let (is, ie, bs, be) = match mode {
        WritingMode::LtrTb => ("left",  "right", "top",    "bottom"),
        WritingMode::RtlTb => ("right", "left",  "top",    "bottom"),
        WritingMode::TbLtr => ("top",   "bottom","left",   "right"),
        WritingMode::TbRtl => ("top",   "bottom","right",  "left"),
    };

    // Helper: build physical property name prefix
    let physical_side = |side: &str| -> Option<(&'static str, &'static str)> {
        // We only need to map side → physical side string. Use the existing bindings.
        let _ = side; // suppress warning
        None
    };
    let _ = physical_side;

    match prop {
        // ── margin ──────────────────────────────────────────────
        "margin-inline-start"  => Some((box_prop("margin", is), "")),
        "margin-inline-end"    => Some((box_prop("margin", ie), "")),
        "margin-block-start"   => Some((box_prop("margin", bs), "")),
        "margin-block-end"     => Some((box_prop("margin", be), "")),
        "margin-inline"        => Some((box_prop("margin", is), box_prop("margin", ie))),
        "margin-block"         => Some((box_prop("margin", bs), box_prop("margin", be))),

        // ── padding ─────────────────────────────────────────────
        "padding-inline-start" => Some((box_prop("padding", is), "")),
        "padding-inline-end"   => Some((box_prop("padding", ie), "")),
        "padding-block-start"  => Some((box_prop("padding", bs), "")),
        "padding-block-end"    => Some((box_prop("padding", be), "")),
        "padding-inline"       => Some((box_prop("padding", is), box_prop("padding", ie))),
        "padding-block"        => Some((box_prop("padding", bs), box_prop("padding", be))),

        // ── border-width ─────────────────────────────────────────
        "border-inline-start-width" => Some((box_prop("border-inline-start", "width"), "")),
        "border-inline-end-width"   => Some((box_prop("border-inline-end",   "width"), "")),
        "border-block-start-width"  => Some((box_prop("border-block-start",  "width"), "")),
        "border-block-end-width"    => Some((box_prop("border-block-end",    "width"), "")),

        // ── inset ────────────────────────────────────────────────
        "inset-inline-start" => Some((is, "")),
        "inset-inline-end"   => Some((ie, "")),
        "inset-block-start"  => Some((bs, "")),
        "inset-block-end"    => Some((be, "")),

        // ── size ─────────────────────────────────────────────────
        "inline-size"  => Some(("width",  "")),
        "block-size"   => Some(("height", "")),
        "min-inline-size"  => Some(("min-width",  "")),
        "max-inline-size"  => Some(("max-width",  "")),
        "min-block-size"   => Some(("min-height", "")),
        "max-block-size"   => Some(("max-height", "")),

        _ => None,
    }
}

/// Build "margin-left", "padding-top", etc. via string tables.
fn box_prop(prefix: &'static str, side: &'static str) -> &'static str {
    // We can't do string concat in const fn on stable, so we use a lookup table.
    match (prefix, side) {
        ("margin",  "left")   => "margin-left",
        ("margin",  "right")  => "margin-right",
        ("margin",  "top")    => "margin-top",
        ("margin",  "bottom") => "margin-bottom",
        ("padding", "left")   => "padding-left",
        ("padding", "right")  => "padding-right",
        ("padding", "top")    => "padding-top",
        ("padding", "bottom") => "padding-bottom",
        ("border-inline-start", "width") => "border-left-width",
        ("border-inline-end",   "width") => "border-right-width",
        ("border-block-start",  "width") => "border-top-width",
        ("border-block-end",    "width") => "border-bottom-width",
        _ => prefix,
    }
}

/// Expand a declaration list, replacing logical properties with physical ones.
/// Returns a new list. Values are preserved as-is.
pub fn expand_logical_properties(
    decls: &[(String, String)],
    mode: WritingMode,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (prop, val) in decls {
        if let Some((phys1, phys2)) = resolve_logical(prop, mode) {
            out.push((phys1.to_string(), val.clone()));
            if !phys2.is_empty() {
                out.push((phys2.to_string(), val.clone()));
            }
        } else {
            out.push((prop.clone(), val.clone()));
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// SELF-TEST
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;

    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] css_completeness: {}", $name); }
        }
    }

    // ── 1. Custom properties basic declare + resolve ─────────────────────────
    {
        let mut scope = CustomPropScope::new();
        scope.declare("--primary", "#3399ff");
        let v = scope.resolve_var("--primary", None);
        check!(v == Some("#3399ff".to_string()), "custom-prop declare/resolve");
    }

    // ── 2. Custom properties fallback ────────────────────────────────────────
    {
        let scope = CustomPropScope::new();
        let v = scope.resolve_var("--missing", Some("red"));
        check!(v == Some("red".to_string()), "custom-prop fallback");
    }

    // ── 3. Cascade var() expansion ───────────────────────────────────────────
    {
        let mut cascade = CssCascade::new();
        let mut root_scope = CustomPropScope::new();
        root_scope.declare("--bg", "#fff");
        cascade.push(root_scope);
        let expanded = cascade.expand_vars("background: var(--bg, #000)");
        check!(expanded.contains("#fff"), "cascade var() expansion");
    }

    // ── 4. calc() addition ───────────────────────────────────────────────────
    {
        let expr = parse_calc_expr("calc(100px + 20px)");
        let ok = expr.map(|e| e.eval(16.0, 800.0, 1280.0, 720.0) as i32 == 120)
                     .unwrap_or(false);
        check!(ok, "calc() addition");
    }

    // ── 5. clamp() clamping ──────────────────────────────────────────────────
    {
        let expr = parse_calc_expr("clamp(10px, 5px, 20px)");
        let ok = expr.map(|e| e.eval(16.0, 800.0, 1280.0, 720.0) as i32 == 10)
                     .unwrap_or(false);
        check!(ok, "clamp() lower bound");
    }

    // ── 6. CSS Nesting expansion ─────────────────────────────────────────────
    {
        let mut parent = CssRule::new(".card");
        parent.add_decl("display", "block");
        let mut child = CssRule::new("& .title");
        child.add_decl("font-size", "1.2em");
        parent.add_child(child);

        let flat = expand_nesting(&parent);
        let selectors: Vec<&str> = flat.iter().map(|(s, _)| s.as_str()).collect();
        check!(selectors.contains(&".card") && selectors.contains(&".card .title"),
               "CSS nesting expansion");
    }

    // ── 7. :has() index build + query ────────────────────────────────────────
    {
        let mut nodes = Vec::new();
        let mut root = DomNode::new(0, "div", vec![], None);
        root.children = vec![1];
        nodes.push(root);
        let child = DomNode::new(1, "a", vec!["active"], Some(0));
        nodes.push(child);

        let idx = HasIndex::build(&nodes);
        // node 0 (div) should match :has(a) and :has(.active)
        check!(idx.node_has(0, "a"),        ":has(a) index");
        check!(idx.node_has(0, ".active"),  ":has(.active) index");
    }

    // ── 8. :has() negative case ──────────────────────────────────────────────
    {
        let nodes = vec![DomNode::new(0, "div", vec![], None)];
        let idx = HasIndex::build(&nodes);
        check!(!idx.node_has(0, "span"), ":has() no false positive");
    }

    // ── 9. Logical properties LTR ────────────────────────────────────────────
    {
        let decls = vec![
            ("margin-inline-start".to_string(), "8px".to_string()),
            ("margin-inline-end".to_string(),   "16px".to_string()),
        ];
        let phys = expand_logical_properties(&decls, WritingMode::LtrTb);
        let keys: Vec<&str> = phys.iter().map(|(k, _)| k.as_str()).collect();
        check!(keys.contains(&"margin-left") && keys.contains(&"margin-right"),
               "logical props LTR");
    }

    // ── 10. Logical properties RTL ───────────────────────────────────────────
    {
        let decls = vec![
            ("margin-inline-start".to_string(), "8px".to_string()),
        ];
        let phys = expand_logical_properties(&decls, WritingMode::RtlTb);
        // RTL: inline-start → right
        let ok = phys.iter().any(|(k, _)| k == "margin-right");
        check!(ok, "logical props RTL");
    }

    if fail == 0 {
        crate::serial_println!("[css_completeness] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[css_completeness] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

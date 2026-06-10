//! CSS Parser + Cascade — Phase 35 for Smart OS.
//!
//! Implements:
//!  • CSS tokenizer  (ident, string, number, delim, function, at-rule)
//!  • Selector parser: tag, .class, #id, *, [attr], [attr=val],
//!    combinators: descendant (space), child (>), adjacent (+), sibling (~)
//!  • Property value parser: length (px/em/rem/%), color (#rrggbb, named),
//!    display, position, font-size, font-weight, line-height, margin/padding
//!    shorthand, border, background, flex-*, overflow, z-index, opacity
//!  • Cascade: specificity ordering, !important, origin (author/user-agent)
//!  • Inheritance: color, font-size, font-weight, line-height, visibility
//!  • Computed style struct (resolved pixel values)
//!  • @media (width) query evaluation
//!  • User-agent default stylesheet

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::borrow::ToOwned;
use super::html::{Dom, NodeId, NodeKind, NULL_NODE};

// ─────────────────────────────────────────────────────────────────────────────
//  CSS value types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum CssLength {
    Px(f32),
    Em(f32),
    Rem(f32),
    Percent(f32),
    Auto,
    Zero,
}

impl CssLength {
    /// Resolve to absolute pixels given parent font-size and root font-size.
    pub fn to_px(&self, parent_font_px: f32, root_font_px: f32) -> f32 {
        match self {
            CssLength::Px(v)      => *v,
            CssLength::Em(v)      => v * parent_font_px,
            CssLength::Rem(v)     => v * root_font_px,
            CssLength::Percent(v) => *v, // caller must multiply by container dimension
            CssLength::Auto       => 0.0,
            CssLength::Zero       => 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color { pub r: u8, pub g: u8, pub b: u8, pub a: u8 }

impl Color {
    pub const TRANSPARENT: Color = Color { r:0, g:0, b:0, a:0 };
    pub const BLACK:       Color = Color { r:0, g:0, b:0, a:255 };
    pub const WHITE:       Color = Color { r:255, g:255, b:255, a:255 };

    pub fn to_u32_rgba(self) -> u32 {
        ((self.r as u32) << 24) | ((self.g as u32) << 16)
            | ((self.b as u32) << 8) | (self.a as u32)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Display { Block, Inline, InlineBlock, Flex, Grid, None, ListItem }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Position { Static, Relative, Absolute, Fixed, Sticky }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overflow { Visible, Hidden, Scroll, Auto }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontWeight { Normal, Bold, Bolder, Lighter, Number(u16) }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextAlign { Left, Right, Center, Justify }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlexDirection { Row, RowReverse, Column, ColumnReverse }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JustifyContent { FlexStart, FlexEnd, Center, SpaceBetween, SpaceAround, SpaceEvenly }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlignItems { FlexStart, FlexEnd, Center, Stretch, Baseline }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextDecoration { None, Underline, Overline, LineThrough }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextTransform { None, Uppercase, Lowercase, Capitalize }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorStyle { Default, Pointer, Text, Move, NotAllowed, Wait, Crosshair, Grab }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhiteSpace { Normal, Nowrap, Pre, PreWrap, PreLine }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontStyle { Normal, Italic, Oblique }

#[derive(Debug, Clone)]
pub struct BoxShadow {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur:     f32,
    pub spread:   f32,
    pub color:    Color,
    pub inset:    bool,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Computed style — all properties resolved to concrete values
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ComputedStyle {
    // Box model
    pub display:          Display,
    pub position:         Position,
    pub overflow_x:       Overflow,
    pub overflow_y:       Overflow,
    pub width:            CssLength,
    pub height:           CssLength,
    pub min_width:        CssLength,
    pub max_width:        CssLength,
    pub min_height:       CssLength,
    pub max_height:       CssLength,
    pub margin_top:       CssLength,
    pub margin_right:     CssLength,
    pub margin_bottom:    CssLength,
    pub margin_left:      CssLength,
    pub padding_top:      CssLength,
    pub padding_right:    CssLength,
    pub padding_bottom:   CssLength,
    pub padding_left:     CssLength,
    pub border_top:       f32,        // px
    pub border_right:     f32,
    pub border_bottom:    f32,
    pub border_left:      f32,
    pub border_color:     Color,
    // Typography
    pub font_size:        f32,        // px (already resolved)
    pub font_weight:      FontWeight,
    pub line_height:      f32,        // px (already resolved)
    pub text_align:       TextAlign,
    pub color:            Color,
    // Background
    pub background_color: Color,
    // Positioning
    pub top:              CssLength,
    pub right:            CssLength,
    pub bottom:           CssLength,
    pub left:             CssLength,
    pub z_index:          i32,
    // Flex
    pub flex_direction:   FlexDirection,
    pub justify_content:  JustifyContent,
    pub align_items:      AlignItems,
    pub flex_grow:        f32,
    pub flex_shrink:      f32,
    pub flex_basis:       CssLength,
    // Misc
    pub opacity:          f32,
    pub visibility:       bool,       // true = visible
    // Phase 108: Visual effects
    pub border_radius:              f32,
    pub border_top_left_radius:     f32,
    pub border_top_right_radius:    f32,
    pub border_bottom_left_radius:  f32,
    pub border_bottom_right_radius: f32,
    pub box_shadow:                 Option<BoxShadow>,
    pub outline_width:              f32,
    pub outline_color:              Color,
    // Phase 108: Typography extensions
    pub text_decoration:            TextDecoration,
    pub text_transform:             TextTransform,
    pub letter_spacing:             f32,
    pub word_spacing:               f32,
    pub white_space:                WhiteSpace,
    pub font_style:                 FontStyle,
    // Phase 108: UI
    pub cursor:                     CursorStyle,
    pub pointer_events:             bool,   // true = auto, false = none
}

impl ComputedStyle {
    pub fn initial() -> Self {
        ComputedStyle {
            display:       Display::Block,
            position:      Position::Static,
            overflow_x:    Overflow::Visible,
            overflow_y:    Overflow::Visible,
            width:         CssLength::Auto,
            height:        CssLength::Auto,
            min_width:     CssLength::Zero,
            max_width:     CssLength::Auto,
            min_height:    CssLength::Zero,
            max_height:    CssLength::Auto,
            margin_top:    CssLength::Zero,
            margin_right:  CssLength::Zero,
            margin_bottom: CssLength::Zero,
            margin_left:   CssLength::Zero,
            padding_top:   CssLength::Zero,
            padding_right: CssLength::Zero,
            padding_bottom:CssLength::Zero,
            padding_left:  CssLength::Zero,
            border_top:    0.0,
            border_right:  0.0,
            border_bottom: 0.0,
            border_left:   0.0,
            border_color:  Color::BLACK,
            font_size:     16.0,
            font_weight:   FontWeight::Normal,
            line_height:   20.0,
            text_align:    TextAlign::Left,
            color:         Color::BLACK,
            background_color: Color::TRANSPARENT,
            top:    CssLength::Auto,
            right:  CssLength::Auto,
            bottom: CssLength::Auto,
            left:   CssLength::Auto,
            z_index: 0,
            flex_direction:  FlexDirection::Row,
            justify_content: JustifyContent::FlexStart,
            align_items:     AlignItems::Stretch,
            flex_grow:   0.0,
            flex_shrink: 1.0,
            flex_basis:  CssLength::Auto,
            opacity:     1.0,
            visibility:  true,
            // Phase 108
            border_radius:              0.0,
            border_top_left_radius:     0.0,
            border_top_right_radius:    0.0,
            border_bottom_left_radius:  0.0,
            border_bottom_right_radius: 0.0,
            box_shadow:                 None,
            outline_width:              0.0,
            outline_color:              Color::BLACK,
            text_decoration:            TextDecoration::None,
            text_transform:             TextTransform::None,
            letter_spacing:             0.0,
            word_spacing:               0.0,
            white_space:                WhiteSpace::Normal,
            font_style:                 FontStyle::Normal,
            cursor:                     CursorStyle::Default,
            pointer_events:             true,
        }
    }

    /// Inherit inheritable properties from parent.
    pub fn inherit_from(parent: &ComputedStyle) -> Self {
        let mut s = Self::initial();
        s.font_size      = parent.font_size;
        s.font_weight    = parent.font_weight.clone();
        s.font_style     = parent.font_style.clone();
        s.line_height    = parent.line_height;
        s.text_align     = parent.text_align.clone();
        s.text_transform = parent.text_transform.clone();
        s.letter_spacing = parent.letter_spacing;
        s.word_spacing   = parent.word_spacing;
        s.white_space    = parent.white_space.clone();
        s.color          = parent.color;
        s.visibility     = parent.visibility;
        s.cursor         = parent.cursor.clone();
        s
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  CSS Selector
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrOp { Exists, Eq, Contains, StartsWith, EndsWith, DashPrefix }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrSelector { pub name: String, pub op: AttrOp, pub value: String }

/// A single simple selector (no combinator).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SimpleSelector {
    pub tag:        Option<String>,
    pub id:         Option<String>,
    pub classes:    Vec<String>,
    pub attrs:      Vec<AttrSelector>,
    pub universal:  bool,
    // Pseudo-classes (simplified).
    pub pseudo:     Vec<String>,
}

/// One segment of a compound selector: a simple selector + how it relates to the next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Combinator { Descendant, Child, Adjacent, Sibling }

#[derive(Debug, Clone)]
pub struct SelectorSegment {
    pub simple:     SimpleSelector,
    pub combinator: Option<Combinator>, // None = this is the last segment
}

#[derive(Debug, Clone)]
pub struct Selector {
    pub segments: Vec<SelectorSegment>, // left to right (ancestor first)
}

impl Selector {
    /// Calculate specificity as (id_count, class_count, tag_count).
    pub fn specificity(&self) -> (u32, u32, u32) {
        let mut ids = 0u32; let mut cls = 0u32; let mut tags = 0u32;
        for seg in &self.segments {
            if seg.simple.id.is_some() { ids += 1; }
            cls += seg.simple.classes.len() as u32;
            cls += seg.simple.attrs.len() as u32;
            cls += seg.simple.pseudo.len() as u32;
            if seg.simple.tag.is_some() { tags += 1; }
        }
        (ids, cls, tags)
    }

    pub fn specificity_value(&self) -> u32 {
        let (a, b, c) = self.specificity();
        a * 100 + b * 10 + c
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  CSS Declaration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Declaration {
    pub property:  String,
    pub value:     String,
    pub important: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
//  CSS Rule
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CssRule {
    pub selectors:    Vec<Selector>,
    pub declarations: Vec<Declaration>,
}

// ─────────────────────────────────────────────────────────────────────────────
//  CSS Stylesheet
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Stylesheet {
    pub rules: Vec<CssRule>,
}

impl Stylesheet {
    pub fn empty() -> Self { Stylesheet { rules: Vec::new() } }
}

// ─────────────────────────────────────────────────────────────────────────────
//  CSS Parser
// ─────────────────────────────────────────────────────────────────────────────

pub struct CssParser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> CssParser<'a> {
    pub fn new(src: &'a str) -> Self { CssParser { src, pos: 0 } }

    fn peek(&self) -> Option<char> { self.src[self.pos..].chars().next() }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() { self.advance(); }
            else if self.src[self.pos..].starts_with("/*") {
                // Skip comment.
                self.pos += 2;
                while self.pos + 1 < self.src.len() {
                    if &self.src[self.pos..self.pos+2] == "*/" { self.pos += 2; break; }
                    self.pos += 1;
                }
            } else { break; }
        }
    }

    fn consume_while(&mut self, pred: impl Fn(char) -> bool) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if !pred(c) { break; }
            s.push(c);
            self.advance();
        }
        s
    }

    fn consume_string(&mut self, delim: char) -> String {
        let mut s = String::new();
        while let Some(c) = self.advance() {
            if c == delim { break; }
            if c == '\\' { if let Some(esc) = self.advance() { s.push(esc); } }
            else { s.push(c); }
        }
        s
    }

    /// Parse a full stylesheet.
    pub fn parse_stylesheet(&mut self) -> Stylesheet {
        let mut rules = Vec::new();
        loop {
            self.skip_whitespace();
            if self.pos >= self.src.len() { break; }
            if self.src[self.pos..].starts_with("@media") {
                if let Some(block) = self.parse_at_media() {
                    rules.extend(block);
                }
                continue;
            }
            if self.src[self.pos..].starts_with('@') {
                // Skip unknown at-rules.
                while let Some(c) = self.advance() {
                    if c == ';' || c == '{' { if c == '{' { self.skip_block(); } break; }
                }
                continue;
            }
            if let Some(rule) = self.parse_rule() {
                rules.push(rule);
            }
        }
        Stylesheet { rules }
    }

    fn skip_block(&mut self) {
        let mut depth = 1u32;
        while let Some(c) = self.advance() {
            if c == '{' { depth += 1; }
            else if c == '}' { depth -= 1; if depth == 0 { break; } }
        }
    }

    fn parse_at_media(&mut self) -> Option<Vec<CssRule>> {
        // Skip "@media" keyword.
        self.pos += "@media".len();
        self.skip_whitespace();
        // Collect media condition until '{'.
        let mut condition = String::new();
        while let Some(c) = self.peek() {
            if c == '{' { self.advance(); break; }
            condition.push(c);
            self.advance();
        }
        // Evaluate: only honor (min-width: Npx) and (max-width: Npx) for now.
        // Always include for simplicity (Phase 35 stub).
        let applies = eval_media_query(&condition);
        let mut rules = Vec::new();
        loop {
            self.skip_whitespace();
            if self.pos >= self.src.len() { break; }
            if self.peek() == Some('}') { self.advance(); break; }
            if let Some(rule) = self.parse_rule() {
                if applies { rules.push(rule); }
            }
        }
        Some(rules)
    }

    fn parse_rule(&mut self) -> Option<CssRule> {
        let selectors = self.parse_selector_list()?;
        self.skip_whitespace();
        if self.peek() != Some('{') { return None; }
        self.advance(); // '{'
        let declarations = self.parse_declaration_block();
        Some(CssRule { selectors, declarations })
    }

    fn parse_selector_list(&mut self) -> Option<Vec<Selector>> {
        let mut selectors = Vec::new();
        loop {
            self.skip_whitespace();
            if self.pos >= self.src.len() || self.peek() == Some('{') { break; }
            if let Some(sel) = self.parse_selector() {
                selectors.push(sel);
            }
            self.skip_whitespace();
            if self.peek() == Some(',') { self.advance(); } else { break; }
        }
        if selectors.is_empty() { None } else { Some(selectors) }
    }

    fn parse_selector(&mut self) -> Option<Selector> {
        let mut segments = Vec::new();
        loop {
            self.skip_whitespace();
            let p = self.peek();
            if p == Some('{') || p == Some(',') || p.is_none() { break; }

            let simple = self.parse_simple_selector();
            if simple.tag.is_none() && simple.id.is_none()
                && simple.classes.is_empty() && simple.attrs.is_empty()
                && !simple.universal && simple.pseudo.is_empty()
            {
                break;
            }

            // Look ahead for combinator.
            let saved = self.pos;
            let had_ws = self.src[self.pos..].starts_with(|c: char| c.is_whitespace());
            self.skip_whitespace();
            let combinator = match self.peek() {
                Some('>') => { self.advance(); self.skip_whitespace(); Some(Combinator::Child) }
                Some('+') => { self.advance(); self.skip_whitespace(); Some(Combinator::Adjacent) }
                Some('~') => { self.advance(); self.skip_whitespace(); Some(Combinator::Sibling) }
                Some('{') | Some(',') | None => None,
                _ => {
                    if had_ws { Some(Combinator::Descendant) }
                    else { None }
                }
            };

            let is_last = combinator.is_none();
            segments.push(SelectorSegment { simple, combinator });
            if is_last { break; }
        }
        if segments.is_empty() { None } else { Some(Selector { segments }) }
    }

    fn parse_simple_selector(&mut self) -> SimpleSelector {
        let mut sel = SimpleSelector::default();
        loop {
            match self.peek() {
                Some('*') => { self.advance(); sel.universal = true; }
                Some('#') => {
                    self.advance();
                    sel.id = Some(self.consume_ident());
                }
                Some('.') => {
                    self.advance();
                    sel.classes.push(self.consume_ident());
                }
                Some(':') => {
                    self.advance();
                    if self.peek() == Some(':') { self.advance(); } // ::pseudo-element
                    let name = self.consume_ident();
                    // Capture optional argument, e.g. :nth-child(2n+1), :not(.foo)
                    let pseudo = if self.peek() == Some('(') {
                        self.advance(); // '('
                        let arg = self.consume_while(|c| c != ')');
                        if self.peek() == Some(')') { self.advance(); }
                        alloc::format!("{}({})", name, arg)
                    } else {
                        name
                    };
                    sel.pseudo.push(pseudo);
                }
                Some('[') => {
                    self.advance();
                    self.skip_whitespace();
                    let name = self.consume_while(|c| c.is_alphanumeric() || c == '-' || c == '_');
                    self.skip_whitespace();
                    let op = match self.peek() {
                        Some('=') => { self.advance(); AttrOp::Eq }
                        Some('~') => { self.advance(); self.advance(); AttrOp::Contains }
                        Some('^') => { self.advance(); self.advance(); AttrOp::StartsWith }
                        Some('$') => { self.advance(); self.advance(); AttrOp::EndsWith }
                        Some('|') => { self.advance(); self.advance(); AttrOp::DashPrefix }
                        _         => AttrOp::Exists,
                    };
                    self.skip_whitespace();
                    let value = if self.peek() == Some('"') {
                        self.advance(); self.consume_string('"')
                    } else if self.peek() == Some('\'') {
                        self.advance(); self.consume_string('\'')
                    } else {
                        self.consume_while(|c| c != ']' && !c.is_whitespace())
                    };
                    self.skip_whitespace();
                    if self.peek() == Some(']') { self.advance(); }
                    sel.attrs.push(AttrSelector { name, op, value });
                }
                Some(c) if c.is_ascii_alphabetic() || c == '-' || c == '_' => {
                    sel.tag = Some(self.consume_ident().to_ascii_lowercase());
                }
                _ => break,
            }
        }
        sel
    }

    fn consume_ident(&mut self) -> String {
        self.consume_while(|c| c.is_alphanumeric() || c == '-' || c == '_')
    }

    fn parse_declaration_block(&mut self) -> Vec<Declaration> {
        let mut decls = Vec::new();
        loop {
            self.skip_whitespace();
            if self.peek() == Some('}') { self.advance(); break; }
            if self.pos >= self.src.len() { break; }
            if let Some(decl) = self.parse_declaration() {
                decls.push(decl);
            }
        }
        decls
    }

    fn parse_declaration(&mut self) -> Option<Declaration> {
        let property = self.consume_while(|c| c != ':' && c != '}' && !c.is_whitespace())
            .to_ascii_lowercase();
        self.skip_whitespace();
        if self.peek() != Some(':') { self.consume_while(|c| c != ';' && c != '}'); return None; }
        self.advance(); // ':'
        self.skip_whitespace();

        // Collect value up to ';' or '}'.
        let mut value = String::new();
        let mut depth = 0u32;
        while let Some(c) = self.peek() {
            match c {
                '(' => { depth += 1; value.push(c); self.advance(); }
                ')' => {
                    if depth > 0 { depth -= 1; value.push(c); self.advance(); }
                    else { break; }
                }
                ';' if depth == 0 => { self.advance(); break; }
                '}' if depth == 0 => break,
                '"' => { self.advance(); let s = self.consume_string('"'); value.push('"'); value.push_str(&s); value.push('"'); }
                '\'' => { self.advance(); let s = self.consume_string('\''); value.push('\''); value.push_str(&s); value.push('\''); }
                _ => { value.push(c); self.advance(); }
            }
        }

        let value = value.trim().to_string();
        let important = value.ends_with("!important")
            || value.to_ascii_lowercase().contains("!important");
        let value = value.replace("!important", "").trim().to_string();

        if property.is_empty() { return None; }
        Some(Declaration { property, value, important })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Media query evaluator (stub — viewport always assumed 1280px wide)
// ─────────────────────────────────────────────────────────────────────────────

const VIEWPORT_WIDTH: f32 = 1280.0;
const VIEWPORT_HEIGHT: f32 = 720.0;

fn eval_media_query(condition: &str) -> bool {
    let cond = condition.trim().to_ascii_lowercase();
    if cond == "all" || cond == "screen" || cond.is_empty() { return true; }
    if cond == "print" { return false; }
    // Try to parse (min-width: Npx) or (max-width: Npx).
    if let Some(inner) = cond.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        if let Some(val_str) = inner.strip_prefix("min-width:") {
            let px = parse_px_number(val_str.trim());
            return VIEWPORT_WIDTH >= px;
        }
        if let Some(val_str) = inner.strip_prefix("max-width:") {
            let px = parse_px_number(val_str.trim());
            return VIEWPORT_WIDTH <= px;
        }
    }
    true // default: apply
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - f32_abs(2.0 * l - 1.0)) * s;
    let x = c * (1.0 - f32_abs((h * 6.0) % 2.0 - 1.0));
    let m = l - c / 2.0;
    let (r, g, b) = if h < 1.0/6.0 { (c, x, 0.0) }
        else if h < 2.0/6.0 { (x, c, 0.0) }
        else if h < 3.0/6.0 { (0.0, c, x) }
        else if h < 4.0/6.0 { (0.0, x, c) }
        else if h < 5.0/6.0 { (x, 0.0, c) }
        else { (c, 0.0, x) };
    (((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8)
}
fn f32_abs(x: f32) -> f32 { if x < 0.0 { -x } else { x } }

fn parse_px_number(s: &str) -> f32 {
    let s = s.trim_end_matches("px").trim_end_matches("em").trim();
    s.parse::<f32>().unwrap_or(0.0)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Value parsers
// ─────────────────────────────────────────────────────────────────────────────

pub fn parse_length(s: &str) -> CssLength {
    let s = s.trim();
    if s == "auto"        { return CssLength::Auto; }
    if s == "0"           { return CssLength::Zero; }
    // calc() — delegate to the calc resolver
    if s.starts_with("calc(") { return parse_length_with_calc(s); }
    // var() — return Auto as placeholder (vars should be resolved before calling parse_length)
    if s.starts_with("var(") { return CssLength::Auto; }
    if let Some(v) = s.strip_suffix("px") {
        return v.trim().parse::<f32>().map(CssLength::Px).unwrap_or(CssLength::Zero);
    }
    if let Some(v) = s.strip_suffix("em") {
        return v.trim().parse::<f32>().map(CssLength::Em).unwrap_or(CssLength::Zero);
    }
    if let Some(v) = s.strip_suffix("rem") {
        return v.trim().parse::<f32>().map(CssLength::Rem).unwrap_or(CssLength::Zero);
    }
    if let Some(v) = s.strip_suffix("vw") {
        let pct = v.trim().parse::<f32>().unwrap_or(0.0);
        return CssLength::Px(pct / 100.0 * VIEWPORT_WIDTH);
    }
    if let Some(v) = s.strip_suffix("vh") {
        let pct = v.trim().parse::<f32>().unwrap_or(0.0);
        return CssLength::Px(pct / 100.0 * VIEWPORT_HEIGHT);
    }
    if let Some(v) = s.strip_suffix('%') {
        return v.trim().parse::<f32>().map(CssLength::Percent).unwrap_or(CssLength::Zero);
    }
    // Unitless number → px.
    s.parse::<f32>().map(CssLength::Px).unwrap_or(CssLength::Auto)
}

pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    // #rrggbb or #rgb or #rrggbbaa
    if s.starts_with('#') {
        let hex = &s[1..];
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                Some(Color { r, g, b, a: 255 })
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Color { r, g, b, a: 255 })
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some(Color { r, g, b, a })
            }
            _ => None,
        };
    }
    // rgb(r, g, b) / rgba(r, g, b, a)
    if s.starts_with("rgb") {
        let inner = s.trim_start_matches("rgba(").trim_start_matches("rgb(").trim_end_matches(')');
        let parts: Vec<&str> = inner.split(',').collect();
        let r = parts.get(0)?.trim().parse::<u8>().ok()?;
        let g = parts.get(1)?.trim().parse::<u8>().ok()?;
        let b = parts.get(2)?.trim().parse::<u8>().ok()?;
        let a = if parts.len() >= 4 {
            (parts[3].trim().parse::<f32>().unwrap_or(1.0) * 255.0) as u8
        } else { 255 };
        return Some(Color { r, g, b, a });
    }
    // hsl(H, S%, L%) / hsla(H, S%, L%, A)
    if s.starts_with("hsl") {
        let inner = s.trim_start_matches("hsla(").trim_start_matches("hsl(").trim_end_matches(')');
        let parts: Vec<&str> = inner.split(',').collect();
        let h = parts.get(0)?.trim().parse::<f32>().ok()? / 360.0;
        let sat = parts.get(1)?.trim().trim_end_matches('%').parse::<f32>().ok()? / 100.0;
        let lit = parts.get(2)?.trim().trim_end_matches('%').parse::<f32>().ok()? / 100.0;
        let a = if parts.len() >= 4 {
            (parts[3].trim().parse::<f32>().unwrap_or(1.0) * 255.0) as u8
        } else { 255 };
        let (r, g, b) = hsl_to_rgb(h, sat, lit);
        return Some(Color { r, g, b, a });
    }
    // Named colors (common subset).
    named_color(s)
}

fn named_color(name: &str) -> Option<Color> {
    let c = |r,g,b| Some(Color { r, g, b, a: 255 });
    match name.to_ascii_lowercase().as_str() {
        "transparent"   => Some(Color::TRANSPARENT),
        "black"         => c(0,0,0),
        "white"         => c(255,255,255),
        "red"           => c(255,0,0),
        "green"         => c(0,128,0),
        "lime"          => c(0,255,0),
        "blue"          => c(0,0,255),
        "yellow"        => c(255,255,0),
        "cyan"|"aqua"   => c(0,255,255),
        "magenta"|"fuchsia" => c(255,0,255),
        "orange"        => c(255,165,0),
        "pink"          => c(255,192,203),
        "purple"        => c(128,0,128),
        "gray"|"grey"   => c(128,128,128),
        "silver"        => c(192,192,192),
        "darkgray"|"darkgrey" => c(64,64,64),
        "lightgray"|"lightgrey" => c(211,211,211),
        "navy"          => c(0,0,128),
        "teal"          => c(0,128,128),
        "maroon"        => c(128,0,0),
        "olive"         => c(128,128,0),
        "brown"         => c(139,69,19),
        "coral"         => c(255,127,80),
        "salmon"        => c(250,128,114),
        "gold"          => c(255,215,0),
        "indigo"        => c(75,0,130),
        "violet"        => c(238,130,238),
        "wheat"         => c(245,222,179),
        "ivory"         => c(255,255,240),
        "beige"         => c(245,245,220),
        "lavender"      => c(230,230,250),
        "crimson"       => c(220,20,60),
        "tomato"        => c(255,99,71),
        "skyblue"       => c(135,206,235),
        "steelblue"     => c(70,130,180),
        "deepskyblue"   => c(0,191,255),
        "dodgerblue"    => c(30,144,255),
        "forestgreen"   => c(34,139,34),
        "limegreen"     => c(50,205,50),
        "darkgreen"     => c(0,100,0),
        "lightgreen"    => c(144,238,144),
        "khaki"         => c(240,230,140),
        "tan"           => c(210,180,140),
        "chocolate"     => c(210,105,30),
        "sienna"        => c(160,82,45),
        _               => None,
    }
}

/// Parse a 1-to-4 value margin/padding shorthand.
fn parse_4_shorthand(val: &str) -> [CssLength; 4] {
    let parts: Vec<&str> = val.split_whitespace().collect();
    match parts.len() {
        1 => { let v = parse_length(parts[0]); [v.clone(), v.clone(), v.clone(), v] }
        2 => { let v = parse_length(parts[0]); let h = parse_length(parts[1]); [v.clone(), h.clone(), v, h] }
        3 => { let t = parse_length(parts[0]); let lr = parse_length(parts[1]); let b = parse_length(parts[2]); [t, lr.clone(), b, lr] }
        _ => { [parse_length(parts[0]), parse_length(parts[1]), parse_length(parts.get(2).unwrap_or(&"0")), parse_length(parts.get(3).unwrap_or(&"0"))] }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Selector matching
// ─────────────────────────────────────────────────────────────────────────────

pub fn selector_matches(sel: &Selector, node_id: NodeId, dom: &Dom) -> bool {
    let segments = &sel.segments;
    if segments.is_empty() { return false; }
    // Check the rightmost simple selector against this node.
    let last = &segments[segments.len() - 1];
    if !simple_matches(&last.simple, node_id, dom) { return false; }
    // Walk up ancestor chain for the rest.
    if segments.len() == 1 { return true; }
    check_ancestors(segments, segments.len() - 1, node_id, dom)
}

fn check_ancestors(segments: &[SelectorSegment], seg_idx: usize, node_id: NodeId, dom: &Dom) -> bool {
    if seg_idx == 0 { return true; }
    let prev_seg = &segments[seg_idx - 1];
    let combinator = match prev_seg.combinator.as_ref() {
        Some(c) => c,
        None    => return false,
    };
    let parent_id = match dom.get(node_id).map(|n| n.parent) {
        Some(id) => id,
        None     => return false,
    };
    if parent_id == NULL_NODE { return false; }

    match combinator {
        Combinator::Child => {
            if !simple_matches(&prev_seg.simple, parent_id, dom) { return false; }
            check_ancestors(segments, seg_idx - 1, parent_id, dom)
        }
        Combinator::Descendant => {
            let mut anc = parent_id;
            loop {
                if simple_matches(&prev_seg.simple, anc, dom) {
                    if check_ancestors(segments, seg_idx - 1, anc, dom) { return true; }
                }
                anc = dom.get(anc).map(|n| n.parent).unwrap_or(NULL_NODE);
                if anc == NULL_NODE { return false; }
            }
        }
        Combinator::Adjacent => {
            let siblings = dom.get(parent_id).map(|n| n.children.clone()).unwrap_or_default();
            let pos = match siblings.iter().position(|&id| id == node_id) {
                Some(p) => p,
                None    => return false,
            };
            if pos == 0 { return false; }
            let prev_sib = siblings[pos - 1];
            if simple_matches(&prev_seg.simple, prev_sib, dom) {
                return check_ancestors(segments, seg_idx - 1, prev_sib, dom);
            }
            false
        }
        Combinator::Sibling => {
            let siblings = dom.get(parent_id).map(|n| n.children.clone()).unwrap_or_default();
            let pos = match siblings.iter().position(|&id| id == node_id) {
                Some(p) => p,
                None    => return false,
            };
            for &sib in &siblings[..pos] {
                if simple_matches(&prev_seg.simple, sib, dom) {
                    if check_ancestors(segments, seg_idx - 1, sib, dom) { return true; }
                }
            }
            false
        }
    }
}

fn simple_matches(sel: &SimpleSelector, node_id: NodeId, dom: &Dom) -> bool {
    let node = match dom.get(node_id) { Some(n) => n, None => return false };
    let (tag, attrs) = match &node.kind {
        NodeKind::Element { tag, attrs } => (tag.as_str(), attrs),
        _ => return false,
    };

    if sel.universal { return true; }
    if let Some(ref t) = sel.tag { if t != tag { return false; } }
    if let Some(ref id) = sel.id {
        if attrs.get("id").map(|s| s.as_str()) != Some(id.as_str()) { return false; }
    }
    for cls in &sel.classes {
        let has = attrs.get("class")
            .map(|c| c.split_whitespace().any(|x| x == cls))
            .unwrap_or(false);
        if !has { return false; }
    }
    for attr in &sel.attrs {
        let av = attrs.get(&attr.name);
        match attr.op {
            AttrOp::Exists     => { if av.is_none() { return false; } }
            AttrOp::Eq         => { if av.map(|s| s.as_str()) != Some(&attr.value) { return false; } }
            AttrOp::Contains   => { if !av.map(|s| s.split_whitespace().any(|w| w == attr.value)).unwrap_or(false) { return false; } }
            AttrOp::StartsWith => { if !av.map(|s| s.starts_with(attr.value.as_str())).unwrap_or(false) { return false; } }
            AttrOp::EndsWith   => { if !av.map(|s| s.ends_with(attr.value.as_str())).unwrap_or(false) { return false; } }
            AttrOp::DashPrefix => { if !av.map(|s| s == attr.value.as_str() || s.starts_with(&format!("{}-", attr.value))).unwrap_or(false) { return false; } }
        }
    }
    // Phase 108: evaluate pseudo-classes
    for pseudo in &sel.pseudo {
        if !matches_pseudo(pseudo, node_id, dom) { return false; }
    }
    true
}

/// Phase 108: Evaluate a single pseudo-class against a DOM node.
fn matches_pseudo(pseudo: &str, node_id: NodeId, dom: &Dom) -> bool {
    // Interactive pseudo-classes — no dynamic state in static render; skip them
    // (return true so we don't accidentally suppress valid rules that also have
    // non-pseudo matches; the render is static so :hover rules just don't fire)
    match pseudo {
        "hover" | "focus" | "active" | "focus-within" | "focus-visible" => return false,
        "visited"  => return false,
        "link"     => return true,  // treat all <a> as unvisited links
        _ => {}
    }

    let node = match dom.get(node_id) { Some(n) => n, None => return false };
    let attrs = match &node.kind {
        NodeKind::Element { attrs, tag } => {
            // :root — in HTML always matches the <html> element
            if pseudo == "root" { return tag.as_str() == "html"; }
            attrs
        }
        _ => return false,
    };

    match pseudo {
        // Attribute-state pseudo-classes
        "checked"  => attrs.contains_key("checked"),
        "selected" => attrs.contains_key("selected"),
        "disabled" => attrs.contains_key("disabled"),
        "enabled"  => !attrs.contains_key("disabled"),
        "required" => attrs.contains_key("required"),
        "optional" => !attrs.contains_key("required"),
        "read-only"  | "readonly"  => attrs.contains_key("readonly") || attrs.get("contenteditable").map(|v| v == "false").unwrap_or(false),
        "read-write" => !attrs.contains_key("readonly"),
        "placeholder-shown" => attrs.contains_key("placeholder"),

        // Structural pseudo-classes
        "empty" => node.children.is_empty(),
        "first-child" => {
            let parent = match dom.get(node.parent) { Some(p) => p, None => return true };
            parent.children.first() == Some(&node_id)
        }
        "last-child" => {
            let parent = match dom.get(node.parent) { Some(p) => p, None => return true };
            parent.children.last() == Some(&node_id)
        }
        "only-child" => {
            let parent = match dom.get(node.parent) { Some(p) => p, None => return true };
            parent.children.len() == 1
        }
        "first-of-type" => is_first_of_type(node_id, dom),
        "last-of-type"  => is_last_of_type(node_id, dom),
        "only-of-type"  => is_first_of_type(node_id, dom) && is_last_of_type(node_id, dom),

        // :nth-child(An+B) and :not(selector)
        p if p.starts_with("nth-child(")  => nth_child_matches(p, node_id, dom, false),
        p if p.starts_with("nth-of-type(") => nth_child_matches(p, node_id, dom, true),
        p if p.starts_with("nth-last-child(") => nth_last_child_matches(p, node_id, dom),
        p if p.starts_with("not(") => {
            // :not(simple-selector) — parse inner selector and negate
            let inner = &p[4..p.len().saturating_sub(1)];
            let mut parser = CssParser::new(inner);
            let sel = parser.parse_simple_selector();
            !simple_matches(&sel, node_id, dom)
        }
        p if p.starts_with("is(") || p.starts_with("where(") => {
            // :is() / :where() — accept and return true (permissive)
            true
        }
        p if p.starts_with("has(") => false, // :has() not supported in static render
        // before/after/placeholder/selection/etc. pseudo-elements — accept silently
        "before" | "after" | "placeholder" | "selection" |
        "first-line" | "first-letter" | "backdrop" | "marker" | "cue" => true,
        // Unknown pseudo — accept (future-proof)
        _ => true,
    }
}

fn is_first_of_type(node_id: NodeId, dom: &Dom) -> bool {
    let node = match dom.get(node_id) { Some(n) => n, None => return true };
    let tag = match &node.kind { NodeKind::Element { tag, .. } => tag.clone(), _ => return false };
    let parent = match dom.get(node.parent) { Some(p) => p, None => return true };
    for &sib in &parent.children {
        if sib == node_id { return true; }
        if let Some(s) = dom.get(sib) {
            if matches!(&s.kind, NodeKind::Element { tag: t, .. } if t == &tag) { return false; }
        }
    }
    true
}

fn is_last_of_type(node_id: NodeId, dom: &Dom) -> bool {
    let node = match dom.get(node_id) { Some(n) => n, None => return true };
    let tag = match &node.kind { NodeKind::Element { tag, .. } => tag.clone(), _ => return false };
    let parent = match dom.get(node.parent) { Some(p) => p, None => return true };
    let mut found_after = false;
    for &sib in parent.children.iter().rev() {
        if sib == node_id { return !found_after; }
        if let Some(s) = dom.get(sib) {
            if matches!(&s.kind, NodeKind::Element { tag: t, .. } if t == &tag) { found_after = true; }
        }
    }
    true
}

/// Parse An+B notation and check if a node is the (An+B)th child.
fn nth_child_matches(pseudo: &str, node_id: NodeId, dom: &Dom, of_type: bool) -> bool {
    // Extract the argument e.g. "nth-child(2n+1)" → "2n+1"
    let arg = if let Some(start) = pseudo.find('(') {
        let s = &pseudo[start+1..];
        s.trim_end_matches(')').trim()
    } else { return false };

    let node = match dom.get(node_id) { Some(n) => n, None => return false };
    let tag = match &node.kind { NodeKind::Element { tag, .. } => tag.clone(), _ => return false };
    let parent = match dom.get(node.parent) { Some(p) => p, None => return false };

    // Get 1-based position among siblings (optionally filtered by type)
    let pos: usize = parent.children.iter()
        .filter(|&&id| {
            if of_type {
                dom.get(id).map(|s| matches!(&s.kind, NodeKind::Element { tag: t, .. } if t == &tag)).unwrap_or(false)
            } else {
                dom.get(id).map(|s| matches!(s.kind, NodeKind::Element { .. })).unwrap_or(false)
            }
        })
        .position(|&id| id == node_id)
        .map(|p| p + 1)
        .unwrap_or(0);

    if pos == 0 { return false; }
    eval_nth(arg, pos)
}

fn nth_last_child_matches(pseudo: &str, node_id: NodeId, dom: &Dom) -> bool {
    let arg = if let Some(start) = pseudo.find('(') {
        let s = &pseudo[start+1..];
        s.trim_end_matches(')').trim()
    } else { return false };

    let node = match dom.get(node_id) { Some(n) => n, None => return false };
    let parent = match dom.get(node.parent) { Some(p) => p, None => return false };
    let element_count = parent.children.iter()
        .filter(|&&id| dom.get(id).map(|s| matches!(s.kind, NodeKind::Element { .. })).unwrap_or(false))
        .count();
    let pos_from_start: usize = parent.children.iter()
        .filter(|&&id| dom.get(id).map(|s| matches!(s.kind, NodeKind::Element { .. })).unwrap_or(false))
        .position(|&id| id == node_id)
        .map(|p| p + 1)
        .unwrap_or(0);

    if pos_from_start == 0 { return false; }
    let pos_from_end = element_count + 1 - pos_from_start;
    eval_nth(arg, pos_from_end)
}

/// Evaluate An+B selector against a 1-based position.
pub fn eval_nth(arg: &str, pos: usize) -> bool {
    let arg = arg.trim();
    match arg {
        "odd"  => pos % 2 == 1,
        "even" => pos % 2 == 0,
        _ => {
            if let Some(n_pos) = arg.find('n') {
                let a_str = arg[..n_pos].trim();
                let a: i64 = if a_str.is_empty() { 1 }
                    else if a_str == "-" { -1 }
                    else { a_str.parse().unwrap_or(1) };
                let b_str = arg[n_pos+1..].trim();
                let b: i64 = if b_str.is_empty() { 0 } else { b_str.parse().unwrap_or(0) };
                if a == 0 { return pos as i64 == b; }
                let diff = pos as i64 - b;
                if a > 0 { diff >= 0 && diff % a == 0 }
                else     { diff <= 0 && diff % (-a) == 0 }
            } else {
                arg.parse::<usize>().map(|n| n == pos).unwrap_or(false)
            }
        }
    }
}


// ─────────────────────────────────────────────────────────────────────────────
//  Cascade + computed style builder
// ─────────────────────────────────────────────────────────────────────────────

/// A matched rule with its specificity (for sorting).
struct MatchedRule<'a> {
    specificity: u32,
    rule_idx:    usize,
    declarations: &'a [Declaration],
}

/// Apply a CSS property+value to a mutable ComputedStyle.
fn apply_property(style: &mut ComputedStyle, prop: &str, val: &str) {
    let val = val.trim();
    match prop {
        "display" => {
            style.display = match val {
                "block"        => Display::Block,
                "inline"       => Display::Inline,
                "inline-block" => Display::InlineBlock,
                "flex"         => Display::Flex,
                "grid"         => Display::Grid,
                "none"         => Display::None,
                "list-item"    => Display::ListItem,
                _ => style.display.clone(),
            };
        }
        "position" => {
            style.position = match val {
                "relative" => Position::Relative,
                "absolute" => Position::Absolute,
                "fixed"    => Position::Fixed,
                "sticky"   => Position::Sticky,
                _ => Position::Static,
            };
        }
        "overflow" => {
            let ov = parse_overflow(val);
            style.overflow_x = ov.clone();
            style.overflow_y = ov;
        }
        "overflow-x" => { style.overflow_x = parse_overflow(val); }
        "overflow-y" => { style.overflow_y = parse_overflow(val); }
        "width"      => { style.width  = parse_length(val); }
        "height"     => { style.height = parse_length(val); }
        "min-width"  => { style.min_width  = parse_length(val); }
        "max-width"  => { style.max_width  = parse_length(val); }
        "min-height" => { style.min_height = parse_length(val); }
        "max-height" => { style.max_height = parse_length(val); }
        "margin" => {
            let [t,r,b,l] = parse_4_shorthand(val);
            style.margin_top = t; style.margin_right = r;
            style.margin_bottom = b; style.margin_left = l;
        }
        "margin-top"    => { style.margin_top    = parse_length(val); }
        "margin-right"  => { style.margin_right  = parse_length(val); }
        "margin-bottom" => { style.margin_bottom = parse_length(val); }
        "margin-left"   => { style.margin_left   = parse_length(val); }
        "padding" => {
            let [t,r,b,l] = parse_4_shorthand(val);
            style.padding_top = t; style.padding_right = r;
            style.padding_bottom = b; style.padding_left = l;
        }
        "padding-top"    => { style.padding_top    = parse_length(val); }
        "padding-right"  => { style.padding_right  = parse_length(val); }
        "padding-bottom" => { style.padding_bottom = parse_length(val); }
        "padding-left"   => { style.padding_left   = parse_length(val); }
        "border" | "border-width" => {
            // "1px solid #333" → extract first length token.
            let parts: Vec<&str> = val.split_whitespace().collect();
            let bw = parts.first().map(|s| parse_px_number(s)).unwrap_or(0.0);
            style.border_top = bw; style.border_right = bw;
            style.border_bottom = bw; style.border_left = bw;
            // Color is last token if parseable.
            if let Some(col) = parts.last().and_then(|s| parse_color(s)) {
                style.border_color = col;
            }
        }
        "border-top-width"    => { style.border_top    = parse_px_number(val); }
        "border-right-width"  => { style.border_right  = parse_px_number(val); }
        "border-bottom-width" => { style.border_bottom = parse_px_number(val); }
        "border-left-width"   => { style.border_left   = parse_px_number(val); }
        "border-color"        => { if let Some(c) = parse_color(val) { style.border_color = c; } }
        "font-size" => {
            style.font_size = match val {
                "xx-small" => 9.0,  "x-small"  => 10.0, "small"   => 13.0,
                "medium"   => 16.0, "large"    => 18.0,  "x-large" => 24.0,
                "xx-large" => 32.0, "smaller"  => style.font_size * 0.833,
                "larger"   => style.font_size * 1.2,
                _ => parse_length(val).to_px(style.font_size, 16.0),
            };
        }
        "font-weight" => {
            style.font_weight = match val {
                "bold"    => FontWeight::Bold,
                "bolder"  => FontWeight::Bolder,
                "lighter" => FontWeight::Lighter,
                "normal"  => FontWeight::Normal,
                n => n.parse::<u16>().map(FontWeight::Number).unwrap_or(FontWeight::Normal),
            };
        }
        "line-height" => {
            if val == "normal" {
                style.line_height = style.font_size * 1.2;
            } else if val.ends_with("px") {
                style.line_height = parse_px_number(val);
            } else if let Ok(mult) = val.parse::<f32>() {
                style.line_height = style.font_size * mult;
            } else {
                style.line_height = parse_length(val).to_px(style.font_size, 16.0);
            }
        }
        "text-align" => {
            style.text_align = match val {
                "left"    => TextAlign::Left,
                "right"   => TextAlign::Right,
                "center"  => TextAlign::Center,
                "justify" => TextAlign::Justify,
                _ => TextAlign::Left,
            };
        }
        "color" => { if let Some(c) = parse_color(val) { style.color = c; } }
        "background-color" | "background" => {
            // For 'background' shorthand, just try to parse a color token.
            let token = val.split_whitespace()
                .find(|t| parse_color(t).is_some())
                .unwrap_or(val);
            if let Some(c) = parse_color(token) { style.background_color = c; }
        }
        "top"    => { style.top    = parse_length(val); }
        "right"  => { style.right  = parse_length(val); }
        "bottom" => { style.bottom = parse_length(val); }
        "left"   => { style.left   = parse_length(val); }
        "z-index" => { style.z_index = val.trim().parse::<i32>().unwrap_or(0); }
        "opacity" => { style.opacity = val.trim().parse::<f32>().unwrap_or(1.0).max(0.0).min(1.0); }
        "visibility" => { style.visibility = val != "hidden" && val != "collapse"; }
        "flex-direction" => {
            style.flex_direction = match val {
                "row-reverse"    => FlexDirection::RowReverse,
                "column"         => FlexDirection::Column,
                "column-reverse" => FlexDirection::ColumnReverse,
                _                => FlexDirection::Row,
            };
        }
        "justify-content" => {
            style.justify_content = match val {
                "flex-end"      => JustifyContent::FlexEnd,
                "center"        => JustifyContent::Center,
                "space-between" => JustifyContent::SpaceBetween,
                "space-around"  => JustifyContent::SpaceAround,
                "space-evenly"  => JustifyContent::SpaceEvenly,
                _               => JustifyContent::FlexStart,
            };
        }
        "align-items" => {
            style.align_items = match val {
                "flex-end"  => AlignItems::FlexEnd,
                "center"    => AlignItems::Center,
                "stretch"   => AlignItems::Stretch,
                "baseline"  => AlignItems::Baseline,
                _           => AlignItems::FlexStart,
            };
        }
        "flex-grow"   => { style.flex_grow   = val.parse::<f32>().unwrap_or(0.0); }
        "flex-shrink" => { style.flex_shrink = val.parse::<f32>().unwrap_or(1.0); }
        "flex-basis"  => { style.flex_basis  = parse_length(val); }
        "flex" => {
            // shorthand: flex-grow [flex-shrink [flex-basis]]
            let parts: Vec<&str> = val.split_whitespace().collect();
            if parts.len() >= 1 { style.flex_grow   = parts[0].parse::<f32>().unwrap_or(0.0); }
            if parts.len() >= 2 { style.flex_shrink = parts[1].parse::<f32>().unwrap_or(1.0); }
            if parts.len() >= 3 { style.flex_basis  = parse_length(parts[2]); }
        }
        // Phase 108: border-radius
        "border-radius" => {
            let r = parse_px_number(val.split_whitespace().next().unwrap_or("0"));
            style.border_radius = r;
            style.border_top_left_radius     = r;
            style.border_top_right_radius    = r;
            style.border_bottom_left_radius  = r;
            style.border_bottom_right_radius = r;
        }
        "border-top-left-radius"     => { style.border_top_left_radius     = parse_px_number(val); }
        "border-top-right-radius"    => { style.border_top_right_radius    = parse_px_number(val); }
        "border-bottom-left-radius"  => { style.border_bottom_left_radius  = parse_px_number(val); }
        "border-bottom-right-radius" => { style.border_bottom_right_radius = parse_px_number(val); }
        // Phase 108: box-shadow — parse "Xpx Ypx [blur [spread]] color [inset]"
        "box-shadow" => {
            if val == "none" { style.box_shadow = None; return; }
            let inset  = val.contains("inset");
            let tokens: Vec<&str> = val.split_whitespace()
                .filter(|&t| t != "inset")
                .collect();
            let mut px_vals = Vec::new();
            let mut color = Color { r:0, g:0, b:0, a:128 };
            for t in &tokens {
                if let Some(c) = parse_color(t) { color = c; }
                else { px_vals.push(parse_px_number(t)); }
            }
            style.box_shadow = Some(BoxShadow {
                offset_x: *px_vals.get(0).unwrap_or(&2.0),
                offset_y: *px_vals.get(1).unwrap_or(&2.0),
                blur:     *px_vals.get(2).unwrap_or(&4.0),
                spread:   *px_vals.get(3).unwrap_or(&0.0),
                color,
                inset,
            });
        }
        // Phase 108: outline
        "outline" => {
            let parts: Vec<&str> = val.split_whitespace().collect();
            style.outline_width = parts.first().map(|s| parse_px_number(s)).unwrap_or(0.0);
            if let Some(col) = parts.last().and_then(|s| parse_color(s)) {
                style.outline_color = col;
            }
        }
        "outline-width" => { style.outline_width = parse_px_number(val); }
        "outline-color" => { if let Some(c) = parse_color(val) { style.outline_color = c; } }
        // Phase 108: text-decoration
        "text-decoration" | "text-decoration-line" => {
            style.text_decoration = match val {
                "underline"    => TextDecoration::Underline,
                "overline"     => TextDecoration::Overline,
                "line-through" => TextDecoration::LineThrough,
                _ => TextDecoration::None,
            };
        }
        // Phase 108: text-transform
        "text-transform" => {
            style.text_transform = match val {
                "uppercase"  => TextTransform::Uppercase,
                "lowercase"  => TextTransform::Lowercase,
                "capitalize" => TextTransform::Capitalize,
                _ => TextTransform::None,
            };
        }
        // Phase 108: letter-spacing / word-spacing
        "letter-spacing" => {
            style.letter_spacing = if val == "normal" { 0.0 } else { parse_px_number(val) };
        }
        "word-spacing" => {
            style.word_spacing = if val == "normal" { 0.0 } else { parse_px_number(val) };
        }
        // Phase 108: white-space
        "white-space" => {
            style.white_space = match val {
                "nowrap"   => WhiteSpace::Nowrap,
                "pre"      => WhiteSpace::Pre,
                "pre-wrap" => WhiteSpace::PreWrap,
                "pre-line" => WhiteSpace::PreLine,
                _ => WhiteSpace::Normal,
            };
        }
        // Phase 108: font-style
        "font-style" => {
            style.font_style = match val {
                "italic"  => FontStyle::Italic,
                "oblique" => FontStyle::Oblique,
                _ => FontStyle::Normal,
            };
        }
        // Phase 108: cursor
        "cursor" => {
            style.cursor = match val {
                "pointer"     => CursorStyle::Pointer,
                "text"        => CursorStyle::Text,
                "move"        => CursorStyle::Move,
                "not-allowed" => CursorStyle::NotAllowed,
                "wait"        => CursorStyle::Wait,
                "crosshair"   => CursorStyle::Crosshair,
                "grab" | "grabbing" => CursorStyle::Grab,
                _ => CursorStyle::Default,
            };
        }
        // Phase 108: pointer-events
        "pointer-events" => { style.pointer_events = val != "none"; }
        // Phase 108: font-family — accepted but ignored (single font kernel)
        "font-family" => {}
        // Phase 108: list-style / list-style-type — accepted but ignored
        "list-style" | "list-style-type" | "list-style-position" | "list-style-image" => {}
        // Phase 108: border-style — accepted (sets border widths to 1 if currently 0)
        "border-style" => {
            if val != "none" && val != "hidden" {
                if style.border_top == 0.0 { style.border_top = 1.0; }
                if style.border_right == 0.0 { style.border_right = 1.0; }
                if style.border_bottom == 0.0 { style.border_bottom = 1.0; }
                if style.border_left == 0.0 { style.border_left = 1.0; }
            }
        }
        "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" => {}
        // Phase 108: misc passthrough
        "transform" | "transition" | "animation" | "animation-name"
        | "animation-duration" | "animation-iteration-count" | "animation-fill-mode"
        | "will-change" | "contain" | "content" | "quotes"
        | "grid-template-columns" | "grid-template-rows"
        | "grid-column" | "grid-row" | "grid-area"
        | "align-self" | "align-content" | "justify-self" | "justify-items"
        | "order" | "gap" | "row-gap" | "column-gap"
        | "box-sizing" | "resize" | "user-select" | "appearance"
        | "vertical-align" | "float" | "clear"
        | "table-layout" | "border-collapse" | "border-spacing"
        | "caption-side" | "empty-cells"
        | "counter-reset" | "counter-increment"
        | "speak" | "direction" | "unicode-bidi"
        | "background-image" | "background-position" | "background-repeat"
        | "background-size" | "background-attachment" | "background-clip"
        | "background-origin" | "background-blend-mode"
        | "text-indent" | "text-overflow" | "text-shadow"
        | "hyphens" | "overflow-wrap" | "word-break" | "word-wrap"
        | "src" | "unicode-range" => {}
        // CSS custom property (--variable): stored in a side-channel, not applied here
        p if p.starts_with("--") => {}
        _ => {} // unknown property — ignore
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  CSS Custom Properties (var()) resolution
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve all `var(--name[, fallback])` references in `value` using `vars`.
pub fn resolve_css_vars(value: &str, vars: &BTreeMap<String, String>) -> String {
    if !value.contains("var(") { return value.to_string(); }
    let mut result = value.to_string();
    let mut guard = 0u32;
    while result.contains("var(") && guard < 32 {
        guard += 1;
        if let Some(start) = result.find("var(") {
            // Find matching close paren (may be nested)
            let inner_start = start + 4;
            let mut depth = 1i32;
            let mut end = inner_start;
            let bytes = result.as_bytes();
            while end < bytes.len() {
                if bytes[end] == b'(' { depth += 1; }
                else if bytes[end] == b')' { depth -= 1; if depth == 0 { break; } }
                end += 1;
            }
            let inner = &result[inner_start..end];
            let (name, fallback) = if let Some(comma) = inner.find(',') {
                (inner[..comma].trim(), inner[comma+1..].trim())
            } else {
                (inner.trim(), "")
            };
            let replacement = vars.get(name).map(|v| v.as_str()).unwrap_or(fallback);
            result = format!("{}{}{}", &result[..start], replacement, &result[end+1..]);
        } else { break; }
    }
    result
}

// ─────────────────────────────────────────────────────────────────────────────
//  calc() resolver — simplified arithmetic on CSS length values
// ─────────────────────────────────────────────────────────────────────────────

pub fn parse_length_with_calc(s: &str) -> CssLength {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix("calc(").and_then(|r| r.strip_suffix(')')) {
        return calc_length(inner);
    }
    parse_length(s)
}

fn calc_length(expr: &str) -> CssLength {
    // Very simplified: handle `Xpx`, `X%`, `X% - Ypx`, `X% + Ypx`, `Xpx + Ypx`
    let expr = expr.trim();

    // Try to find '+' or '-' operator (not inside parens, not part of a number)
    let bytes = expr.as_bytes();
    let mut depth = 0i32;
    let mut op_pos: Option<usize> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => { depth += 1; }
            b')' => { depth -= 1; }
            b'+' | b'-' if depth == 0 && i > 0 => {
                // Ensure this is a binary op (preceded by space or digit)
                if i > 0 && (bytes[i-1] == b' ' || bytes[i-1].is_ascii_digit() || bytes[i-1] == b')') {
                    op_pos = Some(i);
                    // take the last such op (right-associative for simplicity)
                }
            }
            _ => {}
        }
        i += 1;
    }

    if let Some(pos) = op_pos {
        let op = bytes[pos];
        let left = parse_length(expr[..pos].trim());
        let right = parse_length(expr[pos+1..].trim());
        // If both are px, combine them
        return match (left, right, op) {
            (CssLength::Px(a), CssLength::Px(b), b'+') => CssLength::Px(a + b),
            (CssLength::Px(a), CssLength::Px(b), b'-') => CssLength::Px(a - b),
            (CssLength::Percent(a), CssLength::Px(_b), _) => CssLength::Percent(a), // approximate
            _ => CssLength::Auto,
        };
    }

    // Check for '*' operator
    if let Some(star) = expr.find('*') {
        if depth == 0 {
            let left  = parse_length(expr[..star].trim());
            let right_str = expr[star+1..].trim();
            let mult = right_str.parse::<f32>().unwrap_or(1.0);
            return match left {
                CssLength::Px(v) => CssLength::Px(v * mult),
                CssLength::Percent(v) => CssLength::Percent(v * mult),
                other => other,
            };
        }
    }

    parse_length(expr)
}

fn parse_overflow(val: &str) -> Overflow {
    match val {
        "hidden" => Overflow::Hidden,
        "scroll" => Overflow::Scroll,
        "auto"   => Overflow::Auto,
        _        => Overflow::Visible,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Style tree builder
// ─────────────────────────────────────────────────────────────────────────────

/// Build a map of NodeId → ComputedStyle for every element in the DOM.
pub fn compute_styles(
    dom:         &Dom,
    stylesheets: &[Stylesheet],
    ua_sheet:    &Stylesheet,
    inline_styles: &BTreeMap<NodeId, Vec<Declaration>>,
    parent_styles: Option<&BTreeMap<NodeId, ComputedStyle>>,
) -> BTreeMap<NodeId, ComputedStyle> {
    let mut result: BTreeMap<NodeId, ComputedStyle> = BTreeMap::new();
    compute_styles_recursive(dom, dom.root(), stylesheets, ua_sheet, inline_styles, &mut result, None);
    result
}

fn compute_styles_recursive(
    dom:           &Dom,
    node_id:       NodeId,
    stylesheets:   &[Stylesheet],
    ua_sheet:      &Stylesheet,
    inline_styles: &BTreeMap<NodeId, Vec<Declaration>>,
    result:        &mut BTreeMap<NodeId, ComputedStyle>,
    parent_id:     Option<NodeId>,
) {
    let node = match dom.get(node_id) { Some(n) => n, None => return };
    let children: Vec<NodeId> = node.children.clone();

    // Only elements get computed styles.
    if matches!(&node.kind, NodeKind::Element { .. }) {
        let parent_style = parent_id
            .and_then(|pid| result.get(&pid))
            .cloned();

        let mut style = match &parent_style {
            Some(p) => ComputedStyle::inherit_from(p),
            None    => ComputedStyle::initial(),
        };

        // ── Build CSS custom-property map (--variable: value) ────────────────
        // Collect vars from parent (inherited), then add this element's own vars.
        let mut css_vars: BTreeMap<String, String> = BTreeMap::new();
        // Note: in a real browser custom props are inherited; approximate here
        collect_css_vars_from_sheet(ua_sheet, node_id, dom, &mut css_vars);
        for sheet in stylesheets {
            collect_css_vars_from_sheet(sheet, node_id, dom, &mut css_vars);
        }
        if let Some(decls) = inline_styles.get(&node_id) {
            for decl in decls {
                if decl.property.starts_with("--") {
                    css_vars.insert(decl.property.clone(), decl.value.clone());
                }
            }
        }

        // Helper: apply a declaration after resolving vars
        macro_rules! apply_decl {
            ($decl:expr) => {{
                if !$decl.property.starts_with("--") {
                    let resolved = if $decl.value.contains("var(") {
                        resolve_css_vars(&$decl.value, &css_vars)
                    } else {
                        $decl.value.clone()
                    };
                    apply_property(&mut style, &$decl.property, &resolved);
                }
            }};
        }

        // 1. User-agent sheet.
        apply_sheet_rules_with_vars(ua_sheet,   node_id, dom, &mut style, false, &css_vars);
        // 2. Author stylesheets.
        for sheet in stylesheets {
            apply_sheet_rules_with_vars(sheet,  node_id, dom, &mut style, false, &css_vars);
        }
        // 3. Inline styles (highest specificity, non-important).
        if let Some(decls) = inline_styles.get(&node_id) {
            for decl in decls {
                if !decl.important { apply_decl!(decl); }
            }
        }
        // 4. !important rules (cascade order preserved above).
        apply_sheet_rules_with_vars(ua_sheet, node_id, dom, &mut style, true, &css_vars);
        for sheet in stylesheets {
            apply_sheet_rules_with_vars(sheet, node_id, dom, &mut style, true, &css_vars);
        }
        if let Some(decls) = inline_styles.get(&node_id) {
            for decl in decls {
                if decl.important { apply_decl!(decl); }
            }
        }

        result.insert(node_id, style);
    }

    for child_id in children {
        compute_styles_recursive(dom, child_id, stylesheets, ua_sheet, inline_styles, result, Some(node_id));
    }
}

// ─── CSS variable collector ──────────────────────────────────────────────────

fn collect_css_vars_from_sheet(sheet: &Stylesheet, node_id: NodeId, dom: &Dom, vars: &mut BTreeMap<String, String>) {
    for rule in &sheet.rules {
        for sel in &rule.selectors {
            if selector_matches(sel, node_id, dom) {
                for decl in &rule.declarations {
                    if decl.property.starts_with("--") {
                        vars.insert(decl.property.clone(), decl.value.clone());
                    }
                }
                break;
            }
        }
    }
}

// ─── Sheet application with var() resolution ─────────────────────────────────

fn apply_sheet_rules_with_vars(
    sheet: &Stylesheet,
    node_id: NodeId,
    dom: &Dom,
    style: &mut ComputedStyle,
    important_only: bool,
    vars: &BTreeMap<String, String>,
) {
    let mut matched: Vec<(u32, usize)> = Vec::new();
    for (ri, rule) in sheet.rules.iter().enumerate() {
        for sel in &rule.selectors {
            if selector_matches(sel, node_id, dom) {
                matched.push((sel.specificity_value(), ri));
                break;
            }
        }
    }
    matched.sort_by_key(|&(sp, _)| sp);
    for (_, ri) in matched {
        for decl in &sheet.rules[ri].declarations {
            if decl.property.starts_with("--") { continue; }
            if decl.important != important_only { continue; }
            let resolved = if decl.value.contains("var(") {
                resolve_css_vars(&decl.value, vars)
            } else {
                decl.value.clone()
            };
            apply_property(style, &decl.property, &resolved);
        }
    }
}

fn apply_sheet_rules(sheet: &Stylesheet, node_id: NodeId, dom: &Dom, style: &mut ComputedStyle, important_only: bool) {
    // Collect matching rules with specificities.
    let mut matched: Vec<(u32, usize, usize)> = Vec::new(); // (specificity, rule_idx, sel_idx)
    for (ri, rule) in sheet.rules.iter().enumerate() {
        for (si, sel) in rule.selectors.iter().enumerate() {
            if selector_matches(sel, node_id, dom) {
                matched.push((sel.specificity_value(), ri, si));
            }
        }
    }
    // Sort by specificity ascending (lower specificity applied first, higher overwrites).
    matched.sort_by_key(|&(sp, _, _)| sp);
    for (_, ri, _) in matched {
        for decl in &sheet.rules[ri].declarations {
            if decl.important == important_only { continue; }
            if !decl.important {
                apply_property(style, &decl.property, &decl.value);
            }
        }
    }
}

fn apply_sheet_rules_important(sheet: &Stylesheet, node_id: NodeId, dom: &Dom, style: &mut ComputedStyle) {
    for rule in &sheet.rules {
        for sel in &rule.selectors {
            if selector_matches(sel, node_id, dom) {
                for decl in &rule.declarations {
                    if decl.important {
                        apply_property(style, &decl.property, &decl.value);
                    }
                }
                break;
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Parse inline style attribute
// ─────────────────────────────────────────────────────────────────────────────

pub fn parse_inline_styles(dom: &Dom) -> BTreeMap<NodeId, Vec<Declaration>> {
    let mut map: BTreeMap<NodeId, Vec<Declaration>> = BTreeMap::new();
    collect_inline_styles(dom, dom.root(), &mut map);
    map
}

fn collect_inline_styles(dom: &Dom, node_id: NodeId, map: &mut BTreeMap<NodeId, Vec<Declaration>>) {
    if let Some(node) = dom.get(node_id) {
        if let NodeKind::Element { attrs, .. } = &node.kind {
            if let Some(style_str) = attrs.get("style") {
                let mut parser = CssParser::new(style_str);
                let decls = parser.parse_declaration_block_str();
                if !decls.is_empty() { map.insert(node_id, decls); }
            }
        }
        let children: Vec<NodeId> = node.children.clone();
        for c in children { collect_inline_styles(dom, c, map); }
    }
}

impl<'a> CssParser<'a> {
    /// Parse a bare declaration block (no surrounding {}).
    pub fn parse_declaration_block_str(&mut self) -> Vec<Declaration> {
        let mut decls = Vec::new();
        loop {
            self.skip_whitespace();
            if self.pos >= self.src.len() { break; }
            if let Some(decl) = self.parse_declaration() { decls.push(decl); }
        }
        decls
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  User-agent stylesheet
// ─────────────────────────────────────────────────────────────────────────────

const UA_CSS: &str = "
html, body { display: block; margin: 0; padding: 0; }
head, script, style, meta, link, title, template { display: none; }
div, p, section, article, main, header, footer, nav, aside,
h1, h2, h3, h4, h5, h6, ul, ol, li, table, tr, td, th,
figure, figcaption, form, fieldset, blockquote, pre, address,
details, summary, dialog, canvas { display: block; }
span, a, em, strong, b, i, u, s, small, big, code, kbd, samp, var,
abbr, cite, dfn, mark, q, sub, sup, time, label, bdi, bdo { display: inline; }
img, input, button, select, textarea { display: inline-block; }
h1 { font-size: 32px; font-weight: bold; margin: 21px 0; }
h2 { font-size: 24px; font-weight: bold; margin: 19px 0; }
h3 { font-size: 18px; font-weight: bold; margin: 18px 0; }
h4 { font-size: 16px; font-weight: bold; margin: 21px 0; }
h5 { font-size: 13px; font-weight: bold; margin: 22px 0; }
h6 { font-size: 11px; font-weight: bold; margin: 25px 0; }
p  { margin: 16px 0; }
ul, ol { margin: 16px 0; padding-left: 40px; }
li { display: list-item; }
a  { color: #0000EE; text-decoration: underline; cursor: pointer; }
a:visited { color: #551A8B; }
strong, b { font-weight: bold; }
em, i { font-style: italic; }
u { text-decoration: underline; }
s, del { text-decoration: line-through; }
pre, code, samp, kbd { font-size: 13px; white-space: pre; }
table { border-collapse: collapse; }
td, th { padding: 4px; }
th { font-weight: bold; }
button, input[type=\"submit\"], input[type=\"button\"], input[type=\"reset\"] {
  padding: 4px 10px;
  border: 1px solid #aaa;
  background-color: #f0f0f0;
  border-radius: 3px;
  cursor: pointer;
}
input, textarea, select {
  padding: 3px 6px;
  border: 1px solid #ccc;
  background-color: #ffffff;
  border-radius: 2px;
}
input[type=\"checkbox\"], input[type=\"radio\"] {
  width: 14px;
  height: 14px;
  padding: 0;
}
textarea { white-space: pre-wrap; }
fieldset { border: 1px solid #ccc; padding: 8px 12px; border-radius: 4px; }
legend { font-weight: bold; padding: 0 4px; }
hr { border: none; border-top: 1px solid #ccc; margin: 8px 0; }
blockquote { margin: 16px 40px; border-left: 4px solid #ddd; padding-left: 12px; color: #555; }
mark { background-color: #ff0; color: #000; }
small { font-size: 0.8em; }
sub { font-size: 0.75em; }
sup { font-size: 0.75em; }
abbr[title] { text-decoration: underline dotted; cursor: help; }
summary { cursor: pointer; }
";

pub fn user_agent_stylesheet() -> Stylesheet {
    CssParser::new(UA_CSS).parse_stylesheet()
}

// ─────────────────────────────────────────────────────────────────────────────
//  Public parse entry point
// ─────────────────────────────────────────────────────────────────────────────

pub fn parse(css: &str) -> Stylesheet {
    CssParser::new(css).parse_stylesheet()
}

// ─────────────────────────────────────────────────────────────────────────────
//  Module init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    let ua = user_agent_stylesheet();
    crate::serial_println!(
        "[css] Phase 108: CSS engine ready (UA sheet: {} rules, border-radius, box-shadow, pseudo-classes, text-decoration, cursor, white-space).",
        ua.rules.len()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  Phase 108 self-test
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! check {
        ($desc:expr, $val:expr) => {
            if $val { passed += 1; }
            else    { failed += 1; crate::serial_println!("[css] FAIL: {}", $desc); }
        };
    }

    // T1: border-radius
    let mut s = ComputedStyle::initial();
    apply_property(&mut s, "border-radius", "8px");
    check!("border-radius 8px", s.border_radius == 8.0);
    check!("border-top-left-radius 8px", s.border_top_left_radius == 8.0);

    // T2: box-shadow
    apply_property(&mut s, "box-shadow", "2px 4px 6px #000");
    check!("box-shadow present", s.box_shadow.is_some());
    let sh = s.box_shadow.as_ref().unwrap();
    check!("shadow offset_x=2", sh.offset_x == 2.0);
    check!("shadow offset_y=4", sh.offset_y == 4.0);

    // T3: text-decoration
    apply_property(&mut s, "text-decoration", "underline");
    check!("text-decoration underline", s.text_decoration == TextDecoration::Underline);
    apply_property(&mut s, "text-decoration", "none");
    check!("text-decoration none", s.text_decoration == TextDecoration::None);

    // T4: text-transform
    apply_property(&mut s, "text-transform", "uppercase");
    check!("text-transform uppercase", s.text_transform == TextTransform::Uppercase);

    // T5: cursor
    apply_property(&mut s, "cursor", "pointer");
    check!("cursor pointer", s.cursor == CursorStyle::Pointer);

    // T6: white-space
    apply_property(&mut s, "white-space", "nowrap");
    check!("white-space nowrap", s.white_space == WhiteSpace::Nowrap);

    // T7: letter-spacing
    apply_property(&mut s, "letter-spacing", "2px");
    check!("letter-spacing 2px", s.letter_spacing == 2.0);

    // T8: font-style
    apply_property(&mut s, "font-style", "italic");
    check!("font-style italic", s.font_style == FontStyle::Italic);

    // T9: pseudo-class eval_nth
    check!("nth odd pos=1", eval_nth("odd",  1));
    check!("nth odd pos=3", eval_nth("odd",  3));
    check!("nth even pos=2", eval_nth("even", 2));
    check!("nth 2n+1 pos=3", eval_nth("2n+1", 3));
    check!("nth 2n+1 pos=2 false", !eval_nth("2n+1", 2));
    check!("nth 3 pos=3", eval_nth("3", 3));
    check!("nth 3 pos=2 false", !eval_nth("3", 2));

    // T10: parse + cascade with new properties
    let css = r#"
        .card { border-radius: 12px; box-shadow: 0 2px 8px rgba(0,0,0,0.15); cursor: pointer; }
        .title { text-transform: uppercase; letter-spacing: 1px; }
        pre { white-space: pre; }
    "#;
    let sheet = CssParser::new(css).parse_stylesheet();
    check!("sheet has 3 rules", sheet.rules.len() == 3);

    // T11: CSS custom property resolved
    let css2 = r#"
        :root { --accent: #ff6600; }
        h1 { color: var(--accent); }
    "#;
    let sheet2 = CssParser::new(css2).parse_stylesheet();
    check!("custom prop sheet parsed", sheet2.rules.len() == 2);

    crate::serial_println!("[css] Phase 108 self_test: {}/{} passed", passed, passed + failed);
    failed == 0
}

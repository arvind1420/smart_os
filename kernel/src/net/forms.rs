//! Form / Input engine — Phase 109 for Smart OS.
//!
//! Provides interactive form element tracking, rendering, and submission:
//!  • `FormState` — per-document focus + field values (global singleton)
//!  • Input type routing: text / password / email / number / search / url /
//!    checkbox / radio / submit / button / reset / range / file / hidden
//!  • `<textarea>`, `<select>`, `<button>` elements
//!  • Keyboard input → focused field value
//!  • Click handling → focus, toggle checkbox, select option
//!  • Form submission → URL-encode GET / application/x-www-form-urlencoded POST
//!  • `render_form_element()` → paint commands for all form widgets

#![allow(dead_code)]

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use spin::Mutex;

use super::html::{Dom, NodeId, NodeKind, NULL_NODE};
use super::layout::{Rect, LayoutBox};
use super::css::{ComputedStyle, Color};
use super::paint::PaintCmd;

// ─────────────────────────────────────────────────────────────────────────────
//  Input type
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputType {
    Text,
    Password,
    Email,
    Number,
    Search,
    Url,
    Tel,
    Checkbox,
    Radio,
    Submit,
    Button,
    Reset,
    Range,
    File,
    Hidden,
    Color,
    Date,
    Time,
    DatetimeLocal,
}

impl InputType {
    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "password"       => InputType::Password,
            "email"          => InputType::Email,
            "number"         => InputType::Number,
            "search"         => InputType::Search,
            "url"            => InputType::Url,
            "tel"            => InputType::Tel,
            "checkbox"       => InputType::Checkbox,
            "radio"          => InputType::Radio,
            "submit"         => InputType::Submit,
            "button"         => InputType::Button,
            "reset"          => InputType::Reset,
            "range"          => InputType::Range,
            "file"           => InputType::File,
            "hidden"         => InputType::Hidden,
            "color"          => InputType::Color,
            "date"           => InputType::Date,
            "time"           => InputType::Time,
            "datetime-local" => InputType::DatetimeLocal,
            _                => InputType::Text,
        }
    }

    pub fn is_text_like(&self) -> bool {
        matches!(self, InputType::Text | InputType::Password | InputType::Email
            | InputType::Number | InputType::Search | InputType::Url | InputType::Tel)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Form state (global singleton — one per browser context)
// ─────────────────────────────────────────────────────────────────────────────

pub struct FormState {
    /// Node ID of the currently focused input/textarea/select.
    pub focused: Option<NodeId>,
    /// Current value for each text-like input (node_id → string value).
    pub values:  BTreeMap<NodeId, String>,
    /// Cursor position within focused field.
    pub cursor:  usize,
    /// Whether the checkbox/radio at this node is checked.
    pub checked: BTreeMap<NodeId, bool>,
    /// Selected option index for <select> elements.
    pub selected: BTreeMap<NodeId, usize>,
    /// Submission result (action URL, method, encoded body).
    pub last_submit: Option<SubmitEvent>,
}

impl FormState {
    pub fn new() -> Self {
        FormState {
            focused:     None,
            values:      BTreeMap::new(),
            cursor:      0,
            checked:     BTreeMap::new(),
            selected:    BTreeMap::new(),
            last_submit: None,
        }
    }

    /// Focus a node. Clears focus if `node_id == NULL_NODE`.
    pub fn set_focus(&mut self, node_id: NodeId) {
        if node_id == NULL_NODE {
            self.focused = None;
        } else {
            self.focused = Some(node_id);
            let val = self.values.entry(node_id).or_insert_with(String::new);
            self.cursor = val.len();
        }
    }

    /// Get the display value for a node (empty string if none).
    pub fn value(&self, node_id: NodeId) -> &str {
        self.values.get(&node_id).map(|s| s.as_str()).unwrap_or("")
    }

    /// Set value programmatically (e.g. from JS `element.value = ...`).
    pub fn set_value(&mut self, node_id: NodeId, val: &str) {
        let entry = self.values.entry(node_id).or_insert_with(String::new);
        *entry = val.to_string();
        if self.focused == Some(node_id) {
            self.cursor = entry.len();
        }
    }

    pub fn is_focused(&self, node_id: NodeId) -> bool {
        self.focused == Some(node_id)
    }

    pub fn is_checked(&self, node_id: NodeId, dom: &Dom) -> bool {
        // Use runtime tracked state if present, else fall back to DOM attribute.
        if let Some(&v) = self.checked.get(&node_id) { return v; }
        if let Some(n) = dom.get(node_id) {
            if let NodeKind::Element { attrs, .. } = &n.kind {
                return attrs.contains_key("checked");
            }
        }
        false
    }
}

/// Payload returned when a form is submitted.
#[derive(Debug, Clone)]
pub struct SubmitEvent {
    pub action:  String,
    pub method:  String,           // "get" or "post"
    pub body:    String,           // URL-encoded field=value pairs
    pub url:     String,           // full URL after encoding
}

static FORM_STATE: Mutex<FormState> = Mutex::new(FormState {
    focused:     None,
    values:      BTreeMap::new(),
    cursor:      0,
    checked:     BTreeMap::new(),
    selected:    BTreeMap::new(),
    last_submit: None,
});

pub fn with_state<F, R>(f: F) -> R where F: FnOnce(&mut FormState) -> R {
    f(&mut FORM_STATE.lock())
}

// ─────────────────────────────────────────────────────────────────────────────
//  Keyboard input
// ─────────────────────────────────────────────────────────────────────────────

/// Feed a keyboard character to the currently focused input.
/// Returns `true` if the key was consumed.
pub fn handle_key_char(ch: char) -> bool {
    let mut state = FORM_STATE.lock();
    let focused = match state.focused { Some(id) => id, None => return false };
    let cursor = state.cursor;
    let val = state.values.entry(focused).or_insert_with(String::new);
    let pos = cursor.min(val.len());
    // Insert character at cursor position.
    val.insert(pos, ch);
    drop(val);
    state.cursor = pos + ch.len_utf8();
    true
}

/// Handle special keys (backspace, delete, arrow, enter, tab).
/// Returns `true` if consumed.
pub fn handle_key_special(key: u8) -> bool {
    let mut state = FORM_STATE.lock();
    let focused = match state.focused { Some(id) => id, None => return false };

    match key {
        0x08 | 0x7F => {
            // Backspace / Delete
            if state.cursor > 0 {
                let cursor = state.cursor;
                let val = state.values.entry(focused).or_insert_with(String::new);
                // Remove character before cursor.
                let bytes_snap: Vec<u8> = val.as_bytes().to_vec();
                let mut start = cursor - 1;
                while start > 0 && (bytes_snap[start] & 0xC0) == 0x80 { start -= 1; }
                val.drain(start..cursor);
                state.cursor = start;
            }
            true
        }
        0x1B => {
            // Escape — clear focus
            state.focused = None;
            true
        }
        _ => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Click handling
// ─────────────────────────────────────────────────────────────────────────────

/// Handle a click event at the given DOM node. Returns Some(SubmitEvent) on form submit.
pub fn handle_click(node_id: NodeId, dom: &Dom) -> Option<SubmitEvent> {
    let node = dom.get(node_id)?;
    if let NodeKind::Element { tag, attrs } = &node.kind {
        match tag.as_str() {
            "input" => {
                let itype = InputType::from_str(attrs.get("type").map(|s| s.as_str()).unwrap_or("text"));
                match itype {
                    t if t.is_text_like() => {
                        with_state(|s| s.set_focus(node_id));
                    }
                    InputType::Checkbox => {
                        with_state(|s| {
                            let cur = s.is_checked(node_id, dom);
                            s.checked.insert(node_id, !cur);
                        });
                    }
                    InputType::Radio => {
                        // Uncheck others with same name in the form, check this one.
                        let name = attrs.get("name").cloned().unwrap_or_default();
                        if !name.is_empty() {
                            uncheck_radio_group(node_id, &name, dom);
                        }
                        with_state(|s| { s.checked.insert(node_id, true); });
                    }
                    InputType::Submit | InputType::Button => {
                        if let Some(form_id) = find_parent_form(node_id, dom) {
                            return submit_form(form_id, dom);
                        }
                    }
                    InputType::Reset => {
                        if let Some(form_id) = find_parent_form(node_id, dom) {
                            reset_form(form_id, dom);
                        }
                    }
                    _ => {}
                }
            }
            "button" => {
                let btype = attrs.get("type").map(|s| s.as_str()).unwrap_or("submit");
                if btype == "submit" {
                    if let Some(form_id) = find_parent_form(node_id, dom) {
                        return submit_form(form_id, dom);
                    }
                } else if btype == "reset" {
                    if let Some(form_id) = find_parent_form(node_id, dom) {
                        reset_form(form_id, dom);
                    }
                } else {
                    // type="button" — just focus (JS click handlers will handle the rest)
                }
            }
            "textarea" | "select" => {
                with_state(|s| s.set_focus(node_id));
            }
            "label" => {
                // Activate associated control.
                if let Some(for_id) = attrs.get("for") {
                    if let Some(target_id) = find_element_by_id(for_id, dom) {
                        return handle_click(target_id, dom);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn uncheck_radio_group(clicked: NodeId, group_name: &str, dom: &Dom) {
    // Walk the whole DOM and uncheck all radio buttons with the same name.
    let all = collect_all_elements(dom);
    with_state(|s| {
        for nid in all {
            if nid == clicked { continue; }
            if let Some(node) = dom.get(nid) {
                if let NodeKind::Element { tag, attrs } = &node.kind {
                    if tag == "input" {
                        let itype = InputType::from_str(attrs.get("type").map(|s| s.as_str()).unwrap_or(""));
                        if itype == InputType::Radio
                            && attrs.get("name").map(|n| n == group_name).unwrap_or(false)
                        {
                            s.checked.insert(nid, false);
                        }
                    }
                }
            }
        }
    });
}

fn collect_all_elements(dom: &Dom) -> Vec<NodeId> {
    let mut out = Vec::new();
    collect_elements_recursive(dom, dom.root(), &mut out);
    out
}

fn collect_elements_recursive(dom: &Dom, node_id: NodeId, out: &mut Vec<NodeId>) {
    if let Some(node) = dom.get(node_id) {
        if matches!(&node.kind, NodeKind::Element { .. }) { out.push(node_id); }
        let children: Vec<NodeId> = node.children.clone();
        for c in children { collect_elements_recursive(dom, c, out); }
    }
}

fn find_element_by_id(id: &str, dom: &Dom) -> Option<NodeId> {
    for nid in collect_all_elements(dom) {
        if let Some(n) = dom.get(nid) {
            if let NodeKind::Element { attrs, .. } = &n.kind {
                if attrs.get("id").map(|s| s == id).unwrap_or(false) { return Some(nid); }
            }
        }
    }
    None
}

fn find_parent_form(node_id: NodeId, dom: &Dom) -> Option<NodeId> {
    let mut cur = node_id;
    loop {
        let node = dom.get(cur)?;
        if let NodeKind::Element { tag, .. } = &node.kind {
            if tag == "form" { return Some(cur); }
        }
        let parent = node.parent;
        if parent == NULL_NODE { return None; }
        cur = parent;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Form submission
// ─────────────────────────────────────────────────────────────────────────────

pub fn submit_form(form_id: NodeId, dom: &Dom) -> Option<SubmitEvent> {
    let form_node = dom.get(form_id)?;
    let (action, method) = if let NodeKind::Element { attrs, .. } = &form_node.kind {
        let a = attrs.get("action").cloned().unwrap_or_default();
        let m = attrs.get("method").map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "get".into());
        (a, m)
    } else { return None; };

    // Collect all named form fields.
    let fields = collect_form_fields(form_id, dom);
    let body = encode_form_fields(&fields);

    let url = if method == "get" {
        if action.is_empty() { format!("?{}", body) }
        else if action.contains('?') { format!("{}&{}", action, body) }
        else { format!("{}?{}", action, body) }
    } else {
        action.clone()
    };

    let ev = SubmitEvent { action, method, body, url };
    with_state(|s| s.last_submit = Some(ev.clone()));
    Some(ev)
}

pub fn reset_form(form_id: NodeId, dom: &Dom) {
    let fields = collect_all_elements(dom);
    with_state(|s| {
        for nid in fields {
            if let Some(n) = dom.get(nid) {
                if let NodeKind::Element { tag, attrs } = &n.kind {
                    if tag == "input" || tag == "textarea" || tag == "select" {
                        let default_val = attrs.get("value").cloned().unwrap_or_default();
                        s.values.insert(nid, default_val);
                    }
                }
            }
        }
    });
    let _ = form_id;
}

// ─────────────────────────────────────────────────────────────────────────────
//  Form field collection
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FormField {
    pub name:  String,
    pub value: String,
}

pub fn collect_form_fields(form_id: NodeId, dom: &Dom) -> Vec<FormField> {
    let mut fields = Vec::new();
    collect_fields_recursive(form_id, dom, &mut fields);
    fields
}

fn collect_fields_recursive(node_id: NodeId, dom: &Dom, fields: &mut Vec<FormField>) {
    if let Some(node) = dom.get(node_id) {
        if let NodeKind::Element { tag, attrs } = &node.kind {
            let name = match attrs.get("name") { Some(n) if !n.is_empty() => n.clone(), _ => {
                let children: Vec<NodeId> = node.children.clone();
                for c in children { collect_fields_recursive(c, dom, fields); }
                return;
            }};

            let state = FORM_STATE.lock();
            match tag.as_str() {
                "input" => {
                    let itype = InputType::from_str(attrs.get("type").map(|s| s.as_str()).unwrap_or("text"));
                    match itype {
                        InputType::Checkbox | InputType::Radio => {
                            let checked = state.checked.get(&node_id).copied()
                                .unwrap_or_else(|| attrs.contains_key("checked"));
                            if checked {
                                let val = attrs.get("value").cloned().unwrap_or_else(|| "on".to_string());
                                fields.push(FormField { name, value: val });
                            }
                        }
                        InputType::Submit | InputType::Button | InputType::Reset | InputType::Hidden => {
                            let val = attrs.get("value").cloned().unwrap_or_default();
                            if !val.is_empty() || itype == InputType::Hidden {
                                fields.push(FormField { name, value: val });
                            }
                        }
                        InputType::File => {} // not supported
                        _ => {
                            let val = state.values.get(&node_id)
                                .cloned()
                                .unwrap_or_else(|| attrs.get("value").cloned().unwrap_or_default());
                            fields.push(FormField { name, value: val });
                        }
                    }
                }
                "textarea" => {
                    let val = state.values.get(&node_id)
                        .cloned()
                        .unwrap_or_else(|| {
                            // content is a text child node
                            node.children.iter().find_map(|&cid| {
                                dom.get(cid).and_then(|cn| {
                                    if let NodeKind::Text { data } = &cn.kind { Some(data.clone()) } else { None }
                                })
                            }).unwrap_or_default()
                        });
                    fields.push(FormField { name, value: val });
                }
                "select" => {
                    // Find selected option.
                    let sel_idx = state.selected.get(&node_id).copied();
                    let mut opt_idx = 0usize;
                    let opt_val = find_selected_option(node_id, dom, sel_idx, &mut opt_idx);
                    fields.push(FormField { name, value: opt_val.unwrap_or_default() });
                }
                _ => {}
            }
            drop(state);
        }
        let children: Vec<NodeId> = node.children.clone();
        for c in children { collect_fields_recursive(c, dom, fields); }
    }
}

fn find_selected_option(
    select_id: NodeId, dom: &Dom,
    sel_idx: Option<usize>, opt_counter: &mut usize
) -> Option<String> {
    // First find the <option> child with selected attribute, or the one at sel_idx.
    let node = dom.get(select_id)?;
    for &cid in &node.children {
        if let Some(cn) = dom.get(cid) {
            if let NodeKind::Element { tag, attrs } = &cn.kind {
                if tag == "option" {
                    let is_selected = attrs.contains_key("selected")
                        || sel_idx == Some(*opt_counter);
                    if is_selected {
                        let val = attrs.get("value")
                            .cloned()
                            .or_else(|| {
                                // Text content of <option>
                                cn.children.iter().find_map(|&tid| {
                                    dom.get(tid).and_then(|tn| {
                                        if let NodeKind::Text { data } = &tn.kind { Some(data.trim().to_string()) } else { None }
                                    })
                                })
                            });
                        return val;
                    }
                    *opt_counter += 1;
                }
            }
        }
    }
    // Fallback: first option
    for &cid in &node.children {
        if let Some(cn) = dom.get(cid) {
            if let NodeKind::Element { tag, attrs } = &cn.kind {
                if tag == "option" {
                    return attrs.get("value").cloned().or_else(|| {
                        cn.children.iter().find_map(|&tid| {
                            dom.get(tid).and_then(|tn| {
                                if let NodeKind::Text { data } = &tn.kind { Some(data.trim().to_string()) } else { None }
                            })
                        })
                    });
                }
            }
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
//  URL encoding
// ─────────────────────────────────────────────────────────────────────────────

pub fn encode_form_fields(fields: &[FormField]) -> String {
    fields.iter().enumerate().map(|(i, f)| {
        let sep = if i == 0 { "" } else { "&" };
        format!("{}{}={}", sep, url_encode(&f.name), url_encode(&f.value))
    }).collect()
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0xF) as usize] as char);
            }
        }
    }
    out
}

const HEX: &[u8] = b"0123456789ABCDEF";

// ─────────────────────────────────────────────────────────────────────────────
//  Form element rendering
// ─────────────────────────────────────────────────────────────────────────────

const INPUT_BG:      Color = Color { r: 255, g: 255, b: 255, a: 255 };
const INPUT_BORDER:  Color = Color { r: 180, g: 180, b: 180, a: 255 };
const INPUT_FOCUS:   Color = Color { r:  66, g: 133, b: 244, a: 255 };
const INPUT_TEXT:    Color = Color { r:  30, g:  30, b:  30, a: 255 };
const BTN_BG:        Color = Color { r: 240, g: 240, b: 240, a: 255 };
const BTN_BG_HOVER:  Color = Color { r: 224, g: 224, b: 224, a: 255 };
const BTN_BORDER:    Color = Color { r: 170, g: 170, b: 170, a: 255 };
const CHECKBOX_CHECK:Color = Color { r:  66, g: 133, b: 244, a: 255 };
const PLACEHOLDER:   Color = Color { r: 180, g: 180, b: 180, a: 255 };

/// Emit paint commands for a form element.
/// Called from the paint module for `input`, `textarea`, `select`, `button`.
pub fn render_form_element(
    node_id:  NodeId,
    dom:      &Dom,
    lb:       &LayoutBox,
    style:    &ComputedStyle,
    out:      &mut Vec<PaintCmd>,
) {
    let node = match dom.get(node_id) { Some(n) => n, None => return };
    let (tag, attrs) = match &node.kind {
        NodeKind::Element { tag, attrs } => (tag.as_str(), attrs),
        _ => return,
    };

    let br = lb.areas.border_rect();
    let cr = lb.areas.content;
    let is_focused = FORM_STATE.lock().is_focused(node_id);

    match tag {
        "input" => {
            let itype = InputType::from_str(attrs.get("type").map(|s| s.as_str()).unwrap_or("text"));
            let itype2 = itype.clone();
            match itype {
                t if t.is_text_like() => render_text_input(node_id, attrs, &itype2, br, cr, is_focused, style, out),
                InputType::Checkbox   => render_checkbox(node_id, dom, br, is_focused, out),
                InputType::Radio      => render_radio(node_id, dom, br, is_focused, out),
                InputType::Range      => render_range(node_id, br, out),
                InputType::Submit | InputType::Button | InputType::Reset => {
                    let label = attrs.get("value").map(|s| s.as_str()).unwrap_or(
                        if itype2 == InputType::Submit { "Submit" }
                        else if itype2 == InputType::Reset { "Reset" }
                        else { "Button" }
                    );
                    render_button_shape(br, cr, label, is_focused, style.font_size, out);
                }
                InputType::Hidden => {} // invisible
                InputType::Color => render_color_input(br, attrs, out),
                _ => render_text_input(node_id, attrs, &itype, br, cr, is_focused, style, out),
            }
        }
        "textarea" => render_textarea(node_id, attrs, br, cr, is_focused, style, out),
        "select"   => render_select(node_id, dom, br, cr, is_focused, style, out),
        "button"   => {
            // Collect text content from children.
            let label = collect_text_content(node_id, dom);
            let label = if label.trim().is_empty() {
                attrs.get("value").cloned().unwrap_or_else(|| "Button".to_string())
            } else { label };
            render_button_shape(br, cr, &label, is_focused, style.font_size, out);
        }
        _ => {}
    }
}

fn render_text_input(
    node_id: NodeId, attrs: &BTreeMap<String, String>, itype: &InputType,
    br: Rect, cr: Rect, is_focused: bool, style: &ComputedStyle,
    out: &mut Vec<PaintCmd>,
) {
    // Background
    let bg = if style.background_color.a > 0 { style.background_color } else { INPUT_BG };
    out.push(PaintCmd::FillRect { rect: br, color: bg });

    // Border
    let border_col = if is_focused { INPUT_FOCUS } else { INPUT_BORDER };
    let bw = if is_focused { 2.0 } else { 1.0 };
    out.push(PaintCmd::DrawBorder { rect: br, top: bw, right: bw, bottom: bw, left: bw, color: border_col });

    // Text content
    let state = FORM_STATE.lock();
    let raw_val = state.values.get(&node_id).cloned()
        .unwrap_or_else(|| attrs.get("value").cloned().unwrap_or_default());
    drop(state);

    let display_text: String;
    let text_color: Color;

    if raw_val.is_empty() {
        // Show placeholder
        let placeholder = attrs.get("placeholder").cloned().unwrap_or_default();
        display_text = placeholder;
        text_color = PLACEHOLDER;
    } else {
        display_text = if *itype == InputType::Password {
            "•".repeat(raw_val.chars().count())
        } else {
            raw_val.clone()
        };
        text_color = INPUT_TEXT;
    }

    if !display_text.is_empty() {
        out.push(PaintCmd::DrawText {
            x: cr.x + 2.0, y: cr.y + 1.0,
            text: display_text,
            font_px: style.font_size,
            color: text_color,
        });
    }

    // Cursor bar when focused
    if is_focused {
        let cursor_x = cr.x + 2.0 + raw_val.len() as f32 * (style.font_size * 0.5);
        out.push(PaintCmd::FillRect {
            rect: Rect { x: cursor_x, y: cr.y + 2.0, w: 1.5, h: cr.h - 4.0 },
            color: INPUT_TEXT,
        });
    }
}

fn render_checkbox(node_id: NodeId, dom: &Dom, br: Rect, is_focused: bool, out: &mut Vec<PaintCmd>) {
    // Box
    let border = if is_focused { INPUT_FOCUS } else { INPUT_BORDER };
    out.push(PaintCmd::FillRect { rect: br, color: INPUT_BG });
    out.push(PaintCmd::DrawBorder { rect: br, top: 1.5, right: 1.5, bottom: 1.5, left: 1.5, color: border });

    // Checkmark
    if FORM_STATE.lock().is_checked(node_id, dom) {
        // Blue fill + white "✓" text
        out.push(PaintCmd::FillRect { rect: br, color: CHECKBOX_CHECK });
        let cx = br.x + br.w * 0.1;
        let cy = br.y + br.h * 0.1;
        out.push(PaintCmd::DrawText {
            x: cx, y: cy, text: "✓".to_string(), font_px: br.h * 0.8,
            color: Color::WHITE,
        });
    }
}

fn render_radio(node_id: NodeId, dom: &Dom, br: Rect, is_focused: bool, out: &mut Vec<PaintCmd>) {
    // Simple circle approximation with a filled square
    let border = if is_focused { INPUT_FOCUS } else { INPUT_BORDER };
    // Outer ring
    out.push(PaintCmd::FillRect { rect: br, color: INPUT_BG });
    out.push(PaintCmd::DrawBorder { rect: br, top: 1.5, right: 1.5, bottom: 1.5, left: 1.5, color: border });

    if FORM_STATE.lock().is_checked(node_id, dom) {
        // Inner filled dot
        let inset = br.w * 0.25;
        out.push(PaintCmd::FillRect {
            rect: Rect { x: br.x + inset, y: br.y + inset, w: br.w - inset*2.0, h: br.h - inset*2.0 },
            color: CHECKBOX_CHECK,
        });
    }
}

fn render_range(node_id: NodeId, br: Rect, out: &mut Vec<PaintCmd>) {
    let track_h = 4.0;
    let track_y = br.y + (br.h - track_h) / 2.0;
    // Track
    out.push(PaintCmd::FillRect {
        rect: Rect { x: br.x, y: track_y, w: br.w, h: track_h },
        color: Color { r: 200, g: 200, b: 200, a: 255 },
    });
    // Thumb (50% position stub)
    let thumb_x = br.x + br.w * 0.5 - 6.0;
    out.push(PaintCmd::FillRect {
        rect: Rect { x: thumb_x, y: br.y + 2.0, w: 12.0, h: br.h - 4.0 },
        color: CHECKBOX_CHECK,
    });
    let _ = node_id;
}

fn render_color_input(br: Rect, attrs: &BTreeMap<String, String>, out: &mut Vec<PaintCmd>) {
    let val = attrs.get("value").map(|s| s.as_str()).unwrap_or("#000000");
    let color = super::css::parse_color(val).unwrap_or(Color::BLACK);
    out.push(PaintCmd::FillRect { rect: br, color });
    out.push(PaintCmd::DrawBorder { rect: br, top: 1.5, right: 1.5, bottom: 1.5, left: 1.5, color: INPUT_BORDER });
}

fn render_textarea(
    node_id: NodeId, attrs: &BTreeMap<String, String>,
    br: Rect, cr: Rect, is_focused: bool, style: &ComputedStyle,
    out: &mut Vec<PaintCmd>,
) {
    let bg = if style.background_color.a > 0 { style.background_color } else { INPUT_BG };
    out.push(PaintCmd::FillRect { rect: br, color: bg });

    let border_col = if is_focused { INPUT_FOCUS } else { INPUT_BORDER };
    let bw = if is_focused { 2.0 } else { 1.0 };
    out.push(PaintCmd::DrawBorder { rect: br, top: bw, right: bw, bottom: bw, left: bw, color: border_col });

    let state = FORM_STATE.lock();
    let val = state.values.get(&node_id).cloned()
        .unwrap_or_else(|| attrs.get("value").cloned().unwrap_or_default());
    drop(state);

    if val.is_empty() {
        let ph = attrs.get("placeholder").cloned().unwrap_or_default();
        if !ph.is_empty() {
            out.push(PaintCmd::DrawText { x: cr.x+2.0, y: cr.y+2.0, text: ph, font_px: style.font_size, color: PLACEHOLDER });
        }
    } else {
        // Render each line.
        let mut y = cr.y + 2.0;
        for line in val.lines() {
            out.push(PaintCmd::DrawText { x: cr.x+2.0, y, text: line.to_string(), font_px: style.font_size, color: INPUT_TEXT });
            y += style.line_height;
            if y > cr.y + cr.h { break; }
        }
    }

    // Resize grip (bottom-right corner hint)
    out.push(PaintCmd::DrawText {
        x: br.x + br.w - 12.0, y: br.y + br.h - 14.0,
        text: "⋱".to_string(), font_px: 10.0, color: INPUT_BORDER,
    });
}

fn render_select(
    node_id: NodeId, dom: &Dom,
    br: Rect, cr: Rect, is_focused: bool, style: &ComputedStyle,
    out: &mut Vec<PaintCmd>,
) {
    let bg = if style.background_color.a > 0 { style.background_color } else { INPUT_BG };
    out.push(PaintCmd::FillRect { rect: br, color: bg });

    let border_col = if is_focused { INPUT_FOCUS } else { INPUT_BORDER };
    let bw = if is_focused { 2.0 } else { 1.0 };
    out.push(PaintCmd::DrawBorder { rect: br, top: bw, right: bw, bottom: bw, left: bw, color: border_col });

    // Dropdown arrow
    out.push(PaintCmd::DrawText {
        x: br.x + br.w - 18.0, y: cr.y + 1.0, text: "▾".to_string(),
        font_px: style.font_size, color: INPUT_TEXT,
    });

    // Selected option text
    let mut opt_idx = 0;
    let sel_idx = FORM_STATE.lock().selected.get(&node_id).copied();
    if let Some(val) = find_selected_option(node_id, dom, sel_idx, &mut opt_idx) {
        out.push(PaintCmd::DrawText {
            x: cr.x + 2.0, y: cr.y + 1.0, text: val,
            font_px: style.font_size, color: INPUT_TEXT,
        });
    }
    let _ = cr;
}

fn render_button_shape(br: Rect, cr: Rect, label: &str, is_focused: bool, font_px: f32, out: &mut Vec<PaintCmd>) {
    let bg = if is_focused { BTN_BG_HOVER } else { BTN_BG };
    out.push(PaintCmd::FillRect { rect: br, color: bg });

    let border_col = if is_focused { INPUT_FOCUS } else { BTN_BORDER };
    let bw = if is_focused { 2.0 } else { 1.0 };
    out.push(PaintCmd::DrawBorder { rect: br, top: bw, right: bw, bottom: bw, left: bw, color: border_col });

    if !label.is_empty() {
        // Approximate centre align: fixed inset.
        let label_x = cr.x + (cr.w / 2.0 - label.len() as f32 * font_px * 0.3).max(2.0);
        let label_y = cr.y + (cr.h - font_px) / 2.0 + 1.0;
        out.push(PaintCmd::DrawText {
            x: label_x, y: label_y, text: label.to_string(),
            font_px, color: INPUT_TEXT,
        });
    }
    let _ = cr;
}

fn collect_text_content(node_id: NodeId, dom: &Dom) -> String {
    let mut s = String::new();
    if let Some(node) = dom.get(node_id) {
        for &cid in &node.children {
            if let Some(cn) = dom.get(cid) {
                match &cn.kind {
                    NodeKind::Text { data } => s.push_str(data),
                    NodeKind::Element { .. } => s.push_str(&collect_text_content(cid, dom)),
                    _ => {}
                }
            }
        }
    }
    s
}

// ─────────────────────────────────────────────────────────────────────────────
//  DOM attribute helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Get the `type` attribute of an input node.
pub fn input_type_of(node_id: NodeId, dom: &Dom) -> InputType {
    if let Some(n) = dom.get(node_id) {
        if let NodeKind::Element { tag, attrs } = &n.kind {
            if tag == "input" {
                return InputType::from_str(attrs.get("type").map(|s| s.as_str()).unwrap_or("text"));
            }
        }
    }
    InputType::Text
}

/// True if the given node is a focusable form element.
pub fn is_focusable(node_id: NodeId, dom: &Dom) -> bool {
    if let Some(n) = dom.get(node_id) {
        if let NodeKind::Element { tag, attrs } = &n.kind {
            if attrs.contains_key("disabled") { return false; }
            return matches!(tag.as_str(), "input" | "textarea" | "select" | "button" | "a");
        }
    }
    false
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init + self-test
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[forms] Phase 109: Form/Input engine ready (text/checkbox/radio/select/textarea/button, submit/reset, URL-encode).");
}

pub fn self_test() -> bool {
    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! check {
        ($desc:expr, $val:expr) => {
            if $val { passed += 1; }
            else    { failed += 1; crate::serial_println!("[forms] FAIL: {}", $desc); }
        };
    }

    // T1: InputType parsing
    check!("text",     InputType::from_str("text")     == InputType::Text);
    check!("password", InputType::from_str("password") == InputType::Password);
    check!("checkbox", InputType::from_str("checkbox") == InputType::Checkbox);
    check!("radio",    InputType::from_str("radio")    == InputType::Radio);
    check!("submit",   InputType::from_str("submit")   == InputType::Submit);
    check!("unknown → text", InputType::from_str("foo") == InputType::Text);

    // T2: URL encoding
    check!("encode spaces", url_encode("hello world") == "hello+world");
    check!("encode =",      url_encode("a=b")         == "a%3Db");
    check!("encode /",      url_encode("path/file")   == "path%2Ffile");
    check!("alpha pass",    url_encode("abc123")       == "abc123");
    check!("tilde pass",    url_encode("~test")        == "~test");

    // T3: encode_form_fields
    let fields = vec![
        FormField { name: "q".to_string(),    value: "hello world".to_string() },
        FormField { name: "page".to_string(), value: "1".to_string() },
    ];
    let encoded = encode_form_fields(&fields);
    check!("fields encoded", encoded == "q=hello+world&page=1");

    // T4: eval_nth (re-test via this module)
    check!("nth alias: odd pos1",  super::css::eval_nth("odd",  1));
    check!("nth alias: even pos2", super::css::eval_nth("even", 2));

    // T5: FormState focus/value
    let mut state = FormState::new();
    state.set_focus(42);
    check!("focus set",    state.focused == Some(42));
    check!("cursor at 0",  state.cursor == 0);
    state.set_value(42, "hello");
    check!("value stored", state.value(42) == "hello");
    state.set_focus(42);
    check!("cursor at end", state.cursor == 5);

    // T6: reset focus
    state.set_focus(NULL_NODE);
    check!("focus cleared", state.focused == None);

    crate::serial_println!("[forms] Phase 109 self_test: {}/{} passed", passed, passed + failed);
    failed == 0
}

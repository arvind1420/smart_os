//! PDF Reader — Phase 63: PDF Reader App (v0.23.0).
//!
//! Minimal PDF text-extraction viewer (no external crates, pure no_std):
//! • Scans PDF bytes for `N 0 obj … endobj` object markers
//! • Extracts text from content streams (BT/ET, Tj, TJ, ', operators)
//! • Navigates Pages tree to find leaf Page objects
//! • Displays extracted text in a scrollable window with Prev/Next buttons
//! • Falls back to an embedded two-page demo if no PDF file is found in VFS

#![allow(dead_code)]

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::window::{Window, WindowId};
use crate::gui::desktop::DESKTOP;
use crate::gui::theme::*;
use crate::gui::widget::{
    Widget, WidgetKind, Button, StaticLabel, ScrollableText, AppCommand, WidgetAction,
};
use crate::vfs;

// ─── Byte-level helpers ───────────────────────────────────────────────────────

#[inline] fn is_ws(b: u8) -> bool { matches!(b, 0 | 9 | 10 | 12 | 13 | 32) }
#[inline] fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn skip_ws(d: &[u8], p: &mut usize) {
    while *p < d.len() {
        if d[*p] == b'%' {
            while *p < d.len() && d[*p] != b'\n' && d[*p] != b'\r' { *p += 1; }
        } else if is_ws(d[*p]) {
            *p += 1;
        } else {
            break;
        }
    }
}

fn read_token<'a>(d: &'a [u8], p: &mut usize) -> &'a [u8] {
    skip_ws(d, p);
    let start = *p;
    while *p < d.len() && !is_ws(d[*p]) && !is_delim(d[*p]) { *p += 1; }
    &d[start..*p]
}

fn parse_int(tok: &[u8]) -> Option<i64> {
    if tok.is_empty() { return None; }
    let (neg, dig) = if tok[0] == b'-' { (true, &tok[1..]) } else { (false, tok) };
    if dig.is_empty() { return None; }
    let mut n: i64 = 0;
    for &b in dig {
        if !b.is_ascii_digit() { return None; }
        n = n * 10 + (b - b'0') as i64;
    }
    Some(if neg { -n } else { n })
}

/// Read a PDF literal string `(…)` with escape/nesting support.
pub fn read_literal_string(d: &[u8], p: &mut usize) -> Vec<u8> {
    *p += 1; // skip '('
    let mut out = Vec::new();
    let mut depth = 1usize;
    while *p < d.len() && depth > 0 {
        match d[*p] {
            b'\\' => {
                *p += 1;
                if *p >= d.len() { break; }
                let esc = match d[*p] {
                    b'n'  => b'\n', b'r'  => b'\r', b't'  => b'\t',
                    b'b'  => 8,     b'f'  => 12,
                    b'('  => b'(',  b')'  => b')',  b'\\' => b'\\',
                    byte @ b'0'..=b'7' => {
                        let mut oct = byte - b'0';
                        for _ in 0..2 {
                            if *p + 1 < d.len() && d[*p+1] >= b'0' && d[*p+1] <= b'7' {
                                *p += 1; oct = oct * 8 + (d[*p] - b'0');
                            } else { break; }
                        }
                        oct
                    }
                    c => c,
                };
                out.push(esc);
                *p += 1;
            }
            b'(' => { depth += 1; out.push(b'('); *p += 1; }
            b')' => { depth -= 1; if depth > 0 { out.push(b')'); } *p += 1; }
            b => { out.push(b); *p += 1; }
        }
    }
    out
}

/// Read a PDF hex string `<…>`.
pub fn read_hex_string(d: &[u8], p: &mut usize) -> Vec<u8> {
    *p += 1; // skip '<'
    let mut out = Vec::new();
    let mut hi: Option<u8> = None;
    while *p < d.len() && d[*p] != b'>' {
        let nibble = match d[*p] {
            b @ b'0'..=b'9' => b - b'0',
            b @ b'a'..=b'f' => b - b'a' + 10,
            b @ b'A'..=b'F' => b - b'A' + 10,
            _ => { *p += 1; continue; }
        };
        *p += 1;
        if let Some(h) = hi.take() { out.push((h << 4) | nibble); }
        else { hi = Some(nibble); }
    }
    if let Some(h) = hi { out.push(h << 4); }
    if *p < d.len() { *p += 1; } // skip '>'
    out
}

/// Decode a PDF byte string to UTF-8 (handles UTF-16BE BOM + Latin-1 fallback).
pub fn decode_pdf_string(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let mut s = String::new();
        let mut i = 2;
        while i + 1 < bytes.len() {
            let cp = ((bytes[i] as u32) << 8) | (bytes[i+1] as u32);
            i += 2;
            if let Some(c) = char::from_u32(cp) { s.push(c); }
        }
        return s;
    }
    bytes.iter().map(|&b| if b >= 32 || b == b'\n' || b == b'\t' { b as char } else { ' ' }).collect()
}

// ─── Object scanner (no xref required) ────────────────────────────────────────

/// Object entry: obj_num → byte offset into `data`.
#[derive(Clone)]
struct ObjEntry { num: u32, offset: usize }

/// Try to parse `"N G obj"` at the start of `line`.
/// Returns `(obj_num, bytes_consumed)` on success.
fn try_obj_header(line: &[u8]) -> Option<(u32, usize)> {
    let mut p = 0;
    while p < line.len() && (line[p] == b' ' || line[p] == b'\t') { p += 1; }
    let ns = p;
    while p < line.len() && line[p].is_ascii_digit() { p += 1; }
    if p == ns || p >= line.len() || line[p] != b' ' { return None; }
    let num: u32 = core::str::from_utf8(&line[ns..p]).ok()?.parse().ok()?;
    p += 1;
    let gs = p;
    while p < line.len() && line[p].is_ascii_digit() { p += 1; }
    if p == gs || p >= line.len() || line[p] != b' ' { return None; }
    p += 1;
    if p + 3 <= line.len() && &line[p..p+3] == b"obj" { Some((num, p + 3)) }
    else { None }
}

/// Scan `data` for `N 0 obj` object markers; returns their offsets.
pub fn scan_objects(data: &[u8]) -> Vec<ObjEntry> {
    let mut entries = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let at_line_start = i == 0 || data[i-1] == b'\n' || data[i-1] == b'\r';
        if at_line_start {
            if let Some((num, _skip)) = try_obj_header(&data[i..]) {
                entries.push(ObjEntry { num, offset: i });
            }
        }
        i += 1;
    }
    entries
}

/// Return the raw bytes of object `num` (from its `N 0 obj` header to `endobj`).
fn object_bytes<'a>(data: &'a [u8], entries: &[ObjEntry], num: u32) -> Option<&'a [u8]> {
    let e = entries.iter().find(|e| e.num == num)?;
    let start = e.offset;
    let end_needle = b"endobj";
    let search_end = (start + 131_072).min(data.len());
    for i in start..search_end.saturating_sub(end_needle.len()) {
        if &data[i..i+end_needle.len()] == end_needle { return Some(&data[start..i]); }
    }
    Some(&data[start..search_end])
}

// ─── Dict and stream helpers ──────────────────────────────────────────────────

/// Find `/Key` in `obj_bytes` and return the slice starting just after it.
fn dict_value<'a>(obj_bytes: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let needle = format!("/{}", key);
    let nb = needle.as_bytes();
    let limit = obj_bytes.len().saturating_sub(nb.len());
    for i in 0..=limit {
        if &obj_bytes[i..i+nb.len()] == nb {
            let after = i + nb.len();
            let next = obj_bytes.get(after).copied().unwrap_or(0);
            if is_ws(next) || is_delim(next) {
                let val_end = (after + 256).min(obj_bytes.len());
                return Some(&obj_bytes[after..val_end]);
            }
        }
    }
    None
}

/// Extract an indirect reference `N G R` from a value slice.
fn extract_ref(val: &[u8]) -> Option<u32> {
    let mut p = 0;
    let n = parse_int(read_token(val, &mut p))? as u32;
    parse_int(read_token(val, &mut p))?; // gen
    if read_token(val, &mut p) == b"R" { Some(n) } else { None }
}

/// Extract the first string `(…)` or `<…>` from a value slice.
fn extract_string(val: &[u8]) -> Option<Vec<u8>> {
    let mut p = 0;
    skip_ws(val, &mut p);
    if p >= val.len() { return None; }
    if val[p] == b'(' { Some(read_literal_string(val, &mut p)) }
    else if val[p] == b'<' && val.get(p+1).copied() != Some(b'<') { Some(read_hex_string(val, &mut p)) }
    else { None }
}

/// Extract the `stream … endstream` body bytes from an object.
fn stream_data<'a>(obj_bytes: &'a [u8]) -> Option<&'a [u8]> {
    let s_needle = b"stream";
    let e_needle = b"endstream";
    for i in 0..obj_bytes.len().saturating_sub(s_needle.len()) {
        if &obj_bytes[i..i+s_needle.len()] == s_needle {
            let mut start = i + s_needle.len();
            if start < obj_bytes.len() && obj_bytes[start] == b'\r' { start += 1; }
            if start < obj_bytes.len() && obj_bytes[start] == b'\n' { start += 1; }
            for j in start..obj_bytes.len().saturating_sub(e_needle.len()) {
                if &obj_bytes[j..j+e_needle.len()] == e_needle { return Some(&obj_bytes[start..j]); }
            }
            return Some(&obj_bytes[start..]);
        }
    }
    None
}

// ─── startxref finder ────────────────────────────────────────────────────────

/// Find the integer value after the last `startxref` keyword.
pub fn find_startxref_offset(data: &[u8]) -> usize {
    let needle = b"startxref";
    let search = data.len().saturating_sub(1024);
    for i in (search..data.len().saturating_sub(needle.len())).rev() {
        if &data[i..i+needle.len()] == needle {
            let mut p = i + needle.len();
            if let Some(n) = parse_int(read_token(data, &mut p)) { return n as usize; }
        }
    }
    0
}

// ─── Page finder ─────────────────────────────────────────────────────────────

/// Return true if `obj_bytes` is a leaf Page (not an intermediate Pages node).
fn is_leaf_page(obj_bytes: &[u8]) -> bool {
    // Match "/Type /Page" or "/Type/Page" but not ".../Pages"
    for needle in &[b"/Type /Page" as &[u8], b"/Type/Page"] {
        let limit = obj_bytes.len().saturating_sub(needle.len());
        for i in 0..=limit {
            if &obj_bytes[i..i+needle.len()] == *needle {
                let next = obj_bytes.get(i + needle.len()).copied().unwrap_or(0);
                if next != b's' && next != b'S' { return true; }
            }
        }
    }
    false
}

/// Find all leaf Page objects and return their object numbers (sorted).
pub fn find_page_objects(data: &[u8], entries: &[ObjEntry]) -> Vec<u32> {
    let mut pages = Vec::new();
    for e in entries {
        if let Some(ob) = object_bytes(data, entries, e.num) {
            if is_leaf_page(ob) { pages.push(e.num); }
        }
    }
    pages.sort_unstable();
    pages
}

// ─── Content stream text extraction ──────────────────────────────────────────

/// Extract plain text strings from a PDF content stream.
pub fn extract_text(stream: &[u8]) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    let mut pos = 0;
    let mut in_bt = false;
    let mut pending: Vec<Vec<u8>> = Vec::new();
    let mut in_array = false;
    let mut array_strs: Vec<Vec<u8>> = Vec::new();

    while pos < stream.len() {
        skip_ws(stream, &mut pos);
        if pos >= stream.len() { break; }

        match stream[pos] {
            b'(' => {
                let s = read_literal_string(stream, &mut pos);
                if in_array { array_strs.push(s); } else { pending.push(s); }
            }
            b'<' if stream.get(pos + 1).copied() != Some(b'<') => {
                let s = read_hex_string(stream, &mut pos);
                if in_array { array_strs.push(s); } else { pending.push(s); }
            }
            b'[' => { in_array = true; array_strs.clear(); pos += 1; }
            b']' => { in_array = false; pos += 1; }
            b'%' => { while pos < stream.len() && stream[pos] != b'\n' { pos += 1; } }
            _ => {
                let start = pos;
                while pos < stream.len() && !is_ws(stream[pos]) && !is_delim(stream[pos]) {
                    pos += 1;
                }
                if pos == start { pos += 1; continue; }
                let op = &stream[start..pos];

                match op {
                    b"BT" => { in_bt = true; }
                    b"ET" => { in_bt = false; pending.clear(); array_strs.clear(); }
                    b"Tj" | b"'" if in_bt => {
                        if let Some(s) = pending.pop() {
                            let t = decode_pdf_string(&s);
                            if !t.trim().is_empty() { result.push(t); }
                        }
                        pending.clear();
                    }
                    b"TJ" if in_bt => {
                        let mut combined = String::new();
                        for s in array_strs.drain(..) { combined.push_str(&decode_pdf_string(&s)); }
                        let t = combined.trim().to_string();
                        if !t.is_empty() { result.push(t); }
                        pending.clear();
                    }
                    b"Td" | b"TD" | b"T*" | b"Tm" => { pending.clear(); }
                    op if op.iter().all(|b| b.is_ascii_alphabetic() || *b == b'*') => {
                        pending.clear();
                    }
                    _ => {}
                }
            }
        }
    }
    result
}

/// Extract text from the Content stream of page object `page_num`.
fn extract_page_text(data: &[u8], entries: &[ObjEntry], page_num: u32) -> Vec<String> {
    let ob = match object_bytes(data, entries, page_num) { Some(b) => b, None => return vec![] };
    let contents_ref = match dict_value(ob, "Contents").and_then(extract_ref) {
        Some(r) => r,
        None => return vec![],
    };
    let cont_ob = match object_bytes(data, entries, contents_ref) { Some(b) => b, None => return vec![] };
    let sd = match stream_data(cont_ob) { Some(s) => s, None => return vec![] };
    extract_text(sd)
}

// ─── Metadata ─────────────────────────────────────────────────────────────────

fn extract_metadata(data: &[u8], entries: &[ObjEntry]) -> (String, String) {
    for e in entries {
        if let Some(ob) = object_bytes(data, entries, e.num) {
            if let Some(v) = dict_value(ob, "Title") {
                if let Some(bytes) = extract_string(v) {
                    let title = decode_pdf_string(&bytes);
                    if !title.trim().is_empty() {
                        let author = dict_value(ob, "Author")
                            .and_then(extract_string)
                            .map(|b| decode_pdf_string(&b))
                            .unwrap_or_default();
                        return (title, author);
                    }
                }
            }
        }
    }
    (String::from("Untitled"), String::new())
}

// ─── Embedded demo PDF ────────────────────────────────────────────────────────

const DEMO_PDF: &[u8] = b"\
%PDF-1.4\n\
1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n\
2 0 obj\n<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>\nendobj\n\
3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 5 0 R/Resources<<>>>>\nendobj\n\
4 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 6 0 R/Resources<<>>>>\nendobj\n\
5 0 obj\n<</Length 130>>\nstream\n\
BT /F1 14 Tf 72 720 Td (Smart OS PDF Reader) Tj\n\
0 -24 Td (Phase 63 - Demo Document) Tj\n\
0 -24 Td (Page 1 of 2) Tj ET\n\
endstream\nendobj\n\
6 0 obj\n<</Length 100>>\nstream\n\
BT /F1 14 Tf 72 720 Td (Page Two Content) Tj\n\
0 -24 Td (Smart OS kernel v0.23.0) Tj ET\n\
endstream\nendobj\n\
%%EOF\n";

// ─── App state ────────────────────────────────────────────────────────────────

pub struct PdfReaderState {
    pub window_id:    WindowId,
    pub data:         Vec<u8>,
    pub entries:      Vec<ObjEntry>,
    pub pages:        Vec<u32>,
    pub current_page: usize,
    pub title:        String,
    pub status:       String,
}

pub static STATE: Mutex<Option<PdfReaderState>> = Mutex::new(None);

// ─── VFS loader ──────────────────────────────────────────────────────────────

fn load_pdf_data() -> Vec<u8> {
    for path in &["/data/docs/demo.pdf", "/data/demo.pdf", "/disk/demo.pdf"] {
        if let Ok(fd) = vfs::open(path) {
            let mut buf = vec![0u8; 4 * 1024 * 1024];
            if let Ok(n) = vfs::read(fd, &mut buf) {
                vfs::close(fd).ok();
                if n > 4 && &buf[..4] == b"%PDF" {
                    buf.truncate(n);
                    return buf;
                }
            }
            vfs::close(fd).ok();
        }
    }
    DEMO_PDF.to_vec()
}

// ─── Page renderer ────────────────────────────────────────────────────────────

fn render_page_lines(
    data: &[u8],
    entries: &[ObjEntry],
    pages: &[u32],
    idx: usize,
    title: &str,
) -> Vec<(String, Color)> {
    let mut lines: Vec<(String, Color)> = Vec::new();
    lines.push((format!("─── {} — Page {} of {} ───", title, idx + 1, pages.len()), ACCENT_RED));
    lines.push((String::new(), TEXT_PRIMARY));

    if idx >= pages.len() {
        lines.push((String::from("  (page out of range)"), TEXT_SECONDARY));
        return lines;
    }

    let texts = extract_page_text(data, entries, pages[idx]);
    if texts.is_empty() {
        lines.push((String::from("  (no extractable text on this page)"), TEXT_SECONDARY));
    } else {
        for t in texts {
            lines.push((format!("  {}", t), TEXT_PRIMARY));
        }
    }
    lines.push((String::new(), TEXT_PRIMARY));
    lines
}

// ─── App thread ──────────────────────────────────────────────────────────────

pub fn run() {
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() { Some(d) => d, None => return };

        let mut win = crate::gui::window::Window::new("PDF Reader", 90, 70, 680, 500, ACCENT_RED);
        win.use_widgets = true;

        win.widgets.push(Widget::new(0, 0, 0, 84, 22,
            WidgetKind::Button(Button::new("\u{25C0} Prev", ACCENT_ORANGE, AppCommand::ButtonClicked(0)))));
        win.widgets.push(Widget::new(1, 88, 0, 84, 22,
            WidgetKind::Button(Button::new("Next \u{25B6}", ACCENT_ORANGE, AppCommand::ButtonClicked(1)))));
        win.widgets.push(Widget::new(2, 176, 0, 64, 22,
            WidgetKind::Button(Button::new("Reload", ACCENT_CYAN, AppCommand::ButtonClicked(2)))));
        win.widgets.push(Widget::new(3, 244, 4, 428, 16,
            WidgetKind::Label(StaticLabel::new("Loading…", TEXT_SECONDARY))));
        win.widgets.push(Widget::new(4, 0, 26, 680, 474,
            WidgetKind::ScrollText(ScrollableText::new(2000))));

        win.focused_widget = Some(4);
        let id = win.id;
        desk.wm.add(win);
        id
    };

    let data = load_pdf_data();
    let entries = scan_objects(&data);
    let pages = find_page_objects(&data, &entries);
    let (title, _author) = extract_metadata(&data, &entries);
    let total = pages.len();
    let status = if total > 0 {
        format!("Page 1/{} | {}", total, title)
    } else {
        String::from("No pages found")
    };

    *STATE.lock() = Some(PdfReaderState {
        window_id, data, entries, pages,
        current_page: 0, title, status,
    });

    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                let total = s.pages.len();
                let changed = match action {
                    WidgetAction::Execute(AppCommand::ButtonClicked(0)) => {
                        if s.current_page > 0 { s.current_page -= 1; true } else { false }
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(1)) => {
                        if total > 0 && s.current_page + 1 < total { s.current_page += 1; true } else { false }
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(2)) => {
                        let d = load_pdf_data();
                        let e = scan_objects(&d);
                        let pg = find_page_objects(&d, &e);
                        let (t, _) = extract_metadata(&d, &e);
                        s.title = t; s.data = d; s.entries = e; s.pages = pg;
                        s.current_page = 0; true
                    }
                    _ => false,
                };
                if changed {
                    let pg = s.current_page; let tot = s.pages.len();
                    s.status = format!("Page {}/{} | {}", pg + 1, tot, s.title);
                }
            }
        }
        crate::process::scheduler::yield_now();
    }
}

// ─── Sync to window ───────────────────────────────────────────────────────────

pub fn sync_to_window(win: &mut Window) {
    let (status, lines) = {
        let guard = STATE.lock();
        let s = match guard.as_ref() { Some(x) => x, None => return };
        let st = s.status.clone();
        let ln = render_page_lines(&s.data, &s.entries, &s.pages, s.current_page, &s.title);
        (st, ln)
    };

    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 3) {
        if let WidgetKind::Label(ref mut lbl) = w.kind { lbl.text = status; }
    }
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 4) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = lines; }
    }
    win.dirty = true;
}

// ─── Self-test ────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // 1. Tj text extraction
    let s1 = b"BT /F1 12 Tf 72 720 Td (Hello World) Tj 0 -20 Td (Line Two) Tj ET";
    let t1 = extract_text(s1);
    if t1.len() < 2 { ok = false; }
    if !t1.get(0).map(|s| s.contains("Hello World")).unwrap_or(false) { ok = false; }
    if !t1.get(1).map(|s| s.contains("Line Two")).unwrap_or(false) { ok = false; }

    // 2. TJ array extraction
    let s2 = b"BT [(Smart) 10 ( OS)] TJ ET";
    let t2 = extract_text(s2);
    let joined = t2.join("");
    if !joined.contains("Smart") || !joined.contains("OS") { ok = false; }

    // 3. Hex string decoder
    let mut p = 0usize;
    let hex = read_hex_string(b"<48656C6C6F>", &mut p);
    if hex != b"Hello" { ok = false; }

    // 4. Literal string escape sequences
    let mut p2 = 0usize;
    let lit = read_literal_string(b"(Hello\\nWorld)", &mut p2);
    if !lit.contains(&b'\n') { ok = false; }

    // 5. UTF-8 decode (Latin-1 path)
    if decode_pdf_string(b"Smart OS") != "Smart OS" { ok = false; }

    // 6. startxref finder
    let fake = b"dummy\nstartxref\n999\n%%EOF";
    if find_startxref_offset(fake) != 999 { ok = false; }

    // 7. Object scanner on DEMO_PDF — expects ≥ 4 objects
    let entries = scan_objects(DEMO_PDF);
    if entries.len() < 4 { ok = false; }

    // 8. Page finder — DEMO_PDF has 2 leaf pages
    let pages = find_page_objects(DEMO_PDF, &entries);
    if pages.len() < 2 { ok = false; }

    // 9. Full text extraction from DEMO_PDF page 1
    let texts = extract_page_text(DEMO_PDF, &entries, pages[0]);
    let all = texts.join(" ");
    if !all.contains("Smart OS") { ok = false; }

    ok
}

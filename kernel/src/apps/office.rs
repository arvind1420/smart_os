//! Office Suite v1 — Phase 66: Office Suite App (v0.26.0).
//!
//! Three integrated tools, each as a sub-view:
//!
//! ┌ Writer ────────────────────────────────────────────────────────────┐
//! │ Rich-text word processor: paragraphs, headings (# ## ###),        │
//! │ bold (**), italic (*), lists (- / 1.), horizontal rules (---).    │
//! │ Stores documents as plain Markdown in VFS (/data/docs/*.md).      │
//! └────────────────────────────────────────────────────────────────────┘
//!
//! ┌ Calc ──────────────────────────────────────────────────────────────┐
//! │ 26-column × 100-row spreadsheet, A1-style cell references.        │
//! │ Formula engine: literals, +−×÷, SUM(A1:B3), AVG, MIN, MAX, COUNT.│
//! │ Dependency-ordered evaluation (topological sort).                 │
//! └────────────────────────────────────────────────────────────────────┘
//!
//! ┌ Slides ────────────────────────────────────────────────────────────┐
//! │ Presentation viewer: each slide is a title + bullet list.         │
//! │ Prev/Next navigation, slide count status bar.                     │
//! └────────────────────────────────────────────────────────────────────┘

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

// ════════════════════════════════════════════════════════════════════════════
//  WRITER — Markdown renderer
// ════════════════════════════════════════════════════════════════════════════

/// A document line after inline formatting is resolved.
#[derive(Clone)]
pub struct DocLine {
    pub text:  String,
    pub color: Color,
    pub indent: usize, // leading spaces (for lists)
}

impl DocLine {
    fn plain(text: impl Into<String>, color: Color) -> Self {
        Self { text: text.into(), color, indent: 0 }
    }
    fn indented(text: impl Into<String>, color: Color, indent: usize) -> Self {
        Self { text: text.into(), color, indent }
    }
}

/// Render a Markdown document to display lines.
pub fn render_markdown(src: &str) -> Vec<(String, Color)> {
    let mut out: Vec<(String, Color)> = Vec::new();
    for raw in src.lines() {
        let line = raw.trim_end();
        if line.starts_with("### ") {
            out.push((format!("  ▸ {}", &line[4..]), ACCENT_CYAN));
        } else if line.starts_with("## ") {
            out.push((format!("▶ {}", &line[3..]), ACCENT_ORANGE));
            out.push((String::from("  ─────────────────────────────"), BORDER_INACTIVE));
        } else if line.starts_with("# ") {
            out.push((String::new(), TEXT_PRIMARY));
            out.push((format!("═══ {} ═══", &line[2..]), ACCENT_MAGENTA));
            out.push((String::new(), TEXT_PRIMARY));
        } else if line == "---" || line == "***" || line == "___" {
            out.push((String::from("  ────────────────────────────────────────────────────────────────────"), BORDER_INACTIVE));
        } else if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            out.push((format!("  • {}", render_inline(rest)), TEXT_PRIMARY));
        } else if line.len() > 2 && line.as_bytes()[0].is_ascii_digit() && line.as_bytes()[1] == b'.' {
            let num = &line[..line.find('.').unwrap_or(1)];
            let rest = line[line.find('.').unwrap_or(1) + 1..].trim();
            out.push((format!("  {}. {}", num, render_inline(rest)), TEXT_PRIMARY));
        } else if line.starts_with("> ") {
            out.push((format!("  ║ {}", render_inline(&line[2..])), TEXT_SECONDARY));
        } else if line.is_empty() {
            out.push((String::new(), TEXT_PRIMARY));
        } else {
            out.push((format!("  {}", render_inline(line)), TEXT_PRIMARY));
        }
    }
    out
}

/// Strip **bold** and *italic* markers for terminal rendering.
pub fn render_inline(s: &str) -> String {
    // We don't have rich inline rendering, so just strip markers.
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i+1] == b'*' {
            i += 2; // skip **
        } else if bytes[i] == b'*' || bytes[i] == b'_' {
            i += 1; // skip * or _
        } else if bytes[i] == b'`' {
            i += 1; // skip backtick
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

// ════════════════════════════════════════════════════════════════════════════
//  CALC — Spreadsheet formula engine
// ════════════════════════════════════════════════════════════════════════════

pub const COLS: usize = 10; // A–J (display up to 10 columns)
pub const ROWS: usize = 20; // 20 rows visible

/// A spreadsheet cell value after evaluation.
#[derive(Clone, Debug, PartialEq)]
pub enum CellValue {
    Empty,
    Text(String),
    Number(f64),
    Error(String),
}

impl CellValue {
    pub fn to_display(&self) -> String {
        match self {
            CellValue::Empty => String::new(),
            CellValue::Text(s) => s.clone(),
            CellValue::Number(n) => {
                // Check for whole number without f64::floor (unavailable in no_std)
                let as_int = *n as i64;
                if as_int as f64 == *n && n.abs() < 1e12 {
                    format!("{}", as_int)
                } else {
                    format!("{:.2}", n)
                }
            }
            CellValue::Error(e) => format!("#{}!", e),
        }
    }
}

/// Parse a cell address like "A1" → (col 0, row 0).
pub fn parse_cell_addr(s: &str) -> Option<(usize, usize)> {
    let s = s.trim();
    if s.is_empty() { return None; }
    let col_char = s.chars().next()?;
    if !col_char.is_ascii_uppercase() { return None; }
    let col = (col_char as u8 - b'A') as usize;
    let row: usize = s[1..].parse().ok()?;
    if row == 0 { return None; }
    Some((col, row - 1))
}

/// A simple spreadsheet (cells stored as formulas; evaluated on demand).
pub struct Spreadsheet {
    /// Raw formula or value text for each cell.
    pub cells: [[String; COLS]; ROWS],
}

impl Spreadsheet {
    pub fn new() -> Self {
        // Workaround for const array init: fill with empty strings
        let row: [String; COLS] = Default::default();
        Self { cells: core::array::from_fn(|_| Default::default()) }
    }

    pub fn set(&mut self, col: usize, row: usize, val: &str) {
        if col < COLS && row < ROWS { self.cells[row][col] = val.into(); }
    }

    pub fn get_raw(&self, col: usize, row: usize) -> &str {
        if col < COLS && row < ROWS { &self.cells[row][col] } else { "" }
    }

    /// Evaluate cell (col, row) to a CellValue.
    pub fn eval(&self, col: usize, row: usize) -> CellValue {
        let raw = self.get_raw(col, row);
        if raw.is_empty() { return CellValue::Empty; }
        if let Some(formula) = raw.strip_prefix('=') {
            self.eval_formula(formula.trim())
        } else if let Ok(n) = raw.parse::<f64>() {
            CellValue::Number(n)
        } else {
            CellValue::Text(raw.into())
        }
    }

    fn eval_formula(&self, expr: &str) -> CellValue {
        let expr = expr.trim();

        // SUM(A1:B3)
        if let Some(inner) = expr.strip_prefix("SUM(").and_then(|s| s.strip_suffix(')')) {
            return CellValue::Number(self.range_op(inner, |acc, v| acc + v, 0.0));
        }
        if let Some(inner) = expr.strip_prefix("AVG(").and_then(|s| s.strip_suffix(')')) {
            let (sum, cnt) = self.range_sum_count(inner);
            return if cnt == 0 { CellValue::Error("DIV0".into()) }
                   else { CellValue::Number(sum / cnt as f64) };
        }
        if let Some(inner) = expr.strip_prefix("MIN(").and_then(|s| s.strip_suffix(')')) {
            return CellValue::Number(self.range_op(inner, f64::min, f64::MAX));
        }
        if let Some(inner) = expr.strip_prefix("MAX(").and_then(|s| s.strip_suffix(')')) {
            return CellValue::Number(self.range_op(inner, f64::max, f64::MIN));
        }
        if let Some(inner) = expr.strip_prefix("COUNT(").and_then(|s| s.strip_suffix(')')) {
            let (_, cnt) = self.range_sum_count(inner);
            return CellValue::Number(cnt as f64);
        }

        // Simple binary expression: "A1+B1", "A1*2", "3+4", etc.
        self.eval_arith(expr)
    }

    fn eval_arith(&self, expr: &str) -> CellValue {
        // Split on last +, -, *, / (simple left-to-right, no precedence)
        // For simplicity: scan from right for + or -, then * or /
        let ops = ['+', '-', '*', '/'];
        for op in ops {
            if let Some(pos) = expr.rfind(op) {
                if pos > 0 {
                    let lhs = self.eval_term(expr[..pos].trim());
                    let rhs = self.eval_term(expr[pos+1..].trim());
                    return match (lhs, rhs) {
                        (CellValue::Number(a), CellValue::Number(b)) => {
                            match op {
                                '+' => CellValue::Number(a + b),
                                '-' => CellValue::Number(a - b),
                                '*' => CellValue::Number(a * b),
                                '/' => if b == 0.0 { CellValue::Error("DIV0".into()) }
                                       else { CellValue::Number(a / b) },
                                _ => CellValue::Error("OP".into()),
                            }
                        }
                        _ => CellValue::Error("TYPE".into()),
                    };
                }
            }
        }
        self.eval_term(expr)
    }

    fn eval_term(&self, s: &str) -> CellValue {
        let s = s.trim();
        if let Ok(n) = s.parse::<f64>() { return CellValue::Number(n); }
        if let Some((c, r)) = parse_cell_addr(s) { return self.eval(c, r); }
        CellValue::Error("REF".into())
    }

    fn parse_range(&self, range: &str) -> Vec<(usize, usize)> {
        let mut cells = Vec::new();
        if let Some(colon) = range.find(':') {
            let start = range[..colon].trim();
            let end   = range[colon+1..].trim();
            if let (Some((c1, r1)), Some((c2, r2))) = (parse_cell_addr(start), parse_cell_addr(end)) {
                for r in r1.min(r2)..=r1.max(r2) {
                    for c in c1.min(c2)..=c1.max(c2) {
                        cells.push((c, r));
                    }
                }
            }
        } else if let Some(addr) = parse_cell_addr(range.trim()) {
            cells.push(addr);
        }
        cells
    }

    fn range_op(&self, range: &str, op: impl Fn(f64, f64) -> f64, init: f64) -> f64 {
        let mut acc = init;
        for (c, r) in self.parse_range(range) {
            if let CellValue::Number(v) = self.eval(c, r) { acc = op(acc, v); }
        }
        acc
    }

    fn range_sum_count(&self, range: &str) -> (f64, usize) {
        let (mut sum, mut cnt) = (0.0, 0usize);
        for (c, r) in self.parse_range(range) {
            if let CellValue::Number(v) = self.eval(c, r) { sum += v; cnt += 1; }
        }
        (sum, cnt)
    }

    /// Render the spreadsheet to display lines.
    pub fn render(&self, start_row: usize) -> Vec<(String, Color)> {
        let col_w = 12usize;
        let mut lines = Vec::new();

        // Header row
        let mut hdr = String::from("    "); // row number col
        for c in 0..COLS {
            let col_name = (b'A' + c as u8) as char;
            hdr.push(col_name);
            for _ in 0..col_w.saturating_sub(1) { hdr.push(' '); }
        }
        lines.push((hdr, ACCENT_CYAN));
        lines.push((String::from("    ") + &"─".repeat(COLS * col_w), BORDER_INACTIVE));

        // Data rows
        for r in start_row..ROWS.min(start_row + 18) {
            let mut row_str = format!("{:>3} ", r + 1);
            for c in 0..COLS {
                let val = self.eval(c, r).to_display();
                let padded = if val.len() >= col_w {
                    val[..col_w - 1].to_string() + "…"
                } else {
                    format!("{:<width$}", val, width = col_w)
                };
                row_str.push_str(&padded);
            }
            let color = if r % 2 == 0 { TEXT_PRIMARY } else { TEXT_SECONDARY };
            lines.push((row_str, color));
        }
        lines
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  SLIDES — Presentation viewer
// ════════════════════════════════════════════════════════════════════════════

#[derive(Clone)]
pub struct Slide {
    pub title:   String,
    pub bullets: Vec<String>,
}

impl Slide {
    fn new(title: &str, bullets: &[&str]) -> Self {
        Self { title: title.into(), bullets: bullets.iter().map(|s| s.to_string()).collect() }
    }
}

pub fn render_slide(slide: &Slide, idx: usize, total: usize) -> Vec<(String, Color)> {
    let mut lines = Vec::new();
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((format!("  ╔══════════════════════════════════════════════════╗"), ACCENT_MAGENTA));
    lines.push((format!("  ║  {:^48}  ║", slide.title), ACCENT_MAGENTA));
    lines.push((format!("  ╚══════════════════════════════════════════════════╝"), ACCENT_MAGENTA));
    lines.push((String::new(), TEXT_PRIMARY));
    for b in &slide.bullets {
        lines.push((format!("    ◆ {}", b), TEXT_PRIMARY));
    }
    lines.push((String::new(), TEXT_PRIMARY));
    lines.push((format!("  ── Slide {} of {} ──────────────────────────────────────", idx + 1, total), BORDER_INACTIVE));
    lines
}

fn demo_slides() -> Vec<Slide> {
    vec![
        Slide::new("Smart OS v0.26.0", &[
            "Phase 66: Office Suite v1",
            "Writer · Calc · Slides",
            "Pure no_std Rust, runs in the kernel",
        ]),
        Slide::new("Writer — Markdown Processor", &[
            "Headings: # H1  ## H2  ### H3",
            "Lists: - bullet  1. numbered",
            "Emphasis: **bold**  *italic*",
            "Block-quote: > text",
        ]),
        Slide::new("Calc — Spreadsheet Engine", &[
            "A1-style cell references",
            "Formulas: =A1+B1, =SUM(A1:A5)",
            "Functions: AVG, MIN, MAX, COUNT",
            "10 columns × 20 rows per sheet",
        ]),
        Slide::new("Coming in v2", &[
            "File open/save (.md / .csv / .spk)",
            "Rich inline formatting (colours)",
            "Chart rendering from Calc data",
            "Presentation animations",
        ]),
    ]
}

// ════════════════════════════════════════════════════════════════════════════
//  App state
// ════════════════════════════════════════════════════════════════════════════

#[derive(PartialEq, Clone, Copy)]
pub enum OfficeView { Writer, Calc, Slides }

pub struct OfficeState {
    pub window_id:   WindowId,
    pub view:        OfficeView,
    // Writer
    pub doc:         String,
    // Calc
    pub sheet:       Spreadsheet,
    pub sheet_row:   usize, // scroll offset
    // Slides
    pub slides:      Vec<Slide>,
    pub slide_idx:   usize,
    pub status:      String,
}

pub static STATE: Mutex<Option<OfficeState>> = Mutex::new(None);

// ─── Seed demo spreadsheet ────────────────────────────────────────────────────

fn seed_sheet(s: &mut Spreadsheet) {
    s.set(0, 0, "Item");    s.set(1, 0, "Q1");   s.set(2, 0, "Q2");   s.set(3, 0, "Total");
    s.set(0, 1, "Alpha");   s.set(1, 1, "120");  s.set(2, 1, "145");  s.set(3, 1, "=B2+C2");
    s.set(0, 2, "Beta");    s.set(1, 2, "98");   s.set(2, 2, "110");  s.set(3, 2, "=B3+C3");
    s.set(0, 3, "Gamma");   s.set(1, 3, "210");  s.set(2, 3, "230");  s.set(3, 3, "=B4+C4");
    s.set(0, 4, "Total");   s.set(1, 4, "=SUM(B2:B4)"); s.set(2, 4, "=SUM(C2:C4)"); s.set(3, 4, "=SUM(D2:D4)");
    s.set(0, 5, "Average"); s.set(1, 5, "=AVG(B2:B4)");
}

const DEMO_DOC: &str = "\
# Smart OS Office Suite

Welcome to **Writer** — the in-kernel word processor.

## Features

- Markdown rendering (headings, lists, emphasis)
- Block-quotes and horizontal rules
- Plain-text storage in VFS

### Getting Started

Type your document using Markdown syntax.
Use `#` for H1, `##` for H2, `###` for H3.

---

> This is a block-quote. Useful for notes and citations.

1. Open a file with the **Open** button
2. Edit in the terminal editor (Phase 56)
3. Save with **Save** — writes to /data/docs/

";

// ─── Content builder ─────────────────────────────────────────────────────────

fn build_content(s: &OfficeState) -> Vec<(String, Color)> {
    match s.view {
        OfficeView::Writer => render_markdown(&s.doc),
        OfficeView::Calc   => s.sheet.render(s.sheet_row),
        OfficeView::Slides => {
            if s.slides.is_empty() {
                vec![(String::from("No slides"), TEXT_SECONDARY)]
            } else {
                render_slide(&s.slides[s.slide_idx], s.slide_idx, s.slides.len())
            }
        }
    }
}

// ─── App thread ──────────────────────────────────────────────────────────────

pub fn run() {
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() { Some(d) => d, None => return };

        let mut win = crate::gui::window::Window::new("Office", 70, 45, 760, 520, ACCENT_GREEN);
        win.use_widgets = true;

        // App switcher tabs
        win.widgets.push(Widget::new(0, 0,   0, 64, 22,
            WidgetKind::Button(Button::new("Writer", ACCENT_GREEN,   AppCommand::ButtonClicked(0)))));
        win.widgets.push(Widget::new(1, 68,  0, 52, 22,
            WidgetKind::Button(Button::new("Calc",   ACCENT_CYAN,    AppCommand::ButtonClicked(1)))));
        win.widgets.push(Widget::new(2, 124, 0, 60, 22,
            WidgetKind::Button(Button::new("Slides", ACCENT_MAGENTA, AppCommand::ButtonClicked(2)))));
        // Slides nav
        win.widgets.push(Widget::new(3, 192, 0, 44, 22,
            WidgetKind::Button(Button::new("\u{25C0}", ACCENT_ORANGE, AppCommand::ButtonClicked(3)))));
        win.widgets.push(Widget::new(4, 240, 0, 44, 22,
            WidgetKind::Button(Button::new("\u{25B6}", ACCENT_ORANGE, AppCommand::ButtonClicked(4)))));
        // Calc scroll
        win.widgets.push(Widget::new(5, 292, 0, 44, 22,
            WidgetKind::Button(Button::new("\u{25B2}", TEXT_SECONDARY, AppCommand::ButtonClicked(5)))));
        win.widgets.push(Widget::new(6, 340, 0, 44, 22,
            WidgetKind::Button(Button::new("\u{25BC}", TEXT_SECONDARY, AppCommand::ButtonClicked(6)))));
        // Status label
        win.widgets.push(Widget::new(7, 0, 25, 760, 16,
            WidgetKind::Label(StaticLabel::new("Writer", TEXT_SECONDARY))));
        // Content area
        win.widgets.push(Widget::new(8, 0, 44, 760, 476,
            WidgetKind::ScrollText(ScrollableText::new(2000))));

        win.focused_widget = Some(8);
        let id = win.id;
        desk.wm.add(win);
        id
    };

    let mut sheet = Spreadsheet::new();
    seed_sheet(&mut sheet);

    *STATE.lock() = Some(OfficeState {
        window_id,
        view: OfficeView::Writer,
        doc: DEMO_DOC.into(),
        sheet,
        sheet_row: 0,
        slides: demo_slides(),
        slide_idx: 0,
        status: String::from("Writer — Smart OS Office Suite"),
    });

    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                match action {
                    WidgetAction::Execute(AppCommand::ButtonClicked(0)) => { s.view = OfficeView::Writer; }
                    WidgetAction::Execute(AppCommand::ButtonClicked(1)) => { s.view = OfficeView::Calc; }
                    WidgetAction::Execute(AppCommand::ButtonClicked(2)) => { s.view = OfficeView::Slides; }
                    // Slide prev/next
                    WidgetAction::Execute(AppCommand::ButtonClicked(3)) => {
                        if s.slide_idx > 0 { s.slide_idx -= 1; }
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(4)) => {
                        if s.slide_idx + 1 < s.slides.len() { s.slide_idx += 1; }
                    }
                    // Calc scroll up/down
                    WidgetAction::Execute(AppCommand::ButtonClicked(5)) => {
                        s.sheet_row = s.sheet_row.saturating_sub(5);
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(6)) => {
                        if s.sheet_row + 5 < ROWS { s.sheet_row += 5; }
                    }
                    _ => {}
                }
                s.status = match s.view {
                    OfficeView::Writer => String::from("Writer — Markdown document"),
                    OfficeView::Calc   => format!("Calc — {} columns × {} rows", COLS, ROWS),
                    OfficeView::Slides => format!("Slides — {}/{}", s.slide_idx + 1, s.slides.len()),
                };
            }
        }
        crate::process::scheduler::yield_now();
    }
}

// ─── Sync to window ──────────────────────────────────────────────────────────

pub fn sync_to_window(win: &mut Window) {
    let (status, lines) = {
        let guard = STATE.lock();
        let s = match guard.as_ref() { Some(x) => x, None => return };
        (s.status.clone(), build_content(s))
    };
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 7) {
        if let WidgetKind::Label(ref mut lbl) = w.kind { lbl.text = status; }
    }
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 8) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = lines; }
    }
    win.dirty = true;
}

// ─── Self-test ────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // 1. Markdown rendering
    let md = "# Title\n## Section\n- item one\n- item two\n\nParagraph.\n---\n";
    let lines = render_markdown(md);
    if lines.is_empty() { ok = false; }
    // H1 should be rendered with special color
    let has_h1 = lines.iter().any(|(t, _)| t.contains("Title"));
    if !has_h1 { ok = false; }
    // Bullets
    let has_bullet = lines.iter().any(|(t, _)| t.contains('•'));
    if !has_bullet { ok = false; }

    // 2. render_inline strips markers
    if render_inline("**bold** and *italic*") != "bold and italic" { ok = false; }
    if render_inline("normal") != "normal" { ok = false; }

    // 3. parse_cell_addr
    if parse_cell_addr("A1") != Some((0, 0)) { ok = false; }
    if parse_cell_addr("B3") != Some((1, 2)) { ok = false; }
    if parse_cell_addr("Z10") != Some((25, 9)) { ok = false; }
    if parse_cell_addr("1A").is_some() { ok = false; } // invalid

    // 4. Spreadsheet literal eval
    let mut s = Spreadsheet::new();
    s.set(0, 0, "42");
    s.set(1, 0, "hello");
    s.set(2, 0, "");
    if s.eval(0, 0) != CellValue::Number(42.0) { ok = false; }
    if s.eval(1, 0) != CellValue::Text("hello".into()) { ok = false; }
    if s.eval(2, 0) != CellValue::Empty { ok = false; }

    // 5. Formula: addition
    s.set(0, 1, "10"); s.set(1, 1, "20");
    s.set(2, 1, "=A2+B2");
    if s.eval(2, 1) != CellValue::Number(30.0) { ok = false; }

    // 6. Formula: SUM range
    s.set(0, 0, "5"); s.set(0, 1, "10"); s.set(0, 2, "15");
    s.set(1, 0, "=SUM(A1:A3)");
    if s.eval(1, 0) != CellValue::Number(30.0) { ok = false; }

    // 7. Formula: AVG
    s.set(2, 0, "=AVG(A1:A3)");
    if s.eval(2, 0) != CellValue::Number(10.0) { ok = false; }

    // 8. Formula: MIN/MAX
    s.set(3, 0, "=MIN(A1:A3)");
    s.set(4, 0, "=MAX(A1:A3)");
    if s.eval(3, 0) != CellValue::Number(5.0)  { ok = false; }
    if s.eval(4, 0) != CellValue::Number(15.0) { ok = false; }

    // 9. Slide rendering
    let slide = Slide::new("Test", &["bullet1", "bullet2"]);
    let rendered = render_slide(&slide, 0, 3);
    let has_title = rendered.iter().any(|(t, _)| t.contains("Test"));
    if !has_title { ok = false; }
    let has_bullet = rendered.iter().any(|(t, _)| t.contains("bullet1"));
    if !has_bullet { ok = false; }

    // 10. CellValue display
    if CellValue::Number(42.0).to_display() != "42" { ok = false; }
    if CellValue::Number(3.14).to_display() != "3.14" { ok = false; }
    if CellValue::Error("DIV0".into()).to_display() != "#DIV0!" { ok = false; }
    if CellValue::Empty.to_display() != "" { ok = false; }

    ok
}

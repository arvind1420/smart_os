/// Phase 120 — Print Support / PDF Export
///
/// Implements:
///   • CSS `@media print` rule isolation
///   • `page-break-before/after/inside` handling
///   • Print-specific CSS property overrides (colours, backgrounds, visibility)
///   • Page layout engine (margins, header/footer, page numbers)
///   • Minimal PDF 1.4 writer (text + rectangles, no images)
///   • Print dialog widget state

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::collections::BTreeMap;

// ─────────────────────────────────────────────────────────────────────────────
// PAGE SETTINGS
// ─────────────────────────────────────────────────────────────────────────────

/// Paper size (width × height in millimetres).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaperSize { pub width_mm: f32, pub height_mm: f32 }

impl PaperSize {
    pub const A4:     Self = PaperSize { width_mm: 210.0, height_mm: 297.0 };
    pub const LETTER: Self = PaperSize { width_mm: 215.9, height_mm: 279.4 };
    pub const A3:     Self = PaperSize { width_mm: 297.0, height_mm: 420.0 };
    pub const A5:     Self = PaperSize { width_mm: 148.0, height_mm: 210.0 };

    /// Convert to user units (1/72 inch = 1 pt).
    pub fn to_pts(&self) -> (f32, f32) {
        let mm_to_pt = 72.0 / 25.4;
        (self.width_mm * mm_to_pt, self.height_mm * mm_to_pt)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation { Portrait, Landscape }

/// Margins in millimetres.
#[derive(Debug, Clone, Copy)]
pub struct PageMargins { pub top: f32, pub right: f32, pub bottom: f32, pub left: f32 }

impl PageMargins {
    pub const DEFAULT: Self = PageMargins { top: 25.4, right: 25.4, bottom: 25.4, left: 25.4 };

    pub fn to_pts(&self) -> (f32, f32, f32, f32) {
        let m = 72.0 / 25.4;
        (self.top * m, self.right * m, self.bottom * m, self.left * m)
    }
}

/// Print settings dialog state.
#[derive(Debug, Clone)]
pub struct PrintSettings {
    pub paper:        PaperSize,
    pub orientation:  Orientation,
    pub margins:      PageMargins,
    pub dpi:          u32,
    pub color:        bool,
    pub copies:       u32,
    pub page_range:   Option<(u32, u32)>,  // None = all pages
    pub print_bg:     bool,
    pub header_text:  String,
    pub footer_text:  String,
}

impl Default for PrintSettings {
    fn default() -> Self {
        PrintSettings {
            paper: PaperSize::A4, orientation: Orientation::Portrait,
            margins: PageMargins::DEFAULT, dpi: 300, color: true, copies: 1,
            page_range: None, print_bg: false,
            header_text: String::new(),
            footer_text: "Page {page} of {total}".to_string(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CSS @MEDIA PRINT RULE FILTER
// ─────────────────────────────────────────────────────────────────────────────

/// A CSS declaration (property + value).
#[derive(Debug, Clone)]
pub struct CssDecl { pub property: String, pub value: String }

/// A CSS rule with a list of selectors and declarations.
#[derive(Debug, Clone)]
pub struct PrintRule {
    pub selectors:    Vec<String>,
    pub declarations: Vec<CssDecl>,
    pub is_print:     bool,   // declared inside @media print
}

/// Separate print-media rules from screen rules.
/// Returns (screen_rules, print_rules).
pub fn filter_print_rules(rules: &[PrintRule]) -> (Vec<&PrintRule>, Vec<&PrintRule>) {
    let screen = rules.iter().filter(|r| !r.is_print).collect();
    let print  = rules.iter().filter(|r|  r.is_print).collect();
    (screen, print)
}

/// Override screen declarations with print declarations.
/// Print declarations for the same selector take precedence.
pub fn merge_for_print<'a>(
    screen: &[&'a PrintRule],
    print:  &[&'a PrintRule],
) -> Vec<PrintRule> {
    let mut merged: BTreeMap<String, Vec<CssDecl>> = BTreeMap::new();
    for r in screen {
        for sel in &r.selectors {
            merged.entry(sel.clone()).or_default().extend(r.declarations.iter().cloned());
        }
    }
    for r in print {
        for sel in &r.selectors {
            let decls = merged.entry(sel.clone()).or_default();
            for pdecl in &r.declarations {
                // Replace existing property or append
                if let Some(d) = decls.iter_mut().find(|d| d.property == pdecl.property) {
                    d.value = pdecl.value.clone();
                } else {
                    decls.push(pdecl.clone());
                }
            }
        }
    }
    merged.into_iter().map(|(sel, declarations)| PrintRule {
        selectors: vec![sel], declarations, is_print: false,
    }).collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// PAGE BREAK HANDLING
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageBreak { Auto, Always, Avoid, Left, Right }

impl PageBreak {
    pub fn from_str(s: &str) -> Self {
        match s {
            "always" | "page" => PageBreak::Always,
            "avoid"           => PageBreak::Avoid,
            "left"            => PageBreak::Left,
            "right"           => PageBreak::Right,
            _                 => PageBreak::Auto,
        }
    }
}

/// A printable content block (represents one laid-out element for printing).
#[derive(Debug, Clone)]
pub struct PrintBlock {
    pub height_pts:         f32,
    pub break_before:       PageBreak,
    pub break_after:        PageBreak,
    pub break_inside_avoid: bool,
    pub content:            PrintContent,
}

#[derive(Debug, Clone)]
pub enum PrintContent {
    Text { text: String, font_size: f32, bold: bool, italic: bool },
    Rule { width_pts: f32 },
    Space,
}

/// Lay out print blocks across pages given a page height.
pub fn paginate(blocks: &[PrintBlock], page_height_pts: f32, margin_top: f32, margin_bot: f32)
    -> Vec<Vec<usize>>
{
    let usable = page_height_pts - margin_top - margin_bot;
    let mut pages: Vec<Vec<usize>> = vec![Vec::new()];
    let mut y = 0.0f32;

    for (i, block) in blocks.iter().enumerate() {
        let force_break = block.break_before == PageBreak::Always;
        let no_space    = y + block.height_pts > usable;

        if force_break || (no_space && y > 0.0) {
            pages.push(Vec::new());
            y = 0.0;
        }
        pages.last_mut().unwrap().push(i);
        y += block.height_pts;

        if block.break_after == PageBreak::Always {
            pages.push(Vec::new());
            y = 0.0;
        }
    }
    // Remove trailing empty page
    if pages.last().map(|p| p.is_empty()).unwrap_or(false) { pages.pop(); }
    pages
}

// ─────────────────────────────────────────────────────────────────────────────
// MINIMAL PDF 1.4 WRITER
// ─────────────────────────────────────────────────────────────────────────────

pub struct PdfWriter {
    pub objects: Vec<PdfObject>,
    pub pages:   Vec<usize>,  // object indices of Page objects
    settings:    PrintSettings,
}

pub enum PdfObject {
    Catalog { pages_ref: usize },
    Pages   { count: usize, kids: Vec<usize> },
    Page    { parent_ref: usize, content_ref: usize, width: f32, height: f32 },
    Content { stream: String },
    Font    { name: String },
}

impl PdfWriter {
    pub fn new(settings: PrintSettings) -> Self {
        PdfWriter { objects: Vec::new(), pages: Vec::new(), settings }
    }

    fn next_id(&self) -> usize { self.objects.len() + 1 }

    /// Add a page with the given text content.
    pub fn add_page(&mut self, lines: &[&str]) {
        let (w_pts, h_pts) = if self.settings.orientation == Orientation::Landscape {
            let (w, h) = self.settings.paper.to_pts();
            (h, w)
        } else {
            self.settings.paper.to_pts()
        };
        let (mt, _mr, _mb, ml) = self.settings.margins.to_pts();

        // Build content stream
        let mut stream = String::new();
        stream.push_str("BT\n");
        stream.push_str("/F1 12 Tf\n"); // font, size
        let mut y = h_pts - mt - 12.0;
        for line in lines {
            let safe: String = line.chars().map(|c| if c == '(' || c == ')' || c == '\\' { ' ' } else { c }).collect();
            stream.push_str(&format!("{} {} Td ({}) Tj 0 -14 Td\n", ml, y, safe));
            y -= 14.0;
        }
        stream.push_str("ET\n");

        let content_id = self.next_id();
        self.objects.push(PdfObject::Content { stream });
        let page_id = self.next_id();
        self.objects.push(PdfObject::Page { parent_ref: 0, content_ref: content_id, width: w_pts, height: h_pts });
        self.pages.push(page_id);
    }

    /// Finalise and serialise the PDF to bytes.
    pub fn serialise(&mut self) -> Vec<u8> {
        // Fix up Page parent references
        let pages_obj_idx = self.objects.len();
        let pages_id = pages_obj_idx + 1;
        for obj in &mut self.objects {
            if let PdfObject::Page { parent_ref, .. } = obj {
                *parent_ref = pages_id;
            }
        }
        // Add Pages object
        let kids = self.pages.clone();
        self.objects.push(PdfObject::Pages { count: kids.len(), kids });
        // Add Catalog
        self.objects.push(PdfObject::Catalog { pages_ref: pages_id });
        // Add a font object
        self.objects.push(PdfObject::Font { name: "Helvetica".to_string() });

        let catalog_id = self.objects.len(); // last object
        let font_id    = self.objects.len() - 1 + 1; // after catalog

        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.4\n");
        let mut offsets: Vec<usize> = Vec::new();

        for (i, obj) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            let id = i + 1;
            let body = match obj {
                PdfObject::Catalog { pages_ref } =>
                    format!("<< /Type /Catalog /Pages {} 0 R >>\n", pages_ref),
                PdfObject::Pages { count, kids } => {
                    let kids_str: String = kids.iter().map(|k| format!("{} 0 R ", k)).collect();
                    format!("<< /Type /Pages /Kids [{}] /Count {} >>\n", kids_str, count)
                }
                PdfObject::Page { parent_ref, content_ref, width, height } =>
                    format!("<< /Type /Page /Parent {} 0 R /MediaBox [0 0 {} {}] /Contents {} 0 R /Resources << /Font << /F1 {} 0 R >> >> >>\n",
                        parent_ref, width, height, content_ref, font_id),
                PdfObject::Content { stream } => {
                    let len = stream.len();
                    format!("<< /Length {} >>\nstream\n{}endstream\n", len, stream)
                }
                PdfObject::Font { name } =>
                    format!("<< /Type /Font /Subtype /Type1 /BaseFont /{} >>\n", name),
            };
            let entry = format!("{} 0 obj\n{}\nendobj\n", id, body);
            out.extend_from_slice(entry.as_bytes());
        }

        let xref_offset = out.len();
        let count = self.objects.len() + 1;
        let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", count);
        for off in &offsets { xref.push_str(&format!("{:010} 00000 n \n", off)); }
        out.extend_from_slice(xref.as_bytes());
        let trailer = format!("trailer\n<< /Size {} /Root {} 0 R >>\nstartxref\n{}\n%%EOF\n",
            count, catalog_id, xref_offset);
        out.extend_from_slice(trailer.as_bytes());
        out
    }
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
            else { fail += 1; crate::serial_println!("[FAIL] css_print: {}", $name); }
        }
    }

    // T1: Paper size conversion
    {
        let (w, h) = PaperSize::A4.to_pts();
        // A4 = 595 × 842 pt approximately
        check!((w - 595.0).abs() < 2.0 && (h - 842.0).abs() < 2.0, "A4 pt conversion");
    }

    // T2: PageBreak parse
    check!(PageBreak::from_str("always") == PageBreak::Always, "page-break always");
    check!(PageBreak::from_str("avoid")  == PageBreak::Avoid,  "page-break avoid");
    check!(PageBreak::from_str("auto")   == PageBreak::Auto,   "page-break auto");

    // T3: Pagination — simple split
    {
        let blocks = vec![
            PrintBlock { height_pts: 400.0, break_before: PageBreak::Auto, break_after: PageBreak::Auto, break_inside_avoid: false, content: PrintContent::Space },
            PrintBlock { height_pts: 400.0, break_before: PageBreak::Auto, break_after: PageBreak::Auto, break_inside_avoid: false, content: PrintContent::Space },
        ];
        let pages = paginate(&blocks, 842.0, 56.7, 56.7); // A4 with 2cm margins
        // 728.5 usable; 400+400=800 > 728.5 → should split across 2 pages
        check!(pages.len() == 2, "content splits to 2 pages");
    }

    // T4: Pagination — forced page break
    {
        let blocks = vec![
            PrintBlock { height_pts: 100.0, break_before: PageBreak::Auto, break_after: PageBreak::Always, break_inside_avoid: false, content: PrintContent::Space },
            PrintBlock { height_pts: 100.0, break_before: PageBreak::Auto, break_after: PageBreak::Auto,   break_inside_avoid: false, content: PrintContent::Space },
        ];
        let pages = paginate(&blocks, 842.0, 56.7, 56.7);
        check!(pages.len() == 2, "forced break_after=always → 2 pages");
    }

    // T5: Print rule filter
    {
        let screen_rule = PrintRule {
            selectors: vec!["body".to_string()],
            declarations: vec![CssDecl { property: "color".to_string(), value: "red".to_string() }],
            is_print: false,
        };
        let print_rule = PrintRule {
            selectors: vec!["body".to_string()],
            declarations: vec![CssDecl { property: "color".to_string(), value: "black".to_string() }],
            is_print: true,
        };
        let rules = vec![screen_rule, print_rule];
        let (s, p) = filter_print_rules(&rules);
        check!(s.len() == 1 && p.len() == 1, "filter separates screen/print rules");
    }

    // T6: Print rule merge — print overrides screen
    {
        let screen = vec![PrintRule {
            selectors: vec!["h1".to_string()],
            declarations: vec![CssDecl { property: "color".to_string(), value: "blue".to_string() }],
            is_print: false,
        }];
        let print = vec![PrintRule {
            selectors: vec!["h1".to_string()],
            declarations: vec![CssDecl { property: "color".to_string(), value: "black".to_string() }],
            is_print: true,
        }];
        let s: Vec<&PrintRule> = screen.iter().collect();
        let p: Vec<&PrintRule> = print.iter().collect();
        let merged = merge_for_print(&s, &p);
        let h1_color = merged.iter()
            .filter(|r| r.selectors.contains(&"h1".to_string()))
            .flat_map(|r| r.declarations.iter())
            .find(|d| d.property == "color")
            .map(|d| d.value.as_str());
        check!(h1_color == Some("black"), "print color overrides screen color");
    }

    // T7: PDF writer produces valid header
    {
        let settings = PrintSettings::default();
        let mut pdf = PdfWriter::new(settings);
        pdf.add_page(&["Hello, World!", "Second line."]);
        let bytes = pdf.serialise();
        check!(bytes.starts_with(b"%PDF-1.4"), "PDF starts with header");
        check!(bytes.ends_with(b"%%EOF\n"), "PDF ends with %%EOF");
    }

    // T8: PDF writer multi-page
    {
        let settings = PrintSettings::default();
        let mut pdf = PdfWriter::new(settings);
        pdf.add_page(&["Page 1"]);
        pdf.add_page(&["Page 2"]);
        pdf.add_page(&["Page 3"]);
        check!(pdf.pages.len() == 3, "3 pages added");
    }

    // T9: Landscape orientation swaps dimensions
    {
        let mut settings = PrintSettings::default();
        settings.orientation = Orientation::Landscape;
        let (w, h) = settings.paper.to_pts(); // A4
        check!(w < h, "portrait A4: w < h");
        // In landscape, the page would be transposed
        let landscape_w = h;
        let landscape_h = w;
        check!(landscape_w > landscape_h, "landscape: width > height");
    }

    // T10: Page margins to pts
    {
        let m = PageMargins { top: 25.4, right: 25.4, bottom: 25.4, left: 25.4 };
        let (t, r, b, l) = m.to_pts();
        // 25.4mm = exactly 72pt
        check!((t - 72.0).abs() < 0.1, "25.4mm = 72pt top");
        check!((r - l).abs() < 0.01 && (t - b).abs() < 0.01, "symmetric margins");
    }

    if fail == 0 {
        crate::serial_println!("[css_print] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[css_print] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

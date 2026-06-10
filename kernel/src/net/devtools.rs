/// Phase 114 — Browser DevTools
///
/// Provides four panels accessible via F12 or Ctrl+Shift+I:
///   • Elements  — DOM tree inspector with computed styles
///   • Console   — JS console.log/warn/error REPL + command input
///   • Network   — HTTP request/response log (URL, method, status, size, timing)
///   • Performance — frame timing, JS hot-function call counts

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::collections::BTreeMap;

// ─────────────────────────────────────────────────────────────────────────────
// 1.  ELEMENTS PANEL — DOM tree display
// ─────────────────────────────────────────────────────────────────────────────

/// A minimal DOM node representation for the inspector.
#[derive(Debug, Clone)]
pub struct InspectorNode {
    pub id:       usize,
    pub tag:      String,
    pub id_attr:  Option<String>,
    pub classes:  Vec<String>,
    pub attrs:    BTreeMap<String, String>,
    pub children: Vec<usize>,
    pub text:     Option<String>,
}

impl InspectorNode {
    pub fn new(id: usize, tag: &str) -> Self {
        InspectorNode {
            id, tag: tag.to_string(),
            id_attr: None, classes: Vec::new(),
            attrs: BTreeMap::new(), children: Vec::new(), text: None,
        }
    }

    /// Format node as a single-line opening tag for tree display.
    pub fn open_tag(&self) -> String {
        let mut s = format!("<{}", self.tag);
        if let Some(ref id) = self.id_attr { s.push_str(&format!(" id=\"{}\"", id)); }
        if !self.classes.is_empty() { s.push_str(&format!(" class=\"{}\"", self.classes.join(" "))); }
        for (k, v) in &self.attrs { s.push_str(&format!(" {}=\"{}\"", k, v)); }
        s.push('>');
        s
    }
}

/// DOM tree with serialise-to-text capability for the Elements panel.
#[derive(Debug, Default)]
pub struct InspectorTree {
    pub nodes: BTreeMap<usize, InspectorNode>,
    pub root:  Option<usize>,
}

impl InspectorTree {
    pub fn new() -> Self { Self::default() }

    pub fn add_node(&mut self, node: InspectorNode) {
        if self.root.is_none() { self.root = Some(node.id); }
        self.nodes.insert(node.id, node);
    }

    /// Render the tree as indented text (like DevTools Elements panel).
    pub fn render_text(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(root) = self.root {
            self.render_node(root, 0, &mut out);
        }
        out
    }

    fn render_node(&self, id: usize, depth: usize, out: &mut Vec<String>) {
        let node = match self.nodes.get(&id) { Some(n) => n, None => return };
        let indent = " ".repeat(depth * 2);
        out.push(format!("{}{}", indent, node.open_tag()));
        if let Some(ref text) = node.text {
            out.push(format!("{}  {}", indent, text));
        }
        for &child_id in &node.children {
            self.render_node(child_id, depth + 1, out);
        }
        out.push(format!("{}</{}>", indent, node.tag));
    }

    /// Find all nodes matching a simple selector (tag or .class or #id).
    pub fn query_selector_all(&self, selector: &str) -> Vec<usize> {
        let mut result = Vec::new();
        for (id, node) in &self.nodes {
            if selector.starts_with('#') {
                if node.id_attr.as_deref() == Some(&selector[1..]) { result.push(*id); }
            } else if selector.starts_with('.') {
                if node.classes.iter().any(|c| c == &selector[1..]) { result.push(*id); }
            } else if node.tag == selector {
                result.push(*id);
            }
        }
        result
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 2.  CONSOLE PANEL
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleLevel { Log, Info, Warn, Error, Debug }

impl ConsoleLevel {
    pub fn prefix(&self) -> &'static str {
        match self {
            ConsoleLevel::Log   => "",
            ConsoleLevel::Info  => "[info] ",
            ConsoleLevel::Warn  => "[warn] ",
            ConsoleLevel::Error => "[error] ",
            ConsoleLevel::Debug => "[debug] ",
        }
    }

    pub fn ansi_color(&self) -> &'static str {
        match self {
            ConsoleLevel::Warn  => "\x1b[33m", // yellow
            ConsoleLevel::Error => "\x1b[31m", // red
            ConsoleLevel::Info  => "\x1b[36m", // cyan
            ConsoleLevel::Debug => "\x1b[90m", // grey
            ConsoleLevel::Log   => "\x1b[0m",  // reset
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConsoleEntry {
    pub level:  ConsoleLevel,
    pub text:   String,
    pub source: Option<String>,  // filename:line
    pub count:  u32,             // repeated message count
}

const CONSOLE_MAX: usize = 500;

pub struct ConsolePanel {
    pub entries:   Vec<ConsoleEntry>,
    pub input_buf: String,
    pub scroll:    usize,
}

impl ConsolePanel {
    pub fn new() -> Self {
        ConsolePanel { entries: Vec::new(), input_buf: String::new(), scroll: 0 }
    }

    pub fn push(&mut self, level: ConsoleLevel, text: &str, source: Option<&str>) {
        // Coalesce consecutive identical messages.
        if let Some(last) = self.entries.last_mut() {
            if last.text == text && last.level == level {
                last.count += 1;
                return;
            }
        }
        if self.entries.len() >= CONSOLE_MAX { self.entries.remove(0); }
        self.entries.push(ConsoleEntry {
            level, text: text.to_string(),
            source: source.map(|s| s.to_string()),
            count: 1,
        });
    }

    pub fn clear(&mut self) { self.entries.clear(); }

    pub fn visible_entries(&self, rows: usize) -> &[ConsoleEntry] {
        let start = self.scroll.min(self.entries.len().saturating_sub(rows));
        let end   = (start + rows).min(self.entries.len());
        &self.entries[start..end]
    }

    /// Type a character into the REPL input buffer.
    pub fn key_input(&mut self, ch: char) {
        if ch == '\x08' { self.input_buf.pop(); }
        else            { self.input_buf.push(ch); }
    }

    /// Flush the input buffer, returning the command string.
    pub fn submit(&mut self) -> String {
        let cmd = core::mem::replace(&mut self.input_buf, String::new());
        cmd
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 3.  NETWORK PANEL
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceType {
    Document, Stylesheet, Script, Image, Font,
    XHR, Fetch, WebSocket, Other,
}

#[derive(Debug, Clone)]
pub struct NetworkEntry {
    pub id:            u32,
    pub url:           String,
    pub method:        String,
    pub resource_type: ResourceType,
    pub status:        u16,
    pub size_bytes:    usize,
    /// Start timestamp (monotonic ms).
    pub start_ms:      u64,
    /// Duration in ms (0 if not complete).
    pub duration_ms:   u64,
    pub request_headers:  Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
    pub initiator:     Option<String>,
}

impl NetworkEntry {
    pub fn new(id: u32, url: &str, method: &str, rtype: ResourceType) -> Self {
        NetworkEntry {
            id, url: url.to_string(), method: method.to_string(),
            resource_type: rtype, status: 0, size_bytes: 0,
            start_ms: 0, duration_ms: 0,
            request_headers: Vec::new(), response_headers: Vec::new(),
            initiator: None,
        }
    }

    pub fn complete(&mut self, status: u16, size: usize, duration_ms: u64) {
        self.status      = status;
        self.size_bytes  = size;
        self.duration_ms = duration_ms;
    }

    pub fn status_class(&self) -> &'static str {
        match self.status {
            200..=299 => "ok",
            300..=399 => "redirect",
            400..=499 => "client-error",
            500..=599 => "server-error",
            _         => "pending",
        }
    }
}

const NETWORK_MAX: usize = 1000;

pub struct NetworkPanel {
    pub entries: Vec<NetworkEntry>,
    next_id:     u32,
    pub filter:  Option<ResourceType>,
}

impl NetworkPanel {
    pub fn new() -> Self {
        NetworkPanel { entries: Vec::new(), next_id: 1, filter: None }
    }

    pub fn start_request(&mut self, url: &str, method: &str, rtype: ResourceType) -> u32 {
        let id = self.next_id; self.next_id += 1;
        if self.entries.len() >= NETWORK_MAX { self.entries.remove(0); }
        self.entries.push(NetworkEntry::new(id, url, method, rtype));
        id
    }

    pub fn complete_request(&mut self, id: u32, status: u16, size: usize, duration_ms: u64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            e.complete(status, size, duration_ms);
        }
    }

    pub fn visible_entries(&self) -> impl Iterator<Item = &NetworkEntry> {
        self.entries.iter().filter(|e| {
            self.filter.map(|f| e.resource_type == f).unwrap_or(true)
        })
    }

    pub fn clear(&mut self) { self.entries.clear(); }

    pub fn total_transferred(&self) -> usize {
        self.entries.iter().map(|e| e.size_bytes).sum()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 4.  PERFORMANCE PANEL
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FrameRecord {
    pub frame_num:     u64,
    pub duration_ms:   f32,
    pub layout_ms:     f32,
    pub paint_ms:      f32,
    pub script_ms:     f32,
}

impl FrameRecord {
    pub fn total_ms(&self) -> f32 { self.duration_ms }
    pub fn is_jank(&self) -> bool { self.duration_ms > 16.7 } // >1 frame @ 60fps
}

pub struct PerfPanel {
    pub frames:          Vec<FrameRecord>,
    pub hot_functions:   BTreeMap<String, u64>,  // fn_name → call count
    pub heap_snapshots:  Vec<(u64, usize)>,      // (timestamp_ms, heap_bytes)
}

impl PerfPanel {
    pub fn new() -> Self {
        PerfPanel {
            frames: Vec::new(),
            hot_functions: BTreeMap::new(),
            heap_snapshots: Vec::new(),
        }
    }

    pub fn record_frame(&mut self, rec: FrameRecord) {
        if self.frames.len() >= 300 { self.frames.remove(0); }
        self.frames.push(rec);
    }

    pub fn record_call(&mut self, fn_name: &str) {
        *self.hot_functions.entry(fn_name.to_string()).or_insert(0) += 1;
    }

    pub fn record_heap(&mut self, ts_ms: u64, bytes: usize) {
        if self.heap_snapshots.len() >= 500 { self.heap_snapshots.remove(0); }
        self.heap_snapshots.push((ts_ms, bytes));
    }

    pub fn avg_frame_ms(&self) -> f32 {
        if self.frames.is_empty() { return 0.0; }
        let sum: f32 = self.frames.iter().map(|f| f.duration_ms).sum();
        sum / self.frames.len() as f32
    }

    pub fn jank_count(&self) -> usize {
        self.frames.iter().filter(|f| f.is_jank()).count()
    }

    pub fn top_functions(&self, n: usize) -> Vec<(&str, u64)> {
        let mut v: Vec<(&str, u64)> = self.hot_functions.iter()
            .map(|(k, &v)| (k.as_str(), v))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.truncate(n);
        v
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DEVTOOLS PANEL SELECTOR
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevPanel { Elements, Console, Network, Performance }

/// Top-level DevTools state (all four panels).
pub struct DevTools {
    pub active:      DevPanel,
    pub visible:     bool,
    pub elements:    InspectorTree,
    pub console:     ConsolePanel,
    pub network:     NetworkPanel,
    pub perf:        PerfPanel,
}

impl DevTools {
    pub fn new() -> Self {
        DevTools {
            active:   DevPanel::Elements,
            visible:  false,
            elements: InspectorTree::new(),
            console:  ConsolePanel::new(),
            network:  NetworkPanel::new(),
            perf:     PerfPanel::new(),
        }
    }

    pub fn toggle(&mut self) { self.visible = !self.visible; }

    pub fn switch_panel(&mut self, panel: DevPanel) {
        self.active = panel;
    }

    /// Called by the JS engine whenever console.log/warn/error is invoked.
    pub fn js_console(&mut self, level: ConsoleLevel, text: &str, source: Option<&str>) {
        self.console.push(level, text, source);
    }

    /// Called by the network stack when an HTTP request begins.
    pub fn net_start(&mut self, url: &str, method: &str, rtype: ResourceType) -> u32 {
        self.network.start_request(url, method, rtype)
    }

    /// Called when a request completes.
    pub fn net_complete(&mut self, id: u32, status: u16, size: usize, ms: u64) {
        self.network.complete_request(id, status, size, ms);
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
            else { fail += 1; crate::serial_println!("[FAIL] devtools: {}", $name); }
        }
    }

    // T1: DOM tree render
    {
        let mut tree = InspectorTree::new();
        let mut root = InspectorNode::new(0, "html");
        root.children = vec![1];
        tree.add_node(root);
        let mut body = InspectorNode::new(1, "body");
        body.id_attr = Some("main".to_string());
        body.children = vec![2];
        tree.add_node(body);
        let mut p = InspectorNode::new(2, "p");
        p.classes = vec!["text".to_string()];
        p.text = Some("Hello".to_string());
        tree.add_node(p);

        let lines = tree.render_text();
        check!(lines.iter().any(|l| l.contains("<html>")), "render has <html>");
        check!(lines.iter().any(|l| l.contains("id=\"main\"")), "render has id=main");
    }

    // T2: querySelector
    {
        let mut tree = InspectorTree::new();
        let mut div = InspectorNode::new(0, "div");
        div.id_attr = Some("wrap".to_string());
        div.classes = vec!["container".to_string()];
        tree.add_node(div);
        check!(tree.query_selector_all("#wrap").contains(&0), "querySelector #id");
        check!(tree.query_selector_all(".container").contains(&0), "querySelector .class");
        check!(tree.query_selector_all("div").contains(&0), "querySelector tag");
        check!(tree.query_selector_all("span").is_empty(), "querySelector no match");
    }

    // T3: Console panel — coalescing
    {
        let mut con = ConsolePanel::new();
        con.push(ConsoleLevel::Log, "hello", None);
        con.push(ConsoleLevel::Log, "hello", None);
        con.push(ConsoleLevel::Log, "hello", None);
        check!(con.entries.len() == 1, "console coalesces repeated messages");
        check!(con.entries[0].count == 3, "console count=3");
    }

    // T4: Console REPL input
    {
        let mut con = ConsolePanel::new();
        con.key_input('1'); con.key_input('+'); con.key_input('1');
        let cmd = con.submit();
        check!(cmd == "1+1", "console REPL input");
        check!(con.input_buf.is_empty(), "console input cleared after submit");
    }

    // T5: Network panel request lifecycle
    {
        let mut net = NetworkPanel::new();
        let id = net.start_request("https://example.com/", "GET", ResourceType::Document);
        check!(id == 1, "first request id=1");
        net.complete_request(id, 200, 4096, 120);
        let e = net.entries.iter().find(|e| e.id == id).unwrap();
        check!(e.status == 200, "request status=200");
        check!(e.size_bytes == 4096, "request size=4096");
        check!(e.duration_ms == 120, "request duration=120ms");
        check!(e.status_class() == "ok", "status class=ok");
        check!(net.total_transferred() == 4096, "total transferred=4096");
    }

    // T6: Network panel filter
    {
        let mut net = NetworkPanel::new();
        net.start_request("https://x.com/app.js", "GET", ResourceType::Script);
        net.start_request("https://x.com/style.css", "GET", ResourceType::Stylesheet);
        net.filter = Some(ResourceType::Script);
        let visible: Vec<&NetworkEntry> = net.visible_entries().collect();
        check!(visible.len() == 1, "network filter shows only scripts");
    }

    // T7: Perf panel frame recording
    {
        let mut perf = PerfPanel::new();
        for i in 0..10 {
            perf.record_frame(FrameRecord {
                frame_num: i, duration_ms: if i < 5 { 10.0 } else { 30.0 },
                layout_ms: 2.0, paint_ms: 3.0, script_ms: 5.0,
            });
        }
        check!(perf.frames.len() == 10, "perf: 10 frames recorded");
        check!(perf.jank_count() == 5, "perf: 5 jank frames (>16.7ms)");
        let avg = perf.avg_frame_ms();
        check!(avg == 20.0, "perf: avg_frame=20ms");
    }

    // T8: Perf hot functions
    {
        let mut perf = PerfPanel::new();
        for _ in 0..100 { perf.record_call("renderFrame"); }
        for _ in 0..50  { perf.record_call("layoutTree"); }
        let top = perf.top_functions(1);
        check!(top.len() == 1 && top[0].0 == "renderFrame", "top function is renderFrame");
    }

    // T9: DevTools toggle visibility
    {
        let mut dt = DevTools::new();
        check!(!dt.visible, "devtools starts hidden");
        dt.toggle();
        check!(dt.visible, "devtools visible after toggle");
        dt.switch_panel(DevPanel::Network);
        check!(dt.active == DevPanel::Network, "panel switched to Network");
    }

    // T10: JS console → DevTools integration
    {
        let mut dt = DevTools::new();
        dt.js_console(ConsoleLevel::Warn, "low memory", Some("app.js:42"));
        check!(dt.console.entries.len() == 1, "js_console adds entry");
        check!(dt.console.entries[0].level == ConsoleLevel::Warn, "log level=Warn");
        check!(dt.console.entries[0].source == Some("app.js:42".to_string()), "source line");
    }

    if fail == 0 {
        crate::serial_println!("[devtools] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[devtools] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

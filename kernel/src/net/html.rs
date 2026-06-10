//! HTML5 Tokenizer + DOM Builder — Phase 34 for Smart OS.
//!
//! Implements a subset of the WHATWG HTML5 parsing algorithm:
//!  • Tokenizer states: Data, TagOpen, TagName, BeforeAttributeName,
//!    AttributeName, AttributeValue, CharacterReference, etc.
//!  • Token types: DOCTYPE, StartTag, EndTag, Character, Comment, EOF
//!  • Tree construction: document → html → head/body → block/inline elements
//!  • DOM node types: Document, Element, Text, Comment
//!  • Attribute map per element
//!  • Basic HTML entities: &amp; &lt; &gt; &quot; &apos; &nbsp;
//!  • Script/style content extraction (raw text mode)
//!  • Self-closing void elements: area, base, br, col, embed, hr, img, input,
//!    link, meta, param, source, track, wbr
//!
//! The DOM tree is heap-allocated using Rust's alloc.  Each node carries
//! a NodeId (u32 index into the arena).

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::borrow::ToOwned;

// ─────────────────────────────────────────────────────────────────────────────
//  DOM Node types
// ─────────────────────────────────────────────────────────────────────────────

pub type NodeId = u32;
pub const NULL_NODE: NodeId = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Document,
    Doctype { name: String, public: String, system: String },
    Element { tag: String, attrs: BTreeMap<String, String> },
    Text    { data: String },
    Comment { data: String },
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind:     NodeKind,
    pub parent:   NodeId,
    pub children: Vec<NodeId>,
}

impl Node {
    fn new(kind: NodeKind) -> Self {
        Node { kind, parent: NULL_NODE, children: Vec::new() }
    }

    pub fn tag(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Element { tag, .. } => Some(tag),
            _ => None,
        }
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        match &self.kind {
            NodeKind::Element { attrs, .. } => attrs.get(name).map(|s| s.as_str()),
            _ => None,
        }
    }

    pub fn text_content<'a>(&'a self, arena: &'a Dom) -> String {
        let mut out = String::new();
        collect_text(self, arena, &mut out);
        out
    }
}

fn collect_text(node: &Node, arena: &Dom, out: &mut String) {
    match &node.kind {
        NodeKind::Text { data } => out.push_str(data),
        _ => {
            for &child_id in &node.children {
                if let Some(child) = arena.get(child_id) {
                    collect_text(child, arena, out);
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  DOM arena
// ─────────────────────────────────────────────────────────────────────────────

pub struct Dom {
    nodes: Vec<Node>,
}

impl Dom {
    pub fn new() -> Self {
        let mut d = Dom { nodes: Vec::new() };
        // Node 0 = document root.
        d.nodes.push(Node::new(NodeKind::Document));
        d
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id as usize)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id as usize)
    }

    pub fn root(&self) -> NodeId { 0 }

    pub fn create(&mut self, kind: NodeKind) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.nodes.push(Node::new(kind));
        id
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        if let Some(c) = self.get_mut(child) { c.parent = parent; }
        if let Some(p) = self.get_mut(parent) { p.children.push(child); }
    }

    pub fn len(&self) -> usize { self.nodes.len() }

    /// Find the first element with the given tag name (BFS).
    pub fn find_element(&self, tag: &str) -> Option<NodeId> {
        let mut queue = vec![self.root()];
        while let Some(id) = queue.first().cloned() {
            queue.remove(0);
            if let Some(node) = self.get(id) {
                if node.tag().map(|t| t == tag).unwrap_or(false) { return Some(id); }
                queue.extend(node.children.iter().copied());
            }
        }
        None
    }

    /// Find all elements with the given tag name.
    pub fn find_all(&self, tag: &str) -> Vec<NodeId> {
        let mut results = Vec::new();
        self.walk(self.root(), &mut |id, node| {
            if node.tag().map(|t| t == tag).unwrap_or(false) {
                results.push(id);
            }
        });
        results
    }

    /// Find first element matching a CSS selector (very basic: tag, .class, #id).
    pub fn query_selector(&self, sel: &str) -> Option<NodeId> {
        self.query_selector_all(sel).into_iter().next()
    }

    pub fn query_selector_all(&self, sel: &str) -> Vec<NodeId> {
        let mut results = Vec::new();
        let sel = sel.trim();
        self.walk(self.root(), &mut |id, node| {
            if matches_selector(node, sel) { results.push(id); }
        });
        results
    }

    fn walk(&self, id: NodeId, visitor: &mut impl FnMut(NodeId, &Node)) {
        if let Some(node) = self.get(id) {
            visitor(id, node);
            let children: Vec<NodeId> = node.children.clone();
            for child in children { self.walk(child, visitor); }
        }
    }
}

fn matches_selector(node: &Node, sel: &str) -> bool {
    let NodeKind::Element { tag, attrs } = &node.kind else { return false; };
    if sel.starts_with('#') {
        // ID selector
        attrs.get("id").map(|id| id == &sel[1..]).unwrap_or(false)
    } else if sel.starts_with('.') {
        // Class selector
        attrs.get("class")
            .map(|cls| cls.split_whitespace().any(|c| c == &sel[1..]))
            .unwrap_or(false)
    } else if sel.contains('.') {
        // tag.class
        let (t, c) = sel.split_once('.').unwrap_or((sel, ""));
        tag == t && attrs.get("class")
            .map(|cls| cls.split_whitespace().any(|cl| cl == c))
            .unwrap_or(false)
    } else {
        tag.as_str() == sel
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Void elements (no closing tag)
// ─────────────────────────────────────────────────────────────────────────────

const VOID_ELEMENTS: &[&str] = &[
    "area","base","br","col","embed","hr","img","input",
    "link","meta","param","source","track","wbr",
];

fn is_void(tag: &str) -> bool {
    VOID_ELEMENTS.iter().any(|&v| v == tag)
}

// ─────────────────────────────────────────────────────────────────────────────
//  HTML entities
// ─────────────────────────────────────────────────────────────────────────────

fn decode_entity(name: &str) -> Option<char> {
    match name {
        "amp"   => Some('&'),
        "lt"    => Some('<'),
        "gt"    => Some('>'),
        "quot"  => Some('"'),
        "apos"  => Some('\''),
        "nbsp"  => Some('\u{00A0}'),
        "copy"  => Some('©'),
        "reg"   => Some('®'),
        "trade" => Some('™'),
        "mdash" => Some('—'),
        "ndash" => Some('–'),
        "ldquo" => Some('"'),
        "rdquo" => Some('"'),
        "lsquo" => Some('\u{2018}'),
        "rsquo" => Some('\u{2019}'),
        "hellip"=> Some('…'),
        "bull"  => Some('•'),
        "rarr"  => Some('→'),
        "larr"  => Some('←'),
        "uarr"  => Some('↑'),
        "darr"  => Some('↓'),
        "deg"   => Some('°'),
        "plusmn"=> Some('±'),
        "times" => Some('×'),
        "divide"=> Some('÷'),
        "frac12"=> Some('½'),
        "frac14"=> Some('¼'),
        "frac34"=> Some('¾'),
        _       => None,
    }
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '&' { out.push(c); continue; }
        // Collect entity name up to ';'.
        let mut ent = String::new();
        let mut found_semi = false;
        for nc in chars.by_ref() {
            if nc == ';' { found_semi = true; break; }
            if !nc.is_ascii_alphanumeric() && nc != '#' { ent.push(nc); break; }
            ent.push(nc);
        }
        if found_semi {
            if ent.starts_with('#') {
                // Numeric character reference.
                let n = if ent.starts_with("#x") || ent.starts_with("#X") {
                    u32::from_str_radix(&ent[2..], 16).ok()
                } else {
                    ent[1..].parse::<u32>().ok()
                };
                if let Some(cp) = n.and_then(char::from_u32) {
                    out.push(cp);
                    continue;
                }
            } else if let Some(ch) = decode_entity(&ent) {
                out.push(ch);
                continue;
            }
        }
        out.push('&');
        out.push_str(&ent);
        if found_semi { out.push(';'); }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
//  Tokenizer
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Token {
    Doctype { name: String, public: String, system: String },
    StartTag { tag: String, attrs: BTreeMap<String, String>, self_closing: bool },
    EndTag   { tag: String },
    Char     { data: String },
    Comment  { data: String },
    Eof,
}

struct Tokenizer<'a> {
    src:    &'a str,
    pos:    usize,
    tokens: Vec<Token>,
}

impl<'a> Tokenizer<'a> {
    fn new(src: &'a str) -> Self {
        Tokenizer { src, pos: 0, tokens: Vec::new() }
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn starts_with_ci(&self, s: &str) -> bool {
        self.src[self.pos..].to_ascii_lowercase().starts_with(&s.to_ascii_lowercase())
    }

    fn skip(&mut self, n: usize) {
        let mut rem = n;
        while rem > 0 && self.pos < self.src.len() {
            let c = self.src[self.pos..].chars().next().unwrap_or('\0');
            self.pos += c.len_utf8();
            rem -= 1;
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

    /// Tokenize the full input and return all tokens.
    fn tokenize(&mut self) -> Vec<Token> {
        // Raw text tags: script, style — content is not parsed for tags.
        let raw_text_tags = ["script", "style", "textarea"];
        let mut raw_mode: Option<String> = None;

        while self.pos < self.src.len() {
            // Raw text mode: collect until </tag>.
            if let Some(ref rtag) = raw_mode.clone() {
                let end = format!("</{}", rtag);
                if let Some(p) = self.src[self.pos..].find(&end) {
                    let content = self.src[self.pos..self.pos+p].to_string();
                    self.pos += p;
                    self.tokens.push(Token::Char { data: content });
                } else {
                    // No closing tag found — consume rest.
                    let rest = self.src[self.pos..].to_string();
                    self.tokens.push(Token::Char { data: rest });
                    self.pos = self.src.len();
                }
                raw_mode = None;
                continue;
            }

            if self.peek() != Some('<') {
                // Text node.
                let text = self.consume_while(|c| c != '<');
                if !text.is_empty() {
                    self.tokens.push(Token::Char { data: decode_entities(&text) });
                }
                continue;
            }

            // '<' character.
            self.advance(); // consume '<'

            if self.starts_with_ci("!--") {
                // Comment.
                self.skip(3);
                let mut comment = String::new();
                while self.pos < self.src.len() {
                    if self.starts_with_ci("-->") { self.skip(3); break; }
                    comment.push(self.advance().unwrap_or('\0'));
                }
                self.tokens.push(Token::Comment { data: comment });

            } else if self.starts_with_ci("!DOCTYPE") {
                // DOCTYPE declaration.
                self.skip(8);
                self.consume_while(|c| c.is_whitespace());
                let name = self.consume_while(|c| c != ' ' && c != '>' && c != '\n' && c != '\t');
                let rest = self.consume_while(|c| c != '>');
                self.advance(); // '>'
                let lower = rest.to_ascii_lowercase();
                let public = if lower.contains("public") { "public".to_string() } else { String::new() };
                let system = if lower.contains("system") { "system".to_string() } else { String::new() };
                self.tokens.push(Token::Doctype { name, public, system });

            } else if self.peek() == Some('/') {
                // End tag.
                self.advance(); // '/'
                let tag = self.consume_while(|c| c.is_alphanumeric() || c == '-' || c == '_').to_ascii_lowercase();
                self.consume_while(|c| c != '>');
                self.advance(); // '>'
                self.tokens.push(Token::EndTag { tag });

            } else if self.peek().map(|c| c.is_ascii_alphabetic()).unwrap_or(false) {
                // Start tag.
                let tag = self.consume_while(|c| c.is_alphanumeric() || c == '-' || c == '_').to_ascii_lowercase();
                let mut attrs: BTreeMap<String, String> = BTreeMap::new();

                let mut is_self_closing = false;
                loop {
                    self.consume_while(|c| c == ' ' || c == '\t' || c == '\n' || c == '\r');
                    let p = self.peek();
                    if p == Some('>') { self.advance(); break; }
                    if p == Some('/') {
                        self.advance();
                        if self.peek() == Some('>') { self.advance(); }
                        is_self_closing = true;
                        break;
                    }
                    if p.is_none() { break; }

                    // Attribute name.
                    let aname = self.consume_while(|c| c != '=' && c != '>' && c != ' ' && c != '\t' && c != '\n').to_ascii_lowercase();
                    if aname.is_empty() { self.advance(); continue; }

                    self.consume_while(|c| c == ' ' || c == '\t');
                    let aval = if self.peek() == Some('=') {
                        self.advance(); // '='
                        self.consume_while(|c| c == ' ' || c == '\t');
                        if self.peek() == Some('"') {
                            self.advance();
                            let v = self.consume_while(|c| c != '"');
                            self.advance(); // closing "
                            decode_entities(&v)
                        } else if self.peek() == Some('\'') {
                            self.advance();
                            let v = self.consume_while(|c| c != '\'');
                            self.advance(); // closing '
                            decode_entities(&v)
                        } else {
                            let v = self.consume_while(|c| !c.is_whitespace() && c != '>');
                            decode_entities(&v)
                        }
                    } else {
                        aname.clone() // boolean attribute
                    };
                    attrs.insert(aname, aval);
                }
                // Check if this opens a raw text context.
                if !is_self_closing && raw_text_tags.contains(&tag.as_str()) {
                    raw_mode = Some(tag.clone());
                }
                self.tokens.push(Token::StartTag { tag, attrs, self_closing: is_self_closing });

            } else {
                // Malformed — treat '<' as text.
                self.tokens.push(Token::Char { data: "<".to_string() });
            }
        }

        self.tokens.push(Token::Eof);
        core::mem::take(&mut self.tokens)
    }
}


// ─────────────────────────────────────────────────────────────────────────────
//  Tree builder
// ─────────────────────────────────────────────────────────────────────────────

/// Build a DOM tree from HTML source.
pub fn parse(html: &str) -> Dom {
    let mut tok = Tokenizer::new(html);
    let tokens = tok.tokenize();

    let mut dom  = Dom::new();
    let root     = dom.root();
    let mut open_stack: Vec<NodeId> = vec![root]; // current open elements

    for token in tokens {
        let current = *open_stack.last().unwrap_or(&root);
        match token {
            Token::Doctype { name, public, system } => {
                let id = dom.create(NodeKind::Doctype { name, public, system });
                dom.append_child(root, id);
            }

            Token::StartTag { tag, attrs, self_closing } => {
                let id = dom.create(NodeKind::Element { tag: tag.clone(), attrs });
                dom.append_child(current, id);
                if !self_closing && !is_void(&tag) {
                    open_stack.push(id);
                }
            }

            Token::EndTag { tag } => {
                // Pop the stack until we find a matching open tag.
                let mut found = false;
                for i in (0..open_stack.len()).rev() {
                    if let Some(node) = dom.get(open_stack[i]) {
                        if node.tag().map(|t| t == tag).unwrap_or(false) {
                            open_stack.truncate(i);
                            found = true;
                            break;
                        }
                    }
                }
                // If not found, ignore the end tag (matches HTML5 error recovery).
            }

            Token::Char { data } => {
                if data.trim().is_empty() && dom.get(current).and_then(|n| n.tag()).is_none() {
                    // Skip whitespace-only text at document level.
                } else {
                    // Coalesce adjacent text nodes.
                    let last_child = dom.get(current).and_then(|n| n.children.last().copied());
                    let mut coalesced = false;
                    if let Some(lc) = last_child {
                        if let Some(n) = dom.get_mut(lc) {
                            if let NodeKind::Text { data: t } = &mut n.kind {
                                t.push_str(&data);
                                coalesced = true;
                            }
                        }
                    }
                    if !coalesced {
                        let id = dom.create(NodeKind::Text { data });
                        dom.append_child(current, id);
                    }
                }
            }

            Token::Comment { data } => {
                let id = dom.create(NodeKind::Comment { data });
                dom.append_child(current, id);
            }

            Token::Eof => break,
        }
    }
    dom
}

// ─────────────────────────────────────────────────────────────────────────────
//  DOM serializer (debugging / innerHTML)
// ─────────────────────────────────────────────────────────────────────────────

pub fn serialize(dom: &Dom, id: NodeId) -> String {
    let mut out = String::new();
    serialize_node(dom, id, &mut out);
    out
}

fn serialize_node(dom: &Dom, id: NodeId, out: &mut String) {
    let node = match dom.get(id) { Some(n) => n, None => return };
    match &node.kind {
        NodeKind::Document => {
            let children: Vec<NodeId> = node.children.clone();
            for c in children { serialize_node(dom, c, out); }
        }
        NodeKind::Doctype { name, .. } => {
            out.push_str("<!DOCTYPE ");
            out.push_str(name);
            out.push('>');
        }
        NodeKind::Element { tag, attrs } => {
            out.push('<');
            out.push_str(tag);
            for (k, v) in attrs {
                out.push(' ');
                out.push_str(k);
                out.push_str("=\"");
                out.push_str(v);
                out.push('"');
            }
            if is_void(tag) {
                out.push('>');
                return;
            }
            out.push('>');
            let children: Vec<NodeId> = node.children.clone();
            for c in children { serialize_node(dom, c, out); }
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }
        NodeKind::Text { data } => {
            out.push_str(data);
        }
        NodeKind::Comment { data } => {
            out.push_str("<!--");
            out.push_str(data);
            out.push_str("-->");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Convenience helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Extract the `<title>` text from a parsed DOM.
pub fn extract_title(dom: &Dom) -> String {
    if let Some(id) = dom.find_element("title") {
        if let Some(node) = dom.get(id) {
            return node.text_content(dom);
        }
    }
    String::new()
}

/// Extract all hyperlinks from a parsed DOM.
pub fn extract_links(dom: &Dom, base_url: &str) -> Vec<String> {
    let mut links = Vec::new();
    for id in dom.find_all("a") {
        if let Some(node) = dom.get(id) {
            if let Some(href) = node.attr("href") {
                if href.is_empty() { continue; }
                let url = resolve_url(base_url, href);
                links.push(url);
            }
        }
    }
    links
}

/// Resolve a potentially relative URL against a base URL.
pub fn resolve_url(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("//") {
        return href.to_string();
    }
    // Find base origin.
    let origin = if let Some(pos) = base.find("://") {
        let after = &base[pos+3..];
        if let Some(slash) = after.find('/') {
            &base[..pos+3+slash]
        } else {
            base
        }
    } else { base };

    if href.starts_with('/') {
        format!("{}{}", origin, href)
    } else {
        // Relative to current path.
        let path_base = if let Some(last_slash) = base.rfind('/') {
            &base[..last_slash+1]
        } else {
            base
        };
        format!("{}{}", path_base, href)
    }
}

/// Extract all `<img src="...">` URLs.
pub fn extract_images(dom: &Dom, base_url: &str) -> Vec<String> {
    let mut imgs = Vec::new();
    for id in dom.find_all("img") {
        if let Some(node) = dom.get(id) {
            if let Some(src) = node.attr("src") {
                imgs.push(resolve_url(base_url, src));
            }
        }
    }
    imgs
}

/// Extract all script elements from the DOM.
///
/// Returns `(classic_inline, module_inline, src_urls)`:
/// - `classic_inline` — inline `<script>` bodies (no `type` or `type="text/javascript"`)
/// - `module_inline`  — inline `<script type="module">` bodies
/// - `src_urls`       — external `<script src="...">` URLs (any type)
pub fn extract_scripts(dom: &Dom, base_url: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut src_urls      = Vec::new();
    let mut classic_inline = Vec::new();
    let mut module_inline  = Vec::new();
    for id in dom.find_all("script") {
        if let Some(node) = dom.get(id) {
            let is_module = node.attr("type")
                .map(|t| t.eq_ignore_ascii_case("module"))
                .unwrap_or(false);
            if let Some(src) = node.attr("src") {
                src_urls.push(resolve_url(base_url, src));
            } else {
                let text = node.text_content(dom);
                if is_module {
                    module_inline.push(text);
                } else {
                    classic_inline.push(text);
                }
            }
        }
    }
    (classic_inline, module_inline, src_urls)
}

/// Extract all `<link rel="stylesheet" href="...">` URLs.
pub fn extract_stylesheets(dom: &Dom, base_url: &str) -> Vec<String> {
    let mut sheets = Vec::new();
    for id in dom.find_all("link") {
        if let Some(node) = dom.get(id) {
            let rel  = node.attr("rel").unwrap_or("");
            let href = node.attr("href").unwrap_or("");
            if rel.to_ascii_lowercase() == "stylesheet" && !href.is_empty() {
                sheets.push(resolve_url(base_url, href));
            }
        }
    }
    sheets
}

// ─────────────────────────────────────────────────────────────────────────────
//  Module init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[html] HTML5 tokenizer + DOM ready ({} void elements, entity decoder).",
        VOID_ELEMENTS.len());
}

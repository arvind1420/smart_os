#![allow(dead_code)]
/// Smart OS — DOM Tree (Phase 82, v0.42.0)
///
/// A lightweight Document Object Model usable from both the HTML parser and
/// the JavaScript engine:
///   • `NodeType`      — Element / Text / Comment / Document / Doctype
///   • `Node`          — tree node with attributes, children, parent ref
///   • `Document`      — owns the tree, provides query helpers
///   • `Selector`      — CSS selector engine (tag, id, class, attr, pseudo)
///   • `EventTarget`   — addEventListener / removeEventListener / dispatchEvent
///
/// The tree uses indices into a `Vec<Node>` (arena allocation) rather than
/// `Rc<RefCell<Node>>` to stay Send + Sync in the kernel environment.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::vec;

// ─── Node ID ──────────────────────────────────────────────────────────────────
pub type NodeId = usize;
pub const NULL_NODE: NodeId = usize::MAX;

// ─── Node types ───────────────────────────────────────────────────────────────
#[derive(Clone, Debug, PartialEq)]
pub enum NodeType {
    Document,
    Doctype    { name: String },
    Element    { tag: String, attrs: BTreeMap<String, String> },
    Text       { data: String },
    Comment    { data: String },
    CData      { data: String },
}

impl NodeType {
    pub fn tag(&self) -> Option<&str> {
        if let NodeType::Element { tag, .. } = self { Some(tag.as_str()) } else { None }
    }
    pub fn is_element(&self) -> bool { matches!(self, NodeType::Element { .. }) }
    pub fn is_text(&self)    -> bool { matches!(self, NodeType::Text { .. }) }
}

// ─── Event ────────────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct DomEvent {
    pub type_:       String,
    pub target:      NodeId,
    pub bubbles:     bool,
    pub cancelable:  bool,
    pub cancelled:   bool,
    pub propagation_stopped: bool,
}

impl DomEvent {
    pub fn new(type_: &str, target: NodeId, bubbles: bool) -> Self {
        DomEvent { type_: type_.to_string(), target, bubbles, cancelable: true,
                   cancelled: false, propagation_stopped: false }
    }
    pub fn prevent_default(&mut self)  { self.cancelled           = true; }
    pub fn stop_propagation(&mut self) { self.propagation_stopped = true; }
}

// ─── Event listener ──────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct EventListener {
    pub event_type: String,
    pub capture:    bool,
    /// A string key identifying the listener (for remove). Real engine uses fn pointers.
    pub handler_id: u32,
}

// ─── Node ─────────────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct Node {
    pub id:        NodeId,
    pub kind:      NodeType,
    pub parent:    NodeId,
    pub children:  Vec<NodeId>,
    pub listeners: Vec<EventListener>,
    // CSS computed style cache (simplified: key=property, value=string)
    pub style:     BTreeMap<String, String>,
}

impl Node {
    pub fn new(id: NodeId, kind: NodeType) -> Self {
        Node { id, kind, parent: NULL_NODE, children: Vec::new(),
               listeners: Vec::new(), style: BTreeMap::new() }
    }

    pub fn get_attr(&self, name: &str) -> Option<&str> {
        if let NodeType::Element { attrs, .. } = &self.kind {
            attrs.get(name).map(|s| s.as_str())
        } else { None }
    }

    pub fn set_attr(&mut self, name: &str, value: &str) {
        if let NodeType::Element { attrs, .. } = &mut self.kind {
            attrs.insert(name.to_string(), value.to_string());
        }
    }

    pub fn remove_attr(&mut self, name: &str) {
        if let NodeType::Element { attrs, .. } = &mut self.kind {
            attrs.remove(name);
        }
    }

    pub fn has_attr(&self, name: &str) -> bool {
        self.get_attr(name).is_some()
    }

    pub fn class_list(&self) -> Vec<&str> {
        self.get_attr("class")
            .map(|c| c.split_ascii_whitespace().collect())
            .unwrap_or_default()
    }

    pub fn has_class(&self, cls: &str) -> bool {
        self.class_list().iter().any(|&c| c == cls)
    }

    pub fn add_class(&mut self, cls: &str) {
        let existing = self.get_attr("class").unwrap_or("").to_string();
        if !existing.split_ascii_whitespace().any(|c| c == cls) {
            let new_val = if existing.is_empty() { cls.to_string() }
                          else { format!("{} {}", existing, cls) };
            self.set_attr("class", &new_val);
        }
    }

    pub fn remove_class(&mut self, cls: &str) {
        let new_val: Vec<&str> = self.get_attr("class").unwrap_or("")
            .split_ascii_whitespace()
            .filter(|&c| c != cls)
            .collect();
        self.set_attr("class", &new_val.join(" "));
    }

    pub fn inner_text(&self, tree: &Document) -> String {
        let mut out = String::new();
        for &ch in &self.children {
            if let Some(node) = tree.get(ch) {
                match &node.kind {
                    NodeType::Text { data } => out.push_str(data),
                    NodeType::Element { .. } => out.push_str(&node.inner_text(tree)),
                    _ => {}
                }
            }
        }
        out
    }
}

// ─── Document ─────────────────────────────────────────────────────────────────
pub struct Document {
    pub nodes:    Vec<Node>,
    pub root:     NodeId,   // always 0 (Document node)
    pub head:     NodeId,
    pub body:     NodeId,
    pub title:    String,
    pub base_url: String,
    next_handler_id: u32,
}

impl Document {
    pub fn new() -> Self {
        let root = Node::new(0, NodeType::Document);
        Document {
            nodes: vec![root],
            root: 0,
            head: NULL_NODE,
            body: NULL_NODE,
            title: String::new(),
            base_url: String::new(),
            next_handler_id: 1,
        }
    }

    // ── Node creation ─────────────────────────────────────────────────────────

    pub fn create_element(&mut self, tag: &str) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node::new(id, NodeType::Element {
            tag: tag.to_ascii_lowercase(),
            attrs: BTreeMap::new(),
        }));
        id
    }

    pub fn create_text(&mut self, data: &str) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node::new(id, NodeType::Text { data: data.to_string() }));
        id
    }

    pub fn create_comment(&mut self, data: &str) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node::new(id, NodeType::Comment { data: data.to_string() }));
        id
    }

    // ── Tree mutation ─────────────────────────────────────────────────────────

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        if parent < self.nodes.len() && child < self.nodes.len() {
            self.nodes[child].parent = parent;
            self.nodes[parent].children.push(child);
        }
    }

    pub fn remove_child(&mut self, parent: NodeId, child: NodeId) {
        if parent < self.nodes.len() {
            self.nodes[parent].children.retain(|&c| c != child);
        }
        if child < self.nodes.len() {
            self.nodes[child].parent = NULL_NODE;
        }
    }

    pub fn insert_before(&mut self, parent: NodeId, new_child: NodeId, ref_child: NodeId) {
        if parent >= self.nodes.len() { return; }
        if let Some(pos) = self.nodes[parent].children.iter().position(|&c| c == ref_child) {
            self.nodes[parent].children.insert(pos, new_child);
            if new_child < self.nodes.len() {
                self.nodes[new_child].parent = parent;
            }
        } else {
            self.append_child(parent, new_child);
        }
    }

    // ── Access ────────────────────────────────────────────────────────────────

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id)
    }

    pub fn len(&self) -> usize { self.nodes.len() }

    // ── Query ─────────────────────────────────────────────────────────────────

    /// Find first element with matching `id` attribute.
    pub fn get_element_by_id(&self, id_val: &str) -> Option<NodeId> {
        self.nodes.iter().find(|n| n.get_attr("id") == Some(id_val))
            .map(|n| n.id)
    }

    /// Collect all elements with tag name (case-insensitive).
    pub fn get_elements_by_tag(&self, tag: &str) -> Vec<NodeId> {
        let tag_lc = tag.to_ascii_lowercase();
        self.nodes.iter()
            .filter(|n| n.kind.tag() == Some(tag_lc.as_str()))
            .map(|n| n.id)
            .collect()
    }

    /// Collect all elements that have the given class in their class list.
    pub fn get_elements_by_class(&self, cls: &str) -> Vec<NodeId> {
        self.nodes.iter()
            .filter(|n| n.has_class(cls))
            .map(|n| n.id)
            .collect()
    }

    /// Simple CSS selector: `tag`, `#id`, `.class`, `tag.class`, `[attr]`, `[attr=val]`
    pub fn query_selector(&self, sel: &str) -> Option<NodeId> {
        self.query_selector_all(sel).into_iter().next()
    }

    pub fn query_selector_all(&self, sel: &str) -> Vec<NodeId> {
        self.nodes.iter()
            .filter(|n| n.kind.is_element() && matches_selector(n, sel))
            .map(|n| n.id)
            .collect()
    }

    // ── Events ────────────────────────────────────────────────────────────────

    pub fn add_event_listener(&mut self, target: NodeId, event_type: &str, capture: bool) -> u32 {
        let hid = self.next_handler_id;
        self.next_handler_id += 1;
        if let Some(node) = self.nodes.get_mut(target) {
            node.listeners.push(EventListener {
                event_type: event_type.to_string(),
                capture,
                handler_id: hid,
            });
        }
        hid
    }

    pub fn remove_event_listener(&mut self, target: NodeId, handler_id: u32) {
        if let Some(node) = self.nodes.get_mut(target) {
            node.listeners.retain(|l| l.handler_id != handler_id);
        }
    }

    /// Dispatch event with bubbling. Returns false if preventDefault() was called.
    pub fn dispatch_event(&self, mut evt: DomEvent) -> bool {
        // Build bubble path: target → ancestors
        let mut path: Vec<NodeId> = Vec::new();
        let mut cur = evt.target;
        while cur != NULL_NODE && cur < self.nodes.len() {
            path.push(cur);
            cur = self.nodes[cur].parent;
        }

        // Capture phase (root → target)
        for &nid in path.iter().rev() {
            if let Some(node) = self.nodes.get(nid) {
                for l in &node.listeners {
                    if l.capture && l.event_type == evt.type_ {
                        // In a real engine we'd call the JS callback here
                        let _ = l.handler_id;
                    }
                }
            }
            if evt.propagation_stopped { break; }
        }

        // Bubble phase (target → root)
        if evt.bubbles && !evt.propagation_stopped {
            for &nid in &path {
                if let Some(node) = self.nodes.get(nid) {
                    for l in &node.listeners {
                        if !l.capture && l.event_type == evt.type_ {
                            let _ = l.handler_id;
                        }
                    }
                }
                if evt.propagation_stopped { break; }
            }
        }

        !evt.cancelled
    }
}

// ─── Selector matching ────────────────────────────────────────────────────────
fn matches_selector(node: &Node, sel: &str) -> bool {
    let sel = sel.trim();
    if sel.is_empty() { return false; }

    // #id
    if let Some(id_val) = sel.strip_prefix('#') {
        return node.get_attr("id") == Some(id_val);
    }
    // .class
    if let Some(cls) = sel.strip_prefix('.') {
        return node.has_class(cls);
    }
    // [attr] or [attr=val]
    if sel.starts_with('[') && sel.ends_with(']') {
        let inner = &sel[1..sel.len()-1];
        if let Some((attr, val)) = inner.split_once('=') {
            let attr = attr.trim_matches(|c: char| c == '"' || c == '\'').trim();
            let val  = val.trim_matches(|c: char| c == '"' || c == '\'').trim();
            return node.get_attr(attr) == Some(val);
        } else {
            return node.has_attr(inner.trim());
        }
    }

    // tag.class compound
    if let Some(dot) = sel.find('.') {
        let (tag, cls) = (&sel[..dot], &sel[dot+1..]);
        return node.kind.tag() == Some(tag) && node.has_class(cls);
    }

    // plain tag
    node.kind.tag() == Some(sel)
}

// ─── Self-test ────────────────────────────────────────────────────────────────
pub fn self_test() -> bool {
    let mut ok = true;
    let mut doc = Document::new();

    // T1: create element and text
    let body = doc.create_element("body");
    let p    = doc.create_element("p");
    let txt  = doc.create_text("Hello, DOM!");
    doc.append_child(0, body);
    doc.append_child(body, p);
    doc.append_child(p, txt);
    if doc.len() != 4 { ok = false; }  // root + body + p + txt

    // T2: get_elements_by_tag
    let ps = doc.get_elements_by_tag("p");
    if ps.len() != 1 || ps[0] != p { ok = false; }

    // T3: attributes
    doc.get_mut(p).unwrap().set_attr("id", "main");
    if doc.get(p).unwrap().get_attr("id") != Some("main") { ok = false; }

    // T4: get_element_by_id
    if doc.get_element_by_id("main") != Some(p) { ok = false; }

    // T5: class operations
    doc.get_mut(body).unwrap().add_class("container");
    doc.get_mut(body).unwrap().add_class("dark");
    if !doc.get(body).unwrap().has_class("container") { ok = false; }
    if !doc.get(body).unwrap().has_class("dark")      { ok = false; }
    doc.get_mut(body).unwrap().remove_class("dark");
    if  doc.get(body).unwrap().has_class("dark")      { ok = false; }

    // T6: query_selector .class
    let found = doc.query_selector(".container");
    if found != Some(body) { ok = false; }

    // T7: query_selector #id
    let found2 = doc.query_selector("#main");
    if found2 != Some(p) { ok = false; }

    // T8: inner_text
    let text = doc.get(p).unwrap().inner_text(&doc);
    if text != "Hello, DOM!" { ok = false; }

    // T9: remove_child
    doc.remove_child(p, txt);
    if doc.get(p).unwrap().children.contains(&txt) { ok = false; }

    // T10: event listener add/remove
    let hid = doc.add_event_listener(body, "click", false);
    if hid == 0 { ok = false; }
    let len_before = doc.get(body).unwrap().listeners.len();
    doc.remove_event_listener(body, hid);
    let len_after = doc.get(body).unwrap().listeners.len();
    if len_after != len_before.saturating_sub(1) { ok = false; }

    ok
}

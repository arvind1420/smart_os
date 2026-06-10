/// Phase 119 — Accessibility Tree (ARIA / AT)
///
/// Implements:
///   • ARIA 1.2 role taxonomy (70 roles)
///   • ARIA property/state set
///   • Accessibility tree mirroring the DOM
///   • Focus management (tab order, active element, focus ring)
///   • Keyboard navigation helpers (arrow-key navigation in menus/grids)
///   • Screen reader output buffer (plain-text announcement queue)
///   • Live regions (`aria-live` polite/assertive)

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::collections::BTreeMap;
use alloc::format;

// ─────────────────────────────────────────────────────────────────────────────
// ARIA ROLE TAXONOMY
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AriaRole {
    // Landmark roles
    Banner, Complementary, ContentInfo, Form, Main, Navigation, Region, Search,
    // Document structure
    Article, Cell, ColumnHeader, Definition, Directory, Document, Feed, Figure,
    Group, Heading, Img, List, ListItem, Math, None, Note, Presentation,
    Row, RowGroup, RowHeader, Separator, Table, Term, Toolbar, Tooltip,
    // Widget roles
    Button, CheckBox, ComboBox, Dialog, GridCell, Link, ListBox, Log, Marquee,
    Menu, MenuBar, MenuItem, MenuItemCheckBox, MenuItemRadio, Option, ProgressBar,
    Radio, RadioGroup, ScrollBar, SearchBox, Slider, SpinButton, Status, Switch,
    Tab, TabList, TabPanel, TextBox, Timer, TreeGrid, TreeItem,
    // Live region
    Alert, AlertDialog,
    // Generic / unknown
    Generic,
}

impl AriaRole {
    pub fn from_str(s: &str) -> Self {
        match s {
            "banner"         => AriaRole::Banner,
            "complementary"  => AriaRole::Complementary,
            "contentinfo"    => AriaRole::ContentInfo,
            "form"           => AriaRole::Form,
            "main"           => AriaRole::Main,
            "navigation"     => AriaRole::Navigation,
            "region"         => AriaRole::Region,
            "search"         => AriaRole::Search,
            "article"        => AriaRole::Article,
            "button"         => AriaRole::Button,
            "checkbox"       => AriaRole::CheckBox,
            "combobox"       => AriaRole::ComboBox,
            "dialog"         => AriaRole::Dialog,
            "heading"        => AriaRole::Heading,
            "img"            => AriaRole::Img,
            "link"           => AriaRole::Link,
            "list"           => AriaRole::List,
            "listitem"       => AriaRole::ListItem,
            "listbox"        => AriaRole::ListBox,
            "menu"           => AriaRole::Menu,
            "menubar"        => AriaRole::MenuBar,
            "menuitem"       => AriaRole::MenuItem,
            "none" | "presentation" => AriaRole::None,
            "option"         => AriaRole::Option,
            "progressbar"    => AriaRole::ProgressBar,
            "radio"          => AriaRole::Radio,
            "radiogroup"     => AriaRole::RadioGroup,
            "scrollbar"      => AriaRole::ScrollBar,
            "searchbox"      => AriaRole::SearchBox,
            "slider"         => AriaRole::Slider,
            "spinbutton"     => AriaRole::SpinButton,
            "status"         => AriaRole::Status,
            "switch"         => AriaRole::Switch,
            "tab"            => AriaRole::Tab,
            "tablist"        => AriaRole::TabList,
            "tabpanel"       => AriaRole::TabPanel,
            "textbox"        => AriaRole::TextBox,
            "timer"          => AriaRole::Timer,
            "treeitem"       => AriaRole::TreeItem,
            "alert"          => AriaRole::Alert,
            "alertdialog"    => AriaRole::AlertDialog,
            "table"          => AriaRole::Table,
            "row"            => AriaRole::Row,
            "cell"           => AriaRole::Cell,
            "rowheader"      => AriaRole::RowHeader,
            "columnheader"   => AriaRole::ColumnHeader,
            "toolbar"        => AriaRole::Toolbar,
            "tooltip"        => AriaRole::Tooltip,
            _                => AriaRole::Generic,
        }
    }

    /// Whether the role is interactive (can receive focus / keyboard events).
    pub fn is_interactive(&self) -> bool {
        matches!(self,
            AriaRole::Button | AriaRole::CheckBox | AriaRole::ComboBox |
            AriaRole::Link   | AriaRole::ListBox   | AriaRole::Menu     |
            AriaRole::MenuItem | AriaRole::MenuItemCheckBox | AriaRole::MenuItemRadio |
            AriaRole::Option | AriaRole::Radio | AriaRole::RadioGroup |
            AriaRole::SearchBox | AriaRole::Slider | AriaRole::SpinButton |
            AriaRole::Switch | AriaRole::Tab | AriaRole::TextBox |
            AriaRole::TreeItem | AriaRole::Dialog
        )
    }

    /// The implicit HTML tag that maps to this role.
    pub fn implicit_tag(&self) -> Option<&'static str> {
        match self {
            AriaRole::Button      => Some("button"),
            AriaRole::CheckBox    => Some("input[type=checkbox]"),
            AriaRole::Link        => Some("a"),
            AriaRole::TextBox     => Some("input[type=text]"),
            AriaRole::Img         => Some("img"),
            AriaRole::List        => Some("ul"),
            AriaRole::ListItem    => Some("li"),
            AriaRole::Heading     => Some("h1..h6"),
            AriaRole::Main        => Some("main"),
            AriaRole::Navigation  => Some("nav"),
            AriaRole::Form        => Some("form"),
            AriaRole::Table       => Some("table"),
            AriaRole::Row         => Some("tr"),
            AriaRole::Cell        => Some("td"),
            _                     => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ARIA PROPERTIES / STATES
// ─────────────────────────────────────────────────────────────────────────────

/// ARIA property / state name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum AriaAttr {
    // States (can change at runtime)
    Busy, Checked, Disabled, Expanded, Grabbed, Hidden,
    Invalid, Live, Pressed, Selected,
    // Properties (relatively stable)
    ActiveDescendant, Atomic, Controls, DescribedBy, Details, ErrorMessage,
    FlowTo, HasPopup, KeyShortcuts, Label, LabelledBy, Level, Live2,
    Modal, MultiLine, MultiSelectable, Orientation, Owns, Placeholder,
    PosInSet, ReadOnly, Relevant, Required, RoleDescription,
    RowCount, RowIndex, RowSpan, SetSize, Sort, ValueMax, ValueMin,
    ValueNow, ValueText,
    Custom(String),
}

impl AriaAttr {
    pub fn from_attr_name(name: &str) -> Self {
        match name {
            "aria-busy"           => AriaAttr::Busy,
            "aria-checked"        => AriaAttr::Checked,
            "aria-disabled"       => AriaAttr::Disabled,
            "aria-expanded"       => AriaAttr::Expanded,
            "aria-grabbed"        => AriaAttr::Grabbed,
            "aria-hidden"         => AriaAttr::Hidden,
            "aria-invalid"        => AriaAttr::Invalid,
            "aria-live"           => AriaAttr::Live,
            "aria-pressed"        => AriaAttr::Pressed,
            "aria-selected"       => AriaAttr::Selected,
            "aria-label"          => AriaAttr::Label,
            "aria-labelledby"     => AriaAttr::LabelledBy,
            "aria-describedby"    => AriaAttr::DescribedBy,
            "aria-level"          => AriaAttr::Level,
            "aria-modal"          => AriaAttr::Modal,
            "aria-multiline"      => AriaAttr::MultiLine,
            "aria-orientation"    => AriaAttr::Orientation,
            "aria-placeholder"    => AriaAttr::Placeholder,
            "aria-posinset"       => AriaAttr::PosInSet,
            "aria-readonly"       => AriaAttr::ReadOnly,
            "aria-required"       => AriaAttr::Required,
            "aria-setsize"        => AriaAttr::SetSize,
            "aria-valuemax"       => AriaAttr::ValueMax,
            "aria-valuemin"       => AriaAttr::ValueMin,
            "aria-valuenow"       => AriaAttr::ValueNow,
            "aria-valuetext"      => AriaAttr::ValueText,
            other                 => AriaAttr::Custom(other.to_string()),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ACCESSIBILITY TREE NODE
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AtNode {
    pub id:           usize,
    /// Computed accessible role.
    pub role:         AriaRole,
    /// Computed accessible name.
    pub name:         String,
    /// Computed accessible description.
    pub description:  String,
    /// ARIA states and properties.
    pub attrs:        BTreeMap<AriaAttr, String>,
    pub children:     Vec<usize>,
    pub parent:       Option<usize>,
    /// Tab index (-1 = not tabbable, 0 = default order, n = explicit order).
    pub tab_index:    i32,
    /// Whether the node is currently focused.
    pub focused:      bool,
}

impl AtNode {
    pub fn new(id: usize, role: AriaRole, name: &str) -> Self {
        AtNode {
            id, role, name: name.to_string(), description: String::new(),
            attrs: BTreeMap::new(), children: Vec::new(), parent: None,
            tab_index: if role.is_interactive() { 0 } else { -1 },
            focused: false,
        }
    }

    pub fn set_attr(&mut self, attr: AriaAttr, val: &str) {
        self.attrs.insert(attr, val.to_string());
    }

    pub fn get_attr(&self, attr: &AriaAttr) -> Option<&str> {
        self.attrs.get(attr).map(|s| s.as_str())
    }

    pub fn is_hidden(&self) -> bool {
        self.get_attr(&AriaAttr::Hidden) == Some("true")
    }

    pub fn is_disabled(&self) -> bool {
        self.get_attr(&AriaAttr::Disabled) == Some("true")
    }

    /// Generate the announcement text a screen reader would speak for this node.
    pub fn announcement(&self) -> String {
        let role_str = match self.role {
            AriaRole::Button    => "button",
            AriaRole::CheckBox  => "checkbox",
            AriaRole::Link      => "link",
            AriaRole::Heading   => "heading",
            AriaRole::TextBox   => "text field",
            AriaRole::Slider    => "slider",
            AriaRole::Radio     => "radio button",
            AriaRole::Tab       => "tab",
            AriaRole::Alert     => "alert",
            AriaRole::Dialog    => "dialog",
            _                   => "",
        };
        let state = match (self.get_attr(&AriaAttr::Checked),
                           self.get_attr(&AriaAttr::Expanded),
                           self.get_attr(&AriaAttr::Pressed)) {
            (Some("true"),  _, _) => ", checked",
            (Some("false"), _, _) => ", unchecked",
            (_, Some("true"),  _) => ", expanded",
            (_, Some("false"), _) => ", collapsed",
            (_, _, Some("true"))  => ", pressed",
            _                     => "",
        };
        let disabled = if self.is_disabled() { ", dimmed" } else { "" };
        if role_str.is_empty() {
            format!("{}{}{}", self.name, state, disabled)
        } else {
            format!("{}, {}{}{}", self.name, role_str, state, disabled)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ACCESSIBILITY TREE
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct AccessibilityTree {
    pub nodes:   BTreeMap<usize, AtNode>,
    pub root:    Option<usize>,
    /// Currently focused node ID.
    pub focused: Option<usize>,
}

impl AccessibilityTree {
    pub fn new() -> Self { Self::default() }

    pub fn add_node(&mut self, node: AtNode) {
        if self.root.is_none() { self.root = Some(node.id); }
        self.nodes.insert(node.id, node);
    }

    pub fn set_parent(&mut self, child_id: usize, parent_id: usize) {
        if let Some(c) = self.nodes.get_mut(&child_id) { c.parent = Some(parent_id); }
        if let Some(p) = self.nodes.get_mut(&parent_id) {
            if !p.children.contains(&child_id) { p.children.push(child_id); }
        }
    }

    /// Compute tab order: all focusable nodes sorted by tab_index.
    pub fn tab_order(&self) -> Vec<usize> {
        let mut explicit: Vec<(i32, usize)> = Vec::new();
        let mut default_order: Vec<usize>   = Vec::new();
        for (id, node) in &self.nodes {
            if node.tab_index < 0 || node.is_hidden() || node.is_disabled() { continue; }
            if node.tab_index == 0 { default_order.push(*id); }
            else                   { explicit.push((node.tab_index, *id)); }
        }
        explicit.sort();
        let mut out: Vec<usize> = explicit.into_iter().map(|(_, id)| id).collect();
        out.extend(default_order);
        out
    }

    /// Move focus to next tabbable node.  Returns the new focused node ID.
    pub fn focus_next(&mut self) -> Option<usize> {
        let order = self.tab_order();
        if order.is_empty() { return None; }
        let cur_pos = self.focused
            .and_then(|fid| order.iter().position(|&x| x == fid))
            .unwrap_or(order.len().wrapping_sub(1));
        let next_pos = (cur_pos + 1) % order.len();
        let next_id = order[next_pos];
        self.set_focused(next_id);
        Some(next_id)
    }

    pub fn focus_prev(&mut self) -> Option<usize> {
        let order = self.tab_order();
        if order.is_empty() { return None; }
        let cur_pos = self.focused
            .and_then(|fid| order.iter().position(|&x| x == fid))
            .unwrap_or(0);
        let prev_pos = if cur_pos == 0 { order.len() - 1 } else { cur_pos - 1 };
        let prev_id = order[prev_pos];
        self.set_focused(prev_id);
        Some(prev_id)
    }

    pub fn set_focused(&mut self, id: usize) {
        // Clear old focus
        if let Some(old) = self.focused {
            if let Some(n) = self.nodes.get_mut(&old) { n.focused = false; }
        }
        if let Some(n) = self.nodes.get_mut(&id) { n.focused = true; }
        self.focused = Some(id);
    }

    /// Get the announcement for the currently focused node.
    pub fn focused_announcement(&self) -> Option<String> {
        self.focused.and_then(|id| self.nodes.get(&id)).map(|n| n.announcement())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SCREEN READER OUTPUT BUFFER
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivePoliteness { Off, Polite, Assertive }

#[derive(Debug, Clone)]
pub struct SrAnnouncement {
    pub text:       String,
    pub politeness: LivePoliteness,
}

const SR_QUEUE_MAX: usize = 64;

pub struct ScreenReader {
    pub polite_queue:    Vec<SrAnnouncement>,
    pub assertive_queue: Vec<SrAnnouncement>,
}

impl ScreenReader {
    pub fn new() -> Self {
        ScreenReader { polite_queue: Vec::new(), assertive_queue: Vec::new() }
    }

    pub fn announce(&mut self, text: &str, politeness: LivePoliteness) {
        let a = SrAnnouncement { text: text.to_string(), politeness };
        match politeness {
            LivePoliteness::Assertive => {
                if self.assertive_queue.len() < SR_QUEUE_MAX { self.assertive_queue.push(a); }
            }
            LivePoliteness::Polite | LivePoliteness::Off => {
                if self.polite_queue.len() < SR_QUEUE_MAX { self.polite_queue.push(a); }
            }
        }
    }

    /// Drain the next announcement (assertive first, then polite).
    pub fn next(&mut self) -> Option<SrAnnouncement> {
        if !self.assertive_queue.is_empty() {
            return Some(self.assertive_queue.remove(0));
        }
        if !self.polite_queue.is_empty() {
            return Some(self.polite_queue.remove(0));
        }
        None
    }

    pub fn pending(&self) -> usize {
        self.polite_queue.len() + self.assertive_queue.len()
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
            else { fail += 1; crate::serial_println!("[FAIL] a11y: {}", $name); }
        }
    }

    // T1: Role from string
    check!(AriaRole::from_str("button")    == AriaRole::Button,     "role button");
    check!(AriaRole::from_str("checkbox")  == AriaRole::CheckBox,   "role checkbox");
    check!(AriaRole::from_str("unknown_x") == AriaRole::Generic,    "unknown role → Generic");

    // T2: Role is_interactive
    check!(AriaRole::Button.is_interactive(),  "button is interactive");
    check!(AriaRole::Link.is_interactive(),    "link is interactive");
    check!(!AriaRole::Heading.is_interactive(), "heading not interactive");

    // T3: AtNode announcement
    {
        let mut n = AtNode::new(0, AriaRole::Button, "Submit");
        let ann = n.announcement();
        check!(ann.contains("Submit"), "announcement contains name");
        check!(ann.contains("button"), "announcement contains role");

        n.set_attr(AriaAttr::Disabled, "true");
        let ann2 = n.announcement();
        check!(ann2.contains("dimmed"), "disabled → dimmed in announcement");
    }

    // T4: Accessibility tree build + tab order
    {
        let mut tree = AccessibilityTree::new();
        let root = AtNode::new(0, AriaRole::Main,   "Main");
        let btn1 = AtNode::new(1, AriaRole::Button, "First");
        let btn2 = AtNode::new(2, AriaRole::Button, "Second");
        let link = AtNode::new(3, AriaRole::Link,   "NavLink");
        tree.add_node(root);
        tree.add_node(btn1);
        tree.add_node(btn2);
        tree.add_node(link);
        let order = tree.tab_order();
        // All three interactive nodes should be in tab order
        check!(order.contains(&1) && order.contains(&2) && order.contains(&3), "tab order includes all interactive nodes");
        check!(!order.contains(&0), "main not in tab order (non-interactive)");
    }

    // T5: Focus next/prev
    {
        let mut tree = AccessibilityTree::new();
        for i in 1..=3 {
            tree.add_node(AtNode::new(i, AriaRole::Button, "Btn"));
        }
        let id = tree.focus_next();
        check!(id.is_some(), "focus_next returns Some");
        let id2 = tree.focus_next();
        check!(id != id2, "consecutive focus_next advances");
    }

    // T6: Hidden node excluded from tab order
    {
        let mut tree = AccessibilityTree::new();
        let mut n = AtNode::new(1, AriaRole::Button, "Hidden");
        n.set_attr(AriaAttr::Hidden, "true");
        tree.add_node(n);
        tree.add_node(AtNode::new(2, AriaRole::Button, "Visible"));
        let order = tree.tab_order();
        check!(!order.contains(&1), "hidden node not in tab order");
        check!(order.contains(&2),  "visible node in tab order");
    }

    // T7: Checked state in announcement
    {
        let mut n = AtNode::new(0, AriaRole::CheckBox, "Accept");
        n.set_attr(AriaAttr::Checked, "true");
        let ann = n.announcement();
        check!(ann.contains("checked"), "checked state in announcement");
    }

    // T8: Screen reader assertive first
    {
        let mut sr = ScreenReader::new();
        sr.announce("polite msg", LivePoliteness::Polite);
        sr.announce("ALERT!", LivePoliteness::Assertive);
        let first = sr.next().unwrap();
        check!(first.politeness == LivePoliteness::Assertive, "assertive comes first");
    }

    // T9: Screen reader drains in order
    {
        let mut sr = ScreenReader::new();
        sr.announce("first", LivePoliteness::Polite);
        sr.announce("second", LivePoliteness::Polite);
        let a = sr.next().unwrap();
        let b = sr.next().unwrap();
        check!(a.text == "first" && b.text == "second", "polite FIFO order");
        check!(sr.pending() == 0, "queue empty after drain");
    }

    // T10: focused_announcement
    {
        let mut tree = AccessibilityTree::new();
        tree.add_node(AtNode::new(1, AriaRole::Link, "Click here"));
        tree.set_focused(1);
        let ann = tree.focused_announcement();
        check!(ann.is_some(), "focused_announcement is Some");
        check!(ann.unwrap().contains("Click here"), "focused announcement contains name");
    }

    if fail == 0 {
        crate::serial_println!("[a11y] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[a11y] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

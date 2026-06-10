//! Browser render pipeline — HTML → DOM → CSS → display list.
//!
//! Takes raw HTML bytes from the network layer and produces a `Vec<RenderCmd>`
//! plus a total content height.  The list is consumed by `gui::widget::WebPage`,
//! which paints it into the browser content viewport.
//!
//! Pipeline
//!   1. `net::html::parse`               — HTML → DOM
//!   2. extract `<style>` content        — author stylesheets
//!   3. `net::css::parse`                — CSS text → Stylesheet
//!   4. `net::css::parse_inline_styles`  — `<element style="…">`
//!   5. `net::css::compute_styles`       — cascade + inherit → per-node style
//!   6. layout (block flow with inline-text accumulation + wrapping)
//!   7. paint  (RenderCmd emission)
//!
//! Supported in this MVP:
//!   • Block-flow layout (no flex/grid/float yet).
//!   • Inline text wrapping at the viewport edge.
//!   • CSS `color`, `background-color`, `font-size` (mapped to scale 1×/2×).
//!   • Padding-top/bottom + margin-top/bottom for blocks.
//!   • Left padding as indent.
//!   • `display: none` skips the subtree.
//!   • `<img>` → `"[image: …]"` placeholder until the PNG decoder lands.
//!   • `<a href="…">` underlines + cyan colour.
//!   • `<script>` / `<style>` / `<head>` skipped during layout.

#![allow(dead_code)]

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

use crate::gui::theme::{Color as GuiColor, BG_INPUT, TEXT_PRIMARY, ACCENT_CYAN, ACCENT_RED};
use crate::gui::widget::RenderCmd;
use crate::net::css::{
    self, ComputedStyle, Display, Position, Stylesheet, FontWeight, Color as CssColor,
    FlexDirection, JustifyContent, AlignItems, CssLength,
};
use crate::net::html::{self, Dom, NodeId, NodeKind};

// ─────────────────────────────────────────────────────────────────────────────
//  Public entry
// ─────────────────────────────────────────────────────────────────────────────

/// Extract the contents of the document's `<title>` element, if any.
pub fn extract_title(html_bytes: &[u8]) -> Option<String> {
    let body_str = core::str::from_utf8(html_bytes).unwrap_or("");
    let dom = html::parse(body_str);
    let t = html::extract_title(&dom);
    if t.trim().is_empty() { None } else { Some(t.trim().to_string()) }
}

/// Render an HTML document into a display list.
///
/// `base_url` is the absolute URL of the page itself; used to resolve relative
/// `<img src>` references.  Empty string disables image fetching.
///
/// Returns `(commands, total_height, page_background)`.
pub fn build_page(html_bytes: &[u8], viewport_w: u32, base_url: &str) -> (Vec<RenderCmd>, u32, GuiColor) {
    let body_str = core::str::from_utf8(html_bytes).unwrap_or("");
    let dom = html::parse(body_str);

    // Collect <style> blocks.
    let mut sheets: Vec<Stylesheet> = Vec::new();
    for sid in dom.find_all("style") {
        let text = collect_raw_text(&dom, sid);
        if !text.is_empty() {
            sheets.push(css::parse(&text));
        }
    }

    let inline = css::parse_inline_styles(&dom);
    let ua = css::user_agent_stylesheet();
    let styles = css::compute_styles(&dom, &sheets, &ua, &inline, None);

    // Find <body> (fallback to <html>, then root).
    let body_id = dom.find_element("body")
        .or_else(|| dom.find_element("html"))
        .unwrap_or(dom.root());

    let page_bg = styles
        .get(&body_id)
        .map(|s| to_gui_color(s.background_color))
        .filter(|c| !is_invisible(c))
        .unwrap_or(BG_INPUT);

    let mut ctx = LayoutCtx::new(viewport_w, &dom, &styles);
    ctx.base_url = base_url.into();
    layout_block(body_id, &mut ctx);

    (ctx.commands, ctx.cursor_y as u32, page_bg)
}

/// Render a plain-text error message into a display list (used when the network
/// request fails and we still want to fill the viewport).
pub fn build_error_page(url: &str, err: &str) -> (Vec<RenderCmd>, u32, GuiColor) {
    let mut cmds = Vec::new();
    let mut y: i32 = 8;
    cmds.push(RenderCmd::Text {
        x: 12, y, text: format!("Failed to load: {}", url),
        color: ACCENT_RED, scale: 1,
    });
    y += 20;
    cmds.push(RenderCmd::Text {
        x: 12, y, text: format!("Error: {}", err),
        color: TEXT_PRIMARY, scale: 1,
    });
    y += 20;
    cmds.push(RenderCmd::Text {
        x: 12, y, text: "Check that the network is up (about:network).".to_string(),
        color: TEXT_PRIMARY, scale: 1,
    });
    (cmds, (y + 24) as u32, BG_INPUT)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Layout context
// ─────────────────────────────────────────────────────────────────────────────

struct LayoutCtx<'a> {
    viewport_w: u32,
    dom:        &'a Dom,
    styles:     &'a BTreeMap<NodeId, ComputedStyle>,
    commands:   Vec<RenderCmd>,
    /// Current y position in document space.
    cursor_y:   i32,
    /// Pending inline text run for the current block.  Flushed on block transition.
    inline_buf: InlineBuf,
    /// Absolute URL of the page (for resolving relative `<img src>`).
    base_url:   String,
}

impl<'a> LayoutCtx<'a> {
    fn new(viewport_w: u32, dom: &'a Dom, styles: &'a BTreeMap<NodeId, ComputedStyle>) -> Self {
        Self {
            viewport_w,
            dom,
            styles,
            commands: Vec::new(),
            cursor_y: 8,
            inline_buf: InlineBuf::default(),
            base_url: String::new(),
        }
    }
}

#[derive(Default)]
struct InlineBuf {
    /// Accumulated word runs: (text, color, scale, bold).
    runs:  Vec<(String, GuiColor, u8, bool)>,
    /// Indent of the containing block, in pixels.
    indent: u32,
    /// Inherited block font size (for blank lines).
    line_h: u32,
    /// Whether the previous character pushed was whitespace (for collapsing).
    last_ws: bool,
    /// Force underline on the next run? (for <a>)
    underline_next: bool,
}

impl InlineBuf {
    fn is_empty(&self) -> bool {
        self.runs.is_empty() || self.runs.iter().all(|(t, _, _, _)| t.trim().is_empty())
    }

    fn push_text(&mut self, raw: &str, color: GuiColor, scale: u8, bold: bool) {
        // Collapse whitespace, preserve a single space between runs.
        let mut buf = String::new();
        for ch in raw.chars() {
            if ch == '\n' || ch == '\r' || ch == '\t' || ch == ' ' {
                if !self.last_ws && !buf.is_empty() {
                    buf.push(' ');
                    self.last_ws = true;
                } else if buf.is_empty() && !self.runs.is_empty() && !self.last_ws {
                    buf.push(' ');
                    self.last_ws = true;
                }
            } else {
                buf.push(ch);
                self.last_ws = false;
            }
        }
        if !buf.is_empty() {
            self.runs.push((buf, color, scale, bold));
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Layout — block flow
// ─────────────────────────────────────────────────────────────────────────────

fn layout_block(node_id: NodeId, ctx: &mut LayoutCtx) {
    let node = match ctx.dom.get(node_id) { Some(n) => n, None => return };

    let (tag, style) = match (node.tag(), ctx.styles.get(&node_id)) {
        (Some(t), Some(s)) => (t.to_ascii_lowercase(), s.clone()),
        _ => {
            // Non-element (e.g., document root) → just walk children.
            for &c in &node.children.clone() { layout_block(c, ctx); }
            return;
        }
    };

    // Skip metadata / scripted elements entirely.
    if matches!(tag.as_str(), "head"|"meta"|"link"|"title"|"script"|"style"|"noscript"|"template"|"svg"|"path"|"defs"|"use") {
        return;
    }
    if matches!(style.display, Display::None) || !style.visibility {
        return;
    }

    // ── Positioning: absolute/fixed are taken out of flow entirely.
    if matches!(style.position, Position::Absolute | Position::Fixed) {
        layout_positioned(node_id, ctx, &style, &tag);
        return;
    }

    // Compute block geometry — margins/padding only (border boxes are approximate).
    let mt = style.margin_top.to_px(style.font_size, 16.0).max(0.0) as u32;
    let mb = style.margin_bottom.to_px(style.font_size, 16.0).max(0.0) as u32;
    let pt = style.padding_top.to_px(style.font_size, 16.0).max(0.0) as u32;
    let pb = style.padding_bottom.to_px(style.font_size, 16.0).max(0.0) as u32;
    let pl = style.padding_left.to_px(style.font_size, 16.0).max(0.0) as u32;
    let pr = style.padding_right.to_px(style.font_size, 16.0).max(0.0) as u32;

    let block_indent = ctx.inline_buf.indent + pl + default_indent_for(&tag);

    // Treat as block?  Default tag-based table; CSS display can override.
    let is_block = is_block_tag(&tag) || matches!(style.display, Display::Block | Display::Flex | Display::Grid | Display::ListItem);

    if is_block {
        flush_inline(ctx);
        ctx.cursor_y += mt as i32;

        // Remember the start of this block's command range so position:relative
        // can offset everything it produced.
        let block_cmd_start = ctx.commands.len();

        // Background fill (if any) covers padding + content area.  Height is
        // computed AFTER children have been laid out, so emit a placeholder
        // Rect and patch it once we know the height.
        let bg_idx = if !is_invisible(&to_gui_color(style.background_color)) {
            ctx.commands.push(RenderCmd::Rect {
                x: (ctx.inline_buf.indent as i32),
                y: ctx.cursor_y,
                w: ctx.viewport_w.saturating_sub(ctx.inline_buf.indent),
                h: 0, // patched below
                color: to_gui_color(style.background_color),
            });
            Some(ctx.commands.len() - 1)
        } else {
            None
        };

        let block_top = ctx.cursor_y;
        ctx.cursor_y += pt as i32;

        // Heading rule: scale font and bold for h1..h6.
        let mut child_scale = scale_for_font(style.font_size);
        if matches!(tag.as_str(), "h1"|"h2") { child_scale = 2; }

        // Apply per-block indent.
        let saved_indent = ctx.inline_buf.indent;
        let saved_line_h = ctx.inline_buf.line_h;
        ctx.inline_buf.indent = block_indent;
        ctx.inline_buf.line_h = (16 * child_scale as u32).max(20);

        // Special-case content blocks.
        match tag.as_str() {
            "hr" => {
                ctx.cursor_y += 4;
                ctx.commands.push(RenderCmd::HRule {
                    x: block_indent as i32,
                    y: ctx.cursor_y,
                    w: ctx.viewport_w.saturating_sub(block_indent + pr + 4),
                    color: dim_for_hr(&style),
                });
                ctx.cursor_y += 8;
            }
            "li" => {
                // Render bullet as inline prefix then walk children.
                ctx.inline_buf.push_text("• ", to_gui_color(style.color), child_scale, false);
                walk_children_inline(node_id, ctx, child_scale);
                flush_inline(ctx);
            }
            "img" => {
                let alt = node.attr("alt").unwrap_or("image").to_string();
                let src = node.attr("src").unwrap_or("").to_string();
                let width_attr = node.attr("width").and_then(|s| s.parse::<u32>().ok());
                let height_attr = node.attr("height").and_then(|s| s.parse::<u32>().ok());
                emit_image(ctx, &src, &alt, width_attr, height_attr);
            }
            "input" | "textarea" | "select" | "button" => {
                emit_form_control(ctx, &tag, node, &style);
            }
            _ => {
                if matches!(style.display, Display::Flex) {
                    layout_flex(node_id, ctx, &style);
                } else if matches!(style.display, Display::Grid) {
                    layout_grid(node_id, ctx, &style);
                } else {
                    walk_children_block(node_id, ctx, child_scale);
                    flush_inline(ctx);
                }
            }
        }

        ctx.cursor_y += pb as i32;

        // Patch background height.
        if let Some(idx) = bg_idx {
            let h = (ctx.cursor_y - block_top).max(0) as u32;
            if let RenderCmd::Rect { h: ref mut hh, .. } = ctx.commands[idx] {
                *hh = h;
            }
        }

        ctx.cursor_y += mb as i32;

        // ── position: relative — offset our emitted commands without altering flow.
        if matches!(style.position, Position::Relative) {
            let dx = style.left.to_px(style.font_size, 16.0) as i32;
            let dy = style.top.to_px(style.font_size, 16.0) as i32;
            if dx != 0 || dy != 0 {
                for i in block_cmd_start..ctx.commands.len() {
                    let c = ctx.commands[i].clone();
                    ctx.commands[i] = offset_cmd(c, dx, dy);
                }
            }
        }

        ctx.inline_buf.indent = saved_indent;
        ctx.inline_buf.line_h = saved_line_h;
    } else {
        // Inline element — accumulate into the running line.
        let underline = tag == "a" || tag == "u";
        let was_underline = ctx.inline_buf.underline_next;
        if underline { ctx.inline_buf.underline_next = true; }

        let color = if tag == "a" {
            ACCENT_CYAN
        } else {
            to_gui_color(style.color)
        };
        let scale = scale_for_font(style.font_size);
        let bold = matches!(style.font_weight, FontWeight::Bold | FontWeight::Bolder)
            || matches!(style.font_weight, FontWeight::Number(n) if n >= 600)
            || tag == "b" || tag == "strong";

        // Walk children pushing text runs.
        for &child_id in &node.children.clone() {
            if let Some(child) = ctx.dom.get(child_id) {
                match &child.kind {
                    NodeKind::Text { data } => {
                        ctx.inline_buf.push_text(data, color, scale, bold);
                    }
                    NodeKind::Element { .. } => {
                        // Recurse — child element may be another inline.
                        layout_block(child_id, ctx);
                    }
                    _ => {}
                }
            }
        }

        ctx.inline_buf.underline_next = was_underline;
    }
}

fn walk_children_block(parent_id: NodeId, ctx: &mut LayoutCtx, scale: u8) {
    let children: Vec<NodeId> = ctx.dom.get(parent_id)
        .map(|n| n.children.clone())
        .unwrap_or_default();
    for child_id in children {
        let kind = ctx.dom.get(child_id).map(|n| n.kind.clone());
        match kind {
            Some(NodeKind::Text { data }) => {
                let color = ctx.styles.get(&parent_id)
                    .map(|s| to_gui_color(s.color))
                    .unwrap_or(TEXT_PRIMARY);
                ctx.inline_buf.push_text(&data, color, scale, false);
            }
            Some(NodeKind::Element { .. }) => {
                layout_block(child_id, ctx);
            }
            _ => {}
        }
    }
}

fn walk_children_inline(parent_id: NodeId, ctx: &mut LayoutCtx, scale: u8) {
    let children: Vec<NodeId> = ctx.dom.get(parent_id)
        .map(|n| n.children.clone())
        .unwrap_or_default();
    let color = ctx.styles.get(&parent_id)
        .map(|s| to_gui_color(s.color))
        .unwrap_or(TEXT_PRIMARY);
    for child_id in children {
        let kind = ctx.dom.get(child_id).map(|n| n.kind.clone());
        match kind {
            Some(NodeKind::Text { data }) => {
                ctx.inline_buf.push_text(&data, color, scale, false);
            }
            Some(NodeKind::Element { .. }) => {
                layout_block(child_id, ctx);
            }
            _ => {}
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Flex layout
// ─────────────────────────────────────────────────────────────────────────────

/// Render a `display: flex` container.
///
/// Strategy (MVP):
///   1. Identify direct element children that are flex items.
///   2. Lay each child into a *fragment* (a private command buffer rooted at
///      x=0, y=0) using the same viewport that we'd give a block of the
///      container's effective track width.
///   3. Position the resulting fragments along the main axis using
///      `justify-content` and along the cross axis using `align-items`.
///   4. Append the offset commands to the parent buffer.
///
/// Limitations vs. real CSS:
///   • `flex-wrap` ignored (single line only).
///   • `flex-grow`/`flex-shrink` honoured very loosely — items get a fair
///     share of free space along the main axis.
///   • `flex-basis` ignored (intrinsic content size only).
///   • `align-items: stretch` is approximated by using the full cross size.
fn layout_flex(node_id: NodeId, ctx: &mut LayoutCtx, container_style: &ComputedStyle) {
    flush_inline(ctx);

    let children: Vec<NodeId> = ctx.dom.get(node_id)
        .map(|n| n.children.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|c| matches!(ctx.dom.get(*c).map(|n| &n.kind), Some(NodeKind::Element { .. })))
        .filter(|c| ctx.styles.get(c).map(|s| !matches!(s.display, Display::None)).unwrap_or(true))
        .collect();

    if children.is_empty() { return; }

    let total_w = ctx.viewport_w.saturating_sub(ctx.inline_buf.indent + 8);
    let is_row  = matches!(container_style.flex_direction, FlexDirection::Row | FlexDirection::RowReverse);
    let reverse = matches!(container_style.flex_direction,
        FlexDirection::RowReverse | FlexDirection::ColumnReverse);

    // ── 1. Initial intrinsic measurement: each child gets a column of equal share.
    let n = children.len() as u32;
    let track_w = if is_row { (total_w / n.max(1)).max(40) } else { total_w };

    let mut fragments: Vec<Fragment> = Vec::with_capacity(children.len());
    let mut grow_total: f32 = 0.0;
    for &child in &children {
        let s = ctx.styles.get(&child).cloned().unwrap_or_else(ComputedStyle::initial);
        grow_total += s.flex_grow.max(0.0);
        let frag = render_fragment(child, ctx.dom, ctx.styles, track_w, &ctx.base_url);
        fragments.push(frag);
    }

    // ── 2. Distribute main-axis space.
    let (positions, total_main) = if is_row {
        let total_intrinsic: u32 = fragments.iter().map(|f| f.w).sum();
        let extra = total_w.saturating_sub(total_intrinsic);
        let gap_count = (n as i32 - 1).max(0) as u32;
        let mut positions = Vec::with_capacity(fragments.len());
        let mut x = 0u32;
        let gap = match container_style.justify_content {
            JustifyContent::SpaceBetween if n > 1 => extra / gap_count,
            JustifyContent::SpaceAround           => extra / n.max(1),
            JustifyContent::SpaceEvenly           => extra / (n + 1),
            _ => 0,
        };
        let start_offset = match container_style.justify_content {
            JustifyContent::Center                => extra / 2,
            JustifyContent::FlexEnd               => extra,
            JustifyContent::SpaceAround           => gap / 2,
            JustifyContent::SpaceEvenly           => gap,
            _ => 0,
        };
        x = start_offset;

        // If grow_total > 0, distribute extra proportionally to flex-grow values
        // (this overrides justify-content gaps).
        if grow_total > 0.0 && matches!(container_style.justify_content, JustifyContent::FlexStart) {
            for (i, &child) in children.iter().enumerate() {
                let s = ctx.styles.get(&child).cloned().unwrap_or_else(ComputedStyle::initial);
                let extra_for_me = (extra as f32 * (s.flex_grow.max(0.0) / grow_total)) as u32;
                let my_w = fragments[i].w + extra_for_me;
                positions.push((x as i32, 0i32, my_w));
                x = x.saturating_add(my_w);
            }
        } else {
            for f in &fragments {
                positions.push((x as i32, 0i32, f.w));
                x = x.saturating_add(f.w + gap);
            }
        }
        (positions, total_w)
    } else {
        // Column direction.
        let mut positions = Vec::with_capacity(fragments.len());
        let mut y = 0u32;
        for f in &fragments {
            positions.push((0i32, y as i32, f.w));
            y = y.saturating_add(f.h);
        }
        (positions, y)
    };

    // ── 3. Cross-axis alignment.
    let max_cross = if is_row {
        fragments.iter().map(|f| f.h).max().unwrap_or(0)
    } else {
        fragments.iter().map(|f| f.w).max().unwrap_or(0).min(total_w)
    };

    // ── 4. Emit positioned fragment commands into the parent.
    let base_x = ctx.inline_buf.indent as i32;
    let base_y = ctx.cursor_y;

    let mut ordered: Vec<usize> = (0..fragments.len()).collect();
    if reverse { ordered.reverse(); }

    for i in ordered {
        let (mx, my, _slot_w) = positions[i];
        let cross_align = match container_style.align_items {
            AlignItems::Center  => if is_row { (max_cross.saturating_sub(fragments[i].h)) / 2 }
                                   else      { (max_cross.saturating_sub(fragments[i].w)) / 2 },
            AlignItems::FlexEnd => if is_row { max_cross.saturating_sub(fragments[i].h) }
                                   else      { max_cross.saturating_sub(fragments[i].w) },
            _ => 0,
        };
        let dx = base_x + mx + if is_row { 0 } else { cross_align as i32 };
        let dy = base_y + my + if is_row { cross_align as i32 } else { 0 };
        for cmd in &fragments[i].cmds {
            ctx.commands.push(offset_cmd(cmd.clone(), dx, dy));
        }
    }

    // Advance the parent cursor past the flex container.
    ctx.cursor_y = base_y + if is_row { max_cross as i32 } else { total_main as i32 };
}

/// A render fragment: commands rendered with origin (0, 0), plus measured size.
struct Fragment {
    cmds: Vec<RenderCmd>,
    w:    u32,
    h:    u32,
}

fn render_fragment(
    root: NodeId,
    dom: &Dom,
    styles: &BTreeMap<NodeId, ComputedStyle>,
    width: u32,
    base_url: &str,
) -> Fragment {
    let mut sub = LayoutCtx::new(width, dom, styles);
    sub.base_url = base_url.into();
    sub.cursor_y = 0;
    layout_block(root, &mut sub);
    // Flush any trailing inline buffer.
    flush_inline(&mut sub);
    let w = sub.commands.iter().map(|c| cmd_right_edge(c) as u32).max().unwrap_or(width).min(width);
    let h = sub.cursor_y.max(0) as u32;
    Fragment { cmds: sub.commands, w, h }
}

fn cmd_right_edge(cmd: &RenderCmd) -> i32 {
    match cmd {
        RenderCmd::Rect            { x, w, .. } => x + *w as i32,
        RenderCmd::Border          { x, w, .. } => x + *w as i32,
        RenderCmd::HRule           { x, w, .. } => x + *w as i32,
        RenderCmd::Image           { x, w, .. } => x + *w as i32,
        RenderCmd::ImagePlaceholder{ x, w, .. } => x + *w as i32,
        RenderCmd::Text            { x, text, scale, .. } => {
            x + (text.chars().count() as i32) * 8 * (*scale as i32).max(1)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Grid layout
// ─────────────────────────────────────────────────────────────────────────────

/// Render a `display: grid` container.
///
/// MVP behaviour:
///   • Resolves `grid-template-columns` from the element's *inline* style
///     attribute (external CSS isn't threaded here yet).  Falls back to
///     `repeat(N, 1fr)` where N is min(children, 4).
///   • Tracks of unit `fr` share remaining space equally; `px` and `%` are
///     reserved up-front; `auto` behaves like 1fr.
///   • Children auto-flow row-major into the cell grid.
///   • Row heights = max(intrinsic heights of children in that row).
fn layout_grid(node_id: NodeId, ctx: &mut LayoutCtx, _style: &ComputedStyle) {
    flush_inline(ctx);

    let children: Vec<NodeId> = ctx.dom.get(node_id)
        .map(|n| n.children.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|c| matches!(ctx.dom.get(*c).map(|n| &n.kind), Some(NodeKind::Element { .. })))
        .filter(|c| ctx.styles.get(c).map(|s| !matches!(s.display, Display::None)).unwrap_or(true))
        .collect();

    if children.is_empty() { return; }

    let total_w = ctx.viewport_w.saturating_sub(ctx.inline_buf.indent + 8);

    // Resolve column tracks.
    let tracks = inline_grid_template(ctx.dom, node_id)
        .unwrap_or_else(|| {
            // Default: equal columns up to 4.
            let n = children.len().min(4).max(1);
            alloc::vec![GridTrack::Fr(1.0); n]
        });
    let col_widths = resolve_tracks(&tracks, total_w);
    let col_count  = col_widths.len();

    // Walk children row-major, packing into rows.  Each row's height is the
    // tallest fragment within that row.
    let base_x = ctx.inline_buf.indent as i32 + 4;
    let mut row_y = ctx.cursor_y;
    let mut row_fragments: Vec<(usize, Fragment)> = Vec::new();
    let mut col_idx = 0usize;

    let flush_row = |row_y: &mut i32, row_fragments: &mut Vec<(usize, Fragment)>, col_widths: &[u32], base_x: i32, ctx: &mut LayoutCtx| {
        if row_fragments.is_empty() { return; }
        let row_h = row_fragments.iter().map(|(_, f)| f.h).max().unwrap_or(0);
        // Emit each fragment at the right column.
        for (i, frag) in row_fragments.drain(..) {
            let mut x_off = base_x;
            for k in 0..i { x_off += col_widths[k] as i32; }
            for cmd in &frag.cmds {
                ctx.commands.push(offset_cmd(cmd.clone(), x_off, *row_y));
            }
        }
        *row_y += row_h as i32 + 4;
    };

    for child in children {
        let cw = col_widths[col_idx];
        let frag = render_fragment(
            child, ctx.dom, ctx.styles, cw,
            &ctx.base_url,
        );
        row_fragments.push((col_idx, frag));
        col_idx += 1;
        if col_idx >= col_count {
            flush_row(&mut row_y, &mut row_fragments, &col_widths, base_x, ctx);
            col_idx = 0;
        }
    }
    flush_row(&mut row_y, &mut row_fragments, &col_widths, base_x, ctx);
    ctx.cursor_y = row_y;
}

#[derive(Clone)]
enum GridTrack {
    Px(f32),
    Pct(f32),
    Fr(f32),
}

/// Resolve a track list to absolute pixel widths summing to (≤) total.
fn resolve_tracks(tracks: &[GridTrack], total: u32) -> Vec<u32> {
    let mut out = alloc::vec![0u32; tracks.len()];
    let mut fr_sum = 0.0f32;
    let mut reserved = 0u32;
    for (i, t) in tracks.iter().enumerate() {
        match t {
            GridTrack::Px(p)  => { out[i] = *p as u32; reserved = reserved.saturating_add(out[i]); }
            GridTrack::Pct(p) => { out[i] = ((total as f32) * (p / 100.0)) as u32; reserved = reserved.saturating_add(out[i]); }
            GridTrack::Fr(f)  => { fr_sum += f; }
        }
    }
    let free = total.saturating_sub(reserved);
    if fr_sum > 0.0 {
        for (i, t) in tracks.iter().enumerate() {
            if let GridTrack::Fr(f) = t {
                out[i] = ((free as f32) * (f / fr_sum)) as u32;
            }
        }
    }
    out
}

/// Look at `<element style="grid-template-columns: ...">` and parse it into tracks.
fn inline_grid_template(dom: &Dom, node_id: NodeId) -> Option<Vec<GridTrack>> {
    let node = dom.get(node_id)?;
    let attrs = match &node.kind {
        NodeKind::Element { attrs, .. } => attrs,
        _ => return None,
    };
    let style = attrs.get("style")?;
    // Find the declaration.
    let lc = style.to_ascii_lowercase();
    let key = "grid-template-columns";
    let pos = lc.find(key)?;
    let rest = &style[pos + key.len()..];
    let colon = rest.find(':')?;
    let after = &rest[colon + 1..];
    let semi = after.find(';').unwrap_or(after.len());
    let value = after[..semi].trim();
    parse_track_list(value)
}

fn parse_track_list(value: &str) -> Option<Vec<GridTrack>> {
    // Expand repeat(n, X).  Very simplistic — only supports a single repeat()
    // wrapping a single track.
    let mut tokens: Vec<String> = Vec::new();
    let mut chars = value.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() { chars.next(); continue; }
        if value[..].starts_with("repeat(") || tokens.is_empty() && {
            let rem: String = chars.clone().collect();
            rem.starts_with("repeat(")
        } {
            // Consume "repeat("
            for _ in 0..7 { chars.next(); }
            // Read inside until matching )
            let mut inner = alloc::string::String::new();
            let mut depth = 1;
            for c in chars.by_ref() {
                if c == '(' { depth += 1; inner.push(c); }
                else if c == ')' { depth -= 1; if depth == 0 { break; } else { inner.push(c); } }
                else { inner.push(c); }
            }
            // Split on comma.
            let mut parts = inner.splitn(2, ',');
            let n: usize = parts.next()?.trim().parse().ok()?;
            let track = parts.next()?.trim().to_string();
            for _ in 0..n { tokens.push(track.clone()); }
        } else {
            let mut tok = alloc::string::String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() { break; }
                tok.push(c);
                chars.next();
            }
            if !tok.is_empty() { tokens.push(tok); }
        }
    }
    let mut out = Vec::with_capacity(tokens.len());
    for t in tokens {
        out.push(parse_track(&t)?);
    }
    if out.is_empty() { None } else { Some(out) }
}

fn parse_track(s: &str) -> Option<GridTrack> {
    let s = s.trim();
    if s == "auto" || s == "max-content" || s == "min-content" {
        return Some(GridTrack::Fr(1.0));
    }
    if let Some(num) = s.strip_suffix("fr") {
        return num.trim().parse().ok().map(GridTrack::Fr);
    }
    if let Some(num) = s.strip_suffix("px") {
        return num.trim().parse().ok().map(GridTrack::Px);
    }
    if let Some(num) = s.strip_suffix('%') {
        return num.trim().parse().ok().map(GridTrack::Pct);
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
//  Absolute / fixed positioning
// ─────────────────────────────────────────────────────────────────────────────

/// Render a `position: absolute` or `fixed` element out-of-flow.
///
/// Containing block: viewport (for `fixed`; we don't track positioned ancestors
/// in this MVP, so absolute also falls back to viewport).  Offsets via
/// `top`/`left`/`right`/`bottom` (auto values resolve to 0).
fn layout_positioned(node_id: NodeId, ctx: &mut LayoutCtx, style: &ComputedStyle, _tag: &str) {
    // 1. Pick a width for the positioned box.  Default to half the viewport
    //    so popups, banners and absolutely-positioned overlays don't span the
    //    whole page.
    let viewport_w = ctx.viewport_w;
    let width = match style.width {
        CssLength::Px(p)      => p as u32,
        CssLength::Percent(p) => ((viewport_w as f32) * (p / 100.0)) as u32,
        _                     => (viewport_w / 2).max(120),
    }.min(viewport_w);

    // 2. Render content into a fragment so we can measure and move it.
    let frag = render_fragment(
        node_id, ctx.dom, ctx.styles, width,
        &ctx.base_url,
    );

    // 3. Resolve x position.
    let left  = resolve_offset(&style.left,  style.font_size, viewport_w);
    let right = resolve_offset(&style.right, style.font_size, viewport_w);
    let x = match (&style.left, &style.right) {
        (CssLength::Auto, CssLength::Auto) => ctx.inline_buf.indent as i32,
        (CssLength::Auto, _) => (viewport_w as i32) - right - (frag.w as i32),
        _                    => left,
    };

    // 4. Resolve y position.
    let top    = resolve_offset(&style.top,    style.font_size, viewport_w);
    let bottom = resolve_offset(&style.bottom, style.font_size, viewport_w);
    let y = match (&style.top, &style.bottom) {
        (CssLength::Auto, CssLength::Auto) => ctx.cursor_y,
        (CssLength::Auto, _) => ctx.cursor_y - bottom - (frag.h as i32),
        _                    => top,
    };

    // 5. Emit offset commands.  Don't advance cursor_y (out of flow).
    for cmd in frag.cmds {
        ctx.commands.push(offset_cmd(cmd, x, y));
    }
}

fn resolve_offset(len: &CssLength, font_px: f32, container: u32) -> i32 {
    match len {
        CssLength::Px(p)      => *p as i32,
        CssLength::Em(p)      => (*p * font_px) as i32,
        CssLength::Rem(p)     => (*p * 16.0) as i32,
        CssLength::Percent(p) => ((container as f32) * (*p / 100.0)) as i32,
        CssLength::Auto | CssLength::Zero => 0,
    }
}

fn offset_cmd(mut cmd: RenderCmd, dx: i32, dy: i32) -> RenderCmd {
    match &mut cmd {
        RenderCmd::Rect            { x, y, .. } => { *x += dx; *y += dy; }
        RenderCmd::Border          { x, y, .. } => { *x += dx; *y += dy; }
        RenderCmd::HRule           { x, y, .. } => { *x += dx; *y += dy; }
        RenderCmd::Image           { x, y, .. } => { *x += dx; *y += dy; }
        RenderCmd::ImagePlaceholder{ x, y, .. } => { *x += dx; *y += dy; }
        RenderCmd::Text            { x, y, .. } => { *x += dx; *y += dy; }
    }
    cmd
}

// ─────────────────────────────────────────────────────────────────────────────
//  Inline flush — wrap the accumulated runs into one or more lines and paint
// ─────────────────────────────────────────────────────────────────────────────

fn flush_inline(ctx: &mut LayoutCtx) {
    if ctx.inline_buf.is_empty() {
        // Even an empty flush should advance by line-height to preserve
        // intentional blank space.  But only once per "block boundary".
        if !ctx.inline_buf.runs.is_empty() {
            ctx.inline_buf.runs.clear();
            ctx.inline_buf.last_ws = false;
        }
        return;
    }

    let indent = ctx.inline_buf.indent;
    let mut line: Vec<(String, GuiColor, u8, bool)> = Vec::new();
    let mut line_w: u32 = 0;
    let max_w = ctx.viewport_w.saturating_sub(indent + 4);

    // Take ownership so we can drain.
    let runs = core::mem::take(&mut ctx.inline_buf.runs);
    ctx.inline_buf.last_ws = false;

    let mut line_h: u32 = 16;

    for (text, color, scale, bold) in runs {
        let char_w = 8 * scale as u32;
        if scale as u32 * 16 > line_h { line_h = scale as u32 * 16; }

        // Word-split for wrapping.  Punctuation stays attached to the preceding word.
        let words: Vec<&str> = split_words(&text);
        for word in words {
            let w_px = word.chars().count() as u32 * char_w;
            // Does it fit on the current line?
            if line_w + w_px > max_w && !line.is_empty() {
                emit_line(ctx, indent, &mut line, line_h);
                line_w = 0;
                line_h = scale as u32 * 16;
            }
            // Word still too big? Break it brutally.
            if w_px > max_w {
                let mut remaining = word;
                while !remaining.is_empty() {
                    let fit_chars = ((max_w - line_w) / char_w) as usize;
                    if fit_chars == 0 {
                        emit_line(ctx, indent, &mut line, line_h);
                        line_w = 0;
                        continue;
                    }
                    let take = fit_chars.min(remaining.chars().count());
                    let byte_end = remaining
                        .char_indices()
                        .nth(take)
                        .map(|(i, _)| i)
                        .unwrap_or(remaining.len());
                    let chunk = remaining[..byte_end].to_string();
                    line.push((chunk, color, scale, bold));
                    line_w += take as u32 * char_w;
                    remaining = &remaining[byte_end..];
                    if !remaining.is_empty() {
                        emit_line(ctx, indent, &mut line, line_h);
                        line_w = 0;
                    }
                }
            } else {
                line.push((word.to_string(), color, scale, bold));
                line_w += w_px;
            }
        }
    }
    if !line.is_empty() {
        emit_line(ctx, indent, &mut line, line_h);
    }
}

fn emit_line(ctx: &mut LayoutCtx, indent: u32, line: &mut Vec<(String, GuiColor, u8, bool)>, line_h: u32) {
    // Concatenate adjacent runs that share style for fewer commands.
    let mut x = indent as i32;
    for (text, color, scale, bold) in line.drain(..) {
        // Trim leading whitespace at line start.
        let painted = if x == indent as i32 { text.trim_start().to_string() } else { text };
        if painted.is_empty() { continue; }
        let w = painted.chars().count() as i32 * 8 * scale as i32;
        ctx.commands.push(RenderCmd::Text {
            x, y: ctx.cursor_y, text: painted, color, scale,
        });
        if bold {
            // Cheap "bold" effect: stroke the same text at x+1.
            // (Avoids needing a separate bold glyph table.)
            // We have to look it back up; trivial cost.
            if let Some(RenderCmd::Text { text: ref t, .. }) = ctx.commands.last().cloned() {
                let t = t.clone();
                ctx.commands.push(RenderCmd::Text {
                    x: x + 1, y: ctx.cursor_y, text: t, color, scale,
                });
            }
        }
        x += w;
    }
    ctx.cursor_y += line_h as i32 + 2;
}

// ─────────────────────────────────────────────────────────────────────────────
//  Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn collect_raw_text(dom: &Dom, parent: NodeId) -> String {
    let mut out = String::new();
    fn walk(dom: &Dom, id: NodeId, out: &mut String) {
        if let Some(n) = dom.get(id) {
            if let NodeKind::Text { data } = &n.kind {
                out.push_str(data);
            }
            for &c in &n.children { walk(dom, c, out); }
        }
    }
    walk(dom, parent, &mut out);
    out
}

fn to_gui_color(c: CssColor) -> GuiColor {
    GuiColor { r: c.r, g: c.g, b: c.b }
}

fn is_invisible(c: &GuiColor) -> bool {
    // Treat a true-black + zero-alpha as "no background"; we don't track alpha
    // here so use a simple heuristic: pure black is treated as transparent for
    // background fills, otherwise we'd cover the entire page background.
    c.r == 0 && c.g == 0 && c.b == 0
}

fn dim_for_hr(s: &ComputedStyle) -> GuiColor {
    let c = to_gui_color(s.color);
    GuiColor::rgb(c.r / 2 + 60, c.g / 2 + 60, c.b / 2 + 60)
}

fn is_block_tag(tag: &str) -> bool {
    matches!(tag,
        "html"|"body"|"div"|"section"|"article"|"header"|"footer"|"nav"|"main"|"aside"|
        "p"|"h1"|"h2"|"h3"|"h4"|"h5"|"h6"|"ul"|"ol"|"li"|"hr"|"pre"|"blockquote"|"table"|
        "thead"|"tbody"|"tr"|"form"|"figure"|"figcaption"|"address"|"dl"|"dt"|"dd"|"img")
}

fn default_indent_for(tag: &str) -> u32 {
    match tag {
        "ul"|"ol"|"dl"          => 24,
        "blockquote"            => 32,
        _                       => 0,
    }
}

fn scale_for_font(px: f32) -> u8 {
    if px >= 40.0      { 3 }
    else if px >= 22.0 { 2 }
    else               { 1 }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Form controls — render-only (no interactivity yet)
// ─────────────────────────────────────────────────────────────────────────────

fn emit_form_control(ctx: &mut LayoutCtx, tag: &str, node: &crate::net::html::Node, style: &ComputedStyle) {
    flush_inline(ctx);
    let placeholder_color = GuiColor::rgb(140, 140, 150);
    let border_color      = GuiColor::rgb(100, 100, 110);
    let bg_color          = GuiColor::rgb(40, 40, 48);

    match tag {
        "input" => {
            let type_attr = node.attr("type").unwrap_or("text").to_ascii_lowercase();
            match type_attr.as_str() {
                "checkbox" | "radio" => {
                    let size = 14u32;
                    let x = ctx.inline_buf.indent as i32 + 2;
                    let y = ctx.cursor_y + 2;
                    ctx.commands.push(RenderCmd::Border {
                        x, y, w: size, h: size, color: border_color,
                    });
                    if node.attr("checked").is_some() {
                        ctx.commands.push(RenderCmd::Rect {
                            x: x + 3, y: y + 3, w: size - 6, h: size - 6,
                            color: GuiColor::rgb(0, 120, 212),
                        });
                    }
                    ctx.cursor_y += 18;
                }
                _ => {
                    // Render as box with placeholder text or value.
                    let w = 240u32.min(ctx.viewport_w.saturating_sub(ctx.inline_buf.indent + 16));
                    let h = 24u32;
                    let x = ctx.inline_buf.indent as i32 + 2;
                    let y = ctx.cursor_y + 2;
                    ctx.commands.push(RenderCmd::Rect { x, y, w, h, color: bg_color });
                    ctx.commands.push(RenderCmd::Border { x, y, w, h, color: border_color });
                    let display = node.attr("value")
                        .or_else(|| node.attr("placeholder"))
                        .unwrap_or("");
                    let visible = if type_attr == "password" {
                        // Mask password text.
                        "•".repeat(display.chars().count())
                    } else {
                        display.to_string()
                    };
                    if !visible.is_empty() {
                        let c = if node.attr("value").is_some() {
                            to_gui_color(style.color)
                        } else {
                            placeholder_color
                        };
                        ctx.commands.push(RenderCmd::Text {
                            x: x + 6, y: y + 4, text: visible, color: c, scale: 1,
                        });
                    }
                    ctx.cursor_y += h as i32 + 6;
                }
            }
        }
        "textarea" => {
            let w = ctx.viewport_w.saturating_sub(ctx.inline_buf.indent + 16);
            let h = 80u32;
            let x = ctx.inline_buf.indent as i32 + 2;
            let y = ctx.cursor_y + 2;
            ctx.commands.push(RenderCmd::Rect   { x, y, w, h, color: bg_color });
            ctx.commands.push(RenderCmd::Border { x, y, w, h, color: border_color });
            let text = node.text_content(ctx.dom);
            for (i, line) in text.lines().take(4).enumerate() {
                ctx.commands.push(RenderCmd::Text {
                    x: x + 6, y: y + 4 + (i as i32) * 16,
                    text: line.to_string(),
                    color: to_gui_color(style.color),
                    scale: 1,
                });
            }
            ctx.cursor_y += h as i32 + 6;
        }
        "select" => {
            // Show first option as the visible value.
            let w = 200u32.min(ctx.viewport_w.saturating_sub(ctx.inline_buf.indent + 16));
            let h = 24u32;
            let x = ctx.inline_buf.indent as i32 + 2;
            let y = ctx.cursor_y + 2;
            ctx.commands.push(RenderCmd::Rect   { x, y, w, h, color: bg_color });
            ctx.commands.push(RenderCmd::Border { x, y, w, h, color: border_color });
            // Find first <option>.
            let txt = first_option_label(ctx.dom, node).unwrap_or_default();
            ctx.commands.push(RenderCmd::Text {
                x: x + 6, y: y + 4, text: txt,
                color: to_gui_color(style.color), scale: 1,
            });
            // Drop-arrow chevron.
            ctx.commands.push(RenderCmd::Text {
                x: x + w as i32 - 16, y: y + 4, text: "▼".into(),
                color: placeholder_color, scale: 1,
            });
            ctx.cursor_y += h as i32 + 6;
        }
        "button" => {
            let label = node.text_content(ctx.dom);
            let lc = label.trim();
            let txt = if lc.is_empty() { "Button" } else { lc };
            let text_w = txt.chars().count() as u32 * 8;
            let w = (text_w + 24).min(ctx.viewport_w.saturating_sub(ctx.inline_buf.indent + 16));
            let h = 26u32;
            let x = ctx.inline_buf.indent as i32 + 2;
            let y = ctx.cursor_y + 2;
            ctx.commands.push(RenderCmd::Rect   { x, y, w, h, color: GuiColor::rgb(0, 120, 212) });
            ctx.commands.push(RenderCmd::Border { x, y, w, h, color: GuiColor::rgb(40, 160, 240) });
            ctx.commands.push(RenderCmd::Text {
                x: x + 12, y: y + 5, text: txt.to_string(),
                color: GuiColor::rgb(255, 255, 255), scale: 1,
            });
            ctx.cursor_y += h as i32 + 6;
        }
        _ => {}
    }
}

fn first_option_label(dom: &Dom, node: &crate::net::html::Node) -> Option<String> {
    for &child in &node.children {
        if let Some(c) = dom.get(child) {
            if c.tag() == Some("option") {
                let t = c.text_content(dom);
                let t = t.trim();
                if !t.is_empty() { return Some(t.to_string()); }
            }
        }
    }
    None
}

#[derive(Clone, Copy)]
enum ImgFormat { Png, Jpeg, Gif, Webp, Unknown }

struct ImageBuf {
    w:    u32,
    h:    u32,
    data: Vec<u8>, // RGBA8
}

fn sniff_format(bytes: &[u8]) -> ImgFormat {
    if bytes.len() >= 8 && &bytes[..8] == &[137, 80, 78, 71, 13, 10, 26, 10] {
        return ImgFormat::Png;
    }
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return ImgFormat::Jpeg;
    }
    if bytes.len() >= 6 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        return ImgFormat::Gif;
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return ImgFormat::Webp;
    }
    ImgFormat::Unknown
}

/// Resolve and (if budget remains) fetch+decode an image, emitting either
/// an `Image` command or a placeholder text run.
fn emit_image(ctx: &mut LayoutCtx, src: &str, alt: &str, width_attr: Option<u32>, height_attr: Option<u32>) {
    flush_inline(ctx);

    // Resolve URL.
    let url = resolve_url(&ctx.base_url, src);

    // Fast-skip cases we know we can't render or shouldn't fetch.
    let placeholder = |ctx: &mut LayoutCtx, reason: &str| {
        ctx.inline_buf.push_text(
            &format!("[image: {} — {}]", alt, reason),
            GuiColor::rgb(120, 120, 140), 1, false,
        );
        flush_inline(ctx);
    };

    if url.is_empty() {
        placeholder(ctx, "no src");
        return;
    }
    if url.starts_with("data:") {
        placeholder(ctx, "data: URI");
        return;
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        placeholder(ctx, "unsupported scheme");
        return;
    }
    let url_lower = url.to_ascii_lowercase();
    let kind = if url_lower.contains(".png") {
        ImgFormat::Png
    } else if url_lower.contains(".jpg") || url_lower.contains(".jpeg") {
        ImgFormat::Jpeg
    } else if url_lower.contains(".gif") {
        ImgFormat::Gif
    } else if url_lower.contains(".svg") {
        placeholder(ctx, "SVG not supported");
        return;
    } else if url_lower.contains(".webp") {
        placeholder(ctx, "WebP not supported");
        return;
    } else {
        // Unknown extension — try sniffing after fetch.
        ImgFormat::Unknown
    };

    // Compute display dimensions from HTML attributes, or use a sensible default.
    // We do NOT fetch images here — that would nest a TLS connection inside layout,
    // which overflows the thread stack.  The browser will fetch images in a second
    // pass at the top level after build_page() returns.
    let max_w = ctx.viewport_w.saturating_sub(ctx.inline_buf.indent + 8);
    let (display_w, display_h) = match (width_attr, height_attr) {
        (Some(w), Some(h)) if w > 0 && h > 0 && w <= 2048 && h <= 2048 => {
            // Scale down if wider than viewport.
            if w <= max_w { (w, h) } else {
                let ratio = w as f32 / max_w as f32;
                (max_w, (h as f32 / ratio) as u32)
            }
        }
        (Some(w), None) if w > 0 && w <= 2048 => {
            let cw = w.min(max_w);
            (cw, cw * 3 / 4)   // guess 4:3 aspect
        }
        _ => {
            // No size attributes — use a reasonable default box.
            let w = max_w.min(300);
            (w, w * 3 / 4)
        }
    };

    ctx.commands.push(RenderCmd::ImagePlaceholder {
        url: url.clone(),
        alt: alt.to_string(),
        x: ctx.inline_buf.indent as i32 + 4,
        y: ctx.cursor_y,
        w: display_w,
        h: display_h,
    });
    ctx.cursor_y += display_h as i32 + 6;
    let _ = kind; // format known at fetch time
}

/// Resolve `href` against `base` (returns absolute URL).
fn resolve_url(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("data:") {
        return href.into();
    }
    if href.starts_with("//") {
        // Protocol-relative.
        let scheme = if base.starts_with("https") { "https:" } else { "http:" };
        return alloc::format!("{}{}", scheme, href);
    }
    // Find scheme + host of base.
    let (scheme_end, after_scheme) = match base.find("://") {
        Some(p) => (p + 3, &base[p + 3..]),
        None => return href.into(),
    };
    let host_end = after_scheme.find('/').map(|p| scheme_end + p).unwrap_or(base.len());
    let origin = &base[..host_end];

    if href.starts_with('/') {
        return alloc::format!("{}{}", origin, href);
    }
    // Relative path — strip the last path component of base.
    let path_end = base[host_end..].rfind('/').map(|p| host_end + p + 1).unwrap_or(base.len());
    alloc::format!("{}{}", &base[..path_end], href)
}

/// Nearest-neighbour RGBA resample.
fn nearest_resample(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = alloc::vec![0u8; (dw * dh * 4) as usize];
    for y in 0..dh {
        for x in 0..dw {
            let sx = (x as u64 * sw as u64 / dw as u64) as u32;
            let sy = (y as u64 * sh as u64 / dh as u64) as u32;
            let si = ((sy * sw + sx) * 4) as usize;
            let di = ((y * dw + x) * 4) as usize;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    out
}

fn split_words(s: &str) -> Vec<&str> {
    // Split on whitespace but keep each leading space attached as a separator.
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut in_word = false;
    for (i, b) in bytes.iter().enumerate() {
        let is_ws = *b == b' ' || *b == b'\t' || *b == b'\n' || *b == b'\r';
        if is_ws {
            if in_word {
                out.push(&s[start..i]);
                in_word = false;
            }
            // Emit the single space as its own token so the painter preserves it.
            out.push(&s[i..i+1]);
        } else {
            if !in_word { start = i; in_word = true; }
        }
    }
    if in_word { out.push(&s[start..]); }
    out
}

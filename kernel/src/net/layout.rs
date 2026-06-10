//! Layout Engine — Phase 36 for Smart OS.
//!
//! Implements CSS Visual Formatting Model:
//!  • Block Formatting Context (BFC) — vertical stacking, margin collapse
//!  • Inline Formatting Context (IFC) — line boxes, text wrapping
//!  • Flex Formatting Context (FFC) — row / column, flex-grow
//!  • Absolute / Fixed positioning
//!  • Box model (content + padding + border + margin)
//!
//! Output: `LayoutBox` tree — each node has a resolved `Rect` (x, y, w, h)
//! in absolute viewport coordinates.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use super::html::{Dom, NodeId, NodeKind, NULL_NODE};
use super::css::{ComputedStyle, Display, Position, CssLength, FlexDirection,
                 JustifyContent, AlignItems};

// ─────────────────────────────────────────────────────────────────────────────
//  Public types
// ─────────────────────────────────────────────────────────────────────────────

/// Axis-aligned rectangle in layout space.
#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w &&
        py >= self.y && py < self.y + self.h
    }
}

/// Edge sizes (top / right / bottom / left).
#[derive(Debug, Clone, Copy, Default)]
pub struct EdgeSizes {
    pub top:    f32,
    pub right:  f32,
    pub bottom: f32,
    pub left:   f32,
}

impl EdgeSizes {
    pub fn horizontal(&self) -> f32 { self.left + self.right }
    pub fn vertical(&self)   -> f32 { self.top  + self.bottom }
}

/// All four box areas for a node.
#[derive(Debug, Clone, Default)]
pub struct BoxAreas {
    pub content: Rect,
    pub padding: EdgeSizes,
    pub border:  EdgeSizes,
    pub margin:  EdgeSizes,
}

impl BoxAreas {
    /// Outer (margin) rectangle.
    pub fn margin_rect(&self) -> Rect {
        Rect {
            x: self.content.x - self.padding.left   - self.border.left   - self.margin.left,
            y: self.content.y - self.padding.top    - self.border.top    - self.margin.top,
            w: self.content.w + self.padding.horizontal() + self.border.horizontal() + self.margin.horizontal(),
            h: self.content.h + self.padding.vertical()   + self.border.vertical()   + self.margin.vertical(),
        }
    }
    /// Border rectangle.
    pub fn border_rect(&self) -> Rect {
        Rect {
            x: self.content.x - self.padding.left - self.border.left,
            y: self.content.y - self.padding.top  - self.border.top,
            w: self.content.w + self.padding.horizontal() + self.border.horizontal(),
            h: self.content.h + self.padding.vertical()   + self.border.vertical(),
        }
    }
    /// Padding rectangle.
    pub fn padding_rect(&self) -> Rect {
        Rect {
            x: self.content.x - self.padding.left,
            y: self.content.y - self.padding.top,
            w: self.content.w + self.padding.horizontal(),
            h: self.content.h + self.padding.vertical(),
        }
    }
}

/// One line box in an inline formatting context.
#[derive(Debug, Clone)]
pub struct LineBox {
    pub y:      f32,
    pub height: f32,
    pub items:  Vec<InlineItem>,
}

/// One inline item within a line box.
#[derive(Debug, Clone)]
pub struct InlineItem {
    pub node_id: NodeId,
    pub x:       f32,
    pub width:   f32,
    pub height:  f32,
}

/// The result of laying out a single DOM node.
#[derive(Debug, Clone)]
pub struct LayoutBox {
    pub node_id:  NodeId,
    pub areas:    BoxAreas,
    pub lines:    Vec<LineBox>,   // for inline/text containers
    pub children: Vec<LayoutBox>,
}

impl LayoutBox {
    /// Absolute paint rectangle (border-box).
    pub fn paint_rect(&self) -> Rect { self.areas.border_rect() }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Layout configuration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct LayoutConfig {
    pub viewport_width:  f32,
    pub viewport_height: f32,
    pub root_font_px:    f32,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Internal context passed through the recursion
// ─────────────────────────────────────────────────────────────────────────────

struct LayoutCtx<'a> {
    dom:      &'a Dom,
    styles:   &'a BTreeMap<NodeId, ComputedStyle>,
    root_px:  f32,
}

impl<'a> LayoutCtx<'a> {
    fn style(&self, id: NodeId) -> Option<&ComputedStyle> { self.styles.get(&id) }

    /// Resolve a `CssLength` in the context of a parent width / font-size.
    fn resolve_len(&self, len: &CssLength, parent_w: f32, parent_font_px: f32) -> f32 {
        match len {
            CssLength::Px(v)      => *v,
            CssLength::Em(v)      => v * parent_font_px,
            CssLength::Rem(v)     => v * self.root_px,
            CssLength::Percent(v) => v / 100.0 * parent_w,
            CssLength::Auto       => 0.0,
            CssLength::Zero       => 0.0,
        }
    }

    fn resolve_margin(&self, len: &CssLength, parent_w: f32, font_px: f32) -> f32 {
        // Auto margin → caller must handle centering
        match len {
            CssLength::Auto => 0.0,
            other           => self.resolve_len(other, parent_w, font_px),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  No-std float helpers
// ─────────────────────────────────────────────────────────────────────────────

#[inline] fn f32_max(a: f32, b: f32) -> f32 { if a > b { a } else { b } }
#[inline] fn f32_min(a: f32, b: f32) -> f32 { if a < b { a } else { b } }
#[inline] fn f32_abs(x: f32) -> f32 { if x < 0.0 { -x } else { x } }

// ─────────────────────────────────────────────────────────────────────────────
//  Entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Run the layout algorithm on the whole document.
/// Returns a flat map from NodeId → LayoutBox for every laid-out element.
pub fn layout_document(
    dom:     &Dom,
    styles:  &BTreeMap<NodeId, ComputedStyle>,
    config:  LayoutConfig,
) -> BTreeMap<NodeId, LayoutBox> {
    let ctx = LayoutCtx { dom: dom, styles: styles, root_px: config.root_font_px };
    let containing = Rect { x: 0.0, y: 0.0, w: config.viewport_width, h: config.viewport_height };
    let mut out: BTreeMap<NodeId, LayoutBox> = BTreeMap::new();

    if let Some(root_box) = layout_node(&ctx, dom.root(), containing, 0.0) {
        collect_boxes(root_box, &mut out);
    }
    out
}

/// Flatten the LayoutBox tree into the map.
fn collect_boxes(lb: LayoutBox, out: &mut BTreeMap<NodeId, LayoutBox>) {
    for child in lb.children.iter() {
        collect_boxes(child.clone(), out);
    }
    out.insert(lb.node_id, lb);
}

// ─────────────────────────────────────────────────────────────────────────────
//  Per-node layout
// ─────────────────────────────────────────────────────────────────────────────

fn layout_node(
    ctx:       &LayoutCtx,
    node_id:   NodeId,
    containing: Rect,   // content rectangle of the containing block
    current_y: f32,     // current Y cursor in the containing block (BFC)
) -> Option<LayoutBox> {
    let node = ctx.dom.get(node_id)?;

    match &node.kind {
        NodeKind::Document => {
            // Document root: lay out children in a block context
            layout_block_children(ctx, node_id, containing, current_y)
        }

        NodeKind::Element { tag: _, attrs: _ } => {
            let style = ctx.styles.get(&node_id)?;

            // Invisible nodes skip layout
            if style.display == Display::None {
                return None;
            }

            match style.display {
                Display::Block | Display::ListItem => {
                    Some(layout_block(ctx, node_id, style, containing, current_y))
                }
                Display::Flex => {
                    Some(layout_flex(ctx, node_id, style, containing, current_y))
                }
                Display::Inline | Display::InlineBlock => {
                    // Inline elements are handled by their parent's IFC
                    Some(layout_inline_element(ctx, node_id, style, containing, current_y))
                }
                Display::Grid => {
                    // Grid: treat as block for now
                    Some(layout_block(ctx, node_id, style, containing, current_y))
                }
                Display::None => None,
            }
        }

        NodeKind::Text { data } => {
            // Text node: estimate width based on char count × font-size * 0.6
            // Real layout needs a font shaper; this is a placeholder
            let parent_style = if node.parent != NULL_NODE {
                ctx.styles.get(&node.parent)
            } else {
                None
            };
            let font_px = parent_style.map(|s| s.font_size).unwrap_or(ctx.root_px);
            let char_w  = font_px * 0.6;
            let text_w  = f32_min(data.len() as f32 * char_w, containing.w);
            let text_h  = font_px * 1.2; // line-height ≈ 1.2

            let mut areas = BoxAreas::default();
            areas.content = Rect { x: containing.x, y: containing.y + current_y, w: text_w, h: text_h };
            Some(LayoutBox { node_id, areas, lines: Vec::new(), children: Vec::new() })
        }

        NodeKind::Comment { .. } | NodeKind::Doctype { .. } => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Block layout
// ─────────────────────────────────────────────────────────────────────────────

fn layout_block(
    ctx:       &LayoutCtx,
    node_id:   NodeId,
    style:     &ComputedStyle,
    containing: Rect,
    cursor_y:  f32,
) -> LayoutBox {
    let font_px  = style.font_size;
    let par_w    = containing.w;

    // ── Edge sizes ──────────────────────────────────────────────────────────
    let pad = EdgeSizes {
        top:    ctx.resolve_len(&style.padding_top,    par_w, font_px),
        right:  ctx.resolve_len(&style.padding_right,  par_w, font_px),
        bottom: ctx.resolve_len(&style.padding_bottom, par_w, font_px),
        left:   ctx.resolve_len(&style.padding_left,   par_w, font_px),
    };
    let brd = EdgeSizes {
        top: style.border_top, right: style.border_right,
        bottom: style.border_bottom, left: style.border_left,
    };
    let marg = EdgeSizes {
        top:    ctx.resolve_margin(&style.margin_top,    par_w, font_px),
        right:  ctx.resolve_margin(&style.margin_right,  par_w, font_px),
        bottom: ctx.resolve_margin(&style.margin_bottom, par_w, font_px),
        left:   ctx.resolve_margin(&style.margin_left,   par_w, font_px),
    };

    // ── Content width ────────────────────────────────────────────────────────
    let content_w = match &style.width {
        CssLength::Auto => {
            // Shrink-to-fit: fill parent minus margins/borders/padding
            let used = marg.horizontal() + brd.horizontal() + pad.horizontal();
            f32_max(0.0, par_w - used)
        }
        other => {
            let resolved = ctx.resolve_len(other, par_w, font_px);
            // Auto-centering for margin:auto when explicit width
            resolved
        }
    };

    // ── Content X ────────────────────────────────────────────────────────────
    let content_x = {
        // Horizontal auto-centering
        let is_left_auto  = matches!(style.margin_left,  CssLength::Auto);
        let is_right_auto = matches!(style.margin_right, CssLength::Auto);
        if is_left_auto && is_right_auto {
            containing.x + (par_w - content_w - brd.horizontal() - pad.horizontal()) / 2.0
        } else {
            containing.x + marg.left + brd.left + pad.left
        }
    };

    // ── Content Y ────────────────────────────────────────────────────────────
    let content_y = containing.y + cursor_y + marg.top + brd.top + pad.top;

    // ── Explicit height ──────────────────────────────────────────────────────
    let explicit_h: Option<f32> = match &style.height {
        CssLength::Auto => None,
        other           => Some(ctx.resolve_len(other, containing.h, font_px)),
    };

    // ── Child content rectangle ─────────────────────────────────────────────
    let child_containing = Rect { x: content_x, y: content_y, w: content_w, h: 0.0 };

    // ── Absolute / fixed positioning override ────────────────────────────────
    if style.position == Position::Absolute || style.position == Position::Fixed {
        return layout_positioned(ctx, node_id, style, containing, cursor_y,
                                 content_x, content_y, content_w, explicit_h,
                                 pad, brd, marg);
    }

    // ── Lay out children ─────────────────────────────────────────────────────
    let (children, content_h) = layout_block_flow(ctx, node_id, child_containing, explicit_h);

    let mut areas = BoxAreas::default();
    areas.content = Rect { x: content_x, y: content_y, w: content_w, h: content_h };
    areas.padding = pad;
    areas.border  = brd;
    areas.margin  = marg;

    LayoutBox { node_id, areas, lines: Vec::new(), children }
}

/// Lay out children in block flow (vertical stacking), return (children, total_height).
fn layout_block_flow(
    ctx:       &LayoutCtx,
    parent_id: NodeId,
    containing: Rect,
    explicit_h: Option<f32>,
) -> (Vec<LayoutBox>, f32) {
    let node = match ctx.dom.get(parent_id) { Some(n) => n, None => return (Vec::new(), 0.0) };
    let mut children: Vec<LayoutBox> = Vec::new();
    let mut y_cursor: f32 = 0.0;
    let mut prev_margin_bottom: f32 = 0.0;

    for &child_id in &node.children {
        if let Some(child_box) = layout_node(ctx, child_id, containing, y_cursor) {
            // Margin collapse between adjacent block siblings
            let child_marg_top = child_box.areas.margin.top;
            let collapsed = f32_max(prev_margin_bottom, child_marg_top);

            // Adjust y by collapsed margin (remove double-counting)
            let delta = collapsed - prev_margin_bottom - child_marg_top;
            // delta is always <= 0 (collapsing reduces space)

            let outer_h = child_box.areas.content.h
                + child_box.areas.padding.vertical()
                + child_box.areas.border.vertical()
                + child_box.areas.margin.vertical();

            prev_margin_bottom = child_box.areas.margin.bottom;
            y_cursor += outer_h + delta;
            children.push(child_box);
        }
    }

    let content_h = explicit_h.unwrap_or(f32_max(0.0, y_cursor));
    (children, content_h)
}

/// Used for the document root (which has no ComputedStyle).
fn layout_block_children(
    ctx:       &LayoutCtx,
    node_id:   NodeId,
    containing: Rect,
    _cursor_y: f32,
) -> Option<LayoutBox> {
    let (children, content_h) = layout_block_flow(ctx, node_id, containing, None);
    let mut areas = BoxAreas::default();
    areas.content = Rect { x: containing.x, y: containing.y, w: containing.w, h: content_h };
    Some(LayoutBox { node_id, areas, lines: Vec::new(), children })
}

// ─────────────────────────────────────────────────────────────────────────────
//  Absolutely positioned layout
// ─────────────────────────────────────────────────────────────────────────────

fn layout_positioned(
    ctx:       &LayoutCtx,
    node_id:   NodeId,
    style:     &ComputedStyle,
    containing: Rect,
    _cursor_y: f32,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    explicit_h: Option<f32>,
    pad:   EdgeSizes,
    brd:   EdgeSizes,
    marg:  EdgeSizes,
) -> LayoutBox {
    let font_px  = style.font_size;
    let par_w    = containing.w;
    let par_h    = containing.h;

    let x = match &style.left {
        CssLength::Auto => match &style.right {
            CssLength::Auto => content_x,
            r               => containing.x + par_w - ctx.resolve_len(r, par_w, font_px) - content_w,
        },
        l => containing.x + ctx.resolve_len(l, par_w, font_px) + marg.left + brd.left + pad.left,
    };

    let y = match &style.top {
        CssLength::Auto => match &style.bottom {
            CssLength::Auto => content_y,
            b               => containing.y + par_h - ctx.resolve_len(b, par_h, font_px) - explicit_h.unwrap_or(0.0),
        },
        t => containing.y + ctx.resolve_len(t, par_h, font_px) + marg.top + brd.top + pad.top,
    };

    let child_containing = Rect { x, y, w: content_w, h: 0.0 };
    let (children, content_h) = layout_block_flow(ctx, node_id, child_containing, explicit_h);

    let mut areas = BoxAreas::default();
    areas.content = Rect { x, y, w: content_w, h: content_h };
    areas.padding = pad;
    areas.border  = brd;
    areas.margin  = marg;

    LayoutBox { node_id, areas, lines: Vec::new(), children }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Inline layout (simplified)
// ─────────────────────────────────────────────────────────────────────────────

fn layout_inline_element(
    ctx:       &LayoutCtx,
    node_id:   NodeId,
    style:     &ComputedStyle,
    containing: Rect,
    cursor_y:  f32,
) -> LayoutBox {
    let font_px  = style.font_size;
    let par_w    = containing.w;

    let pad = EdgeSizes {
        top:    ctx.resolve_len(&style.padding_top,    par_w, font_px),
        right:  ctx.resolve_len(&style.padding_right,  par_w, font_px),
        bottom: ctx.resolve_len(&style.padding_bottom, par_w, font_px),
        left:   ctx.resolve_len(&style.padding_left,   par_w, font_px),
    };
    let brd = EdgeSizes {
        top: style.border_top, right: style.border_right,
        bottom: style.border_bottom, left: style.border_left,
    };
    let marg = EdgeSizes {
        top:    ctx.resolve_margin(&style.margin_top,    par_w, font_px),
        right:  ctx.resolve_margin(&style.margin_right,  par_w, font_px),
        bottom: ctx.resolve_margin(&style.margin_bottom, par_w, font_px),
        left:   ctx.resolve_margin(&style.margin_left,   par_w, font_px),
    };

    let content_w = match &style.width {
        CssLength::Auto => par_w - marg.horizontal() - brd.horizontal() - pad.horizontal(),
        other           => ctx.resolve_len(other, par_w, font_px),
    };
    let content_h = match &style.height {
        CssLength::Auto => font_px * 1.2,
        other           => ctx.resolve_len(other, containing.h, font_px),
    };

    let content_x = containing.x + marg.left + brd.left + pad.left;
    let content_y = containing.y + cursor_y  + marg.top  + brd.top  + pad.top;

    let child_containing = Rect { x: content_x, y: content_y, w: content_w, h: content_h };
    let (children, actual_h) = layout_block_flow(ctx, node_id, child_containing, Some(content_h));

    let mut areas = BoxAreas::default();
    areas.content = Rect { x: content_x, y: content_y, w: content_w, h: actual_h };
    areas.padding = pad;
    areas.border  = brd;
    areas.margin  = marg;

    LayoutBox { node_id, areas, lines: Vec::new(), children }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Flex layout
// ─────────────────────────────────────────────────────────────────────────────

fn layout_flex(
    ctx:       &LayoutCtx,
    node_id:   NodeId,
    style:     &ComputedStyle,
    containing: Rect,
    cursor_y:  f32,
) -> LayoutBox {
    let font_px = style.font_size;
    let par_w   = containing.w;
    let par_h   = containing.h;

    let pad = EdgeSizes {
        top:    ctx.resolve_len(&style.padding_top,    par_w, font_px),
        right:  ctx.resolve_len(&style.padding_right,  par_w, font_px),
        bottom: ctx.resolve_len(&style.padding_bottom, par_w, font_px),
        left:   ctx.resolve_len(&style.padding_left,   par_w, font_px),
    };
    let brd = EdgeSizes {
        top: style.border_top, right: style.border_right,
        bottom: style.border_bottom, left: style.border_left,
    };
    let marg = EdgeSizes {
        top:    ctx.resolve_margin(&style.margin_top,    par_w, font_px),
        right:  ctx.resolve_margin(&style.margin_right,  par_w, font_px),
        bottom: ctx.resolve_margin(&style.margin_bottom, par_w, font_px),
        left:   ctx.resolve_margin(&style.margin_left,   par_w, font_px),
    };

    let content_w = match &style.width {
        CssLength::Auto => f32_max(0.0, par_w - marg.horizontal() - brd.horizontal() - pad.horizontal()),
        other           => ctx.resolve_len(other, par_w, font_px),
    };
    let explicit_h: Option<f32> = match &style.height {
        CssLength::Auto => None,
        other           => Some(ctx.resolve_len(other, par_h, font_px)),
    };

    let content_x = containing.x + marg.left + brd.left + pad.left;
    let content_y = containing.y + cursor_y  + marg.top  + brd.top  + pad.top;

    // Determine main/cross axis
    let is_row = matches!(style.flex_direction, FlexDirection::Row | FlexDirection::RowReverse);
    let is_rev = matches!(style.flex_direction, FlexDirection::RowReverse | FlexDirection::ColumnReverse);

    // Collect flex items (direct children with computed styles)
    let node = match ctx.dom.get(node_id) { Some(n) => n, None => {
        let mut areas = BoxAreas::default();
        areas.content = Rect { x: content_x, y: content_y, w: content_w, h: 0.0 };
        return LayoutBox { node_id, areas, lines: Vec::new(), children: Vec::new() };
    }};

    let child_ids: Vec<NodeId> = node.children.iter().copied()
        .filter(|&id| ctx.style(id).map(|s| s.display != Display::None).unwrap_or(false))
        .collect();

    let n = child_ids.len();
    if n == 0 {
        let content_h = explicit_h.unwrap_or(0.0);
        let mut areas = BoxAreas::default();
        areas.content = Rect { x: content_x, y: content_y, w: content_w, h: content_h };
        areas.padding = pad; areas.border = brd; areas.margin = marg;
        return LayoutBox { node_id, areas, lines: Vec::new(), children: Vec::new() };
    }

    // First pass: get intrinsic sizes for each item (auto-sized)
    let item_containing = Rect { x: content_x, y: content_y, w: content_w, h: explicit_h.unwrap_or(0.0) };
    let mut item_boxes: Vec<LayoutBox> = child_ids.iter().map(|&id| {
        layout_node(ctx, id, item_containing, 0.0)
            .unwrap_or_else(|| {
                let mut areas = BoxAreas::default();
                areas.content = Rect { x: content_x, y: content_y, w: 0.0, h: 0.0 };
                LayoutBox { node_id: id, areas, lines: Vec::new(), children: Vec::new() }
            })
    }).collect();

    // Second pass: distribute space
    let (content_h, placed_children) = if is_row {
        // Row: distribute horizontal space
        let total_item_w: f32 = item_boxes.iter().map(|b| b.areas.margin_rect().w).sum();
        let free_space = f32_max(0.0, content_w - total_item_w);
        let gap = distribute_gap(free_space, n, &style.justify_content);

        let mut x_cursor = content_x + gap.start;
        let mut max_h: f32 = 0.0;

        let order: Vec<usize> = if is_rev {
            (0..n).rev().collect()
        } else {
            (0..n).collect()
        };

        for &i in &order {
            let item = &mut item_boxes[i];
            let outer_w = item.areas.margin_rect().w;
            let outer_h = item.areas.margin_rect().h;

            // Align on cross axis (vertical)
            let item_content_y = match style.align_items {
                AlignItems::Center  => content_y + (explicit_h.unwrap_or(outer_h) - outer_h) / 2.0,
                AlignItems::FlexEnd => content_y + explicit_h.unwrap_or(outer_h) - outer_h,
                AlignItems::Stretch => {
                    item.areas.content.h = explicit_h.unwrap_or(item.areas.content.h);
                    content_y
                }
                _ => content_y,
            };

            item.areas.content.x = x_cursor + item.areas.margin.left + item.areas.border.left + item.areas.padding.left;
            item.areas.content.y = item_content_y + item.areas.margin.top + item.areas.border.top + item.areas.padding.top;

            x_cursor += outer_w + gap.between;
            max_h = f32_max(max_h, outer_h);
        }

        (explicit_h.unwrap_or(max_h), item_boxes)
    } else {
        // Column: distribute vertical space
        let total_item_h: f32 = item_boxes.iter().map(|b| b.areas.margin_rect().h).sum();
        let free_space = f32_max(0.0, explicit_h.unwrap_or(total_item_h) - total_item_h);
        let gap = distribute_gap(free_space, n, &style.justify_content);

        let mut y_cursor_inner = content_y + gap.start;
        let mut max_w: f32 = 0.0;

        let order: Vec<usize> = if is_rev { (0..n).rev().collect() } else { (0..n).collect() };

        for &i in &order {
            let item = &mut item_boxes[i];
            let outer_h = item.areas.margin_rect().h;
            let outer_w = item.areas.margin_rect().w;

            let item_content_x = match style.align_items {
                AlignItems::Center  => content_x + (content_w - outer_w) / 2.0,
                AlignItems::FlexEnd => content_x + content_w - outer_w,
                AlignItems::Stretch => {
                    item.areas.content.w = content_w - item.areas.padding.horizontal() - item.areas.border.horizontal() - item.areas.margin.horizontal();
                    content_x
                }
                _ => content_x,
            };

            item.areas.content.x = item_content_x + item.areas.margin.left + item.areas.border.left + item.areas.padding.left;
            item.areas.content.y = y_cursor_inner + item.areas.margin.top  + item.areas.border.top  + item.areas.padding.top;

            y_cursor_inner += outer_h + gap.between;
            max_w = f32_max(max_w, outer_w);
        }

        (y_cursor_inner - content_y, item_boxes)
    };

    let mut areas = BoxAreas::default();
    areas.content = Rect { x: content_x, y: content_y, w: content_w, h: content_h };
    areas.padding = pad;
    areas.border  = brd;
    areas.margin  = marg;

    LayoutBox { node_id, areas, lines: Vec::new(), children: placed_children }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Gap distribution helper (justify-content)
// ─────────────────────────────────────────────────────────────────────────────

struct GapSpec { start: f32, between: f32 }

fn distribute_gap(free: f32, n: usize, jc: &JustifyContent) -> GapSpec {
    if n == 0 { return GapSpec { start: 0.0, between: 0.0 }; }
    let nf = n as f32;
    match jc {
        JustifyContent::FlexStart    => GapSpec { start: 0.0,           between: 0.0 },
        JustifyContent::FlexEnd      => GapSpec { start: free,           between: 0.0 },
        JustifyContent::Center       => GapSpec { start: free / 2.0,    between: 0.0 },
        JustifyContent::SpaceBetween => GapSpec { start: 0.0,           between: if n > 1 { free / (nf - 1.0) } else { 0.0 } },
        JustifyContent::SpaceAround  => GapSpec { start: free / nf / 2.0, between: free / nf },
        JustifyContent::SpaceEvenly  => GapSpec { start: free / (nf + 1.0), between: free / (nf + 1.0) },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Hit testing
// ─────────────────────────────────────────────────────────────────────────────

/// Find the deepest LayoutBox that contains (px, py).
pub fn hit_test<'a>(
    boxes:  &'a BTreeMap<NodeId, LayoutBox>,
    px:     f32,
    py:     f32,
) -> Option<&'a LayoutBox> {
    let mut best: Option<&LayoutBox> = None;
    for lb in boxes.values() {
        if lb.paint_rect().contains(px, py) {
            // Prefer deeper nodes (higher NodeId usually = deeper in DOM)
            if best.is_none() || lb.node_id > best.unwrap().node_id {
                best = Some(lb);
            }
        }
    }
    best
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[layout] Box model layout engine ready (Phase 36).");
}

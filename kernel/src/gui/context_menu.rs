/// Smart OS Context Menu — Right-click popup menu system.
///
/// Provides a floating context menu with themed items, hover highlighting,
/// and click-to-select behavior. Integrates with the compositor for rendering.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use super::compositor::Compositor;
use super::theme::*;

/// Actions that a context menu item can trigger.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MenuAction {
    Copy,
    Paste,
    Cut,
    Close,
    Minimize,
    Maximize,
    Refresh,
    NewFile,
    Delete,
    Properties,
    None,
}

/// A single item in a context menu.
pub struct MenuItem {
    pub label: String,
    pub action: MenuAction,
    pub enabled: bool,
}

/// A visible context menu with position, items, and hover state.
pub struct ContextMenu {
    pub x: usize,
    pub y: usize,
    pub items: Vec<MenuItem>,
    pub visible: bool,
    pub selected: Option<usize>,
}

/// Global context menu state.
pub static CONTEXT_MENU: Mutex<Option<ContextMenu>> = Mutex::new(None);

const ITEM_HEIGHT: usize = 22;
const PADDING_LEFT: usize = 8;
const PADDING_TOTAL: usize = 24;
const CHAR_WIDTH: usize = 8;

/// Show a context menu at the given screen coordinates.
pub fn show(x: usize, y: usize, items: Vec<MenuItem>) {
    *CONTEXT_MENU.lock() = Some(ContextMenu {
        x,
        y,
        items,
        visible: true,
        selected: None,
    });
}

/// Hide and destroy the current context menu.
pub fn hide() {
    *CONTEXT_MENU.lock() = None;
}

/// Returns true if a context menu is currently visible.
pub fn is_visible() -> bool {
    if let Some(ref menu) = *CONTEXT_MENU.lock() {
        menu.visible
    } else {
        false
    }
}

/// Render the context menu onto the compositor back buffer.
pub fn render(comp: &mut Compositor) {
    let lock = CONTEXT_MENU.lock();
    let menu = match lock.as_ref() {
        Some(m) if m.visible => m,
        _ => return,
    };

    if menu.items.is_empty() {
        return;
    }

    // Compute width from longest label
    let max_label_len = menu.items.iter().map(|i| i.label.len()).max().unwrap_or(4);
    let menu_width = max_label_len * CHAR_WIDTH + PADDING_TOTAL;
    let menu_height = menu.items.len() * ITEM_HEIGHT;

    // Clamp to screen bounds
    let mx = if menu.x + menu_width > comp.width {
        comp.width.saturating_sub(menu_width)
    } else {
        menu.x
    };
    let my = if menu.y + menu_height > comp.height {
        comp.height.saturating_sub(menu_height)
    } else {
        menu.y
    };

    // Background fill
    comp.fill_rect(mx, my, menu_width, menu_height, BG_PANEL);

    // 1px border in accent cyan
    comp.draw_rect(mx, my, menu_width, menu_height, ACCENT_CYAN);

    // Draw each item
    for (i, item) in menu.items.iter().enumerate() {
        let item_y = my + i * ITEM_HEIGHT;

        // Highlight selected (hovered) item
        if menu.selected == Some(i) && item.enabled {
            let highlight = Color::rgb(35, 35, 60);
            comp.fill_rect(mx + 1, item_y, menu_width - 2, ITEM_HEIGHT, highlight);
        }

        // Text color
        let color = if item.enabled { TEXT_PRIMARY } else { TEXT_MUTED };

        // Center text vertically within the item row: (22 - 16) / 2 = 3px offset
        let text_y = item_y + 3;
        comp.draw_text(mx + PADDING_LEFT, text_y, &item.label, color);
    }
}

/// Handle a mouse click at (mx, my). Returns the action if an enabled item was clicked.
pub fn handle_click(mx: i32, my: i32) -> Option<MenuAction> {
    let lock = CONTEXT_MENU.lock();
    let menu = match lock.as_ref() {
        Some(m) if m.visible => m,
        _ => return None,
    };

    if menu.items.is_empty() {
        return None;
    }

    let max_label_len = menu.items.iter().map(|i| i.label.len()).max().unwrap_or(4);
    let menu_width = max_label_len * CHAR_WIDTH + PADDING_TOTAL;
    let menu_x = menu.x as i32;
    let menu_y = menu.y as i32;
    let menu_h = (menu.items.len() * ITEM_HEIGHT) as i32;

    // Check bounds
    if mx < menu_x || mx >= menu_x + menu_width as i32 || my < menu_y || my >= menu_y + menu_h {
        return None;
    }

    let index = ((my - menu_y) as usize) / ITEM_HEIGHT;
    if index < menu.items.len() && menu.items[index].enabled {
        Some(menu.items[index].action)
    } else {
        None
    }
}

/// Update the hover/selected index based on mouse position.
pub fn update_hover(mx: i32, my: i32) {
    let mut lock = CONTEXT_MENU.lock();
    let menu = match lock.as_mut() {
        Some(m) if m.visible => m,
        _ => return,
    };

    if menu.items.is_empty() {
        menu.selected = None;
        return;
    }

    let max_label_len = menu.items.iter().map(|i| i.label.len()).max().unwrap_or(4);
    let menu_width = (max_label_len * CHAR_WIDTH + PADDING_TOTAL) as i32;
    let menu_x = menu.x as i32;
    let menu_y = menu.y as i32;
    let menu_h = (menu.items.len() * ITEM_HEIGHT) as i32;

    if mx >= menu_x && mx < menu_x + menu_width && my >= menu_y && my < menu_y + menu_h {
        let index = ((my - menu_y) as usize) / ITEM_HEIGHT;
        if index < menu.items.len() {
            menu.selected = Some(index);
        } else {
            menu.selected = None;
        }
    } else {
        menu.selected = None;
    }
}

/// Create an enabled menu item.
pub fn menu_item(label: &str, action: MenuAction) -> MenuItem {
    MenuItem {
        label: String::from(label),
        action,
        enabled: true,
    }
}

/// Create a disabled (greyed out) menu item.
pub fn menu_item_disabled(label: &str, action: MenuAction) -> MenuItem {
    MenuItem {
        label: String::from(label),
        action,
        enabled: false,
    }
}

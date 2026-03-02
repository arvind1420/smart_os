/// Display Server for Smart OS — Phase 12.
///
/// Provides a syscall-based protocol for user-space programs to create
/// and manage GUI windows. Programs use SYS_DISPLAY_CMD to create windows,
/// draw text/rectangles, and SYS_DISPLAY_EVENT to receive input events.

use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::format;
use spin::Mutex;
use crate::serial_println;
use super::window::{Window, WindowId};
use super::theme::Color;

// ═══════════════════════════════════════════════════════════════
//  Display command numbers (arg0 to SYS_DISPLAY_CMD)
// ═══════════════════════════════════════════════════════════════

pub const CMD_CREATE_WINDOW: u64 = 0;
pub const CMD_DESTROY_WINDOW: u64 = 1;
pub const CMD_SET_TITLE: u64 = 2;
pub const CMD_DRAW_TEXT: u64 = 3;
pub const CMD_FILL_RECT: u64 = 4;
pub const CMD_CLEAR: u64 = 5;
pub const CMD_REDRAW: u64 = 6;
pub const CMD_ADD_WIDGET: u64 = 7;
pub const CMD_HUB_PUBLISH: u64 = 8;
pub const CMD_HUB_QUERY: u64 = 9;

// ═══════════════════════════════════════════════════════════════
//  Display event types
// ═══════════════════════════════════════════════════════════════

pub const EVENT_KEY_PRESS: u8 = 1;
pub const EVENT_MOUSE_CLICK: u8 = 2;
pub const EVENT_WINDOW_CLOSE: u8 = 3;

/// A display event delivered to user-space.
#[derive(Debug, Clone, Copy)]
pub struct DisplayEvent {
    pub event_type: u8,
    pub window_id: u32,
    pub data: [u32; 4],
}

impl DisplayEvent {
    /// Serialize to bytes for writing to user buffer (20 bytes).
    pub fn to_bytes(&self) -> [u8; 20] {
        let mut buf = [0u8; 20];
        buf[0] = self.event_type;
        buf[1] = 0; // padding
        buf[2..4].copy_from_slice(&[0, 0]); // padding
        buf[4..8].copy_from_slice(&self.window_id.to_le_bytes());
        buf[8..12].copy_from_slice(&self.data[0].to_le_bytes());
        buf[12..16].copy_from_slice(&self.data[1].to_le_bytes());
        buf[16..20].copy_from_slice(&self.data[2].to_le_bytes());
        buf
    }
}

// ═══════════════════════════════════════════════════════════════
//  Display server state
// ═══════════════════════════════════════════════════════════════

/// Info about a display-server-managed window.
struct DisplayWindowInfo {
    owner_pid: u64,
}

/// The display server.
struct DisplayServer {
    /// Map from window_id to info.
    windows: BTreeMap<WindowId, DisplayWindowInfo>,
    /// Per-process event queues.
    event_queues: BTreeMap<u64, VecDeque<DisplayEvent>>,
    /// Track next window position for staggering.
    next_x: usize,
    next_y: usize,
}

impl DisplayServer {
    fn new() -> Self {
        Self {
            windows: BTreeMap::new(),
            event_queues: BTreeMap::new(),
            next_x: 100,
            next_y: 100,
        }
    }
}

static DISPLAY_SERVER: Mutex<Option<DisplayServer>> = Mutex::new(None);

/// Initialize the display server.
pub fn init() {
    *DISPLAY_SERVER.lock() = Some(DisplayServer::new());
    serial_println!("[display_server] Display server initialized.");
}

// ═══════════════════════════════════════════════════════════════
//  Command handling
// ═══════════════════════════════════════════════════════════════

/// Handle a display command from user-space.
/// args layout varies by cmd (see plan).
/// Returns a result value (e.g., window_id for create, 0 for success, u64::MAX for error).
pub fn handle_cmd(pid: u64, cmd: u64, arg1: u64, arg2: u64, arg3: u64, _arg4: u64) -> u64 {
    match cmd {
        CMD_CREATE_WINDOW => create_window(pid, arg1 as usize, arg2 as usize),
        CMD_DESTROY_WINDOW => destroy_window(pid, arg1 as WindowId),
        CMD_SET_TITLE => set_title(pid, arg1 as WindowId, arg2, arg3 as usize),
        CMD_DRAW_TEXT => draw_text(pid, arg1 as WindowId, arg2, arg3, _arg4),
        CMD_FILL_RECT => fill_rect(pid, arg1 as WindowId, arg2, arg3, _arg4),
        CMD_CLEAR => clear_window(pid, arg1 as WindowId),
        CMD_ADD_WIDGET => add_widget(pid, arg1 as WindowId, arg2, arg3, _arg4),
        CMD_HUB_PUBLISH => hub_publish(pid, arg1, arg2, arg3),
        CMD_HUB_QUERY => hub_query(pid, arg1, arg2, arg3),
        CMD_REDRAW => { /* redraw happens automatically in render loop */ 0 }
        _ => u64::MAX,
    }
}

fn create_window(pid: u64, width: usize, height: usize) -> u64 {
    let w = if width == 0 { 300 } else { width.min(800) };
    let h = if height == 0 { 200 } else { height.min(600) };

    let mut ds = DISPLAY_SERVER.lock();
    let ds = match ds.as_mut() {
        Some(d) => d,
        None => return u64::MAX,
    };

    // Stagger window positions
    let x = ds.next_x;
    let y = ds.next_y;
    ds.next_x = (ds.next_x + 30) % 400 + 50;
    ds.next_y = (ds.next_y + 30) % 300 + 50;

    let accent = Color { r: 0, g: 200, b: 150 }; // teal accent
    let window = Window::new("User App", x, y, w, h, accent);
    let wid = window.id;

    // Add to desktop window manager
    let mut desktop = super::desktop::DESKTOP.lock();
    if let Some(d) = desktop.as_mut() {
        d.wm.add(window);
    }
    drop(desktop);

    // Track ownership
    ds.windows.insert(wid, DisplayWindowInfo {
        owner_pid: pid,
    });

    // Ensure event queue exists for this process
    if !ds.event_queues.contains_key(&pid) {
        ds.event_queues.insert(pid, VecDeque::new());
    }

    serial_println!("[display_server] pid {} created window {} ({}x{})", pid, wid, w, h);
    wid as u64
}

fn destroy_window(pid: u64, wid: WindowId) -> u64 {
    let mut ds = DISPLAY_SERVER.lock();
    let ds = match ds.as_mut() {
        Some(d) => d,
        None => return u64::MAX,
    };

    // Verify ownership
    if let Some(info) = ds.windows.get(&wid) {
        if info.owner_pid != pid {
            return u64::MAX; // not owner
        }
    } else {
        return u64::MAX; // not a display-server window
    }

    ds.windows.remove(&wid);

    // Remove from desktop WM
    let mut desktop = super::desktop::DESKTOP.lock();
    if let Some(d) = desktop.as_mut() {
        d.wm.windows.retain(|w| w.id != wid);
    }

    serial_println!("[display_server] pid {} destroyed window {}", pid, wid);
    0
}

fn set_title(pid: u64, wid: WindowId, title_ptr: u64, title_len: usize) -> u64 {
    // Verify ownership
    {
        let ds = DISPLAY_SERVER.lock();
        let ds = match ds.as_ref() {
            Some(d) => d,
            None => return u64::MAX,
        };
        match ds.windows.get(&wid) {
            Some(info) if info.owner_pid == pid => {}
            _ => return u64::MAX,
        }
    }

    // Read title from user memory
    let title_len = title_len.min(64);
    let title = unsafe {
        let ptr = title_ptr as *const u8;
        if ptr.is_null() || title_ptr >= 0x8000_0000_0000 {
            return u64::MAX;
        }
        let slice = core::slice::from_raw_parts(ptr, title_len);
        core::str::from_utf8(slice).unwrap_or("User App")
    };

    let mut desktop = super::desktop::DESKTOP.lock();
    if let Some(d) = desktop.as_mut() {
        for w in d.wm.windows.iter_mut() {
            if w.id == wid {
                w.title = String::from(title);
                break;
            }
        }
    }
    0
}

fn draw_text(pid: u64, wid: WindowId, xy_packed: u64, text_ptr: u64, text_len_64: u64) -> u64 {
    let x = (xy_packed >> 16) as usize & 0xFFFF;
    let y = (xy_packed & 0xFFFF) as usize;
    let text_len = (text_len_64 as usize).min(256);

    // Verify ownership
    {
        let ds = DISPLAY_SERVER.lock();
        match ds.as_ref().and_then(|d| d.windows.get(&wid)) {
            Some(info) if info.owner_pid == pid => {}
            _ => return u64::MAX,
        }
    }

    // Read text from user memory
    let text = unsafe {
        let ptr = text_ptr as *const u8;
        if ptr.is_null() || text_ptr >= 0x8000_0000_0000 {
            return u64::MAX;
        }
        let slice = core::slice::from_raw_parts(ptr, text_len);
        core::str::from_utf8(slice).unwrap_or("")
    };

    // Draw text into the window's content_lines at the given position.
    // We use content_lines as a simple text buffer — each draw_text appends a line.
    let line = format!("@{}:{} {}", x, y, text);
    let mut desktop = super::desktop::DESKTOP.lock();
    if let Some(d) = desktop.as_mut() {
        for w in d.wm.windows.iter_mut() {
            if w.id == wid {
                w.content_lines.push(line);
                break;
            }
        }
    }
    0
}

fn fill_rect(pid: u64, wid: WindowId, xy_packed: u64, wh_packed: u64, color: u64) -> u64 {
    let _x = (xy_packed >> 16) as usize & 0xFFFF;
    let _y = (xy_packed & 0xFFFF) as usize;
    let _w = (wh_packed >> 16) as usize & 0xFFFF;
    let _h = (wh_packed & 0xFFFF) as usize;
    let _r = ((color >> 16) & 0xFF) as u8;
    let _g = ((color >> 8) & 0xFF) as u8;
    let _b = (color & 0xFF) as u8;

    // Verify ownership
    {
        let ds = DISPLAY_SERVER.lock();
        match ds.as_ref().and_then(|d| d.windows.get(&wid)) {
            Some(info) if info.owner_pid == pid => {}
            _ => return u64::MAX,
        }
    }

    // For now, fill_rect is tracked as a content_line marker.
    // The actual pixel drawing happens in the compositor during render.
    let line = format!("!rect {}:{} {}x{} #{:02x}{:02x}{:02x}", _x, _y, _w, _h, _r, _g, _b);
    let mut desktop = super::desktop::DESKTOP.lock();
    if let Some(d) = desktop.as_mut() {
        for w in d.wm.windows.iter_mut() {
            if w.id == wid {
                w.content_lines.push(line);
                break;
            }
        }
    }
    0
}

fn clear_window(_pid: u64, wid: WindowId) -> u64 {
    let mut desktop = super::desktop::DESKTOP.lock();
    if let Some(d) = desktop.as_mut() {
        for w in d.wm.windows.iter_mut() {
            if w.id == wid {
                w.content_lines.clear();
                break;
            }
        }
    }
    0
}

fn add_widget(pid: u64, wid: WindowId, kind: u64, xy_packed: u64, wh_packed: u64) -> u64 {
    // Verify ownership
    {
        let ds = DISPLAY_SERVER.lock();
        match ds.as_ref().and_then(|d| d.windows.get(&wid)) {
            Some(info) if info.owner_pid == pid => {}
            _ => return u64::MAX,
        }
    }

    let x = (xy_packed >> 16) as usize & 0xFFFF;
    let y = (xy_packed & 0xFFFF) as usize;
    let w = (wh_packed >> 16) as usize & 0xFFFF;
    let h = (wh_packed & 0xFFFF) as usize;

    let mut desktop = super::desktop::DESKTOP.lock();
    if let Some(d) = desktop.as_mut() {
        if let Some(win) = d.wm.get_mut(wid) {
            use super::widget::{Widget, WidgetKind, Button, AppCommand};
            win.use_widgets = true;
            let id = win.widgets.len() as u8;
            
            let widget_kind = match kind {
                0 => WidgetKind::Button(Button::new("Button", win.accent, AppCommand::ButtonClicked(id))),
                _ => return u64::MAX,
            };

            win.widgets.push(Widget::new(id, x, y, w, h, widget_kind));
            win.dirty = true;
            return id as u64;
        }
    }
    u64::MAX
}

fn hub_publish(pid: u64, topic_ptr: u64, topic_len: u64, payload_ptr: u64) -> u64 {
    let topic = unsafe {
        let ptr = topic_ptr as *const u8;
        if ptr.is_null() || topic_ptr >= 0x8000_0000_0000 { return u64::MAX; }
        let slice = core::slice::from_raw_parts(ptr, topic_len as usize);
        core::str::from_utf8(slice).unwrap_or("unknown")
    };
    // Simplified payload handling
    super::ipc::publish(pid, topic, smartpack::Value::UInt64(payload_ptr));
    0
}

fn hub_query(_pid: u64, topic_ptr: u64, topic_len: u64, _out_ptr: u64) -> u64 {
    let topic = unsafe {
        let ptr = topic_ptr as *const u8;
        if ptr.is_null() || topic_ptr >= 0x8000_0000_0000 { return u64::MAX; }
        let slice = core::slice::from_raw_parts(ptr, topic_len as usize);
        core::str::from_utf8(slice).unwrap_or("unknown")
    };
    let results = super::ipc::query(topic);
    results.len() as u64
}

// ═══════════════════════════════════════════════════════════════
//  Event delivery
// ═══════════════════════════════════════════════════════════════

/// Poll for a pending display event for a process.
/// Returns Some(event) if one is queued, None otherwise.
pub fn poll_event(pid: u64) -> Option<DisplayEvent> {
    let mut ds = DISPLAY_SERVER.lock();
    let ds = ds.as_mut()?;
    ds.event_queues.get_mut(&pid)?.pop_front()
}

/// Route a key event to the display server (called from input.rs).
pub fn route_key_event(wid: WindowId, ascii: u8, scancode: u8) {
    let mut ds = DISPLAY_SERVER.lock();
    let ds = match ds.as_mut() {
        Some(d) => d,
        None => return,
    };
    if let Some(info) = ds.windows.get(&wid) {
        let pid = info.owner_pid;
        let event = DisplayEvent {
            event_type: EVENT_KEY_PRESS,
            window_id: wid as u32,
            data: [ascii as u32, scancode as u32, 0, 0],
        };
        ds.event_queues.entry(pid).or_insert_with(VecDeque::new).push_back(event);
    }
}

/// Route a mouse click event to the display server.
pub fn route_mouse_click(wid: WindowId, x: u16, y: u16, button: u8) {
    let mut ds_guard = DISPLAY_SERVER.lock();
    let ds = match ds_guard.as_mut() {
        Some(d) => d,
        None => return,
    };
    if let Some(info) = ds.windows.get(&wid) {
        let pid = info.owner_pid;
        
        // Check if a widget was clicked
        let mut widget_id = 0xFFu32;
        let mut desktop = super::desktop::DESKTOP.lock();
        if let Some(d) = desktop.as_mut() {
            if let Some(win) = d.wm.get_mut(wid) {
                // Adjust coordinates relative to content area
                // (Simplified: assuming x,y already relative to content)
                for widget in &win.widgets {
                    if widget.contains(x as usize, y as usize) {
                        widget_id = widget.id as u32;
                        break;
                    }
                }
            }
        }
        drop(desktop);

        let event = DisplayEvent {
            event_type: EVENT_MOUSE_CLICK,
            window_id: wid as u32,
            data: [x as u32, y as u32, widget_id, button as u32],
        };
        ds.event_queues.entry(pid).or_insert_with(VecDeque::new).push_back(event);
    }
}

/// Route a window close event (user clicked X button).
pub fn route_close_event(wid: WindowId) {
    let mut ds = DISPLAY_SERVER.lock();
    let ds = match ds.as_mut() {
        Some(d) => d,
        None => return,
    };
    if let Some(info) = ds.windows.get(&wid) {
        let pid = info.owner_pid;
        let event = DisplayEvent {
            event_type: EVENT_WINDOW_CLOSE,
            window_id: wid as u32,
            data: [0; 4],
        };
        ds.event_queues.entry(pid).or_insert_with(VecDeque::new).push_back(event);
    }
}

/// Check if a window is owned by the display server.
pub fn is_display_window(wid: WindowId) -> bool {
    let ds = DISPLAY_SERVER.lock();
    ds.as_ref().map(|d| d.windows.contains_key(&wid)).unwrap_or(false)
}

// ═══════════════════════════════════════════════════════════════
//  Cleanup and status
// ═══════════════════════════════════════════════════════════════

/// Clean up all windows owned by a process (called from exit_process_full).
pub fn cleanup_process(pid: u64) {
    let mut ds = DISPLAY_SERVER.lock();
    let ds = match ds.as_mut() {
        Some(d) => d,
        None => return,
    };

    // Collect window IDs owned by this process
    let wids: alloc::vec::Vec<WindowId> = ds.windows.iter()
        .filter(|(_, info)| info.owner_pid == pid)
        .map(|(&wid, _)| wid)
        .collect();

    for wid in &wids {
        ds.windows.remove(wid);
    }

    // Remove from desktop WM
    if !wids.is_empty() {
        let mut desktop = super::desktop::DESKTOP.lock();
        if let Some(d) = desktop.as_mut() {
            d.wm.windows.retain(|w| !wids.contains(&w.id));
        }
    }

    // Clean up event queue
    ds.event_queues.remove(&pid);
}

/// Get display server status: (window_count, total_queued_events).
pub fn status() -> (usize, usize) {
    let ds = DISPLAY_SERVER.lock();
    match ds.as_ref() {
        Some(d) => {
            let events: usize = d.event_queues.values().map(|q| q.len()).sum();
            (d.windows.len(), events)
        }
        None => (0, 0),
    }
}

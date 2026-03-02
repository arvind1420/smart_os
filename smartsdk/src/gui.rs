use super::syscall::*;

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

pub const WIDGET_BUTTON: u64 = 0;

pub const EVENT_KEY_PRESS: u8 = 1;
pub const EVENT_MOUSE_CLICK: u8 = 2;
pub const EVENT_WINDOW_CLOSE: u8 = 3;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct GuiEvent {
    pub event_type: u8,
    _pad: u8,
    _pad2: u16,
    pub window_id: u32,
    pub data: [u32; 4],
}

pub struct Window {
    id: u64,
}

impl Window {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let id = syscall3(SYS_DISPLAY_CMD, CMD_CREATE_WINDOW, width as u64, height as u64);
        if id == u64::MAX { None } else { Some(Self { id }) }
    }

    pub fn draw_text(&self, x: u16, y: u16, text: &str) {
        let xy_packed = ((x as u64) << 16) | (y as u64);
        syscall5(SYS_DISPLAY_CMD, CMD_DRAW_TEXT, self.id, xy_packed, text.as_ptr() as u64, text.len() as u64);
    }

    pub fn fill_rect(&self, x: u16, y: u16, w: u16, h: u16, color: u32) {
        let xy_packed = ((x as u64) << 16) | (y as u64);
        let wh_packed = ((w as u64) << 16) | (h as u64);
        syscall5(SYS_DISPLAY_CMD, CMD_FILL_RECT, self.id, xy_packed, wh_packed, color as u64);
    }

    pub fn clear(&self) {
        syscall2(SYS_DISPLAY_CMD, CMD_CLEAR, self.id);
    }

    pub fn add_button(&self, x: u16, y: u16, w: u16, h: u16) -> u8 {
        let xy_packed = ((x as u64) << 16) | (y as u64);
        let wh_packed = ((w as u64) << 16) | (h as u64);
        syscall5(SYS_DISPLAY_CMD, CMD_ADD_WIDGET, self.id, WIDGET_BUTTON, xy_packed, wh_packed) as u8
    }

    pub fn hub_publish(&self, topic: &str, data_ptr: u64) {
        syscall4(SYS_DISPLAY_CMD, CMD_HUB_PUBLISH, topic.as_ptr() as u64, topic.len() as u64, data_ptr);
    }

    pub fn hub_query(&self, topic: &str) -> u64 {
        syscall4(SYS_DISPLAY_CMD, CMD_HUB_QUERY, topic.as_ptr() as u64, topic.len() as u64, 0)
    }

    pub fn poll_event() -> Option<GuiEvent> {
        let mut event = GuiEvent {
            event_type: 0,
            _pad: 0,
            _pad2: 0,
            window_id: 0,
            data: [0; 4],
        };
        let ret = syscall1(SYS_DISPLAY_EVENT, &mut event as *mut GuiEvent as u64);
        if ret == 1 {
            Some(event)
        } else {
            None
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        syscall2(SYS_DISPLAY_CMD, CMD_DESTROY_WINDOW, self.id);
    }
}

use super::syscall::*;

const CMD_CREATE_WINDOW: u64 = 0;
const CMD_DRAW_TEXT: u64 = 3;

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
        syscall4(SYS_DISPLAY_CMD, CMD_DRAW_TEXT, self.id, xy_packed, text.as_ptr() as u64);
    }
}

/// Smart OS GUI — Futuristic Desktop Environment
///
/// A software-rendered compositor with a cyberpunk neon dark theme.
/// Phase 2: windows, taskbar, system info
/// Phase 3: mouse cursor, window dragging, animation, input handling
/// Phase 4: interactive widgets, keyboard routing, application framework
/// Phase 9: clipboard, toast notifications

pub mod font;
pub mod font_large;
pub mod theme;
pub mod compositor;
pub mod bidi;
pub mod ime;
pub mod window;
pub mod widgets;
pub mod desktop;
pub mod mouse_cursor;
pub mod animation;
pub mod input;
pub mod widget;
pub mod clipboard;
pub mod notification;
pub mod context_menu;
pub mod alt_tab;
pub mod wm2;
pub mod virtual_desktop;
pub mod theming;
pub mod display_server;
pub mod ipc;
pub mod xr;
pub mod wayland;
pub mod holographic;
pub mod accessibility;

/// Initialize the GUI subsystem.
pub fn init(fb_addr: *mut u8, width: usize, height: usize, stride: usize, bpp: usize, is_bgr: bool) {
    compositor::init(fb_addr, width, height, stride, bpp, is_bgr);
    desktop::init();
    xr::init();
    wayland::init();
    holographic::init();
    accessibility::init();
    crate::serial_println!("[gui] GUI compositor initialized ({}x{}).", width, height);
}

/// Render one frame of the desktop.
pub fn render_frame() {
    desktop::render();
    compositor::flip();
}

/// Handle a mouse event from the driver layer (called by mouse plugin).
pub fn handle_mouse_event(event: crate::drivers::mouse::MouseEvent) {
    input::handle_mouse_event(event);
}

/// Handle a keyboard event from the driver layer (called by keyboard plugin).
pub fn handle_key_event(event: crate::drivers::keyboard::KeyEvent) {
    input::handle_key_event(event);
}

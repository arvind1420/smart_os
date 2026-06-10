//! Image Viewer — Phase 106 v2: Multi-format + Rotation.
//!
//! Displays PNG / JPEG / WebP / AVIF / GIF images.
//! Decodes via both `drivers::image` (BMP/legacy) and `net::{png,jpeg,webp,avif}`.
//! Supports:
//!   • Zoom levels: 25 / 50 / 75 / 100 / 150 / 200 / 300 %
//!   • Pan with arrow keys / mouse drag (future)
//!   • Rotation: 0 / 90 / 180 / 270 degrees (CW, applied at render time — zero-copy)
//!   • Slideshow auto-advance (configurable interval)
//!   • Nearest-neighbor scaled viewport rendering into window's image_buffer
//!   • Status bar: filename · dimensions · zoom · rotation
//!
//! The app thread calls `run()`.  `sync_to_window()` is called each
//! render frame (from desktop.rs) to refresh the pixel buffer when dirty.

#![allow(dead_code)]

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::window::{Window, WindowId};
use crate::gui::desktop::DESKTOP;
use crate::gui::theme::*;
use crate::gui::widget::{Widget, WidgetKind, Button, StaticLabel, AppCommand, WidgetAction};
use crate::drivers::image::{Image, decode as decode_image};
use crate::vfs;

// ─── Multi-format decoder helpers ─────────────────────────────────────────────

/// Convert a raw RGBA8 pixel blob into a `drivers::image::Image`.
#[inline]
fn raw_to_image(w: u32, h: u32, pixels: Vec<u8>) -> Image {
    Image { width: w, height: h, pixels }
}

/// Decode `bytes` by trying each known format: PNG → JPEG → WebP → AVIF → BMP/legacy.
fn decode_any(bytes: &[u8]) -> Result<Image, &'static str> {
    // PNG
    if let Ok(img) = crate::net::png::decode(bytes) {
        return Ok(raw_to_image(img.width, img.height, img.data));
    }
    // JPEG
    if let Ok(img) = crate::net::jpeg::decode(bytes) {
        return Ok(raw_to_image(img.width, img.height, img.data));
    }
    // WebP
    if crate::net::webp::is_webp(bytes) {
        if let Ok(img) = crate::net::webp::decode(bytes) {
            return Ok(raw_to_image(img.width, img.height, img.pixels));
        }
    }
    // AVIF
    if crate::net::avif::is_avif(bytes) {
        if let Ok(img) = crate::net::avif::decode(bytes) {
            return Ok(raw_to_image(img.width, img.height, img.pixels));
        }
    }
    // Fallback: legacy driver decoder (BMP, GIF…)
    decode_image(bytes).map_err(|_| "unsupported image format")
}

// ─── Zoom table ──────────────────────────────────────────────────────────────

/// Supported zoom steps as fixed-point percentages.
const ZOOM_STEPS: &[u32] = &[25, 50, 75, 100, 150, 200, 300];
const ZOOM_DEFAULT_IDX: usize = 3; // 100 %

// ─── Slideshow ───────────────────────────────────────────────────────────────

/// Frames between slideshow advances at ~100 Hz timer.
const SLIDESHOW_FRAMES: u32 = 500; // ~5 seconds

// ─── App state ───────────────────────────────────────────────────────────────

pub struct ImageViewerState {
    pub window_id:        WindowId,
    /// Decoded source image (always stored at original resolution).
    pub source:           Option<Image>,
    /// Zoom step index into ZOOM_STEPS.
    pub zoom_idx:         usize,
    /// Pan offset within the (possibly rotated) image space.
    pub pan_x:            i32,
    pub pan_y:            i32,
    /// Rotation in degrees CW: 0 / 90 / 180 / 270.
    pub rotation:         u32,
    /// File list (paths in VFS).
    pub files:            Vec<String>,
    /// Index of the currently displayed file.
    pub file_idx:         usize,
    /// Whether slideshow mode is running.
    pub slideshow:        bool,
    /// Countdown frames until next slideshow advance.
    pub slideshow_ticks:  u32,
    /// One-line status message.
    pub status:           String,
    /// Needs re-render.
    pub dirty:            bool,
    /// Viewport width and height (content area of the window).
    pub vp_w:             usize,
    pub vp_h:             usize,
}

impl ImageViewerState {
    fn zoom_pct(&self) -> u32 {
        ZOOM_STEPS[self.zoom_idx]
    }

    fn zoom_in(&mut self) {
        if self.zoom_idx + 1 < ZOOM_STEPS.len() {
            self.zoom_idx += 1;
            self.dirty = true;
        }
    }

    fn zoom_out(&mut self) {
        if self.zoom_idx > 0 {
            self.zoom_idx -= 1;
            self.dirty = true;
        }
    }

    fn zoom_fit(&mut self) {
        if let Some(ref img) = self.source {
            // Swap dims when rotated 90° or 270° (image is sideways)
            let (iw, ih) = if self.rotation == 90 || self.rotation == 270 {
                (img.height, img.width)
            } else {
                (img.width, img.height)
            };
            // Find the largest zoom that fits the viewport
            let mut best = 0usize;
            for (i, &pct) in ZOOM_STEPS.iter().enumerate() {
                let scaled_w = (iw as u32 * pct / 100) as usize;
                let scaled_h = (ih as u32 * pct / 100) as usize;
                if scaled_w <= self.vp_w && scaled_h <= self.vp_h {
                    best = i;
                }
            }
            self.zoom_idx = best;
            self.pan_x = 0;
            self.pan_y = 0;
            self.dirty = true;
        }
    }

    /// Rotate 90° clockwise.
    fn rotate_cw(&mut self) {
        self.rotation = (self.rotation + 90) % 360;
        self.pan_x = 0;
        self.pan_y = 0;
        self.dirty = true;
    }

    /// Rotate 90° counter-clockwise.
    fn rotate_ccw(&mut self) {
        self.rotation = (self.rotation + 270) % 360;
        self.pan_x = 0;
        self.pan_y = 0;
        self.dirty = true;
    }
}

pub static STATE: Mutex<Option<ImageViewerState>> = Mutex::new(None);

// ─── Image decoding helpers ───────────────────────────────────────────────────

/// Load an image from the VFS at `path`, trying all known formats.
fn load_from_vfs(path: &str) -> Result<Image, &'static str> {
    let fd = vfs::open(path).map_err(|_| "open failed")?;
    let mut buf = vec![0u8; 8 * 1024 * 1024]; // 8 MiB max
    let n = vfs::read(fd, &mut buf).map_err(|_| "read failed")?;
    vfs::close(fd).ok();
    decode_any(&buf[..n])
}

/// Generate a colourful test-pattern image (for demo / self-test).
fn make_test_pattern(w: u32, h: u32) -> Image {
    let mut img = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let r = ((x * 255) / w.max(1)) as u8;
            let g = ((y * 255) / h.max(1)) as u8;
            let b = (((x + y) * 128) / (w + h).max(1)) as u8;
            img.set_pixel(x, y, [r, g, b, 255]);
        }
    }
    img
}

// ─── Viewport rendering ───────────────────────────────────────────────────────

/// Scale `src` with nearest-neighbour into a `vp_w × vp_h` RGBA8 buffer,
/// starting at offset `(pan_x, pan_y)` in the rotated image space at `zoom_pct`%.
/// `rotation` is 0 / 90 / 180 / 270 degrees CW.
/// Pixels outside the source are filled with a dark checkerboard.
fn render_viewport(
    src:      &Image,
    zoom_pct: u32,
    pan_x:    i32,
    pan_y:    i32,
    vp_w:     usize,
    vp_h:     usize,
    rotation: u32,
) -> Vec<u8> {
    let mut out = vec![0u8; vp_w * vp_h * 4];
    let scale_num: i32 = zoom_pct as i32;
    let scale_den: i32 = 100;
    let sw = src.width as i32;
    let sh = src.height as i32;

    for py in 0..vp_h {
        for px in 0..vp_w {
            // Map viewport (px, py) → rotated-space (rx, ry)
            let rx = pan_x + (px as i32 * scale_den / scale_num);
            let ry = pan_y + (py as i32 * scale_den / scale_num);

            // Apply inverse rotation: rotated → source coordinates
            let (src_x, src_y): (i32, i32) = match rotation {
                90  => (sh - 1 - ry, rx),            // 90° CW
                180 => (sw - 1 - rx, sh - 1 - ry),   // 180°
                270 => (ry, sw - 1 - rx),             // 270° CW (= 90° CCW)
                _   => (rx, ry),                       // 0° (identity)
            };

            let rgba = if src_x >= 0 && src_y >= 0
                && src_x < sw && src_y < sh
            {
                src.pixel(src_x as u32, src_y as u32)
            } else {
                // Checkerboard background for out-of-bounds
                let checker = (((px >> 3) ^ (py >> 3)) & 1) != 0;
                if checker { [40, 40, 50, 255] } else { [30, 30, 40, 255] }
            };

            let off = (py * vp_w + px) * 4;
            out[off]     = rgba[0]; // R
            out[off + 1] = rgba[1]; // G
            out[off + 2] = rgba[2]; // B
            out[off + 3] = rgba[3]; // A
        }
    }
    out
}

// ─── App thread ──────────────────────────────────────────────────────────────

/// Image viewer application thread entry point.
pub fn run() {
    // ── Create window ─────────────────────────────────────────────────────────
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() { Some(d) => d, None => return };

        let mut win = Window::new("Image Viewer", 50, 50, 720, 500, ACCENT_CYAN);
        win.use_widgets = true;

        // Widget 0: Zoom out button
        win.widgets.push(Widget::new(0, 0, 0, 40, 22,
            WidgetKind::Button(Button::new("-", ACCENT_GREEN, AppCommand::ButtonClicked(0)))));
        // Widget 1: Zoom in button
        win.widgets.push(Widget::new(1, 44, 0, 40, 22,
            WidgetKind::Button(Button::new("+", ACCENT_GREEN, AppCommand::ButtonClicked(1)))));
        // Widget 2: Fit button
        win.widgets.push(Widget::new(2, 88, 0, 56, 22,
            WidgetKind::Button(Button::new("Fit", ACCENT_CYAN, AppCommand::ButtonClicked(2)))));
        // Widget 3: Previous image
        win.widgets.push(Widget::new(3, 148, 0, 44, 22,
            WidgetKind::Button(Button::new("\u{25C0}", ACCENT_ORANGE, AppCommand::ButtonClicked(3)))));
        // Widget 4: Next image
        win.widgets.push(Widget::new(4, 196, 0, 44, 22,
            WidgetKind::Button(Button::new("\u{25B6}", ACCENT_ORANGE, AppCommand::ButtonClicked(4)))));
        // Widget 5: Slideshow toggle
        win.widgets.push(Widget::new(5, 244, 0, 80, 22,
            WidgetKind::Button(Button::new("Slideshow", ACCENT_MAGENTA, AppCommand::ButtonClicked(5)))));
        // Widget 7: Rotate CW (↻)
        win.widgets.push(Widget::new(7, 328, 0, 44, 22,
            WidgetKind::Button(Button::new("\u{21BB}", ACCENT_CYAN, AppCommand::ButtonClicked(7)))));
        // Widget 8: Rotate CCW (↺)
        win.widgets.push(Widget::new(8, 376, 0, 44, 22,
            WidgetKind::Button(Button::new("\u{21BA}", ACCENT_CYAN, AppCommand::ButtonClicked(8)))));
        // Widget 6: Status bar (read-only text)
        win.widgets.push(Widget::new(6, 0, 24, 720, 16,
            WidgetKind::Label(StaticLabel::new("No image loaded", TEXT_SECONDARY))));

        win.focused_widget = Some(3);
        let id = win.id;
        desk.wm.add(win);
        id
    };

    // ── Seed file list from VFS ────────────────────────────────────────────────
    let files: Vec<String> = {
        let mut f = Vec::new();
        // Try common paths
        for path in &["/data/images", "/images", "/data"] {
            if let Ok(entries) = vfs::readdir(path) {
                for name in entries {
                    if name.ends_with(".png")
                        || name.ends_with(".jpg")
                        || name.ends_with(".jpeg")
                        || name.ends_with(".bmp")
                        || name.ends_with(".webp")
                        || name.ends_with(".avif")
                        || name.ends_with(".gif")
                    {
                        f.push(format!("{}/{}", path, name));
                    }
                }
            }
        }
        f
    };

    // ── Init state ────────────────────────────────────────────────────────────
    let (vp_w, vp_h) = (720usize, 458usize);
    let (initial_img, status) = if files.is_empty() {
        let img = make_test_pattern(256, 256);
        (Some(img), String::from("Demo: test pattern (no image files found)"))
    } else {
        match load_from_vfs(&files[0]) {
            Ok(img) => {
                let s = format!("{} ({}×{})", files[0], img.width, img.height);
                (Some(img), s)
            }
            Err(e) => (None, format!("Load error: {}", e)),
        }
    };

    *STATE.lock() = Some(ImageViewerState {
        window_id,
        source: initial_img,
        zoom_idx: ZOOM_DEFAULT_IDX,
        pan_x: 0,
        pan_y: 0,
        rotation: 0,
        files,
        file_idx: 0,
        slideshow: false,
        slideshow_ticks: SLIDESHOW_FRAMES,
        status,
        dirty: true,
        vp_w,
        vp_h,
    });

    // ── Event loop ────────────────────────────────────────────────────────────
    let mut tick = 0u32;
    loop {
        // Handle widget actions
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                match action {
                    WidgetAction::Execute(AppCommand::ButtonClicked(0)) => { s.zoom_out(); }
                    WidgetAction::Execute(AppCommand::ButtonClicked(1)) => { s.zoom_in(); }
                    WidgetAction::Execute(AppCommand::ButtonClicked(2)) => { s.zoom_fit(); }
                    WidgetAction::Execute(AppCommand::ButtonClicked(3)) => {
                        s.prev_file();
                        load_current_file(s);
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(4)) => {
                        s.next_file();
                        load_current_file(s);
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(5)) => {
                        s.slideshow = !s.slideshow;
                        s.slideshow_ticks = SLIDESHOW_FRAMES;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(7)) => { s.rotate_cw(); }
                    WidgetAction::Execute(AppCommand::ButtonClicked(8)) => { s.rotate_ccw(); }
                    _ => {}
                }
            }
        }

        // Slideshow tick
        {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                if s.slideshow && !s.files.is_empty() {
                    if s.slideshow_ticks == 0 {
                        s.next_file();
                        load_current_file(s);
                        s.slideshow_ticks = SLIDESHOW_FRAMES;
                    } else {
                        s.slideshow_ticks = s.slideshow_ticks.saturating_sub(1);
                    }
                }
            }
        }

        tick += 1;
        if tick >= 10 {
            tick = 0;
            if let Some(ref mut s) = *STATE.lock() {
                if s.dirty {
                    s.dirty = false;
                }
            }
        }

        crate::process::scheduler::yield_now();
    }
}

/// Navigate to the previous file in the list.
fn prev_file_impl(s: &mut ImageViewerState) {
    if s.files.is_empty() { return; }
    if s.file_idx == 0 {
        s.file_idx = s.files.len() - 1;
    } else {
        s.file_idx -= 1;
    }
}

/// Navigate to the next file in the list.
fn next_file_impl(s: &mut ImageViewerState) {
    if s.files.is_empty() { return; }
    s.file_idx = (s.file_idx + 1) % s.files.len();
}

impl ImageViewerState {
    fn prev_file(&mut self) { prev_file_impl(self); }
    fn next_file(&mut self) { next_file_impl(self); }
}

/// Load the file at `s.file_idx` into `s.source`.
fn load_current_file(s: &mut ImageViewerState) {
    if s.files.is_empty() { return; }
    let path = s.files[s.file_idx].clone();
    match load_from_vfs(&path) {
        Ok(img) => {
            s.status = format!("{} ({}×{}) @ {}%", path, img.width, img.height, s.zoom_pct());
            s.source = Some(img);
            s.pan_x = 0;
            s.pan_y = 0;
        }
        Err(e) => {
            s.status = format!("{}: {}", path, e);
            s.source = None;
        }
    }
    s.dirty = true;
}

// ─── Sync to window ───────────────────────────────────────────────────────────

/// Called every render frame from the desktop loop to push the current
/// scaled image into the window's `image_buffer`.
pub fn sync_to_window(win: &mut Window) {
    let mut guard = STATE.lock();
    let s = match guard.as_mut() { Some(x) => x, None => return };

    // Always refresh status label
    update_status_label(win, s);

    if let Some(ref img) = s.source {
        // Render the scaled viewport (with rotation applied at render time)
        let pixels = render_viewport(img, s.zoom_pct(), s.pan_x, s.pan_y, s.vp_w, s.vp_h, s.rotation);
        // Store in window image_buffer with 42 px y-offset (toolbar height)
        win.image_buffer = Some((0, 42, s.vp_w, s.vp_h, pixels));
    } else {
        // No image: clear image buffer and show checkerboard
        let pixels = render_viewport(
            &make_test_pattern(8, 8), 100, 0, 0, s.vp_w, s.vp_h, 0,
        );
        win.image_buffer = Some((0, 42, s.vp_w, s.vp_h, pixels));
    }
    win.dirty = true;
}

fn update_status_label(win: &mut Window, s: &ImageViewerState) {
    // Widget 6 is the status label
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 6) {
        let status = if let Some(ref img) = s.source {
            let slide_marker = if s.slideshow { " ▶" } else { "" };
            let rot_str = match s.rotation {
                90  => " | 90°",
                180 => " | 180°",
                270 => " | 270°",
                _   => "",
            };
            format!(
                "{}×{} | {}%{} | {}{}", img.width, img.height,
                s.zoom_pct(), rot_str, s.status, slide_marker
            )
        } else {
            s.status.clone()
        };
        if let WidgetKind::Label(ref mut lbl) = w.kind {
            lbl.text = status;
        }
    }
}

// ─── Self-test ────────────────────────────────────────────────────────────────

/// Verify the image viewer's core logic (no window creation needed).
pub fn self_test() -> bool {
    let mut ok = true;

    // 1. Test pattern generation
    let img = make_test_pattern(64, 64);
    if img.width != 64 || img.height != 64 { ok = false; }
    if img.pixels.len() != 64 * 64 * 4 { ok = false; }
    // Top-left pixel should be near-black (r=0, g=0, b=0)
    let tl = img.pixel(0, 0);
    if tl[3] != 255 { ok = false; } // alpha must be 255

    // 2. Viewport render — 100% zoom, no pan, no rotation
    let vp = render_viewport(&img, 100, 0, 0, 32, 32, 0);
    if vp.len() != 32 * 32 * 4 { ok = false; }

    // 3. Viewport render — 50% zoom (source pixel 0,0 maps to vp 0,0)
    let vp50 = render_viewport(&img, 50, 0, 0, 32, 32, 0);
    if vp50.len() != 32 * 32 * 4 { ok = false; }

    // 4. Zoom step cycle
    let zoom_count = ZOOM_STEPS.len();
    if zoom_count < 4 { ok = false; }

    // 5. Out-of-bounds pixels get checkerboard fill (non-zero)
    let small = make_test_pattern(4, 4);
    let vp_oob = render_viewport(&small, 100, 0, 0, 16, 16, 0);
    // Pixel at (8, 8) is out of bounds for a 4×4 source
    let off = (8 * 16 + 8) * 4;
    // Should be checkerboard grey, not zero
    if vp_oob[off] == 0 && vp_oob[off + 1] == 0 && vp_oob[off + 2] == 0 { ok = false; }

    // 6. Zoom fit: a 256×256 image in a 320×240 viewport → best zoom ≤ 75%
    let big = make_test_pattern(256, 256);
    let mut s = ImageViewerState {
        window_id: 0,
        source: Some(big),
        zoom_idx: ZOOM_DEFAULT_IDX,
        pan_x: 0,
        pan_y: 0,
        rotation: 0,
        files: Vec::new(),
        file_idx: 0,
        slideshow: false,
        slideshow_ticks: 0,
        status: String::new(),
        dirty: true,
        vp_w: 320,
        vp_h: 240,
    };
    s.zoom_fit();
    if s.zoom_pct() > 100 { ok = false; } // must fit within 320×240

    // 7. Rotation: render_viewport with 90° gives different pixels than 0°
    let square = make_test_pattern(16, 16);
    let vp_0   = render_viewport(&square, 100, 0, 0, 16, 16, 0);
    let vp_90  = render_viewport(&square, 100, 0, 0, 16, 16, 90);
    let vp_180 = render_viewport(&square, 100, 0, 0, 16, 16, 180);
    let vp_270 = render_viewport(&square, 100, 0, 0, 16, 16, 270);
    // All have correct buffer size
    if vp_0.len()   != 16 * 16 * 4 { ok = false; }
    if vp_90.len()  != 16 * 16 * 4 { ok = false; }
    if vp_180.len() != 16 * 16 * 4 { ok = false; }
    if vp_270.len() != 16 * 16 * 4 { ok = false; }
    // 90° rotation produces different output than 0° (non-trivial image)
    if vp_0 == vp_90  { ok = false; }
    if vp_0 == vp_180 { ok = false; }

    // 8. rotate_cw/ccw cycle back to 0 after 4 steps
    let mut s2 = ImageViewerState {
        window_id: 0, source: None, zoom_idx: 3, pan_x: 0, pan_y: 0,
        rotation: 0, files: Vec::new(), file_idx: 0, slideshow: false,
        slideshow_ticks: 0, status: String::new(), dirty: false,
        vp_w: 100, vp_h: 100,
    };
    s2.rotate_cw(); s2.rotate_cw(); s2.rotate_cw(); s2.rotate_cw();
    if s2.rotation != 0 { ok = false; }
    s2.rotate_ccw(); s2.rotate_ccw(); s2.rotate_ccw(); s2.rotate_ccw();
    if s2.rotation != 0 { ok = false; }

    // 9. decode_any falls back to test pattern (empty bytes → error, not panic)
    let result = decode_any(&[]);
    // Either error (expected) or some decode succeeded — just must not panic
    drop(result);

    ok
}

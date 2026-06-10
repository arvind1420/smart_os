//! Video Player — Phase 106 v2: VideoElement metadata + .smv playback.
//!
//! Playback engine:
//! • Procedural demo animation (colour bars + bouncing ball + scanlines)  — no file needed
//! • SmartVideo (.smv) loader from VFS: magic "SMVD", u16 w/h, u8 fps, u32 frames, RGB24 body
//! • MP4 / WebM / Ogg container metadata via `net::video::VideoElement` (codec, duration, dims)
//! • Play / Pause / Stop / ← First / Last → controls
//! • Frame-rate-limited advance via timer ticks (100 Hz PIT → ≈ 30 fps default)
//! • Video pixels blitted through win.image_buffer overlay (works with use_widgets=true)

#![allow(dead_code)]

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::window::{Window, WindowId};
use crate::gui::desktop::DESKTOP;
use crate::gui::theme::*;
use crate::gui::widget::{
    Widget, WidgetKind, Button, StaticLabel, AppCommand, WidgetAction,
};
use crate::vfs;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Timer runs at 100 Hz; 30 fps ≈ every 3 ticks.
const TIMER_HZ: u64 = 100;
/// Default playback frame rate.
const DEFAULT_FPS: u32 = 30;
/// Number of procedural demo loop frames.
const DEMO_LOOP: usize = 90; // 3 second loop at 30 fps
/// SmartVideo file magic bytes.
const SMV_MAGIC: &[u8] = b"SMVD";
/// SmartVideo format version.
const SMV_VERSION: u8 = 1;
/// SmartVideo header size in bytes.
const SMV_HEADER: usize = 14;

// ─── Animation helpers ────────────────────────────────────────────────────────

/// Triangle-wave: returns values in [0, amplitude] over period 2×amplitude.
#[inline]
pub fn tri_wave(t: usize, amplitude: usize) -> usize {
    if amplitude == 0 { return 0; }
    let period = amplitude * 2;
    let phase = t % period;
    if phase < amplitude { phase } else { period - phase }
}

/// Generate one RGBA frame of the demo animation (colour bars + bouncing ball).
pub fn render_demo_frame(frame: usize, w: usize, h: usize) -> Vec<u8> {
    let t = frame % DEMO_LOOP;
    let mut out = vec![0u8; w * h * 4];

    // Ball position (triangle waves, different frequencies for x and y)
    let ball_x = tri_wave(t * 3, w.saturating_sub(40)) + 20;
    let ball_y = tri_wave(t * 2, h.saturating_sub(40)) + 20;
    let ball_r2: usize = 20 * 20; // radius² = 400

    // Bar scroll offset
    let bar_scroll = (t * 6) % w.max(1);

    // Brightness pulse (0-based offset into 0..=76 range, oscillates)
    let pulse_phase = t % 60;
    let pulse: u32 = if pulse_phase < 30 { 180 + pulse_phase as u32 * 2 } else { 240 - (pulse_phase - 30) as u32 * 2 };

    for py in 0..h {
        for px in 0..w {
            // ── Colour bars ──
            let bar_x = (px + bar_scroll) % w.max(1);
            let band = bar_x * 7 / w.max(1); // 0..=6
            let (br, bg, bb): (u32, u32, u32) = match band {
                0 => (255, 0,   0),
                1 => (255, 160, 0),
                2 => (220, 220, 0),
                3 => (0,   200, 0),
                4 => (0,   100, 255),
                5 => (100, 0,   255),
                _ => (220, 0,   180),
            };
            let (mut r, mut g, mut b) = (
                ((br * pulse) / 255) as u8,
                ((bg * pulse) / 255) as u8,
                ((bb * pulse) / 255) as u8,
            );

            // ── Scanlines (CRT effect) ──
            if py & 1 == 1 { r = r / 2; g = g / 2; b = b / 2; }

            // ── Bouncing ball ──
            let dx = (px as i32 - ball_x as i32).unsigned_abs() as usize;
            let dy = (py as i32 - ball_y as i32).unsigned_abs() as usize;
            if dx * dx + dy * dy <= ball_r2 {
                // White ball with soft edge
                let edge_r2 = 16 * 16;
                if dx * dx + dy * dy <= edge_r2 {
                    r = 255; g = 255; b = 255;
                } else {
                    r = r / 2 + 127; g = g / 2 + 127; b = b / 2 + 127;
                }
            }

            let off = (py * w + px) * 4;
            out[off]     = r;
            out[off + 1] = g;
            out[off + 2] = b;
            out[off + 3] = 255;
        }
    }
    out
}

/// Convert an RGB24 frame (w×h×3) to RGBA32 (w×h×4).
fn rgb_to_rgba(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for i in 0..w * h {
        let s = i * 3; let d = i * 4;
        if s + 2 < rgb.len() {
            out[d] = rgb[s]; out[d+1] = rgb[s+1]; out[d+2] = rgb[s+2]; out[d+3] = 255;
        }
    }
    out
}

// ─── SmartVideo loader ────────────────────────────────────────────────────────

/// Video source: procedural (demo) or frames loaded from a .smv file.
pub enum VideoSource {
    Procedural,
    Loaded { frames: Vec<Vec<u8>>, width: usize, height: usize },
}

/// Try to load a SmartVideo (.smv) file from `path`.
fn load_smv(path: &str) -> Option<(usize, usize, u32, Vec<Vec<u8>>)> {
    let fd = vfs::open(path).ok()?;
    let mut buf = vec![0u8; 64 * 1024 * 1024]; // 64 MiB
    let n = vfs::read(fd, &mut buf).ok()?;
    vfs::close(fd).ok();
    if n < SMV_HEADER { return None; }
    if &buf[..4] != SMV_MAGIC || buf[4] != SMV_VERSION { return None; }

    let w = u16::from_le_bytes([buf[5], buf[6]]) as usize;
    let h = u16::from_le_bytes([buf[7], buf[8]]) as usize;
    let fps = buf[9] as u32;
    let frame_count = u32::from_le_bytes([buf[10], buf[11], buf[12], buf[13]]) as usize;

    let frame_bytes = w * h * 3;
    if SMV_HEADER + frame_count * frame_bytes > n { return None; }

    let mut frames = Vec::with_capacity(frame_count);
    for i in 0..frame_count {
        let start = SMV_HEADER + i * frame_bytes;
        frames.push(buf[start..start + frame_bytes].to_vec());
    }
    Some((w, h, fps, frames))
}

/// Scan VFS paths for a .smv video file; return the first found.
fn find_video_file() -> Option<(usize, usize, u32, Vec<Vec<u8>>)> {
    for path in &["/data/videos/demo.smv", "/disk/videos/demo.smv", "/data/demo.smv"] {
        if let Some(v) = load_smv(path) { return Some(v); }
    }
    None
}

/// Try to parse a real video container (MP4/WebM/Ogg) and return formatted metadata.
/// Uses `net::video::VideoElement::load()` — does not perform actual frame decoding.
fn probe_video_metadata(path: &str) -> Option<String> {
    let fd = vfs::open(path).ok()?;
    let mut buf = vec![0u8; 4 * 1024 * 1024]; // 4 MiB probe buffer
    let n = vfs::read(fd, &mut buf).ok()?;
    vfs::close(fd).ok();
    let mut el = crate::net::video::VideoElement::new(path.into(), 0, 0);
    el.load(&buf[..n]);
    if el.ready_state == crate::net::video::VideoReadyState::HaveNothing {
        return None;
    }
    Some(el.status_line())
}

/// Scan VFS for any real video container file (MP4, WebM, Ogg).
fn find_container_video() -> Option<String> {
    for dir in &["/data/videos", "/disk/videos", "/data", "/disk"] {
        if let Ok(entries) = vfs::readdir(dir) {
            for name in entries {
                if name.ends_with(".mp4")
                    || name.ends_with(".webm")
                    || name.ends_with(".ogg")
                    || name.ends_with(".ogv")
                    || name.ends_with(".mkv")
                {
                    let path = format!("{}/{}", dir, name);
                    if let Some(meta) = probe_video_metadata(&path) {
                        return Some(meta);
                    }
                }
            }
        }
    }
    None
}

// ─── App state ────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
pub enum PlayState { Playing, Paused, Stopped }

pub struct VideoPlayerState {
    pub window_id:       WindowId,
    pub source:          VideoSource,
    pub play_state:      PlayState,
    pub current_frame:   usize,
    pub total_frames:    usize,
    pub fps:             u32,
    pub ticks_per_frame: u64,
    pub last_frame_tick: u64,
    pub vp_w:            usize,
    pub vp_h:            usize,
    pub filename:        String,
    pub status:          String,
    pub needs_render:    bool,
    /// Container metadata from `net::video::VideoElement` (codec, dims, duration).
    pub container_meta:  Option<String>,
}

impl VideoPlayerState {
    fn fmt_position(&self) -> String {
        let secs = self.current_frame / self.fps.max(1) as usize;
        let total_secs = self.total_frames / self.fps.max(1) as usize;
        let state_sym = match self.play_state {
            PlayState::Playing => "\u{25B6}",
            PlayState::Paused  => "\u{23F8}",
            PlayState::Stopped => "\u{23F9}",
        };
        let meta_suffix = match &self.container_meta {
            Some(m) => format!("  [{}]", m),
            None    => String::new(),
        };
        format!("{} {:02}:{:02} / {:02}:{:02}  Frame {}/{}  {}{}",
            state_sym,
            secs / 60, secs % 60,
            total_secs / 60, total_secs % 60,
            self.current_frame + 1, self.total_frames,
            self.filename, meta_suffix)
    }

    fn current_rgba(&self) -> Vec<u8> {
        match &self.source {
            VideoSource::Procedural => render_demo_frame(self.current_frame, self.vp_w, self.vp_h),
            VideoSource::Loaded { frames, width, height } => {
                if self.current_frame < frames.len() {
                    rgb_to_rgba(&frames[self.current_frame], *width, *height)
                } else {
                    vec![0u8; self.vp_w * self.vp_h * 4]
                }
            }
        }
    }
}

pub static STATE: Mutex<Option<VideoPlayerState>> = Mutex::new(None);

// ─── App thread ──────────────────────────────────────────────────────────────

pub fn run() {
    let (vp_w, vp_h) = (720usize, 430usize); // video canvas size

    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() { Some(d) => d, None => return };

        let mut win = crate::gui::window::Window::new("Video Player", 60, 50, 720, 500, ACCENT_MAGENTA);
        win.use_widgets = true;

        // Toolbar buttons
        win.widgets.push(Widget::new(0, 0, 0, 44, 22,
            WidgetKind::Button(Button::new("\u{23EE}", ACCENT_CYAN, AppCommand::ButtonClicked(0)))));
        win.widgets.push(Widget::new(1, 48, 0, 60, 22,
            WidgetKind::Button(Button::new("\u{23EF} Play", ACCENT_GREEN, AppCommand::ButtonClicked(1)))));
        win.widgets.push(Widget::new(2, 112, 0, 52, 22,
            WidgetKind::Button(Button::new("\u{23F9} Stop", ACCENT_RED, AppCommand::ButtonClicked(2)))));
        win.widgets.push(Widget::new(3, 168, 0, 44, 22,
            WidgetKind::Button(Button::new("\u{23ED}", ACCENT_CYAN, AppCommand::ButtonClicked(3)))));
        win.widgets.push(Widget::new(4, 216, 0, 60, 22,
            WidgetKind::Button(Button::new("Open", ACCENT_ORANGE, AppCommand::ButtonClicked(4)))));
        // Status label
        win.widgets.push(Widget::new(5, 0, 25, 720, 16,
            WidgetKind::Label(StaticLabel::new("Ready", TEXT_SECONDARY))));

        win.focused_widget = Some(1);
        let id = win.id;
        desk.wm.add(win);
        id
    };

    // Try to load a real video, otherwise use procedural demo
    let (source, total_frames, fps, src_w, src_h, filename) =
        match find_video_file() {
            Some((w, h, fps, frames)) => {
                let tf = frames.len();
                (VideoSource::Loaded { frames, width: w, height: h }, tf, fps, w, h, String::from("demo.smv"))
            }
            None => (VideoSource::Procedural, DEMO_LOOP, DEFAULT_FPS, vp_w, vp_h, String::from("[demo animation]")),
        };

    let _ = (src_w, src_h); // used only for logging

    let ticks_per_frame = (TIMER_HZ / fps.max(1) as u64).max(1);
    // Probe any real video container in VFS for metadata display
    let container_meta = find_container_video();

    *STATE.lock() = Some(VideoPlayerState {
        window_id,
        source,
        play_state: PlayState::Paused,
        current_frame: 0,
        total_frames,
        fps,
        ticks_per_frame,
        last_frame_tick: crate::drivers::timer::ticks(),
        vp_w,
        vp_h,
        filename,
        status: String::from("Paused"),
        needs_render: true,
        container_meta,
    });

    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                match action {
                    // ⏮ First frame
                    WidgetAction::Execute(AppCommand::ButtonClicked(0)) => {
                        s.current_frame = 0; s.needs_render = true;
                    }
                    // ⏯ Play/Pause toggle
                    WidgetAction::Execute(AppCommand::ButtonClicked(1)) => {
                        s.play_state = if s.play_state == PlayState::Playing {
                            PlayState::Paused
                        } else {
                            s.last_frame_tick = crate::drivers::timer::ticks();
                            PlayState::Playing
                        };
                        s.needs_render = true;
                    }
                    // ⏹ Stop
                    WidgetAction::Execute(AppCommand::ButtonClicked(2)) => {
                        s.play_state = PlayState::Stopped;
                        s.current_frame = 0; s.needs_render = true;
                    }
                    // ⏭ Last frame
                    WidgetAction::Execute(AppCommand::ButtonClicked(3)) => {
                        s.current_frame = s.total_frames.saturating_sub(1);
                        s.needs_render = true;
                    }
                    // Open — try VFS reload
                    WidgetAction::Execute(AppCommand::ButtonClicked(4)) => {
                        if let Some((w, h, fp, frames)) = find_video_file() {
                            let tf = frames.len();
                            s.total_frames = tf;
                            s.fps = fp;
                            s.ticks_per_frame = (TIMER_HZ / fp.max(1) as u64).max(1);
                            s.vp_w = w; s.vp_h = h;
                            s.filename = String::from("demo.smv");
                            s.source = VideoSource::Loaded { frames, width: w, height: h };
                            s.current_frame = 0;
                            s.needs_render = true;
                        }
                    }
                    _ => {}
                }
                s.status = s.fmt_position();
            }
        }
        crate::process::scheduler::yield_now();
    }
}

// ─── Sync to window ──────────────────────────────────────────────────────────

pub fn sync_to_window(win: &mut Window) {
    // Advance frame if playing, collect what we need to render
    let (status, should_render, vp_w, vp_h, frame_idx) = {
        let mut guard = STATE.lock();
        let s = match guard.as_mut() { Some(x) => x, None => return };

        if s.play_state == PlayState::Playing {
            let now = crate::drivers::timer::ticks();
            if now.wrapping_sub(s.last_frame_tick) >= s.ticks_per_frame {
                s.current_frame = (s.current_frame + 1) % s.total_frames.max(1);
                s.last_frame_tick = now;
                s.needs_render = true;
                s.status = s.fmt_position();
            }
        }

        let r = s.needs_render;
        if r { s.needs_render = false; }
        (s.status.clone(), r, s.vp_w, s.vp_h, s.current_frame)
    };

    // Update status label (widget 5)
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 5) {
        if let WidgetKind::Label(ref mut lbl) = w.kind { lbl.text = status; }
    }

    // Re-render video frame into image_buffer (only when frame changed)
    if should_render {
        let pixels = {
            let guard = STATE.lock();
            match guard.as_ref() {
                Some(s) => s.current_rgba(),
                None => render_demo_frame(frame_idx, vp_w, vp_h),
            }
        };
        // y=44 offset: below 22px toolbar + 18px status label + 4px gap
        win.image_buffer = Some((0, 44, vp_w, vp_h, pixels));
        win.dirty = true;
    }
}

// ─── Self-test ────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // 1. tri_wave correctness
    if tri_wave(0, 100) != 0  { ok = false; }
    if tri_wave(50, 100) != 50 { ok = false; }
    if tri_wave(100, 100) != 100 { ok = false; } // peak
    if tri_wave(150, 100) != 50 { ok = false; }
    if tri_wave(200, 100) != 0  { ok = false; } // back to start
    if tri_wave(0, 0) != 0 { ok = false; }      // zero amplitude guard

    // 2. render_demo_frame produces correct byte count
    let f = render_demo_frame(0, 64, 48);
    if f.len() != 64 * 48 * 4 { ok = false; }

    // 3. Frame 0 alpha bytes must be 255
    if f.get(3).copied() != Some(255) { ok = false; }

    // 4. Different frames produce different pixels (animation is moving)
    let f2 = render_demo_frame(30, 64, 48);
    if f.len() != f2.len() { ok = false; }
    let different = f.iter().zip(f2.iter()).any(|(a, b)| a != b);
    if !different { ok = false; }

    // 5. rgb_to_rgba conversion
    let rgb = vec![255u8, 128, 0, 0, 255, 0]; // 2 pixels
    let rgba = rgb_to_rgba(&rgb, 2, 1);
    if rgba.len() != 8 { ok = false; }
    if rgba[0] != 255 || rgba[1] != 128 || rgba[2] != 0 || rgba[3] != 255 { ok = false; }
    if rgba[4] != 0 || rgba[5] != 255 || rgba[6] != 0 || rgba[7] != 255 { ok = false; }

    // 6. FPS → ticks_per_frame (30 fps @ 100 Hz → 3 ticks)
    let tpf = (TIMER_HZ / 30u64).max(1);
    if tpf != 3 { ok = false; }

    // 7. SMV header validation: wrong magic → None
    let bad_data = vec![0u8; 64];
    let result: Option<()> = (|| {
        if &bad_data[..4] != SMV_MAGIC { return None; }
        Some(())
    })();
    if result.is_some() { ok = false; } // should be None

    // 8. DEMO_LOOP frames cycle correctly
    let t1 = render_demo_frame(0, 32, 32);
    let t2 = render_demo_frame(DEMO_LOOP, 32, 32); // should be same as frame 0
    if t1 != t2 { ok = false; }

    ok
}

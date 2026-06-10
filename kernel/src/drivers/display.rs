/// Display Modesetting & VSync — Phase 16 for Smart OS.
///
/// Provides:
///  • Display mode enumeration (from DRM connectors)
///  • Double-buffer page-flip management
///  • VSync-locked 60 FPS render loop using HPET uptime
///  • Frame timing statistics (avg frame time, dropped frames)
///
/// Architecture
/// ────────────
///  The `DisplayManager` owns two framebuffer handles (front/back).
///  Each frame:
///   1. Compositor draws into the back buffer.
///   2. `page_flip()` swaps front ↔ back and calls DRM set_crtc on
///      the new front buffer (the one just drawn into).
///   3. `vsync_wait()` spins until the next 16.667 ms boundary.
///
/// Without hardware VSync (VirtIO-GPU has no interrupt line), we
/// simulate it with HPET uptime: sleep the remaining time in the
/// 16.667 ms budget after page flip completes.
///
/// The render loop (`run_frame_loop`) is called from the compositor
/// thread and never returns.

use spin::Mutex;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};
use alloc::vec::Vec;

use super::drm::DRM;
use super::gpu_mem;
use super::gpu_mem::PixelFormat;

// ─────────────────────────────────────────────────────────────────────────────
//  Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Target refresh period in microseconds (60 Hz = 16 666.67 µs).
pub const VSYNC_PERIOD_US: u64 = 16_667;

/// Target frame rate.
pub const TARGET_FPS: u32 = 60;

// ─────────────────────────────────────────────────────────────────────────────
//  Frame statistics
// ─────────────────────────────────────────────────────────────────────────────

/// Total frames presented since boot.
pub static FRAME_COUNT: AtomicU64 = AtomicU64::new(0);
/// Frames that missed the VSync deadline (took > VSYNC_PERIOD_US).
pub static DROPPED_FRAMES: AtomicU64 = AtomicU64::new(0);
/// Last frame duration in microseconds.
pub static LAST_FRAME_US: AtomicU64 = AtomicU64::new(0);
/// Running average frame duration (exponential moving avg × 1000).
pub static AVG_FRAME_US: AtomicU64 = AtomicU64::new(16_667);
/// Whether the render loop is active.
pub static RENDER_LOOP_ACTIVE: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
//  Display configuration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct ModeInfo {
    pub connector_id: u32,
    pub width:        u32,
    pub height:       u32,
    pub refresh_hz:   u32,
}

impl ModeInfo {
    pub fn pixels(&self) -> u32 { self.width * self.height }

    pub fn aspect_ratio(&self) -> (u32, u32) {
        let g = gcd(self.width, self.height);
        (self.width / g, self.height / g)
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 { let t = b; b = a % b; a = t; }
    a
}

// ─────────────────────────────────────────────────────────────────────────────
//  Double-buffer state
// ─────────────────────────────────────────────────────────────────────────────

/// Owns two GPU BO handles for double-buffering.
pub struct DoubleBuffer {
    /// The buffer currently on-screen (just presented).
    pub front: u32,
    /// The buffer being drawn into.
    pub back:  u32,
    /// Connector the scanout is bound to.
    pub connector_id: u32,
    /// Display geometry.
    pub width:  u32,
    pub height: u32,
}

impl DoubleBuffer {
    /// Swap front and back, then issue SET_SCANOUT + TRANSFER + FLUSH on new front.
    pub fn page_flip(&mut self) {
        // back → front
        core::mem::swap(&mut self.front, &mut self.back);

        // Present the new front buffer.
        gpu_mem::bind_primary_scanout(self.front);
        gpu_mem::upload_and_flush(self.front);

        FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    /// Virtual address of the back (draw) buffer.
    pub fn back_virt(&self) -> Option<u64> {
        let alloc = gpu_mem::GPU_ALLOC.lock();
        alloc.as_ref()?.get_bo(self.back)?.contiguous_virt()
    }

    /// Virtual address of the front buffer (for readback or screenshot).
    pub fn front_virt(&self) -> Option<u64> {
        let alloc = gpu_mem::GPU_ALLOC.lock();
        alloc.as_ref()?.get_bo(self.front)?.contiguous_virt()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Display Manager
// ─────────────────────────────────────────────────────────────────────────────

pub struct DisplayManager {
    /// Connected displays discovered from DRM.
    pub connectors: Vec<ModeInfo>,
    /// Primary display mode.
    pub primary: Option<ModeInfo>,
    /// Double-buffer handles for the primary display.
    pub dbuf: Option<DoubleBuffer>,
}

pub static DISPLAY: Mutex<Option<DisplayManager>> = Mutex::new(None);

impl DisplayManager {
    /// Enumerate connectors from the DRM driver and pick the primary mode.
    fn enumerate_connectors() -> Vec<ModeInfo> {
        let drm = DRM.lock();
        let driver = match drm.active_driver.as_ref() {
            Some(d) => d,
            None => return alloc::vec![],
        };
        driver.get_connectors().into_iter()
            .filter(|c| c.connected)
            .filter_map(|c| c.current_mode.map(|m| ModeInfo {
                connector_id: c.id,
                width:        m.width,
                height:       m.height,
                refresh_hz:   m.refresh_rate,
            }))
            .collect()
    }

    /// Allocate double-buffer BOs for the given mode.
    fn alloc_double_buffer(mode: &ModeInfo, is_bgr: bool) -> Option<DoubleBuffer> {
        let fmt = if is_bgr {
            gpu_mem::PixelFormat::Bgrx8888
        } else {
            gpu_mem::PixelFormat::Rgbx8888
        };

        let front = gpu_mem::alloc_bo(mode.width, mode.height, fmt).ok()?;
        let back  = gpu_mem::alloc_bo(mode.width, mode.height, fmt).ok()?;

        // Bind front to scanout immediately.
        gpu_mem::bind_primary_scanout(front);

        crate::serial_println!(
            "[display] Double-buffer: front={} back={} {}x{}@{}Hz",
            front, back, mode.width, mode.height, mode.refresh_hz
        );

        Some(DoubleBuffer {
            front,
            back,
            connector_id: mode.connector_id,
            width:  mode.width,
            height: mode.height,
        })
    }
}

/// Initialise the display manager: enumerate connectors, allocate double-buffer.
/// `is_bgr` must match the VirtIO-GPU/iGPU pixel format agreed during GPU init.
pub fn init(is_bgr: bool) {
    let connectors = DisplayManager::enumerate_connectors();
    let primary = connectors.first().copied();

    crate::serial_println!("[display] {} connector(s) found.", connectors.len());
    for m in &connectors {
        let (ar_w, ar_h) = m.aspect_ratio();
        crate::serial_println!(
            "[display]   connector={} {}x{}@{}Hz ({}:{})",
            m.connector_id, m.width, m.height, m.refresh_hz, ar_w, ar_h
        );
    }

    let dbuf = primary.as_ref().and_then(|m| DisplayManager::alloc_double_buffer(m, is_bgr));

    if dbuf.is_none() && primary.is_some() {
        crate::serial_println!("[display] warn: double-buffer allocation failed; single-buffer mode.");
    }

    *DISPLAY.lock() = Some(DisplayManager { connectors, primary, dbuf });

    crate::serial_println!("[display] Display modesetting ready.");
}

// ─────────────────────────────────────────────────────────────────────────────
//  VSync timing
// ─────────────────────────────────────────────────────────────────────────────

/// Current uptime in microseconds (wraps after ~585 000 years).
#[inline]
pub fn now_us() -> u64 {
    // Prefer HPET; fall back to timer tick count (1 ms resolution).
    let hpet_us = super::hpet::uptime_us();
    if hpet_us > 0 {
        hpet_us
    } else {
        // Fallback: use the timer uptime in ms × 1000 → µs.
        crate::drivers::timer::uptime_ms() * 1000
    }
}

/// Spin-wait until `deadline_us`.  Returns the overshoot in µs.
fn spin_until(deadline_us: u64) -> u64 {
    loop {
        let now = now_us();
        if now >= deadline_us { return now - deadline_us; }
        core::hint::spin_loop();
    }
}

/// Wait for the next VSync boundary from `frame_start_us`.
/// Returns actual frame duration in µs.
pub fn vsync_wait(frame_start_us: u64) -> u64 {
    let deadline = frame_start_us + VSYNC_PERIOD_US;
    let overshoot = spin_until(deadline);
    let frame_dur = now_us() - frame_start_us;

    // Update stats.
    LAST_FRAME_US.store(frame_dur, Ordering::Relaxed);
    let avg = AVG_FRAME_US.load(Ordering::Relaxed);
    let new_avg = (avg * 15 + frame_dur) / 16; // EMA α=1/16
    AVG_FRAME_US.store(new_avg, Ordering::Relaxed);
    if overshoot > 0 {
        DROPPED_FRAMES.fetch_add(1, Ordering::Relaxed);
    }

    frame_dur
}

// ─────────────────────────────────────────────────────────────────────────────
//  Page-flip helper (called by compositor each frame)
// ─────────────────────────────────────────────────────────────────────────────

/// Present the back buffer, wait for VSync, return frame duration µs.
/// If no GPU double-buffer is active, this is a no-op (software FB path).
pub fn present_and_vsync(frame_start_us: u64) -> u64 {
    // Page-flip if GPU double-buffer is available.
    {
        let mut disp = DISPLAY.lock();
        if let Some(dm) = disp.as_mut() {
            if let Some(dbuf) = dm.dbuf.as_mut() {
                dbuf.page_flip();
            }
        }
    }
    vsync_wait(frame_start_us)
}

/// Return the back-buffer virtual address for the compositor to draw into.
pub fn back_buffer_virt() -> Option<u64> {
    let disp = DISPLAY.lock();
    disp.as_ref()?.dbuf.as_ref()?.back_virt()
}

/// Return the current display resolution.
pub fn display_resolution() -> (u32, u32) {
    let disp = DISPLAY.lock();
    if let Some(dm) = disp.as_ref() {
        if let Some(m) = dm.primary.as_ref() {
            return (m.width, m.height);
        }
    }
    (1024, 768) // sensible default
}

// ─────────────────────────────────────────────────────────────────────────────
//  Frame-rate limiter (for apps that call this directly)
// ─────────────────────────────────────────────────────────────────────────────

/// Simple rate-limiter state (per-caller).
pub struct FrameTimer {
    last_us: u64,
}

impl FrameTimer {
    pub const fn new() -> Self { FrameTimer { last_us: 0 } }

    /// Wait until the next frame budget.  Returns true if this is a "fresh" frame.
    pub fn tick(&mut self) -> bool {
        let now = now_us();
        if self.last_us == 0 {
            self.last_us = now;
            return true;
        }
        let elapsed = now - self.last_us;
        if elapsed < VSYNC_PERIOD_US {
            spin_until(self.last_us + VSYNC_PERIOD_US);
        }
        self.last_us = now_us();
        true
    }

    /// Non-blocking check: is it time for a new frame?
    pub fn ready(&self) -> bool {
        now_us() - self.last_us >= VSYNC_PERIOD_US
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Frame stats
// ─────────────────────────────────────────────────────────────────────────────

pub fn print_stats() {
    let frames   = FRAME_COUNT.load(Ordering::Relaxed);
    let dropped  = DROPPED_FRAMES.load(Ordering::Relaxed);
    let last     = LAST_FRAME_US.load(Ordering::Relaxed);
    let avg      = AVG_FRAME_US.load(Ordering::Relaxed);
    let fps      = if avg > 0 { 1_000_000 / avg } else { 0 };
    crate::serial_println!(
        "[display] frames={} dropped={} last={}µs avg={}µs (~{} fps)",
        frames, dropped, last, avg, fps
    );
}

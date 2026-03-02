/// Direct Rendering Manager (DRM) & Kernel Mode Setting (KMS) for Smart OS.
///
/// This module provides the generic abstraction layer for hardware-accelerated
/// graphics. It replaces the simple UEFI framebuffer with a modular architecture
/// that supports multiple displays, hardware cursors, and 2D/3D blitting via GPU drivers.

use alloc::vec::Vec;
use spin::Mutex;

/// A frame buffer object (a chunk of video memory).
pub struct FramebufferObj {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u8,
    pub phys_addr: u64,
}

/// A display output (connector/monitor).
pub struct Connector {
    pub id: u32,
    pub connected: bool,
    pub current_mode: Option<DisplayMode>,
}

/// A display mode (resolution & refresh rate).
#[derive(Clone, Copy)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_rate: u32,
}

/// The generic trait that all GPU drivers (Intel, AMD, VirtIO-GPU) must implement.
pub trait GpuDriver: Send {
    /// Initialize the GPU hardware.
    fn init(&mut self) -> Result<(), &'static str>;
    
    /// Get available connectors.
    fn get_connectors(&self) -> Vec<Connector>;
    
    /// Allocate a new framebuffer in video memory.
    fn alloc_framebuffer(&mut self, width: u32, height: u32, format: u32) -> Result<FramebufferObj, &'static str>;
    
    /// Set the active framebuffer for a connector (page flip).
    fn set_crtc(&mut self, connector_id: u32, fb_id: u32) -> Result<(), &'static str>;
    
    /// Hardware 2D Blit: copy memory from one area of VRAM to another.
    fn blit(&mut self, src_fb: u32, dst_fb: u32, src_x: u32, src_y: u32, dst_x: u32, dst_y: u32, w: u32, h: u32) -> Result<(), &'static str>;
    
    /// Fill a rectangle with a solid color using the hardware.
    fn fill_rect(&mut self, dst_fb: u32, x: u32, y: u32, w: u32, h: u32, color: u32) -> Result<(), &'static str>;
}

/// The global DRM subsystem state.
pub struct DrmState {
    pub active_driver: Option<alloc::boxed::Box<dyn GpuDriver>>,
}

pub static DRM: Mutex<DrmState> = Mutex::new(DrmState { active_driver: None });

/// Initialize the DRM subsystem.
pub fn init() {
    crate::serial_println!("[drm] Direct Rendering Manager initialized.");
}

/// Register a GPU driver with the DRM subsystem.
pub fn register_driver(driver: alloc::boxed::Box<dyn GpuDriver>) {
    let mut drm = DRM.lock();
    drm.active_driver = Some(driver);
    crate::serial_println!("[drm] GPU driver registered.");
}

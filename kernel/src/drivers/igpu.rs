/// Intel Graphics (iGPU) Driver for Smart OS.
///
/// Implements the GpuDriver trait with support for the Blitter Command Streamer (BCS).
/// Handles hardware-accelerated 2D blits and fills on Gen9+ hardware.

use alloc::vec::Vec;
use alloc::boxed::Box;
use super::pci;
use super::drm::{GpuDriver, Connector, DisplayMode, FramebufferObj};

pub const INTEL_VENDOR_ID: u16 = 0x8086;

// BCS (Blitter) Register Offsets
const BCS_RING_BUFFER_TAIL: u32 = 0x22030;
const BCS_RING_BUFFER_START: u32 = 0x22038;
const BCS_RING_BUFFER_CTL: u32 = 0x2203C;

// BCS Commands
const XY_COLOR_BLT: u32 = (0x2 << 29) | (0x50 << 22) | (0x4); // 2D, Op 50h, length 6 (0-indexed)
const XY_SRC_COPY_BLT: u32 = (0x2 << 29) | (0x53 << 22) | (0x6); // length 8

pub struct IntelGpu {
    pci_device: pci::PciDevice,
    mmio_base: u64,
    
    // Ring Buffer
    ring_phys: u64,
    ring_virt: *mut u32,
    ring_tail: u32,
    ring_size: u32,
}

unsafe impl Send for IntelGpu {}

impl IntelGpu {
    pub fn new(dev: pci::PciDevice) -> Self {
        Self {
            pci_device: dev,
            mmio_base: 0,
            ring_phys: 0,
            ring_virt: core::ptr::null_mut(),
            ring_tail: 0,
            ring_size: 16 * 1024, // 16 KB ring
        }
    }

    unsafe fn write_mmio(&self, offset: u32, val: u32) {
        unsafe { core::ptr::write_volatile((self.mmio_base + offset as u64) as *mut u32, val); }
    }

    fn ring_begin(&mut self, _dwords: u32) {
        // In a real driver, we'd wait for space
    }

    fn ring_emit(&mut self, data: u32) {
        unsafe {
            *self.ring_virt.add((self.ring_tail / 4) as usize) = data;
            self.ring_tail = (self.ring_tail + 4) % self.ring_size;
        }
    }

    fn ring_advance(&mut self) {
        unsafe {
            self.write_mmio(BCS_RING_BUFFER_TAIL, self.ring_tail);
        }
    }
}

impl GpuDriver for IntelGpu {
    fn init(&mut self) -> Result<(), &'static str> {
        pci::enable_bus_master(&self.pci_device);
        let mmio_phys = pci::bar0_mmio_base(&self.pci_device).ok_or("iGPU BAR0 is not MMIO")?;
        let phys_offset = crate::memory::paging::phys_offset().as_u64();
        self.mmio_base = phys_offset + mmio_phys;

        // Allocate BCS Ring Buffer
        let ring_frame = crate::memory::frame::alloc_frame().ok_or("No memory for iGPU ring")?;
        self.ring_phys = ring_frame.start_address().as_u64();
        self.ring_virt = (phys_offset + self.ring_phys) as *mut u32;
        
        unsafe {
            core::ptr::write_bytes(self.ring_virt, 0, self.ring_size as usize);
            
            // Setup BCS Ring
            self.write_mmio(BCS_RING_BUFFER_START, self.ring_phys as u32);
            self.write_mmio(BCS_RING_BUFFER_CTL, (self.ring_size - 4096) | 1); // Enable
        }

        crate::serial_println!("[igpu] Intel BCS (Blitter) hardware active.");
        Ok(())
    }

    fn get_connectors(&self) -> Vec<Connector> {
        let mut connectors = Vec::new();
        connectors.push(Connector {
            id: 1,
            connected: true,
            current_mode: Some(DisplayMode { width: 1920, height: 1080, refresh_rate: 60 }),
        });
        connectors
    }

    fn alloc_framebuffer(&mut self, width: u32, height: u32, _format: u32) -> Result<FramebufferObj, &'static str> {
        let bytes = (width * height * 4) as usize;
        let pages = (bytes + 4095) / 4096;
        let mut first_phys = 0;
        let phys_offset = crate::memory::paging::phys_offset().as_u64();

        for i in 0..pages {
            let frame = crate::memory::frame::alloc_frame().ok_or("FB allocation failed")?;
            if i == 0 { first_phys = frame.start_address().as_u64(); }
        }

        Ok(FramebufferObj {
            id: (first_phys & 0xFFFFFFFF) as u32,
            width, height, pitch: width * 4, bpp: 32,
            phys_addr: first_phys,
            virt_addr: phys_offset + first_phys,
        })
    }

    fn set_crtc(&mut self, _connector_id: u32, _fb_id: u32) -> Result<(), &'static str> {
        Ok(()) // Flip handled by compositor for now
    }

    fn blit(&mut self, src_fb: u32, dst_fb: u32, src_x: u32, src_y: u32, dst_x: u32, dst_y: u32, w: u32, h: u32) -> Result<(), &'static str> {
        self.ring_begin(8);
        self.ring_emit(XY_SRC_COPY_BLT);
        self.ring_emit(0xCC << 16 | (4 * 1920)); // ROP and pitch
        self.ring_emit((dst_y << 16) | dst_x);
        self.ring_emit(((dst_y + h) << 16) | (dst_x + w));
        self.ring_emit(dst_fb); // Destination Address (actually GTT offset)
        self.ring_emit((src_y << 16) | src_x);
        self.ring_emit(4 * 1920); // Source pitch
        self.ring_emit(src_fb); // Source Address
        self.ring_advance();
        Ok(())
    }

    fn fill_rect(&mut self, dst_fb: u32, x: u32, y: u32, w: u32, h: u32, color: u32) -> Result<(), &'static str> {
        self.ring_begin(6);
        self.ring_emit(XY_COLOR_BLT);
        self.ring_emit(0xF0 << 16 | (4 * 1920)); // ROP and pitch
        self.ring_emit((y << 16) | x);
        self.ring_emit(((y + h) << 16) | (x + w));
        self.ring_emit(dst_fb); // Destination Address
        self.ring_emit(color);
        self.ring_advance();
        Ok(())
    }
}

pub fn init() -> Result<(), &'static str> {
    if let Some(dev) = pci::find_by_class(0x03, 0x00, 0x00) {
        if dev.vendor_id == INTEL_VENDOR_ID {
            let mut igpu = IntelGpu::new(dev);
            igpu.init()?;
            super::drm::register_driver(Box::new(igpu));
            return Ok(());
        }
    }
    Err("No Intel iGPU found")
}

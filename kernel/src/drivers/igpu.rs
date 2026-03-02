/// Intel Graphics (iGPU) Driver for Smart OS.
///
/// A minimal driver for Intel Integrated Graphics (Gen9+).
/// Implements the `GpuDriver` trait for the DRM subsystem.

use alloc::vec::Vec;
use alloc::boxed::Box;
use super::pci;
use super::drm::{GpuDriver, Connector, DisplayMode, FramebufferObj};

pub const INTEL_VENDOR_ID: u16 = 0x8086;

// Example Gen9+ Device IDs (Skylake/Kaby Lake/Coffee Lake)
pub const INTEL_IGPU_SKL: u16 = 0x1912; 
pub const INTEL_IGPU_KBL: u16 = 0x5916;

/// Intel Graphics Driver State.
pub struct IntelGpu {
    pci_device: pci::PciDevice,
    mmio_base: u64,
    gmadr_base: u64, // Graphics Memory Address
}

impl IntelGpu {
    pub fn new(dev: pci::PciDevice) -> Self {
        Self {
            pci_device: dev,
            mmio_base: 0,
            gmadr_base: 0,
        }
    }
}

impl GpuDriver for IntelGpu {
    fn init(&mut self) -> Result<(), &'static str> {
        // Enable Bus Mastering and MMIO access
        pci::enable_bus_master(&self.pci_device);

        // Intel GPUs typically use BAR0 for MMIO (Registers) and BAR2 for GMADR (Aperture)
        let mmio_phys = pci::bar0_mmio_base(&self.pci_device).ok_or("iGPU BAR0 is not MMIO")?;
        
        let phys_offset = crate::memory::paging::phys_offset().as_u64();
        self.mmio_base = phys_offset + mmio_phys;
        
        // Note: Full ring buffer (Blitter/Render ring) setup goes here.
        crate::serial_println!("[igpu] Intel Graphics initialized. MMIO @ {:#X}", self.mmio_base);
        
        Ok(())
    }

    fn get_connectors(&self) -> Vec<Connector> {
        // Mocking a connected DisplayPort for Step 1
        let mut connectors = Vec::new();
        connectors.push(Connector {
            id: 1,
            connected: true,
            current_mode: Some(DisplayMode { width: 1920, height: 1080, refresh_rate: 60 }),
        });
        connectors
    }

    fn alloc_framebuffer(&mut self, width: u32, height: u32, _format: u32) -> Result<FramebufferObj, &'static str> {
        // In a real implementation, we would allocate physical frames using
        // the Global GTT (Graphics Translation Table) so the GPU can access them.
        Ok(FramebufferObj {
            id: 1,
            width,
            height,
            pitch: width * 4,
            bpp: 32,
            phys_addr: 0, // Placeholder
        })
    }

    fn set_crtc(&mut self, _connector_id: u32, _fb_id: u32) -> Result<(), &'static str> {
        // Write the Framebuffer physical address to the Plane Surface register (e.g. PRI_SURF)
        Ok(())
    }

    fn blit(&mut self, _src_fb: u32, _dst_fb: u32, _src_x: u32, _src_y: u32, _dst_x: u32, _dst_y: u32, _w: u32, _h: u32) -> Result<(), &'static str> {
        // Submit an MI_BLT command to the Blitter Ring (BCS)
        Ok(())
    }

    fn fill_rect(&mut self, _dst_fb: u32, _x: u32, _y: u32, _w: u32, _h: u32, _color: u32) -> Result<(), &'static str> {
        // Submit an XY_COLOR_BLT command to the Blitter Ring (BCS)
        Ok(())
    }
}

/// Probe the PCI bus for an Intel iGPU and register it.
pub fn init() -> Result<(), &'static str> {
    let mut found_dev = None;
    
    // We search for class 0x03 (Display Controller), subclass 0x00 (VGA)
    if let Some(dev) = pci::find_by_class(0x03, 0x00, 0x00) {
        if dev.vendor_id == INTEL_VENDOR_ID {
            found_dev = Some(dev);
        }
    }

    let dev = found_dev.ok_or("No Intel iGPU found")?;
    
    let mut igpu = IntelGpu::new(dev);
    igpu.init()?;
    
    super::drm::register_driver(Box::new(igpu));
    
    Ok(())
}

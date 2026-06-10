/// Hardware drivers for Smart OS.
///
/// Phase 2: PIC, Timer, Keyboard
/// Phase 3: PS/2 Mouse
/// Phase 6: PCI, VirtIO (block + net), disk filesystem
/// Phase 8: FAT32, USB xHCI + HID
/// Phase 9: RTC

pub mod pic;
pub mod timer;
pub mod keyboard;
pub mod mouse;
pub mod rtc;
pub mod e1000;
pub mod nvme;
pub mod drm;
pub mod igpu;
pub mod amdgpu;
pub mod hpet;
pub mod hda;

/// Generic interface for block storage devices (HDD, SSD, VirtIO).
pub trait BlockDevice: Send {
    /// Read blocks from the device.
    fn read_blocks(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), &'static str>;
    /// Write blocks to the device.
    fn write_blocks(&mut self, sector: u64, buf: &[u8]) -> Result<(), &'static str>;
    /// Total capacity in sectors.
    fn capacity(&self) -> u64;
}

#[allow(dead_code)]
pub mod pci;
#[allow(dead_code)]
pub mod virtio;
#[allow(dead_code)]
pub mod virtio_blk;
#[allow(dead_code)]
pub mod virtio_net;
pub mod virtio_gpu;
pub mod gpu_mem;
pub mod gpu2d;
pub mod display;
#[allow(dead_code)]
pub mod diskfs;
#[allow(dead_code)]
pub mod fat32;
pub mod ext4;
#[allow(dead_code)]
pub mod acpi;
pub mod xhci;
pub mod usb_hid;
pub mod gdb_stub;
pub mod hotplug;
pub mod uvc;
pub mod haptic;
pub mod uart;
pub mod lpt;
pub mod ide;
pub mod vga_emu;
pub mod ahci;
pub mod wifi;
pub mod bluetooth;
pub mod hda_new;
pub mod hw_compat;
pub mod ntfs;
pub mod tpm;
pub mod vulkan;
pub mod bci;
pub mod gles;
pub mod glsl;
pub mod gpu3d;
pub mod compositor;
pub mod font;
pub mod image;

/// Initialize core hardware drivers (PIC, timer, keyboard, mouse).
pub fn init() {
    pic::init();
    timer::init();
    keyboard::init();
    mouse::init();
    hotplug::init();
    uvc::init();
    haptic::init();
    uart::init();
    lpt::init();
    ide::init();
    ahci::init();
    wifi::init();
    bluetooth::init();
    hda_new::init();
    hw_compat::init();
    ntfs::init();
    tpm::init();
    vulkan::init();
    bci::init();
    vga_emu::init();
}

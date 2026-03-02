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
#[allow(dead_code)]
pub mod diskfs;
#[allow(dead_code)]
pub mod fat32;
#[allow(dead_code)]
pub mod xhci;
#[allow(dead_code)]
pub mod usb_hid;
#[allow(dead_code)]
pub mod gdb_stub;
pub mod acpi;

/// Initialize core hardware drivers (PIC, timer, keyboard, mouse).
pub fn init() {
    pic::init();
    timer::init();
    keyboard::init();
    mouse::init();
}

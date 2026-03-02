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
pub mod drm;
pub mod igpu;
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

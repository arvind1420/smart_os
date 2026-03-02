/// PCI Bus Scanner for Smart OS.
///
/// Scans the PCI configuration space via I/O ports 0xCF8/0xCFC
/// to discover devices. Used to find VirtIO block and network devices.

use alloc::vec::Vec;
use x86_64::instructions::port::Port;

/// PCI configuration address port.
const PCI_CONFIG_ADDR: u16 = 0xCF8;
/// PCI configuration data port.
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// VirtIO vendor ID.
pub const VIRTIO_VENDOR: u16 = 0x1AF4;
/// VirtIO legacy block device ID.
pub const VIRTIO_BLK_DEV: u16 = 0x1001;
/// VirtIO legacy network device ID.
pub const VIRTIO_NET_DEV: u16 = 0x1000;

/// A discovered PCI device.
#[derive(Debug, Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub header_type: u8,
    pub bars: [u32; 6],
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
}

/// Build a PCI config space address for a given bus/device/function/offset.
fn pci_address(bus: u8, device: u8, func: u8, offset: u8) -> u32 {
    let bus = bus as u32;
    let device = device as u32;
    let func = func as u32;
    let offset = (offset & 0xFC) as u32; // align to 4 bytes
    (1 << 31) | (bus << 16) | (device << 11) | (func << 8) | offset
}

/// Read 32 bits from PCI configuration space.
pub fn pci_config_read32(bus: u8, device: u8, func: u8, offset: u8) -> u32 {
    let addr = pci_address(bus, device, func, offset);
    unsafe {
        Port::<u32>::new(PCI_CONFIG_ADDR).write(addr);
        Port::<u32>::new(PCI_CONFIG_DATA).read()
    }
}

/// Write 32 bits to PCI configuration space.
pub fn pci_config_write32(bus: u8, device: u8, func: u8, offset: u8, value: u32) {
    let addr = pci_address(bus, device, func, offset);
    unsafe {
        Port::<u32>::new(PCI_CONFIG_ADDR).write(addr);
        Port::<u32>::new(PCI_CONFIG_DATA).write(value);
    }
}

/// Read 16 bits from PCI configuration space.
pub fn pci_config_read16(bus: u8, device: u8, func: u8, offset: u8) -> u16 {
    let val32 = pci_config_read32(bus, device, func, offset & 0xFC);
    let shift = ((offset & 2) as u32) * 8;
    (val32 >> shift) as u16
}

/// Read all 6 Base Address Registers for a device.
fn read_bars(bus: u8, device: u8, func: u8) -> [u32; 6] {
    let mut bars = [0u32; 6];
    for i in 0..6 {
        bars[i] = pci_config_read32(bus, device, func, 0x10 + (i as u8) * 4);
    }
    bars
}

/// Enable bus mastering for a PCI device (required for DMA).
pub fn enable_bus_master(dev: &PciDevice) {
    let cmd = pci_config_read16(dev.bus, dev.device, dev.function, 0x04);
    let new_cmd = cmd | 0x04; // set bit 2 = bus master
    let full = pci_config_read32(dev.bus, dev.device, dev.function, 0x04);
    let new_full = (full & 0xFFFF_0000) | (new_cmd as u32);
    pci_config_write32(dev.bus, dev.device, dev.function, 0x04, new_full);
}

/// Enable I/O space access for a PCI device.
pub fn enable_io_space(dev: &PciDevice) {
    let cmd = pci_config_read16(dev.bus, dev.device, dev.function, 0x04);
    let new_cmd = cmd | 0x01; // set bit 0 = I/O space
    let full = pci_config_read32(dev.bus, dev.device, dev.function, 0x04);
    let new_full = (full & 0xFFFF_0000) | (new_cmd as u32);
    pci_config_write32(dev.bus, dev.device, dev.function, 0x04, new_full);
}

/// Probe a single PCI bus/device/function and return a PciDevice if present.
fn probe_function(bus: u8, device: u8, func: u8) -> Option<PciDevice> {
    let vendor_device = pci_config_read32(bus, device, func, 0x00);
    let vendor_id = (vendor_device & 0xFFFF) as u16;
    if vendor_id == 0xFFFF {
        return None;
    }
    let device_id = (vendor_device >> 16) as u16;

    let class_rev = pci_config_read32(bus, device, func, 0x08);
    let class_code = (class_rev >> 24) as u8;
    let subclass = ((class_rev >> 16) & 0xFF) as u8;

    let header = pci_config_read32(bus, device, func, 0x0C);
    let header_type = ((header >> 16) & 0xFF) as u8;

    let interrupt = pci_config_read32(bus, device, func, 0x3C);
    let interrupt_line = (interrupt & 0xFF) as u8;
    let interrupt_pin = ((interrupt >> 8) & 0xFF) as u8;

    let bars = read_bars(bus, device, func);

    Some(PciDevice {
        bus,
        device,
        function: func,
        vendor_id,
        device_id,
        class_code,
        subclass,
        header_type,
        bars,
        interrupt_line,
        interrupt_pin,
    })
}

/// Scan PCI bus 0 and return all discovered devices.
pub fn scan_bus() -> Vec<PciDevice> {
    let mut devices = Vec::new();

    for device_slot in 0..32u8 {
        let vendor = pci_config_read32(0, device_slot, 0, 0x00) & 0xFFFF;
        if vendor as u16 == 0xFFFF {
            continue;
        }

        // Check function 0
        if let Some(dev) = probe_function(0, device_slot, 0) {
            let multifunction = dev.header_type & 0x80 != 0;
            devices.push(dev);

            // Check other functions only if multifunction
            if multifunction {
                for func in 1..8u8 {
                    if let Some(dev) = probe_function(0, device_slot, func) {
                        devices.push(dev);
                    }
                }
            }
        }
    }

    devices
}

/// Find the first device matching a vendor/device ID pair.
pub fn find_device(vendor: u16, device_id: u16) -> Option<PciDevice> {
    for device_slot in 0..32u8 {
        for func in 0..8u8 {
            if let Some(dev) = probe_function(0, device_slot, func) {
                if dev.vendor_id == vendor && dev.device_id == device_id {
                    return Some(dev);
                }
                // Only check function 0 if not multifunction
                if func == 0 && dev.header_type & 0x80 == 0 {
                    break;
                }
            } else if func == 0 {
                break;
            }
        }
    }
    None
}

/// Find all devices matching a vendor/device ID pair.
pub fn find_all_devices(vendor: u16, device_id: u16) -> Vec<PciDevice> {
    let mut results = Vec::new();
    for device_slot in 0..32u8 {
        for func in 0..8u8 {
            if let Some(dev) = probe_function(0, device_slot, func) {
                if dev.vendor_id == vendor && dev.device_id == device_id {
                    results.push(dev);
                }
                if func == 0 && dev.header_type & 0x80 == 0 {
                    break;
                }
            } else if func == 0 {
                break;
            }
        }
    }
    results
}

/// Find a PCI device by class/subclass/prog_if.
pub fn find_by_class(class: u8, subclass: u8, prog_if: u8) -> Option<PciDevice> {
    for device_slot in 0..32u8 {
        for func in 0..8u8 {
            if let Some(dev) = probe_function(0, device_slot, func) {
                let class_rev = pci_config_read32(dev.bus, dev.device, dev.function, 0x08);
                let dev_prog_if = ((class_rev >> 8) & 0xFF) as u8;
                if dev.class_code == class && dev.subclass == subclass && dev_prog_if == prog_if {
                    return Some(dev);
                }
                if func == 0 && dev.header_type & 0x80 == 0 {
                    break;
                }
            } else if func == 0 {
                break;
            }
        }
    }
    None
}

/// Get the MMIO base address from BAR0 (for memory-mapped devices).
pub fn bar0_mmio_base(dev: &PciDevice) -> Option<u64> {
    let bar0 = dev.bars[0];
    if bar0 & 1 == 0 {
        let bar_type = (bar0 >> 1) & 0x3;
        if bar_type == 2 {
            // 64-bit BAR
            let lo = (bar0 & 0xFFFF_FFF0) as u64;
            let hi = dev.bars[1] as u64;
            Some((hi << 32) | lo)
        } else {
            Some((bar0 & 0xFFFF_FFF0) as u64)
        }
    } else {
        None
    }
}

/// Get the I/O port base from BAR0 (for legacy VirtIO devices).
pub fn bar0_io_base(dev: &PciDevice) -> Option<u16> {
    let bar0 = dev.bars[0];
    if bar0 & 1 == 1 {
        // I/O space BAR
        Some((bar0 & 0xFFFF_FFFC) as u16)
    } else {
        None
    }
}

/// Initialize: scan PCI bus and log discovered devices.
pub fn init() {
    let devices = scan_bus();
    crate::serial_println!("[pci] Found {} device(s):", devices.len());
    for dev in &devices {
        let kind = match (dev.vendor_id, dev.device_id) {
            (VIRTIO_VENDOR, VIRTIO_BLK_DEV) => "VirtIO Block",
            (VIRTIO_VENDOR, VIRTIO_NET_DEV) => "VirtIO Net",
            (VIRTIO_VENDOR, _) => "VirtIO (other)",
            _ => "",
        };
        crate::serial_println!(
            "[pci]   bus={} dev={} fn={} vendor={:#06X} device={:#06X} class={:#04X}/{:#04X} IRQ={} {}",
            dev.bus, dev.device, dev.function,
            dev.vendor_id, dev.device_id,
            dev.class_code, dev.subclass,
            dev.interrupt_line,
            kind,
        );
    }
}

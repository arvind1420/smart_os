/// Universal Hardware Hotplug Manager
///
/// Phase 19: Event-driven device tree for dynamic PCI/USB hotplugging.

use crate::serial_println;
use alloc::vec::Vec;
use spin::Mutex;

pub struct Device {
    pub id: u32,
    pub name: &'static str,
    pub plugged_in: bool,
}

pub static DEVICE_TREE: Mutex<Vec<Device>> = Mutex::new(Vec::new());

pub fn init() {
    serial_println!("[hotplug] Universal Hardware Hotplug Manager initialized.");
}

pub fn handle_device_event(device_id: u32, name: &'static str, plugged_in: bool) {
    let mut tree = DEVICE_TREE.lock();
    if plugged_in {
        serial_println!("[hotplug] Device '{}' (ID: {}) connected. Loading driver...", name, device_id);
        tree.push(Device { id: device_id, name, plugged_in: true });
    } else {
        serial_println!("[hotplug] Device ID: {} disconnected. Unloading driver...", device_id);
        tree.retain(|d| d.id != device_id);
    }
}

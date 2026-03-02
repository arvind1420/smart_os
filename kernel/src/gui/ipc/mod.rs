/// Desktop Hub Service
///
/// Implements a secure IPC registry where apps can publish and subscribe
/// to high-level actions (e.g. "open_file", "copy_text").

use alloc::string::String;
use alloc::collections::BTreeMap;
use spin::Mutex;

pub struct DesktopHub {
    pub registered_apps: BTreeMap<u64, String>,
}

pub static DESKTOP_HUB: Mutex<DesktopHub> = Mutex::new(DesktopHub {
    registered_apps: BTreeMap::new(),
});

pub fn register_app(pid: u64, name: &str) {
    let mut hub = DESKTOP_HUB.lock();
    hub.registered_apps.insert(pid, String::from(name));
}

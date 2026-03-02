/// Desktop Hub Service
///
/// Implements a secure IPC registry where apps can publish and subscribe
/// to high-level actions and share semantic data.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;
use smartpack::Value;

#[derive(Clone)]
pub struct HubMessage {
    pub sender_pid: u64,
    pub topic: String,
    pub payload: Value,
}

pub struct DesktopHub {
    pub registered_apps: BTreeMap<u64, String>,
    pub topics: BTreeMap<String, Vec<HubMessage>>,
}

pub static DESKTOP_HUB: Mutex<DesktopHub> = Mutex::new(DesktopHub {
    registered_apps: BTreeMap::new(),
    topics: BTreeMap::new(),
});

pub fn register_app(pid: u64, name: &str) {
    let mut hub = DESKTOP_HUB.lock();
    hub.registered_apps.insert(pid, String::from(name));
}

pub fn publish(pid: u64, topic: &str, payload: Value) {
    let mut hub = DESKTOP_HUB.lock();
    let msg = HubMessage {
        sender_pid: pid,
        topic: String::from(topic),
        payload,
    };
    hub.topics.entry(String::from(topic)).or_insert_with(Vec::new).push(msg);
}

pub fn query(topic: &str) -> Vec<HubMessage> {
    let hub = DESKTOP_HUB.lock();
    hub.topics.get(topic).cloned().unwrap_or_default()
}

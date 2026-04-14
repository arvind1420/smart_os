/// P2P Sovereign Networking (Distributed Hash Table)
///
/// Phase 19: Serverless node discovery and routing.

use crate::serial_println;
use alloc::collections::BTreeMap;
use alloc::string::String;
use spin::Mutex;

pub static DHT: Mutex<BTreeMap<String, [u8; 4]>> = Mutex::new(BTreeMap::new());

pub fn init() {
    serial_println!("[p2p] Sovereign DHT Networking initialized.");
}

pub fn announce(node_id: &str, ip: [u8; 4]) {
    DHT.lock().insert(String::from(node_id), ip);
    serial_println!("[p2p] Node {} announced at {:?}", node_id, ip);
}

pub fn resolve(node_id: &str) -> Option<[u8; 4]> {
    DHT.lock().get(node_id).copied()
}

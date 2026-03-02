/// Named port registry for IPC service discovery.
///
/// Services register named ports (e.g., "vfs", "gui", "devmgr").
/// Clients look up ports by name to find the right channel.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use spin::Mutex;
use super::channel::Channel;

/// Global port registry.
static REGISTRY: Mutex<Option<PortRegistry>> = Mutex::new(None);

pub struct PortRegistry {
    ports: BTreeMap<String, Arc<Channel>>,
}

impl PortRegistry {
    fn new() -> Self {
        Self {
            ports: BTreeMap::new(),
        }
    }
}

/// Initialize the port registry.
pub fn init() {
    *REGISTRY.lock() = Some(PortRegistry::new());
}

/// Register a named port with a new channel.
pub fn register(name: &str, capacity: usize) -> Arc<Channel> {
    let channel = Arc::new(Channel::new(name, capacity));
    let mut reg = REGISTRY.lock();
    if let Some(ref mut registry) = *reg {
        registry.ports.insert(String::from(name), channel.clone());
    }
    crate::serial_println!("[ipc] Registered port '{}'", name);
    channel
}

/// Look up a named port.
pub fn lookup(name: &str) -> Option<Arc<Channel>> {
    let reg = REGISTRY.lock();
    reg.as_ref().and_then(|r| r.ports.get(name).cloned())
}

/// List all registered port names.
pub fn list_ports() -> alloc::vec::Vec<String> {
    let reg = REGISTRY.lock();
    reg.as_ref()
        .map(|r| r.ports.keys().cloned().collect())
        .unwrap_or_default()
}

/// Get port statistics: (name, pending_message_count) for all ports.
pub fn port_stats() -> alloc::vec::Vec<(String, usize)> {
    let reg = REGISTRY.lock();
    reg.as_ref()
        .map(|r| r.ports.iter().map(|(k, ch)| (k.clone(), ch.len())).collect())
        .unwrap_or_default()
}

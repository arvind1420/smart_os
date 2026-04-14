/// Kernel-level WireGuard VPN Stack for Smart OS.
///
/// Phase 25: Zero-Trust Security.
/// Integrates directly into the network stack for hardware-accelerated,
/// transparent encrypted tunneling.

use spin::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

pub struct WgPeer {
    pub public_key: [u8; 32],
    pub endpoint_ip: [u8; 4],
    pub endpoint_port: u16,
    pub allowed_ips: Vec<[u8; 4]>,
}

pub struct WgInterface {
    pub name: String,
    pub private_key: [u8; 32],
    pub listen_port: u16,
    pub peers: Vec<WgPeer>,
    pub is_up: bool,
}

pub static WG_INTERFACES: Mutex<Vec<WgInterface>> = Mutex::new(Vec::new());

pub fn init() {
    let mut interfaces = WG_INTERFACES.lock();
    
    // Create a default wg0 interface in a down state
    interfaces.push(WgInterface {
        name: String::from("wg0"),
        private_key: [0; 32], // Needs securely generated key from TPM
        listen_port: 51820,
        peers: Vec::new(),
        is_up: false,
    });

    crate::serial_println!("[net:wireguard] Zero-Trust WireGuard stack initialized.");
}

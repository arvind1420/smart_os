/// Wireless Networking (802.11) Framework for Smart OS.
///
/// Phase 22: Initial support for Intel Wi-Fi (iwlwifi) and generic 802.11 stack.
/// Provides the interface for scanning, connecting, and data transmission.

use alloc::vec::Vec;
use alloc::string::String;
use spin::Mutex;
use crate::drivers::pci::{self, PciDevice};

/// Standard Intel Wi-Fi Vendor ID.
pub const INTEL_VENDOR: u16 = 0x8086;

/// Common Intel Wi-Fi Device IDs (Simplified list).
pub const IWLWIFI_DEVICE_7260: u16 = 0x08B1;
pub const IWLWIFI_DEVICE_8265: u16 = 0x24FD;
pub const IWLWIFI_DEVICE_AX200: u16 = 0x2723;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiState {
    Disconnected,
    Scanning,
    Associating,
    Connected,
}

pub trait WirelessDevice: Send {
    fn get_mac(&self) -> [u8; 6];
    fn scan(&mut self) -> Result<Vec<String>, &'static str>;
    fn connect(&mut self, ssid: &str, password: &str) -> Result<(), &'static str>;
    fn send_packet(&mut self, data: &[u8]) -> Result<(), &'static str>;
    fn get_state(&self) -> WifiState;
}

pub struct IntelWifi {
    pub pci: PciDevice,
    pub state: WifiState,
    pub mac: [u8; 6],
}

impl WirelessDevice for IntelWifi {
    fn get_mac(&self) -> [u8; 6] { self.mac }
    
    fn scan(&mut self) -> Result<Vec<String>, &'static str> {
        self.state = WifiState::Scanning;
        // Mock scan results
        Ok(alloc::vec!["SmartOS_Guest".into(), "Enterprise_Secure".into(), "Legacy_2GHz".into()])
    }

    fn connect(&mut self, _ssid: &str, _password: &str) -> Result<(), &'static str> {
        self.state = WifiState::Associating;
        // In real driver: load firmware, send association request
        self.state = WifiState::Connected;
        Ok(())
    }

    fn send_packet(&mut self, _data: &[u8]) -> Result<(), &'static str> {
        if self.state != WifiState::Connected { return Err("Not connected"); }
        Ok(())
    }

    fn get_state(&self) -> WifiState { self.state }
}

pub static WIFI_DEVICES: Mutex<Vec<alloc::boxed::Box<dyn WirelessDevice>>> = Mutex::new(Vec::new());

pub fn init() {
    // Probe for Intel Wi-Fi cards
    let devices = pci::scan_bus();
    let mut wifi_list = WIFI_DEVICES.lock();

    for dev in devices {
        if dev.vendor_id == INTEL_VENDOR && 
           (dev.device_id == IWLWIFI_DEVICE_7260 || 
            dev.device_id == IWLWIFI_DEVICE_8265 || 
            dev.device_id == IWLWIFI_DEVICE_AX200) {
            
            crate::serial_println!("[wifi] Found Intel Wi-Fi controller ({:#06X})", dev.device_id);
            
            let wifi = IntelWifi {
                pci: dev,
                state: WifiState::Disconnected,
                mac: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55], // Placeholder
            };
            wifi_list.push(alloc::boxed::Box::new(wifi));
        }
    }

    if !wifi_list.is_empty() {
        crate::serial_println!("[wifi] Wireless networking initialized ({} device(s) found).", wifi_list.len());
    }
}

pub fn is_available() -> bool {
    !WIFI_DEVICES.lock().is_empty()
}

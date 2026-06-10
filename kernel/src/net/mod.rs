/// Network stack for Smart OS.
///
/// Phase 35: DHCP client added. IP config is now dynamic via NetConfig.
/// Falls back to QEMU SLIRP defaults when DHCP is unavailable.

use spin::Mutex;

pub mod ethernet;
pub mod arp;
pub mod ipv4;
pub mod udp;
pub mod tcp;
pub mod dns;
pub mod ipv6;
pub mod http;
pub mod icmp;
pub mod p2p;
pub mod xr;
pub mod wireguard;
pub mod iot;
pub mod cloud;
pub mod tls;
pub mod dhcp;
pub mod http_client;
pub mod http2;
pub mod quic;
pub mod html;
pub mod css;
pub mod layout;
pub mod paint;
pub mod js_lexer;
pub mod js_ast;
pub mod js_parser;
pub mod js_interp;
pub mod browser_chrome;
pub mod browser;
pub mod canvas;
pub mod websocket;
pub mod web_storage;
pub mod web_workers;
pub mod web_audio;
pub mod fetch;
pub mod css_anim;
pub mod webgl;
pub mod service_worker;
pub mod png;
pub mod jpeg;
pub mod gif;
pub mod inflate;
pub mod cookies;
pub mod sop;
pub mod csp;
pub mod dom;
pub mod svg;
pub mod webp;
pub mod avif;
pub mod woff;
pub mod video;
pub mod webrtc;
pub mod perf;
pub mod hardening;
pub mod browser_polish;
pub mod forms;
pub mod wasm;
pub mod js_completeness;
pub mod css_completeness;
pub mod tab_process;
pub mod js_jit;
pub mod vp8_full;
pub mod devtools;
pub mod webauthn;
pub mod mse;
pub mod webext;
pub mod a11y;
pub mod css_print;
pub mod wpt;
pub mod webcrypto;
pub mod streams;
pub mod pwa;
pub mod url_api;
pub mod observers;
pub mod forms_events;
pub mod web_components;
pub mod css_modern;
pub mod browser_persistence;
pub mod security_polish;
pub mod perf_hints;
pub mod crash_reporter;

/// Static Ethernet broadcast MAC.
pub const BROADCAST_MAC: [u8; 6] = [0xFF; 6];

// Fallback IP config (QEMU SLIRP defaults — used before DHCP completes).
const LOCAL_IP_DEFAULT:   [u8; 4] = [10, 0, 2, 15];
const GATEWAY_IP_DEFAULT: [u8; 4] = [10, 0, 2,  2];
const SUBNET_MASK_DEFAULT:[u8; 4] = [255, 255, 255, 0];
const DNS_DEFAULT:        [u8; 4] = [8, 8, 8, 8];

// Keep backward-compat constants for any code that hasn't migrated yet.
pub const LOCAL_IP:   [u8; 4] = LOCAL_IP_DEFAULT;
pub const GATEWAY_IP: [u8; 4] = GATEWAY_IP_DEFAULT;
pub const SUBNET_MASK:[u8; 4] = SUBNET_MASK_DEFAULT;

/// Live network configuration — updated by DHCP on lease acquisition.
pub struct NetConfig {
    pub local_ip:         [u8; 4],
    pub gateway:          [u8; 4],
    pub subnet:           [u8; 4],
    pub dns:              [u8; 4],
    pub lease_secs:       u32,
    pub dhcp_server:      [u8; 4],
    pub acquired_time_s:  u64,
    pub dhcp_configured:  bool,
}

pub static NET_CONFIG: Mutex<NetConfig> = Mutex::new(NetConfig {
    local_ip:        LOCAL_IP_DEFAULT,
    gateway:         GATEWAY_IP_DEFAULT,
    subnet:          SUBNET_MASK_DEFAULT,
    dns:             DNS_DEFAULT,
    lease_secs:      0,
    dhcp_server:     [0, 0, 0, 0],
    acquired_time_s:  0,
    dhcp_configured: false,
});

/// Current local IP (DHCP-assigned or static fallback).
#[inline]
pub fn local_ip() -> [u8; 4] { NET_CONFIG.lock().local_ip }

/// Current gateway IP.
#[inline]
pub fn gateway_ip() -> [u8; 4] { NET_CONFIG.lock().gateway }

/// Current subnet mask.
#[inline]
pub fn subnet_mask() -> [u8; 4] { NET_CONFIG.lock().subnet }

/// Current DNS server IP.
#[inline]
pub fn dns_server() -> [u8; 4] { NET_CONFIG.lock().dns }

/// Get the MAC address of the active network device.
pub fn mac_address() -> Option<[u8; 6]> {
    if crate::drivers::virtio_net::is_available() {
        crate::drivers::virtio_net::mac_address()
    } else if crate::drivers::e1000::is_available() {
        crate::drivers::e1000::mac_address()
    } else {
        None
    }
}

/// Send a raw Ethernet frame via the active network device.
pub fn send_frame(frame: &[u8]) -> Result<(), &'static str> {
    if crate::drivers::virtio_net::is_available() {
        crate::drivers::virtio_net::send_frame(frame)
    } else if crate::drivers::e1000::is_available() {
        crate::drivers::e1000::send_frame(frame)
    } else {
        Err("No network device available")
    }
}

/// Initialize the network stack and attempt DHCP.
pub fn init() {
    arp::init();
    udp::init();
    tcp::init();
    tls::init();
    p2p::init();
    xr::init();
    wireguard::init();
    iot::init();
    cloud::init();

    let ip = local_ip();
    let gw = gateway_ip();
    crate::serial_println!(
        "[net] Stack initialized: IP={}.{}.{}.{}, gateway={}.{}.{}.{}",
        ip[0], ip[1], ip[2], ip[3],
        gw[0], gw[1], gw[2], gw[3],
    );

    // Attempt DHCP if a network device is present.
    if mac_address().is_some() {
        match dhcp::discover() {
            Ok(()) => {
                let ip = local_ip();
                let gw = gateway_ip();
                crate::serial_println!(
                    "[net] DHCP configured: {}.{}.{}.{} via {}.{}.{}.{}",
                    ip[0], ip[1], ip[2], ip[3],
                    gw[0], gw[1], gw[2], gw[3],
                );
            }
            Err(e) => {
                crate::serial_println!("[net] DHCP failed ({}), using static IP.", e);
            }
        }
    }
}

/// Process an incoming raw Ethernet frame.
pub fn handle_rx_frame(frame: &[u8]) {
    if let Some((header, payload)) = ethernet::parse(frame) {
        let ethertype = u16::from_be_bytes([header.ethertype[0], header.ethertype[1]]);
        match ethertype {
            ethernet::ETHERTYPE_ARP => arp::handle_arp(payload),
            ethernet::ETHERTYPE_IPV4 => {
                if let Some((ip_hdr, ip_payload)) = ipv4::parse(payload) {
                    match ip_hdr.protocol {
                        ipv4::PROTO_UDP => udp::handle_udp(ip_hdr.src_ip, ip_payload),
                        ipv4::PROTO_TCP => tcp::handle_tcp(ip_hdr.src_ip, ip_payload),
                        _ => {}
                    }
                }
            }
            ipv6::ETHERTYPE_IPV6 => {
                ipv6::handle_ipv6(payload);
            }
            _ => {}
        }
    }
}

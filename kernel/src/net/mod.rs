/// Network stack for Smart OS.
///
/// Minimal stack: Ethernet + ARP + IPv4 + UDP.
/// Hardcoded for QEMU SLIRP user-mode networking.

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

/// Network configuration (QEMU SLIRP defaults).
pub const LOCAL_IP: [u8; 4] = [10, 0, 2, 15];
pub const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];
pub const SUBNET_MASK: [u8; 4] = [255, 255, 255, 0];
pub const BROADCAST_MAC: [u8; 6] = [0xFF; 6];

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

/// Initialize the network stack.
pub fn init() {
    arp::init();
    udp::init();
    tcp::init();
    p2p::init();
    xr::init();
    wireguard::init();
    iot::init();

    // Pre-populate ARP cache with gateway
    // (will be resolved on first send if needed)
    crate::serial_println!(
        "[net] Stack initialized: IP={}.{}.{}.{}, gateway={}.{}.{}.{}",
        LOCAL_IP[0], LOCAL_IP[1], LOCAL_IP[2], LOCAL_IP[3],
        GATEWAY_IP[0], GATEWAY_IP[1], GATEWAY_IP[2], GATEWAY_IP[3],
    );
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
                        _ => {} // ignore other protocols
                    }
                }
            }
            ipv6::ETHERTYPE_IPV6 => {
                ipv6::handle_ipv6(payload);
            }
            _ => {} // ignore other ethertypes
        }
    }
}

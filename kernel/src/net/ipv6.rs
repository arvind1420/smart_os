/// IPv6 Protocol Skeleton for Smart OS.
///
/// Phase 10: Minimal IPv6 support — header parsing/building,
/// link-local address generation, ICMPv6 neighbor solicitation.
/// This is a skeleton for future full IPv6 implementation.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use crate::serial_println;

/// IPv6 header (fixed 40 bytes).
#[derive(Debug, Clone)]
pub struct Ipv6Header {
    /// Version (4 bits, always 6), traffic class (8 bits), flow label (20 bits).
    pub version_tc_fl: u32,
    /// Payload length (not including this header).
    pub payload_length: u16,
    /// Next header (protocol).
    pub next_header: u8,
    /// Hop limit (equivalent to TTL).
    pub hop_limit: u8,
    /// Source address (128 bits).
    pub src_addr: [u8; 16],
    /// Destination address (128 bits).
    pub dst_addr: [u8; 16],
}

/// IPv6 Next Header values.
pub const NEXT_HEADER_TCP: u8 = 6;
pub const NEXT_HEADER_UDP: u8 = 17;
pub const NEXT_HEADER_ICMPV6: u8 = 58;

/// EtherType for IPv6.
pub const ETHERTYPE_IPV6: u16 = 0x86DD;

/// Whether IPv6 is enabled.
static IPV6_ENABLED: AtomicBool = AtomicBool::new(false);

/// Our link-local IPv6 address (fe80::).
static LINK_LOCAL_ADDR: spin::Mutex<[u8; 16]> = spin::Mutex::new([0u8; 16]);

/// Initialize IPv6 subsystem.
pub fn init() {
    // Generate link-local address from MAC (EUI-64)
    let mac = crate::net::mac_address().unwrap_or([0; 6]);
    let addr = generate_link_local(&mac);
    *LINK_LOCAL_ADDR.lock() = addr;

    IPV6_ENABLED.store(true, Ordering::Relaxed);

    serial_println!(
        "[ipv6] IPv6 initialized. Link-local: {}",
        format_ipv6(&addr),
    );
}

/// Generate a link-local IPv6 address from a MAC address (EUI-64).
///
/// fe80::xxxx:xxff:fexx:xxxx
fn generate_link_local(mac: &[u8; 6]) -> [u8; 16] {
    let mut addr = [0u8; 16];

    // fe80:: prefix
    addr[0] = 0xfe;
    addr[1] = 0x80;
    // bytes 2-7 are zero (interface ID starts at byte 8)

    // EUI-64 from MAC: insert ff:fe in the middle, flip U/L bit
    addr[8] = mac[0] ^ 0x02; // flip U/L bit
    addr[9] = mac[1];
    addr[10] = mac[2];
    addr[11] = 0xFF;
    addr[12] = 0xFE;
    addr[13] = mac[3];
    addr[14] = mac[4];
    addr[15] = mac[5];

    addr
}

/// Parse an IPv6 header from a byte slice.
///
/// Returns (header, payload) or None.
pub fn parse(data: &[u8]) -> Option<(Ipv6Header, &[u8])> {
    if data.len() < 40 {
        return None;
    }

    let version = (data[0] >> 4) & 0xF;
    if version != 6 {
        return None;
    }

    let version_tc_fl = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let payload_length = u16::from_be_bytes([data[4], data[5]]);
    let next_header = data[6];
    let hop_limit = data[7];

    let mut src_addr = [0u8; 16];
    let mut dst_addr = [0u8; 16];
    src_addr.copy_from_slice(&data[8..24]);
    dst_addr.copy_from_slice(&data[24..40]);

    let header = Ipv6Header {
        version_tc_fl,
        payload_length,
        next_header,
        hop_limit,
        src_addr,
        dst_addr,
    };

    let payload_end = 40 + payload_length as usize;
    let payload = if payload_end <= data.len() {
        &data[40..payload_end]
    } else {
        &data[40..]
    };

    Some((header, payload))
}

/// Build an IPv6 packet.
pub fn build(
    src: &[u8; 16],
    dst: &[u8; 16],
    next_header: u8,
    hop_limit: u8,
    payload: &[u8],
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(40 + payload.len());

    // Version (6) + traffic class (0) + flow label (0)
    let vtf: u32 = 0x6000_0000;
    packet.extend_from_slice(&vtf.to_be_bytes());

    // Payload length
    let plen = payload.len() as u16;
    packet.extend_from_slice(&plen.to_be_bytes());

    // Next header
    packet.push(next_header);

    // Hop limit
    packet.push(hop_limit);

    // Source address
    packet.extend_from_slice(src);

    // Destination address
    packet.extend_from_slice(dst);

    // Payload
    packet.extend_from_slice(payload);

    packet
}

/// Build an ICMPv6 Neighbor Solicitation message.
pub fn build_neighbor_solicitation(target: &[u8; 16]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(24);

    // Type: Neighbor Solicitation (135)
    msg.push(135);
    // Code: 0
    msg.push(0);
    // Checksum (placeholder, would need pseudo-header calc)
    msg.push(0);
    msg.push(0);
    // Reserved
    msg.extend_from_slice(&[0u8; 4]);
    // Target address
    msg.extend_from_slice(target);

    msg
}

/// Format an IPv6 address as a string.
pub fn format_ipv6(addr: &[u8; 16]) -> String {
    alloc::format!(
        "{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}",
        addr[0], addr[1], addr[2], addr[3],
        addr[4], addr[5], addr[6], addr[7],
        addr[8], addr[9], addr[10], addr[11],
        addr[12], addr[13], addr[14], addr[15],
    )
}

/// Get our link-local address.
pub fn link_local_addr() -> [u8; 16] {
    *LINK_LOCAL_ADDR.lock()
}

/// Check if IPv6 is enabled.
pub fn is_enabled() -> bool {
    IPV6_ENABLED.load(Ordering::Relaxed)
}

/// Handle an incoming IPv6 packet.
pub fn handle_ipv6(data: &[u8]) {
    if let Some((header, payload)) = parse(data) {
        match header.next_header {
            NEXT_HEADER_ICMPV6 => {
                handle_icmpv6(&header, payload);
            }
            NEXT_HEADER_UDP => {
                // Future: dispatch to UDP handler
                serial_println!("[ipv6] Received UDPv6 packet ({} bytes)", payload.len());
            }
            NEXT_HEADER_TCP => {
                // Future: dispatch to TCP handler
                serial_println!("[ipv6] Received TCPv6 packet ({} bytes)", payload.len());
            }
            _ => {
                // Ignore unknown next header
            }
        }
    }
}

/// Handle ICMPv6 messages.
fn handle_icmpv6(header: &Ipv6Header, data: &[u8]) {
    if data.is_empty() {
        return;
    }

    let icmp_type = data[0];
    match icmp_type {
        128 => {
            // Echo Request — would send Echo Reply
            serial_println!("[ipv6] ICMPv6 Echo Request from {}", format_ipv6(&header.src_addr));
        }
        135 => {
            // Neighbor Solicitation
            serial_println!("[ipv6] Neighbor Solicitation from {}", format_ipv6(&header.src_addr));
        }
        136 => {
            // Neighbor Advertisement
            serial_println!("[ipv6] Neighbor Advertisement from {}", format_ipv6(&header.src_addr));
        }
        _ => {}
    }
}

/// Get IPv6 stats.
pub fn ipv6_stats() -> (bool, String) {
    let enabled = is_enabled();
    let addr = if enabled {
        format_ipv6(&link_local_addr())
    } else {
        String::from("disabled")
    };
    (enabled, addr)
}

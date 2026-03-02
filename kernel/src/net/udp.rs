/// UDP (User Datagram Protocol) for Smart OS.
///
/// Provides simple bind/send/recv socket-like API over IPv4.

use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;
use super::{ethernet, arp, ipv4, LOCAL_IP, GATEWAY_IP, SUBNET_MASK, BROADCAST_MAC};

const UDP_HEADER_LEN: usize = 8;

/// Received UDP datagram: (source IP, source port, data).
pub type UdpDatagram = ([u8; 4], u16, Vec<u8>);

/// Bound UDP sockets: port → queue of received datagrams.
static UDP_SOCKETS: Mutex<BTreeMap<u16, VecDeque<UdpDatagram>>> = Mutex::new(BTreeMap::new());

/// Initialize UDP subsystem.
pub fn init() {
    // Sockets start empty
}

/// Bind a UDP port for receiving.
pub fn bind(port: u16) -> Result<(), &'static str> {
    let mut sockets = UDP_SOCKETS.lock();
    if sockets.contains_key(&port) {
        return Err("Port already bound");
    }
    sockets.insert(port, VecDeque::new());
    Ok(())
}

/// Send a UDP datagram.
pub fn send(dst_ip: [u8; 4], dst_port: u16, src_port: u16, data: &[u8]) -> Result<(), &'static str> {
    let our_mac = crate::net::mac_address()
        .ok_or("No network device")?;

    // Build UDP header + data
    let udp_length = (UDP_HEADER_LEN + data.len()) as u16;
    let mut udp_packet = Vec::with_capacity(udp_length as usize);
    udp_packet.extend_from_slice(&src_port.to_be_bytes());
    udp_packet.extend_from_slice(&dst_port.to_be_bytes());
    udp_packet.extend_from_slice(&udp_length.to_be_bytes());
    udp_packet.extend_from_slice(&[0u8; 2]); // checksum = 0 (optional in IPv4)
    udp_packet.extend_from_slice(data);

    // Build IPv4 packet
    let ip_packet = ipv4::build(LOCAL_IP, dst_ip, ipv4::PROTO_UDP, &udp_packet);

    // Determine destination MAC
    let dst_mac = if is_local(dst_ip) {
        // Same subnet: ARP for destination directly
        resolve_or_broadcast(dst_ip)
    } else {
        // Different subnet: ARP for gateway
        resolve_or_broadcast(GATEWAY_IP)
    };

    // Build Ethernet frame
    let frame = ethernet::build(dst_mac, our_mac, ethernet::ETHERTYPE_IPV4, &ip_packet);

    crate::net::send_frame(&frame)
}

/// Receive a UDP datagram on a bound port (non-blocking).
pub fn recv(port: u16) -> Option<UdpDatagram> {
    let mut sockets = UDP_SOCKETS.lock();
    if let Some(queue) = sockets.get_mut(&port) {
        queue.pop_front()
    } else {
        None
    }
}

/// Handle an incoming UDP packet (called from IPv4 handler).
pub fn handle_udp(src_ip: [u8; 4], payload: &[u8]) {
    if payload.len() < UDP_HEADER_LEN {
        return;
    }

    let src_port = u16::from_be_bytes([payload[0], payload[1]]);
    let dst_port = u16::from_be_bytes([payload[2], payload[3]]);
    let udp_length = u16::from_be_bytes([payload[4], payload[5]]) as usize;

    if udp_length < UDP_HEADER_LEN || udp_length > payload.len() {
        return;
    }

    let data = &payload[UDP_HEADER_LEN..udp_length];

    let mut sockets = UDP_SOCKETS.lock();
    if let Some(queue) = sockets.get_mut(&dst_port) {
        // Limit queue size to prevent memory exhaustion
        if queue.len() < 64 {
            queue.push_back((src_ip, src_port, data.to_vec()));
        }
    }
}

/// Check if an IP is on the local subnet.
fn is_local(ip: [u8; 4]) -> bool {
    for i in 0..4 {
        if (ip[i] & SUBNET_MASK[i]) != (LOCAL_IP[i] & SUBNET_MASK[i]) {
            return false;
        }
    }
    true
}

/// Resolve IP to MAC, or return broadcast if not cached.
fn resolve_or_broadcast(ip: [u8; 4]) -> [u8; 6] {
    if let Some(mac) = arp::resolve(ip) {
        mac
    } else {
        // Send ARP request and use broadcast for now
        arp::send_arp_request(ip);
        BROADCAST_MAC
    }
}

/// Get count of bound sockets (for diagnostics).
pub fn socket_count() -> usize {
    UDP_SOCKETS.lock().len()
}

/// Get pending datagram count for a port.
pub fn pending_count(port: u16) -> usize {
    UDP_SOCKETS.lock().get(&port).map(|q| q.len()).unwrap_or(0)
}

/// ICMP (Internet Control Message Protocol) for Smart OS.
///
/// Implements ICMP Echo Request/Reply (ping) over the IPv4 stack.

use alloc::vec::Vec;

const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_ECHO_REPLY:   u8 = 0;
const ICMP_HEADER_SIZE:  usize = 8;
const ECHO_DATA_SIZE:    usize = 56; // Standard ping payload

/// Compute the 16-bit one's complement checksum.
fn checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i+1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Build an ICMP Echo Request packet.
fn build_echo_request(seq: u32) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(ICMP_HEADER_SIZE + ECHO_DATA_SIZE);

    // ICMP header
    pkt.push(ICMP_ECHO_REQUEST); // type
    pkt.push(0);                 // code
    pkt.push(0); pkt.push(0);   // checksum placeholder
    pkt.push(0x13); pkt.push(0x37); // identifier
    let seq16 = seq as u16;
    pkt.push((seq16 >> 8) as u8);
    pkt.push(seq16 as u8);

    // Payload: 56 bytes of incrementing data
    for i in 0..ECHO_DATA_SIZE {
        pkt.push(i as u8);
    }

    // Fill checksum
    let cksum = checksum(&pkt);
    pkt[2] = (cksum >> 8) as u8;
    pkt[3] = cksum as u8;

    pkt
}

/// Send an ICMP echo request to `dest_ip` and wait for a reply.
/// Returns the round-trip time in microseconds on success.
pub fn send_ping(dest_ip: [u8; 4], seq: u32) -> Result<u64, &'static str> {
    let icmp_pkt = build_echo_request(seq);

    // Build IPv4 packet with protocol 1 (ICMP)
    let src_ip = super::local_ip();
    let ip_pkt = super::ipv4::build(src_ip, dest_ip, 1, &icmp_pkt);

    // Resolve destination MAC via ARP
    let dst_mac = super::arp::resolve(dest_ip).unwrap_or(super::BROADCAST_MAC);
    let src_mac = super::mac_address().unwrap_or([0u8; 6]);

    let eth_frame = super::ethernet::build(dst_mac, src_mac, 0x0800, &ip_pkt);

    // Transmit
    let t_start = crate::drivers::timer::ticks();
    crate::net::send_frame(&eth_frame)?;

    // Poll for ICMP reply (up to 200ms)
    let deadline = t_start + 20; // 20 ticks = 200ms at 100Hz
    loop {
        let now = crate::drivers::timer::ticks();
        if now >= deadline { return Err("timeout"); }

        if let Some(reply) = poll_icmp_reply(dest_ip, seq) {
            let rtt = (crate::drivers::timer::ticks() - t_start) * 10_000; // ticks to µs
            let _ = reply;
            return Ok(rtt);
        }
        core::hint::spin_loop();
    }
}

/// Poll the network RX queue for an ICMP echo reply from the given source IP and seq.
fn poll_icmp_reply(_src_ip: [u8; 4], _seq: u32) -> Option<Vec<u8>> {
    // In a full implementation this would drain the RX ring buffer looking for
    // IPv4/ICMP packets with type=0 (echo reply) and matching identifier/seq.
    // For now we return None (timeout path) since we don't have an RX demux queue.
    None
}

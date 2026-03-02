/// IPv4 packet parsing and building for Smart OS.

use alloc::vec::Vec;

pub const PROTO_UDP: u8 = 17;
pub const PROTO_TCP: u8 = 6;
pub const IPV4_HEADER_LEN: usize = 20; // minimum header, no options

/// IPv4 header (20 bytes, no options).
#[derive(Debug, Clone, Copy)]
pub struct Ipv4Header {
    pub version_ihl: u8,
    pub dscp_ecn: u8,
    pub total_length: u16,
    pub identification: u16,
    pub flags_fragment: u16,
    pub ttl: u8,
    pub protocol: u8,
    pub checksum: u16,
    pub src_ip: [u8; 4],
    pub dst_ip: [u8; 4],
}

/// Parse an IPv4 packet into header and payload.
pub fn parse(data: &[u8]) -> Option<(Ipv4Header, &[u8])> {
    if data.len() < IPV4_HEADER_LEN {
        return None;
    }

    let version = data[0] >> 4;
    if version != 4 {
        return None;
    }

    let ihl = (data[0] & 0x0F) as usize * 4;
    if ihl < IPV4_HEADER_LEN || data.len() < ihl {
        return None;
    }

    let total_length = u16::from_be_bytes([data[2], data[3]]);
    let actual_len = (total_length as usize).min(data.len());

    let mut src_ip = [0u8; 4];
    let mut dst_ip = [0u8; 4];
    src_ip.copy_from_slice(&data[12..16]);
    dst_ip.copy_from_slice(&data[16..20]);

    let header = Ipv4Header {
        version_ihl: data[0],
        dscp_ecn: data[1],
        total_length,
        identification: u16::from_be_bytes([data[4], data[5]]),
        flags_fragment: u16::from_be_bytes([data[6], data[7]]),
        ttl: data[8],
        protocol: data[9],
        checksum: u16::from_be_bytes([data[10], data[11]]),
        src_ip,
        dst_ip,
    };

    let payload = &data[ihl..actual_len];
    Some((header, payload))
}

/// Build an IPv4 packet.
pub fn build(src_ip: [u8; 4], dst_ip: [u8; 4], protocol: u8, payload: &[u8]) -> Vec<u8> {
    let total_length = (IPV4_HEADER_LEN + payload.len()) as u16;

    let mut packet = Vec::with_capacity(total_length as usize);

    // Version (4) + IHL (5) = 0x45
    packet.push(0x45);
    // DSCP + ECN
    packet.push(0x00);
    // Total length
    packet.extend_from_slice(&total_length.to_be_bytes());
    // Identification
    static ID_COUNTER: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(1);
    let id = ID_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    packet.extend_from_slice(&id.to_be_bytes());
    // Flags (Don't Fragment) + Fragment offset
    packet.extend_from_slice(&0x4000u16.to_be_bytes());
    // TTL
    packet.push(64);
    // Protocol
    packet.push(protocol);
    // Checksum placeholder (will be filled in)
    packet.extend_from_slice(&[0u8; 2]);
    // Source IP
    packet.extend_from_slice(&src_ip);
    // Destination IP
    packet.extend_from_slice(&dst_ip);

    // Calculate and fill in checksum
    let cksum = compute_checksum(&packet[..IPV4_HEADER_LEN]);
    packet[10] = (cksum >> 8) as u8;
    packet[11] = (cksum & 0xFF) as u8;

    // Payload
    packet.extend_from_slice(payload);

    packet
}

/// Compute IPv4 header checksum (ones-complement sum of 16-bit words).
pub fn compute_checksum(header: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let len = header.len();
    let mut i = 0;

    while i + 1 < len {
        let word = ((header[i] as u32) << 8) | (header[i + 1] as u32);
        sum += word;
        i += 2;
    }

    // Handle odd byte
    if i < len {
        sum += (header[i] as u32) << 8;
    }

    // Fold carries
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !sum as u16
}

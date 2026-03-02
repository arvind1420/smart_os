/// Ethernet frame parsing and building.

use alloc::vec::Vec;

pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETH_HEADER_LEN: usize = 14;

/// Ethernet frame header (14 bytes).
#[derive(Debug, Clone, Copy)]
pub struct EthernetHeader {
    pub dst_mac: [u8; 6],
    pub src_mac: [u8; 6],
    pub ethertype: [u8; 2], // big-endian
}

/// Parse an Ethernet frame into header and payload.
pub fn parse(frame: &[u8]) -> Option<(EthernetHeader, &[u8])> {
    if frame.len() < ETH_HEADER_LEN {
        return None;
    }

    let mut dst_mac = [0u8; 6];
    let mut src_mac = [0u8; 6];
    dst_mac.copy_from_slice(&frame[0..6]);
    src_mac.copy_from_slice(&frame[6..12]);
    let ethertype = [frame[12], frame[13]];

    let header = EthernetHeader { dst_mac, src_mac, ethertype };
    let payload = &frame[ETH_HEADER_LEN..];
    Some((header, payload))
}

/// Build an Ethernet frame from components.
pub fn build(dst_mac: [u8; 6], src_mac: [u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(ETH_HEADER_LEN + payload.len());
    frame.extend_from_slice(&dst_mac);
    frame.extend_from_slice(&src_mac);
    frame.extend_from_slice(&ethertype.to_be_bytes());
    frame.extend_from_slice(payload);

    // Pad to minimum Ethernet frame size (60 bytes without FCS)
    while frame.len() < 60 {
        frame.push(0);
    }

    frame
}

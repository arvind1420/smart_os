/// ARP (Address Resolution Protocol) for Smart OS.
///
/// Handles ARP requests/replies and maintains an IP→MAC cache.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;
use super::{ethernet, BROADCAST_MAC};

const ARP_HW_ETHERNET: u16 = 1;
const ARP_PROTO_IPV4: u16 = 0x0800;
const ARP_OP_REQUEST: u16 = 1;
const ARP_OP_REPLY: u16 = 2;
const ARP_PACKET_LEN: usize = 28;

/// ARP cache: IP address → MAC address.
static ARP_CACHE: Mutex<BTreeMap<[u8; 4], [u8; 6]>> = Mutex::new(BTreeMap::new());

/// Initialize ARP subsystem.
pub fn init() {
    // Cache starts empty; entries added on ARP replies
}

/// Handle an incoming ARP packet.
pub fn handle_arp(payload: &[u8]) {
    if payload.len() < ARP_PACKET_LEN {
        return;
    }

    let operation = u16::from_be_bytes([payload[6], payload[7]]);
    let mut sender_mac = [0u8; 6];
    let mut sender_ip = [0u8; 4];
    let mut target_ip = [0u8; 4];

    sender_mac.copy_from_slice(&payload[8..14]);
    sender_ip.copy_from_slice(&payload[14..18]);
    // target_mac at 18..24 (not needed for cache)
    target_ip.copy_from_slice(&payload[24..28]);

    // Always update cache with sender info
    ARP_CACHE.lock().insert(sender_ip, sender_mac);

    match operation {
        ARP_OP_REQUEST => {
            // If they're asking for our MAC, send a reply
            if target_ip == super::local_ip() {
                send_arp_reply(sender_mac, sender_ip);
            }
        }
        ARP_OP_REPLY => {
            // Already cached above
        }
        _ => {}
    }
}

/// Send an ARP reply to the given target.
fn send_arp_reply(target_mac: [u8; 6], target_ip: [u8; 4]) {
    let our_mac = match crate::net::mac_address() {
        Some(m) => m,
        None => return,
    };

    let mut arp = Vec::with_capacity(ARP_PACKET_LEN);
    arp.extend_from_slice(&ARP_HW_ETHERNET.to_be_bytes());
    arp.extend_from_slice(&ARP_PROTO_IPV4.to_be_bytes());
    arp.push(6); // hardware address length
    arp.push(4); // protocol address length
    arp.extend_from_slice(&ARP_OP_REPLY.to_be_bytes());
    arp.extend_from_slice(&our_mac);      // sender MAC (us)
    arp.extend_from_slice(&super::local_ip());      // sender IP (us)
    arp.extend_from_slice(&target_mac);    // target MAC
    arp.extend_from_slice(&target_ip);     // target IP

    let frame = ethernet::build(target_mac, our_mac, ethernet::ETHERTYPE_ARP, &arp);
    crate::net::send_frame(&frame).ok();
}

/// Send an ARP request for the given IP address.
pub fn send_arp_request(target_ip: [u8; 4]) {
    let our_mac = match crate::net::mac_address() {
        Some(m) => m,
        None => return,
    };

    let mut arp = Vec::with_capacity(ARP_PACKET_LEN);
    arp.extend_from_slice(&ARP_HW_ETHERNET.to_be_bytes());
    arp.extend_from_slice(&ARP_PROTO_IPV4.to_be_bytes());
    arp.push(6);
    arp.push(4);
    arp.extend_from_slice(&ARP_OP_REQUEST.to_be_bytes());
    arp.extend_from_slice(&our_mac);
    arp.extend_from_slice(&super::local_ip());
    arp.extend_from_slice(&[0u8; 6]);     // target MAC = unknown
    arp.extend_from_slice(&target_ip);

    let frame = ethernet::build(BROADCAST_MAC, our_mac, ethernet::ETHERTYPE_ARP, &arp);
    crate::net::send_frame(&frame).ok();
}

/// Look up the MAC address for an IP. Returns None if not in cache.
pub fn resolve(ip: [u8; 4]) -> Option<[u8; 6]> {
    let cache = ARP_CACHE.lock();
    cache.get(&ip).copied()
}

/// Get ARP cache size (for diagnostics).
pub fn cache_size() -> usize {
    ARP_CACHE.lock().len()
}

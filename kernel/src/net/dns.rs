/// DNS resolver for Smart OS.
///
/// Implements a minimal DNS client (RFC 1035) for resolving hostnames to IPv4
/// addresses. Uses the OS UDP stack to query a DNS server (QEMU SLIRP at 10.0.2.3)
/// and caches results with TTL-based expiration.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;

/// QEMU SLIRP DNS server address.
const DNS_SERVER: [u8; 4] = [10, 0, 2, 3];

/// DNS standard port.
const DNS_PORT: u16 = 53;

/// Local port used for DNS queries.
const DNS_LOCAL_PORT: u16 = 10053;

/// Number of poll iterations before timeout (~2 seconds at scheduler tick rate).
const RECV_TIMEOUT_ITERS: usize = 200;

/// DNS record type A (IPv4 address).
const QTYPE_A: u16 = 1;

/// DNS class IN (Internet).
const QCLASS_IN: u16 = 1;

/// DNS cache: hostname -> (resolved IP, expiry timestamp in ms).
static DNS_CACHE: Mutex<BTreeMap<String, ([u8; 4], u64)>> = Mutex::new(BTreeMap::new());

/// Initialize the DNS subsystem (lazy init, nothing required).
pub fn init() {}

/// Resolve a hostname to an IPv4 address.
///
/// Checks the cache first. On cache miss, sends a DNS query to the configured
/// server and waits for a response with timeout.
pub fn resolve(name: &str) -> Result<[u8; 4], &'static str> {
    let now_ms = crate::drivers::timer::uptime_ms();

    // Check cache
    {
        let cache = DNS_CACHE.lock();
        if let Some(&(ip, expire)) = cache.get(name) {
            if now_ms < expire {
                return Ok(ip);
            }
        }
    }

    // Generate transaction ID from timer ticks
    let tx_id = (crate::drivers::timer::ticks() & 0xFFFF) as u16;

    // Build DNS query
    let query = build_query(name, tx_id);

    // Bind our local port (ignore error if already bound)
    let _ = super::udp::bind(DNS_LOCAL_PORT);

    // Send query to DNS server
    super::udp::send(DNS_SERVER, DNS_PORT, DNS_LOCAL_PORT, &query)
        .map_err(|_| "DNS send failed")?;

    // Poll for response with timeout
    for _ in 0..RECV_TIMEOUT_ITERS {
        if let Some((src_ip, _src_port, data)) = super::udp::recv(DNS_LOCAL_PORT) {
            // Verify it came from our DNS server and matches transaction ID
            if src_ip == DNS_SERVER && data.len() >= 12 {
                let resp_id = u16::from_be_bytes([data[0], data[1]]);
                if resp_id == tx_id {
                    if let Some((ip, ttl)) = parse_response(&data) {
                        // Cache the result
                        let ttl_ms = (ttl as u64) * 1000;
                        let expire = now_ms + ttl_ms.max(5000); // minimum 5s cache
                        DNS_CACHE.lock().insert(String::from(name), (ip, expire));
                        return Ok(ip);
                    } else {
                        return Err("DNS parse failed");
                    }
                }
            }
        }
        crate::process::scheduler::yield_now();
    }

    Err("DNS resolve timeout")
}

/// Build a DNS query packet for a hostname.
///
/// Constructs a standard recursive query with one question for an A record.
fn build_query(name: &str, tx_id: u16) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(64);

    // Header (12 bytes)
    pkt.extend_from_slice(&tx_id.to_be_bytes());     // Transaction ID
    pkt.extend_from_slice(&0x0100u16.to_be_bytes());  // Flags: standard query, recursion desired
    pkt.extend_from_slice(&1u16.to_be_bytes());        // QDCOUNT: 1 question
    pkt.extend_from_slice(&0u16.to_be_bytes());        // ANCOUNT: 0
    pkt.extend_from_slice(&0u16.to_be_bytes());        // NSCOUNT: 0
    pkt.extend_from_slice(&0u16.to_be_bytes());        // ARCOUNT: 0

    // Question: encode domain name as length-prefixed labels
    for label in name.split('.') {
        let len = label.len();
        if len > 0 && len <= 63 {
            pkt.push(len as u8);
            pkt.extend_from_slice(label.as_bytes());
        }
    }
    pkt.push(0x00); // Root label terminator

    // QTYPE = A (1), QCLASS = IN (1)
    pkt.extend_from_slice(&QTYPE_A.to_be_bytes());
    pkt.extend_from_slice(&QCLASS_IN.to_be_bytes());

    pkt
}

/// Parse a DNS response and extract the first A record.
///
/// Returns the IPv4 address and TTL on success.
fn parse_response(data: &[u8]) -> Option<([u8; 4], u32)> {
    if data.len() < 12 {
        return None;
    }

    // Read answer count from header
    let ancount = u16::from_be_bytes([data[4], data[5]]);
    if ancount == 0 {
        return None;
    }

    // Skip header (12 bytes)
    let mut pos = 12;

    // Skip question section: scan the QNAME (labels terminated by 0x00)
    // then skip QTYPE (2) + QCLASS (2) = 4 bytes
    pos = skip_name(data, pos)?;
    pos += 4; // QTYPE + QCLASS
    if pos > data.len() {
        return None;
    }

    // Parse answer records, looking for first A record
    for _ in 0..ancount {
        if pos >= data.len() {
            return None;
        }

        // Skip answer name (may use compression pointer)
        pos = skip_name(data, pos)?;

        // Need at least 10 bytes: type(2) + class(2) + ttl(4) + rdlength(2)
        if pos + 10 > data.len() {
            return None;
        }

        let rtype = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let _rclass = u16::from_be_bytes([data[pos + 2], data[pos + 3]]);
        let ttl = u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
        let rdlength = u16::from_be_bytes([data[pos + 8], data[pos + 9]]) as usize;
        pos += 10;

        if pos + rdlength > data.len() {
            return None;
        }

        // A record: type=1, rdlength=4
        if rtype == QTYPE_A && rdlength == 4 {
            let ip = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
            return Some((ip, ttl));
        }

        // Skip this record's data
        pos += rdlength;
    }

    None
}

/// Skip a DNS name at the given position, handling compression pointers.
///
/// Returns the position immediately after the name encoding.
fn skip_name(data: &[u8], mut pos: usize) -> Option<usize> {
    let mut jumped = false;
    let mut end_pos = 0;

    loop {
        if pos >= data.len() {
            return None;
        }

        let len = data[pos] as usize;

        if len == 0 {
            // Root label terminator
            pos += 1;
            break;
        } else if len & 0xC0 == 0xC0 {
            // Compression pointer: 2-byte pointer (0xC0xx)
            if pos + 1 >= data.len() {
                return None;
            }
            if !jumped {
                end_pos = pos + 2; // After the pointer is where we continue
            }
            let offset = ((len & 0x3F) << 8) | (data[pos + 1] as usize);
            pos = offset;
            jumped = true;
        } else {
            // Regular label: skip length byte + label bytes
            pos += 1 + len;
        }
    }

    if jumped {
        Some(end_pos)
    } else {
        Some(pos)
    }
}

/// Get the number of cached DNS entries (for diagnostics).
pub fn cache_count() -> usize {
    DNS_CACHE.lock().len()
}

/// Clear the DNS cache.
pub fn clear_cache() {
    DNS_CACHE.lock().clear();
}

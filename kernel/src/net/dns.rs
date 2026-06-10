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

/// Number of poll iterations before timeout.
/// Each iteration yields to the scheduler so the net-rx thread can run.
/// 10 000 iterations ≈ 2-5 seconds of real time — enough for VirtualBox NAT DNS.
const RECV_TIMEOUT_ITERS: usize = 10_000;

/// DNS record type A (IPv4 address).
const QTYPE_A: u16 = 1;

/// DNS class IN (Internet).
const QCLASS_IN: u16 = 1;

/// DNS cache: hostname -> (resolved IP, expiry timestamp in ms).
static DNS_CACHE: Mutex<BTreeMap<String, ([u8; 4], u64)>> = Mutex::new(BTreeMap::new());

/// Initialize the DNS subsystem (lazy init, nothing required).
pub fn init() {}

fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut parts = s.split('.');
    let a = parts.next()?.parse::<u8>().ok()?;
    let b = parts.next()?.parse::<u8>().ok()?;
    let c = parts.next()?.parse::<u8>().ok()?;
    let d = parts.next()?.parse::<u8>().ok()?;
    if parts.next().is_none() {
        Some([a, b, c, d])
    } else {
        None
    }
}

fn tls_recv_all(id: super::tls::TlsConnId, buf: &mut [u8]) -> Result<(), &'static str> {
    let mut pos = 0;
    let mut idle = 0usize;
    const TIMEOUT: usize = 50_000;
    while pos < buf.len() {
        match super::tls::recv(id, &mut buf[pos..]) {
            Ok(0) => {
                idle += 1;
                if idle > TIMEOUT {
                    return Err("DoT recv timeout");
                }
                crate::process::scheduler::yield_now();
            }
            Ok(n) => {
                pos += n;
                idle = 0;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn resolve_dot_one(server: &str, name: &str) -> Result<([u8; 4], u32), &'static str> {
    let id = super::tls::connect(server, 853)?;

    // Generate transaction ID from timer ticks
    let tx_id = (crate::drivers::timer::ticks() & 0xFFFF) as u16;
    let query = build_query(name, tx_id);

    let mut pkt = Vec::with_capacity(2 + query.len());
    let len = query.len() as u16;
    pkt.extend_from_slice(&len.to_be_bytes());
    pkt.extend_from_slice(&query);

    if let Err(e) = super::tls::send(id, &pkt) {
        let _ = super::tls::close(id);
        return Err(e);
    }

    // Read 2-byte response length
    let mut len_buf = [0u8; 2];
    if let Err(e) = tls_recv_all(id, &mut len_buf) {
        let _ = super::tls::close(id);
        return Err(e);
    }
    let resp_len = u16::from_be_bytes(len_buf) as usize;
    if resp_len > 65535 || resp_len < 12 {
        let _ = super::tls::close(id);
        return Err("DoT response length invalid");
    }

    let mut resp_buf = alloc::vec![0u8; resp_len];
    if let Err(e) = tls_recv_all(id, &mut resp_buf) {
        let _ = super::tls::close(id);
        return Err(e);
    }

    let _ = super::tls::close(id);

    let resp_id = u16::from_be_bytes([resp_buf[0], resp_buf[1]]);
    if resp_id == tx_id {
        if let Some((ip, ttl)) = parse_response(&resp_buf) {
            Ok((ip, ttl))
        } else {
            Err("DoT parse response failed")
        }
    } else {
        Err("DoT transaction ID mismatch")
    }
}

fn resolve_dot_query(name: &str) -> Result<([u8; 4], u32), &'static str> {
    let servers = ["1.1.1.1", "8.8.8.8"];
    let mut last_err = "No servers tried";
    for server in &servers {
        crate::serial_println!("[dns] trying DoT resolution via {}", server);
        match resolve_dot_one(server, name) {
            Ok(res) => return Ok(res),
            Err(e) => {
                crate::serial_println!("[dns] DoT resolver {} failed: {}", server, e);
                last_err = e;
            }
        }
    }
    Err(last_err)
}

fn insert_cache(name: &str, ip: [u8; 4], expire: u64, now_ms: u64) {
    let mut cache = DNS_CACHE.lock();
    // 1. Evict expired entries
    cache.retain(|_, &mut (_, exp)| now_ms < exp);

    // 2. If size still >= 512, evict the entry with the oldest expiration time (min expire timestamp)
    if cache.len() >= 512 {
        let mut min_key = None;
        let mut min_val = u64::MAX;
        for (k, &(_, exp)) in cache.iter() {
            if exp < min_val {
                min_val = exp;
                min_key = Some(k.clone());
            }
        }
        if let Some(k) = min_key {
            cache.remove(&k);
            crate::serial_println!("[dns] evicted cache entry '{}' due to cache size limit", k);
        }
    }

    cache.insert(String::from(name), (ip, expire));
}

/// Resolve a hostname to an IPv4 address.
///
/// Checks the cache first. On cache miss, sends a DNS query to the configured
/// server and waits for a response with timeout.
pub fn resolve(name: &str) -> Result<[u8; 4], &'static str> {
    let now_ms = crate::drivers::timer::uptime_ms();

    if let Some(ip) = parse_ipv4(name) {
        return Ok(ip);
    }

    // Check cache
    {
        let cache = DNS_CACHE.lock();
        if let Some(&(ip, expire)) = cache.get(name) {
            if now_ms < expire {
                crate::serial_println!("[dns] cache hit: {} → {}.{}.{}.{}", name, ip[0], ip[1], ip[2], ip[3]);
                return Ok(ip);
            }
        }
    }

    crate::serial_println!("[dns] resolving: {}", name);

    // Try DoT (DNS over TLS) first
    match resolve_dot_query(name) {
        Ok((ip, ttl)) => {
            let ttl_ms = (ttl as u64) * 1000;
            let expire = now_ms + ttl_ms.max(5000);
            insert_cache(name, ip, expire, now_ms);
            crate::serial_println!("[dns] resolved (DoT): {} → {}.{}.{}.{}", name, ip[0], ip[1], ip[2], ip[3]);
            return Ok(ip);
        }
        Err(e) => {
            crate::serial_println!("[dns] WARNING: DoT failed, falling back to UDP: {}", e);
        }
    }

    // Generate transaction ID from timer ticks
    let tx_id = (crate::drivers::timer::ticks() & 0xFFFF) as u16;

    // Build DNS query
    let query = build_query(name, tx_id);

    // Bind our local port (ignore error if already bound)
    let _ = super::udp::bind(DNS_LOCAL_PORT);

    // Use DHCP-assigned DNS if available, otherwise fall back to VirtualBox NAT proxy
    let dns_server = {
        let cfg = super::NET_CONFIG.lock();
        if cfg.dhcp_configured && cfg.dns != [0, 0, 0, 0] {
            cfg.dns
        } else {
            DNS_SERVER // 10.0.2.3 — VirtualBox NAT internal DNS proxy
        }
    };

    // Send query to DNS server
    super::udp::send(dns_server, DNS_PORT, DNS_LOCAL_PORT, &query)
        .map_err(|_| "DNS send failed")?;

    // Poll for response with timeout
    for _ in 0..RECV_TIMEOUT_ITERS {
        if let Some((src_ip, _src_port, data)) = super::udp::recv(DNS_LOCAL_PORT) {
            // Verify it came from our DNS server and matches transaction ID
            if src_ip == dns_server && data.len() >= 12 {
                let resp_id = u16::from_be_bytes([data[0], data[1]]);
                if resp_id == tx_id {
                    if let Some((ip, ttl)) = parse_response(&data) {
                        // Cache the result
                        let ttl_ms = (ttl as u64) * 1000;
                        let expire = now_ms + ttl_ms.max(5000); // minimum 5s cache
                        insert_cache(name, ip, expire, now_ms);
                        crate::serial_println!("[dns] resolved: {} → {}.{}.{}.{}", name, ip[0], ip[1], ip[2], ip[3]);
                        return Ok(ip);
                    } else {
                        crate::serial_println!("[dns] parse failed for {}", name);
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

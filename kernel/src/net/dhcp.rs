/// DHCP client for Smart OS — Phase 35.
///
/// Implements the DISCOVER → OFFER → REQUEST → ACK four-way handshake
/// over UDP port 68 (client) ↔ 67 (server).
/// On success updates `net::NET_CONFIG` with the leased IP, mask, gateway, DNS.

use alloc::vec::Vec;

const CLIENT_PORT: u16 = 68;
const SERVER_PORT: u16 = 67;

const DHCP_MAGIC: [u8; 4] = [0x63, 0x82, 0x53, 0x63];
const BROADCAST_IP: [u8; 4] = [255, 255, 255, 255];
const ZERO_IP:      [u8; 4] = [0, 0, 0, 0];

// DHCP message type codes (option 53)
const DHCPDISCOVER: u8 = 1;
const DHCPOFFER:    u8 = 2;
const DHCPREQUEST:  u8 = 3;
const DHCPACK:      u8 = 5;

// Option codes
const OPT_SUBNET_MASK:   u8 = 1;
const OPT_ROUTER:        u8 = 3;
const OPT_DNS:           u8 = 6;
const OPT_REQUESTED_IP:  u8 = 50;
const OPT_LEASE_TIME:    u8 = 51;
const OPT_MSG_TYPE:      u8 = 53;
const OPT_SERVER_ID:     u8 = 54;
const OPT_PARAM_REQUEST: u8 = 55;
const OPT_END:           u8 = 255;

// ── Packet builder ────────────────────────────────────────────────────────────

fn build_packet(xid: u32, chaddr: [u8; 6], ciaddr: [u8; 4], options: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(300);
    p.push(1);                                   // op: BOOTREQUEST
    p.push(1);                                   // htype: Ethernet
    p.push(6);                                   // hlen: MAC length
    p.push(0);                                   // hops
    p.extend_from_slice(&xid.to_be_bytes());     // xid
    p.extend_from_slice(&[0u8; 2]);              // secs
    p.extend_from_slice(&[0x80, 0x00]);          // flags: broadcast
    p.extend_from_slice(&ciaddr);                // ciaddr (0 during discover/request)
    p.extend_from_slice(&ZERO_IP);               // yiaddr
    p.extend_from_slice(&ZERO_IP);               // siaddr
    p.extend_from_slice(&ZERO_IP);               // giaddr
    p.extend_from_slice(&chaddr);               // chaddr[6]
    p.extend_from_slice(&[0u8; 10]);             // chaddr padding to 16
    p.extend_from_slice(&[0u8; 64]);             // sname
    p.extend_from_slice(&[0u8; 128]);            // file
    p.extend_from_slice(&DHCP_MAGIC);            // magic cookie
    p.extend_from_slice(options);
    p
}

// ── Option parser ─────────────────────────────────────────────────────────────

fn get_opt<'a>(opts: &'a [u8], code: u8) -> Option<&'a [u8]> {
    let mut i = 0;
    while i < opts.len() {
        let c = opts[i];
        if c == OPT_END { break; }
        if c == 0 { i += 1; continue; } // PAD
        if i + 1 >= opts.len() { break; }
        let len = opts[i + 1] as usize;
        if i + 2 + len > opts.len() { break; }
        if c == code { return Some(&opts[i + 2..i + 2 + len]); }
        i += 2 + len;
    }
    None
}

fn ip4(v: &[u8]) -> Option<[u8; 4]> {
    if v.len() >= 4 { Some([v[0], v[1], v[2], v[3]]) } else { None }
}

// ── Response parser ───────────────────────────────────────────────────────────

fn parse_response(data: &[u8], xid: u32) -> Option<([u8; 4], &[u8])> {
    if data.len() < 240 { return None; }
    if data[0] != 2 { return None; } // must be BOOTREPLY
    if u32::from_be_bytes([data[4], data[5], data[6], data[7]]) != xid { return None; }
    if data[236..240] != DHCP_MAGIC { return None; }
    let yiaddr = [data[16], data[17], data[18], data[19]];
    Some((yiaddr, &data[240..]))
}

// ── Receive loop ──────────────────────────────────────────────────────────────

/// Poll the NIC inline so DHCP can receive its reply even before the net-rx
/// thread has been spawned (which happens *after* net::init() returns).
fn poll_nic_once(rx_buf: &mut [u8]) {
    let len = if crate::drivers::virtio_net::is_available() {
        crate::drivers::virtio_net::recv_frame(rx_buf).unwrap_or(0)
    } else if crate::drivers::e1000::is_available() {
        crate::drivers::e1000::recv_frame(rx_buf).unwrap_or(0)
    } else {
        0
    };
    if len > 0 {
        super::handle_rx_frame(&rx_buf[..len]);
    }
}

fn wait_for(xid: u32, expected_type: u8) -> Option<(Vec<u8>, [u8; 4])> {
    let mut rx_buf = [0u8; 2048];
    for _ in 0..8000usize {
        // Drive the NIC RX ring directly — the net-rx thread may not exist yet.
        poll_nic_once(&mut rx_buf);

        if let Some((_src, _port, data)) = super::udp::recv(CLIENT_PORT) {
            if let Some((yiaddr, opts)) = parse_response(&data, xid) {
                if get_opt(opts, OPT_MSG_TYPE).map(|v| v.first().copied()) == Some(Some(expected_type)) {
                    return Some((data, yiaddr));
                }
            }
        }
        crate::process::scheduler::yield_now();
    }
    None
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Run a full DHCP DISCOVER→OFFER→REQUEST→ACK cycle.
/// Updates `net::NET_CONFIG` on success.
pub fn discover() -> Result<(), &'static str> {
    let mac = crate::net::mac_address().ok_or("DHCP: no network device")?;

    // Bind our receive port (ignore "already bound" errors)
    let _ = super::udp::bind(CLIENT_PORT);

    // Deterministic-but-varied XID from timer
    let xid = (crate::drivers::timer::ticks() & 0xFFFF_FFFF) as u32 | 0xDC00_0000;

    // ── DISCOVER ──────────────────────────────────────────────────────────────
    let disc_opts: &[u8] = &[
        OPT_MSG_TYPE,      1, DHCPDISCOVER,
        OPT_PARAM_REQUEST, 4, OPT_SUBNET_MASK, OPT_ROUTER, OPT_DNS, OPT_LEASE_TIME,
        OPT_END,
    ];
    let disc_pkt = build_packet(xid, mac, ZERO_IP, disc_opts);

    crate::serial_println!("[dhcp] DISCOVER xid={:#010x}", xid);
    super::udp::send_raw(ZERO_IP, BROADCAST_IP, SERVER_PORT, CLIENT_PORT, &disc_pkt)?;

    // ── OFFER ─────────────────────────────────────────────────────────────────
    let (offer_data, offered_ip) = wait_for(xid, DHCPOFFER)
        .ok_or("DHCP: no OFFER received (timeout)")?;

    crate::serial_println!("[dhcp] OFFER: {}.{}.{}.{}",
        offered_ip[0], offered_ip[1], offered_ip[2], offered_ip[3]);

    let offer_opts = &offer_data[240.min(offer_data.len())..];
    let server_id = get_opt(offer_opts, OPT_SERVER_ID).and_then(ip4).unwrap_or(ZERO_IP);

    // ── REQUEST ───────────────────────────────────────────────────────────────
    let mut req_opts: Vec<u8> = Vec::new();
    req_opts.extend_from_slice(&[OPT_MSG_TYPE, 1, DHCPREQUEST]);
    req_opts.extend_from_slice(&[OPT_SERVER_ID, 4]);
    req_opts.extend_from_slice(&server_id);
    req_opts.extend_from_slice(&[OPT_REQUESTED_IP, 4]);
    req_opts.extend_from_slice(&offered_ip);
    req_opts.extend_from_slice(&[OPT_PARAM_REQUEST, 4, OPT_SUBNET_MASK, OPT_ROUTER, OPT_DNS, OPT_LEASE_TIME]);
    req_opts.push(OPT_END);
    let req_pkt = build_packet(xid, mac, ZERO_IP, &req_opts);

    crate::serial_println!("[dhcp] REQUEST for {}.{}.{}.{}",
        offered_ip[0], offered_ip[1], offered_ip[2], offered_ip[3]);
    super::udp::send_raw(ZERO_IP, BROADCAST_IP, SERVER_PORT, CLIENT_PORT, &req_pkt)?;

    // ── ACK ───────────────────────────────────────────────────────────────────
    let (ack_data, acked_ip) = wait_for(xid, DHCPACK)
        .ok_or("DHCP: no ACK received (timeout)")?;

    let ack_opts = &ack_data[240.min(ack_data.len())..];
    let subnet   = get_opt(ack_opts, OPT_SUBNET_MASK).and_then(ip4).unwrap_or([255, 255, 255, 0]);
    let gateway  = get_opt(ack_opts, OPT_ROUTER).and_then(ip4).unwrap_or([acked_ip[0], acked_ip[1], acked_ip[2], 1]);
    let dns      = get_opt(ack_opts, OPT_DNS).and_then(ip4).unwrap_or([8, 8, 8, 8]);
    let lease    = get_opt(ack_opts, OPT_LEASE_TIME)
        .and_then(|v| if v.len() >= 4 { Some(u32::from_be_bytes([v[0], v[1], v[2], v[3]])) } else { None })
        .unwrap_or(86400);
    let server_id = get_opt(ack_opts, OPT_SERVER_ID).and_then(ip4).unwrap_or(server_id);

    // Apply lease to global config
    {
        let mut cfg = super::NET_CONFIG.lock();
        cfg.local_ip        = acked_ip;
        cfg.gateway         = gateway;
        cfg.subnet          = subnet;
        cfg.dns             = dns;
        cfg.lease_secs      = lease;
        cfg.dhcp_server     = server_id;
        cfg.acquired_time_s = crate::drivers::timer::uptime_secs();
        cfg.dhcp_configured = true;
    }

    crate::serial_println!(
        "[dhcp] ACK: ip={}.{}.{}.{} gw={}.{}.{}.{} dns={}.{}.{}.{} lease={}s",
        acked_ip[0], acked_ip[1], acked_ip[2], acked_ip[3],
        gateway[0],  gateway[1],  gateway[2],  gateway[3],
        dns[0],      dns[1],      dns[2],      dns[3],
        lease,
    );

    Ok(())
}

/// Information about the current DHCP lease (for terminal display).
pub struct LeaseInfo {
    pub configured:      bool,
    pub local_ip:        [u8; 4],
    pub gateway:         [u8; 4],
    pub subnet:          [u8; 4],
    pub dns:             [u8; 4],
    pub lease_secs:      u32,
    pub dhcp_server:     [u8; 4],
    pub acquired_time_s: u64,
}

pub fn lease_info() -> LeaseInfo {
    let c = super::NET_CONFIG.lock();
    LeaseInfo {
        configured:      c.dhcp_configured,
        local_ip:        c.local_ip,
        gateway:         c.gateway,
        subnet:          c.subnet,
        dns:             c.dns,
        lease_secs:      c.lease_secs,
        dhcp_server:     c.dhcp_server,
        acquired_time_s: c.acquired_time_s,
    }
}

/// Send a DHCPREQUEST to renew or rebind the current lease.
pub fn renew(server_ip: [u8; 4], client_ip: [u8; 4], broadcast: bool) -> Result<(), &'static str> {
    let mac = crate::net::mac_address().ok_or("DHCP: no network device")?;

    // Bind our receive port (ignore already bound error)
    let _ = super::udp::bind(CLIENT_PORT);

    // Deterministic-but-varied XID
    let xid = (crate::drivers::timer::ticks() & 0xFFFF_FFFF) as u32 | 0xDD00_0000;

    // DHCPREQUEST options for renewal (RFC 2131: no Option 50 or 54)
    let mut req_opts: Vec<u8> = Vec::new();
    req_opts.extend_from_slice(&[OPT_MSG_TYPE, 1, DHCPREQUEST]);
    req_opts.extend_from_slice(&[OPT_PARAM_REQUEST, 4, OPT_SUBNET_MASK, OPT_ROUTER, OPT_DNS, OPT_LEASE_TIME]);
    req_opts.push(OPT_END);

    let req_pkt = build_packet(xid, mac, client_ip, &req_opts);
    let dest_ip = if broadcast { BROADCAST_IP } else { server_ip };

    crate::serial_println!(
        "[dhcp] Sending DHCPREQUEST xid={:#010x} ciaddr={}.{}.{}.{} dest={}.{}.{}.{}",
        xid, client_ip[0], client_ip[1], client_ip[2], client_ip[3],
        dest_ip[0], dest_ip[1], dest_ip[2], dest_ip[3]
    );

    super::udp::send_raw(client_ip, dest_ip, SERVER_PORT, CLIENT_PORT, &req_pkt)?;

    // Wait for DHCPACK
    let (ack_data, acked_ip) = wait_for(xid, DHCPACK)
        .ok_or("DHCP: no ACK received during renewal (timeout)")?;

    let ack_opts = &ack_data[240.min(ack_data.len())..];
    let subnet   = get_opt(ack_opts, OPT_SUBNET_MASK).and_then(ip4).unwrap_or([255, 255, 255, 0]);
    let gateway  = get_opt(ack_opts, OPT_ROUTER).and_then(ip4).unwrap_or([acked_ip[0], acked_ip[1], acked_ip[2], 1]);
    let dns      = get_opt(ack_opts, OPT_DNS).and_then(ip4).unwrap_or([8, 8, 8, 8]);
    let lease    = get_opt(ack_opts, OPT_LEASE_TIME)
        .and_then(|v| if v.len() >= 4 { Some(u32::from_be_bytes([v[0], v[1], v[2], v[3]])) } else { None })
        .unwrap_or(86400);
    let server_id = get_opt(ack_opts, OPT_SERVER_ID).and_then(ip4).unwrap_or(server_ip);

    // Apply lease to global config
    {
        let mut cfg = super::NET_CONFIG.lock();
        cfg.local_ip        = acked_ip;
        cfg.gateway         = gateway;
        cfg.subnet          = subnet;
        cfg.dns             = dns;
        cfg.lease_secs      = lease;
        cfg.dhcp_server     = server_id;
        cfg.acquired_time_s = crate::drivers::timer::uptime_secs();
        cfg.dhcp_configured = true;
    }

    crate::serial_println!(
        "[dhcp] Renewal ACK: ip={}.{}.{}.{} gw={}.{}.{}.{} dns={}.{}.{}.{} lease={}s",
        acked_ip[0], acked_ip[1], acked_ip[2], acked_ip[3],
        gateway[0],  gateway[1],  gateway[2],  gateway[3],
        dns[0],      dns[1],      dns[2],      dns[3],
        lease,
    );

    Ok(())
}

fn sleep_ms(ms: u64) {
    if ms == 0 {
        crate::process::scheduler::yield_now();
        return;
    }
    let current_tick = crate::drivers::timer::ticks();
    let wake_at = current_tick + (ms / 10).max(1);
    crate::process::scheduler::block_current_thread(
        crate::process::wait::WaitReason::Sleep { wake_at_tick: wake_at }
    );
}

/// Background daemon thread running the DHCP lease renewal loop.
pub fn renewal_worker() {
    crate::serial_println!("[dhcp] Renewal background worker active.");
    loop {
        // Sleep for 10 seconds.
        sleep_ms(10000);

        let (configured, local_ip, dhcp_server, lease_secs, acquired_time_s) = {
            let cfg = super::NET_CONFIG.lock();
            (cfg.dhcp_configured, cfg.local_ip, cfg.dhcp_server, cfg.lease_secs, cfg.acquired_time_s)
        };

        if configured && lease_secs > 0 {
            let now = crate::drivers::timer::uptime_secs();
            let elapsed = now.saturating_sub(acquired_time_s);

            if elapsed >= lease_secs as u64 {
                // 1. Fully expired -> clear and rediscover
                crate::serial_println!("[dhcp] Lease expired! Clearing IP configuration and running discovery...");
                {
                    let mut cfg = super::NET_CONFIG.lock();
                    cfg.dhcp_configured = false;
                    cfg.local_ip        = super::LOCAL_IP_DEFAULT;
                    cfg.gateway         = super::GATEWAY_IP_DEFAULT;
                    cfg.subnet          = super::SUBNET_MASK_DEFAULT;
                    cfg.dns             = super::DNS_DEFAULT;
                    cfg.lease_secs      = 0;
                    cfg.dhcp_server     = [0, 0, 0, 0];
                    cfg.acquired_time_s = 0;
                }
                if let Err(e) = discover() {
                    crate::serial_println!("[dhcp] rediscover failed: {}", e);
                }
            } else if elapsed >= (lease_secs as u64 * 7 / 8) {
                // 2. T2 threshold -> Broadcast rebind
                crate::serial_println!("[dhcp] T2 timer expired ({}s/{}s). Rebinding...", elapsed, lease_secs);
                if let Err(e) = renew(dhcp_server, local_ip, true) {
                    crate::serial_println!("[dhcp] Rebind failed: {}", e);
                }
            } else if elapsed >= (lease_secs as u64 / 2) {
                // 3. T1 threshold -> Unicast renew
                crate::serial_println!("[dhcp] T1 timer expired ({}s/{}s). Renewing...", elapsed, lease_secs);
                if let Err(e) = renew(dhcp_server, local_ip, false) {
                    crate::serial_println!("[dhcp] Renewal failed: {}", e);
                }
            }
        }
    }
}

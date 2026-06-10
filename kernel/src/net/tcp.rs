/// TCP (Transmission Control Protocol) for Smart OS.
///
/// Simplified TCP implementation supporting connect/listen/accept/send/recv/close.
/// Features: 3-way handshake, sliding window (fixed 8KB), retransmission (2s timeout),
/// connection state machine, and graceful close (FIN sequence).

use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use super::{ethernet, arp, ipv4, BROADCAST_MAC};

// ═══════════════════════════════════════════════════════════════
//  Constants
// ═══════════════════════════════════════════════════════════════

pub const TCP_HEADER_MIN: usize = 20;
const RECV_BUF_MAX: usize = 16384;
const SEND_BUF_MAX: usize = 16384;
const WINDOW_SIZE: u16 = 8192;
const RETRANSMIT_TIMEOUT: u64 = 200; // ~2 seconds at 100 ticks/s
const MAX_RETRANSMITS: u8 = 5;

// TCP flags
const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;

// ═══════════════════════════════════════════════════════════════
//  TCP Header
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct TcpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq_num: u32,
    pub ack_num: u32,
    pub data_offset: u8,
    pub flags: u8,
    pub window_size: u16,
    pub checksum: u16,
    pub urgent_ptr: u16,
}

/// Parse a TCP segment from raw bytes.
pub fn parse(data: &[u8]) -> Option<(TcpHeader, &[u8])> {
    if data.len() < TCP_HEADER_MIN {
        return None;
    }

    let data_offset = (data[12] >> 4) as usize * 4;
    if data_offset < TCP_HEADER_MIN || data.len() < data_offset {
        return None;
    }

    let header = TcpHeader {
        src_port: u16::from_be_bytes([data[0], data[1]]),
        dst_port: u16::from_be_bytes([data[2], data[3]]),
        seq_num: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
        ack_num: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
        data_offset: data[12] >> 4,
        flags: data[13],
        window_size: u16::from_be_bytes([data[14], data[15]]),
        checksum: u16::from_be_bytes([data[16], data[17]]),
        urgent_ptr: u16::from_be_bytes([data[18], data[19]]),
    };

    let payload = &data[data_offset..];
    Some((header, payload))
}

/// Build a TCP segment with pseudo-header checksum.
fn build_segment(
    src_port: u16,
    dst_port: u16,
    seq_num: u32,
    ack_num: u32,
    flags: u8,
    window: u16,
    payload: &[u8],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
) -> Vec<u8> {
    let tcp_len = TCP_HEADER_MIN + payload.len();
    let mut segment = Vec::with_capacity(tcp_len);

    segment.extend_from_slice(&src_port.to_be_bytes());
    segment.extend_from_slice(&dst_port.to_be_bytes());
    segment.extend_from_slice(&seq_num.to_be_bytes());
    segment.extend_from_slice(&ack_num.to_be_bytes());
    // Data offset (5 = 20 bytes / 4) in upper nibble, reserved in lower
    segment.push(0x50);
    segment.push(flags);
    segment.extend_from_slice(&window.to_be_bytes());
    // Checksum placeholder
    segment.extend_from_slice(&[0u8; 2]);
    // Urgent pointer
    segment.extend_from_slice(&[0u8; 2]);
    // Payload
    segment.extend_from_slice(payload);

    // Compute TCP checksum with pseudo-header
    let cksum = tcp_checksum(src_ip, dst_ip, &segment);
    segment[16] = (cksum >> 8) as u8;
    segment[17] = (cksum & 0xFF) as u8;

    segment
}

/// Compute TCP checksum including pseudo-header.
fn tcp_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], tcp_segment: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // Pseudo-header: src_ip(4) + dst_ip(4) + zero(1) + protocol(1) + tcp_length(2)
    sum += ((src_ip[0] as u32) << 8) | (src_ip[1] as u32);
    sum += ((src_ip[2] as u32) << 8) | (src_ip[3] as u32);
    sum += ((dst_ip[0] as u32) << 8) | (dst_ip[1] as u32);
    sum += ((dst_ip[2] as u32) << 8) | (dst_ip[3] as u32);
    sum += ipv4::PROTO_TCP as u32;
    sum += tcp_segment.len() as u32;

    // TCP segment
    let mut i = 0;
    while i + 1 < tcp_segment.len() {
        sum += ((tcp_segment[i] as u32) << 8) | (tcp_segment[i + 1] as u32);
        i += 2;
    }
    if i < tcp_segment.len() {
        sum += (tcp_segment[i] as u32) << 8;
    }

    // Fold carries
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !sum as u16
}

// ═══════════════════════════════════════════════════════════════
//  Connection State Machine
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    LastAck,
    TimeWait,
}

/// Transmission Control Block (per-connection state).
pub struct Tcb {
    pub state: TcpState,
    pub local_port: u16,
    pub remote_port: u16,
    pub remote_ip: [u8; 4],
    // Sequence numbers
    pub snd_una: u32,          // oldest unacknowledged
    pub snd_nxt: u32,          // next to send
    pub rcv_nxt: u32,          // next expected from remote
    // Windows
    pub snd_wnd: u16,
    pub rcv_wnd: u16,
    // Buffers
    pub recv_buf: VecDeque<u8>,
    pub send_buf: VecDeque<u8>,
    // Retransmission
    pub retransmit_seg: Vec<u8>,   // last unacked raw segment
    pub retransmit_tick: u64,
    pub retransmit_count: u8,
    // Timing
    pub time_wait_tick: u64,
}

impl Tcb {
    fn new(local_port: u16, remote_ip: [u8; 4], remote_port: u16) -> Self {
        Self {
            state: TcpState::Closed,
            local_port,
            remote_port,
            remote_ip,
            snd_una: 0,
            snd_nxt: 0,
            rcv_nxt: 0,
            snd_wnd: WINDOW_SIZE,
            rcv_wnd: WINDOW_SIZE,
            recv_buf: VecDeque::new(),
            send_buf: VecDeque::new(),
            retransmit_seg: Vec::new(),
            retransmit_tick: 0,
            retransmit_count: 0,
            time_wait_tick: 0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  Global State
// ═══════════════════════════════════════════════════════════════

pub type TcpSocketId = u64;

static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(1);
static TICK_COUNTER: AtomicU64 = AtomicU64::new(0);
static NEXT_EPHEMERAL: AtomicU64 = AtomicU64::new(49152);

/// Active TCP connections: socket_id → TCB.
static TCP_CONNECTIONS: Mutex<BTreeMap<TcpSocketId, Tcb>> = Mutex::new(BTreeMap::new());

/// Listening ports: port → queue of accepted socket IDs.
static TCP_LISTENERS: Mutex<BTreeMap<u16, VecDeque<TcpSocketId>>> = Mutex::new(BTreeMap::new());

/// ISN counter for generating initial sequence numbers.
static ISN_COUNTER: AtomicU64 = AtomicU64::new(1000);

fn alloc_socket_id() -> TcpSocketId {
    NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
}

fn generate_isn() -> u32 {
    let tick = crate::drivers::timer::uptime_secs() as u64;
    let counter = ISN_COUNTER.fetch_add(1, Ordering::Relaxed);
    ((tick.wrapping_mul(250000)).wrapping_add(counter)) as u32
}

// ═══════════════════════════════════════════════════════════════
//  Public API
// ═══════════════════════════════════════════════════════════════

/// Initialize the TCP subsystem.
pub fn init() {
    crate::serial_println!("[tcp] TCP stack initialized.");
}

/// Open a TCP connection (active open, sends SYN).
pub fn connect(remote_ip: [u8; 4], remote_port: u16, local_port: u16) -> Result<TcpSocketId, &'static str> {
    crate::serial_println!(
        "[tcp] connect → {}.{}.{}.{}:{} (local:{})",
        remote_ip[0], remote_ip[1], remote_ip[2], remote_ip[3],
        remote_port, local_port
    );

    let sock_id = alloc_socket_id();
    let isn = generate_isn();

    let mut tcb = Tcb::new(local_port, remote_ip, remote_port);
    tcb.snd_nxt = isn.wrapping_add(1);
    tcb.snd_una = isn;
    tcb.state = TcpState::SynSent;
    tcb.retransmit_tick = TICK_COUNTER.load(Ordering::Relaxed);

    // Send SYN
    send_tcp_raw(local_port, remote_port, isn, 0, SYN, WINDOW_SIZE, &[], remote_ip);

    TCP_CONNECTIONS.lock().insert(sock_id, tcb);
    Ok(sock_id)
}

/// Start listening on a port (passive open).
pub fn listen(port: u16) -> Result<(), &'static str> {
    let mut listeners = TCP_LISTENERS.lock();
    if listeners.contains_key(&port) {
        return Err("Port already listening");
    }
    listeners.insert(port, VecDeque::new());
    Ok(())
}

/// Accept a pending connection on a listening port (non-blocking).
pub fn accept(port: u16) -> Option<TcpSocketId> {
    let mut listeners = TCP_LISTENERS.lock();
    if let Some(queue) = listeners.get_mut(&port) {
        queue.pop_front()
    } else {
        None
    }
}

/// Send data on an established connection.
pub fn send(sock_id: TcpSocketId, data: &[u8]) -> Result<usize, &'static str> {
    let mut conns = TCP_CONNECTIONS.lock();
    let tcb = conns.get_mut(&sock_id).ok_or("Invalid socket")?;

    if tcb.state != TcpState::Established && tcb.state != TcpState::CloseWait {
        return Err("Connection not established");
    }

    // Queue data into send buffer
    let space = SEND_BUF_MAX.saturating_sub(tcb.send_buf.len());
    let to_send = data.len().min(space);
    if to_send == 0 {
        return Err("Send buffer full");
    }

    for &b in &data[..to_send] {
        tcb.send_buf.push_back(b);
    }

    // Try to transmit immediately
    flush_send_buf(sock_id, tcb);

    Ok(to_send)
}

/// Receive data from an established connection (non-blocking).
pub fn recv(sock_id: TcpSocketId, buf: &mut [u8]) -> Result<usize, &'static str> {
    let mut conns = TCP_CONNECTIONS.lock();
    let tcb = conns.get_mut(&sock_id).ok_or("Invalid socket")?;

    if tcb.recv_buf.is_empty() {
        if tcb.state == TcpState::CloseWait || tcb.state == TcpState::Closed
            || tcb.state == TcpState::TimeWait
        {
            return Err("Connection closed by peer");
        }
        return Ok(0); // No data available
    }

    let n = buf.len().min(tcb.recv_buf.len());
    for i in 0..n {
        buf[i] = tcb.recv_buf.pop_front().unwrap();
    }
    Ok(n)
}

/// Close a TCP connection (initiates FIN sequence).
pub fn close(sock_id: TcpSocketId) -> Result<(), &'static str> {
    let mut conns = TCP_CONNECTIONS.lock();
    let tcb = conns.get_mut(&sock_id).ok_or("Invalid socket")?;

    match tcb.state {
        TcpState::Established => {
            // Send FIN+ACK
            send_tcp_raw(
                tcb.local_port, tcb.remote_port,
                tcb.snd_nxt, tcb.rcv_nxt,
                FIN | ACK, WINDOW_SIZE, &[],
                tcb.remote_ip,
            );
            tcb.snd_nxt = tcb.snd_nxt.wrapping_add(1);
            tcb.state = TcpState::FinWait1;
            tcb.retransmit_tick = TICK_COUNTER.load(Ordering::Relaxed);
            Ok(())
        }
        TcpState::CloseWait => {
            // Send FIN+ACK
            send_tcp_raw(
                tcb.local_port, tcb.remote_port,
                tcb.snd_nxt, tcb.rcv_nxt,
                FIN | ACK, WINDOW_SIZE, &[],
                tcb.remote_ip,
            );
            tcb.snd_nxt = tcb.snd_nxt.wrapping_add(1);
            tcb.state = TcpState::LastAck;
            Ok(())
        }
        TcpState::SynSent | TcpState::SynReceived => {
            // Abort: send RST and remove
            send_tcp_raw(
                tcb.local_port, tcb.remote_port,
                tcb.snd_nxt, 0,
                RST, 0, &[],
                tcb.remote_ip,
            );
            conns.remove(&sock_id);
            Ok(())
        }
        _ => {
            // Already closing or closed — just remove
            conns.remove(&sock_id);
            Ok(())
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  Incoming Packet Handler
// ═══════════════════════════════════════════════════════════════

/// Handle an incoming TCP segment (called from IPv4 handler).
pub fn handle_tcp(src_ip: [u8; 4], payload: &[u8]) {
    let (hdr, data) = match parse(payload) {
        Some(h) => h,
        None => return,
    };

    // First check for existing connection by (remote_ip, remote_port, local_port)
    let mut conns = TCP_CONNECTIONS.lock();
    let mut found_sock = None;
    for (&sock_id, tcb) in conns.iter() {
        if tcb.remote_ip == src_ip
            && tcb.remote_port == hdr.src_port
            && tcb.local_port == hdr.dst_port
        {
            found_sock = Some(sock_id);
            break;
        }
    }

    if let Some(sock_id) = found_sock {
        let tcb = conns.get_mut(&sock_id).unwrap();
        process_segment(sock_id, tcb, &hdr, data, src_ip);
        return;
    }

    // Release connections lock — we no longer need it for the rest of this function
    drop(conns);

    // Check if this is a SYN to a listening port
    if hdr.flags & SYN != 0 && hdr.flags & ACK == 0 {
        let mut listeners = TCP_LISTENERS.lock();
        if let Some(accept_queue) = listeners.get_mut(&hdr.dst_port) {
            // Create a new TCB for this incoming connection
            let sock_id = alloc_socket_id();
            let isn = generate_isn();
            let mut tcb = Tcb::new(hdr.dst_port, src_ip, hdr.src_port);
            tcb.rcv_nxt = hdr.seq_num.wrapping_add(1);
            tcb.snd_nxt = isn.wrapping_add(1);
            tcb.snd_una = isn;
            tcb.snd_wnd = hdr.window_size;
            tcb.state = TcpState::SynReceived;
            tcb.retransmit_tick = TICK_COUNTER.load(Ordering::Relaxed);

            // Send SYN+ACK
            send_tcp_raw(
                hdr.dst_port, hdr.src_port,
                isn, tcb.rcv_nxt,
                SYN | ACK, WINDOW_SIZE, &[],
                src_ip,
            );

            // Queue for accept()
            if accept_queue.len() < 16 {
                accept_queue.push_back(sock_id);
            }
            TCP_CONNECTIONS.lock().insert(sock_id, tcb);
            return;
        }
    }

    // No matching connection or listener — send RST (if not already RST)
    if hdr.flags & RST == 0 {
        if hdr.flags & ACK != 0 {
            send_tcp_raw(
                hdr.dst_port, hdr.src_port,
                hdr.ack_num, 0,
                RST, 0, &[],
                src_ip,
            );
        } else {
            let ack = hdr.seq_num.wrapping_add(data.len() as u32);
            send_tcp_raw(
                hdr.dst_port, hdr.src_port,
                0, ack,
                RST | ACK, 0, &[],
                src_ip,
            );
        }
    }
}

/// Process a segment for an existing connection.
fn process_segment(
    _sock_id: TcpSocketId,
    tcb: &mut Tcb,
    hdr: &TcpHeader,
    data: &[u8],
    _src_ip: [u8; 4],
) {
    // Handle RST
    if hdr.flags & RST != 0 {
        tcb.state = TcpState::Closed;
        return;
    }

    match tcb.state {
        TcpState::SynSent => {
            // Expecting SYN+ACK
            if hdr.flags & SYN != 0 && hdr.flags & ACK != 0 {
                tcb.rcv_nxt = hdr.seq_num.wrapping_add(1);
                tcb.snd_una = hdr.ack_num;
                tcb.snd_wnd = hdr.window_size;
                tcb.state = TcpState::Established;
                tcb.retransmit_seg.clear();
                tcb.retransmit_count = 0;

                crate::serial_println!(
                    "[tcp] ESTABLISHED {}.{}.{}.{}:{}",
                    tcb.remote_ip[0], tcb.remote_ip[1], tcb.remote_ip[2], tcb.remote_ip[3],
                    tcb.remote_port
                );

                // Send ACK
                send_tcp_raw(
                    tcb.local_port, tcb.remote_port,
                    tcb.snd_nxt, tcb.rcv_nxt,
                    ACK, WINDOW_SIZE, &[],
                    tcb.remote_ip,
                );
            }
        }

        TcpState::SynReceived => {
            // Expecting ACK to complete handshake
            if hdr.flags & ACK != 0 {
                tcb.snd_una = hdr.ack_num;
                tcb.snd_wnd = hdr.window_size;
                tcb.state = TcpState::Established;
                tcb.retransmit_seg.clear();
                tcb.retransmit_count = 0;

                // Process any data in this segment
                if !data.is_empty() {
                    accept_data(tcb, hdr.seq_num, data);
                }
            }
        }

        TcpState::Established => {
            if hdr.flags & ACK != 0 {
                // Advance snd_una
                if seq_after(hdr.ack_num, tcb.snd_una) {
                    tcb.snd_una = hdr.ack_num;
                    tcb.retransmit_seg.clear();
                    tcb.retransmit_count = 0;
                }
                tcb.snd_wnd = hdr.window_size;
            }

            // Accept incoming data
            if !data.is_empty() {
                accept_data(tcb, hdr.seq_num, data);
            }

            // Handle FIN
            if hdr.flags & FIN != 0 {
                tcb.rcv_nxt = hdr.seq_num.wrapping_add(data.len() as u32).wrapping_add(1);
                tcb.state = TcpState::CloseWait;
                // Send ACK for FIN
                send_tcp_raw(
                    tcb.local_port, tcb.remote_port,
                    tcb.snd_nxt, tcb.rcv_nxt,
                    ACK, WINDOW_SIZE, &[],
                    tcb.remote_ip,
                );
            }
        }

        TcpState::FinWait1 => {
            if hdr.flags & ACK != 0 {
                tcb.snd_una = hdr.ack_num;
                if hdr.flags & FIN != 0 {
                    // Simultaneous close
                    tcb.rcv_nxt = hdr.seq_num.wrapping_add(1);
                    tcb.state = TcpState::TimeWait;
                    tcb.time_wait_tick = TICK_COUNTER.load(Ordering::Relaxed);
                    send_tcp_raw(
                        tcb.local_port, tcb.remote_port,
                        tcb.snd_nxt, tcb.rcv_nxt,
                        ACK, WINDOW_SIZE, &[],
                        tcb.remote_ip,
                    );
                } else {
                    tcb.state = TcpState::FinWait2;
                }
            }
        }

        TcpState::FinWait2 => {
            if hdr.flags & FIN != 0 {
                tcb.rcv_nxt = hdr.seq_num.wrapping_add(1);
                tcb.state = TcpState::TimeWait;
                tcb.time_wait_tick = TICK_COUNTER.load(Ordering::Relaxed);
                send_tcp_raw(
                    tcb.local_port, tcb.remote_port,
                    tcb.snd_nxt, tcb.rcv_nxt,
                    ACK, WINDOW_SIZE, &[],
                    tcb.remote_ip,
                );
            }
            // Accept data before FIN
            if !data.is_empty() {
                accept_data(tcb, hdr.seq_num, data);
            }
        }

        TcpState::LastAck => {
            if hdr.flags & ACK != 0 {
                tcb.state = TcpState::Closed;
            }
        }

        TcpState::TimeWait => {
            // Ignore everything in TIME_WAIT (will be cleaned up by timer)
        }

        _ => {}
    }
}

/// Accept data into receive buffer (in-order only, no reassembly).
fn accept_data(tcb: &mut Tcb, seq: u32, data: &[u8]) {
    if seq == tcb.rcv_nxt && tcb.recv_buf.len() + data.len() <= RECV_BUF_MAX {
        for &b in data {
            tcb.recv_buf.push_back(b);
        }
        tcb.rcv_nxt = tcb.rcv_nxt.wrapping_add(data.len() as u32);
    }

    // Always ACK
    send_tcp_raw(
        tcb.local_port, tcb.remote_port,
        tcb.snd_nxt, tcb.rcv_nxt,
        ACK, WINDOW_SIZE, &[],
        tcb.remote_ip,
    );
}

/// Flush send buffer: transmit data segments.
fn flush_send_buf(_sock_id: TcpSocketId, tcb: &mut Tcb) {
    while !tcb.send_buf.is_empty() {
        let max_seg = (tcb.snd_wnd as usize).min(1460); // MSS
        let to_send = tcb.send_buf.len().min(max_seg);
        if to_send == 0 {
            break;
        }

        let mut payload = Vec::with_capacity(to_send);
        for _ in 0..to_send {
            payload.push(tcb.send_buf.pop_front().unwrap());
        }

        send_tcp_raw(
            tcb.local_port, tcb.remote_port,
            tcb.snd_nxt, tcb.rcv_nxt,
            ACK | PSH, WINDOW_SIZE,
            &payload,
            tcb.remote_ip,
        );

        // Save for retransmission
        tcb.retransmit_seg = payload;
        tcb.retransmit_tick = TICK_COUNTER.load(Ordering::Relaxed);
        tcb.snd_nxt = tcb.snd_nxt.wrapping_add(to_send as u32);
    }
}

// ═══════════════════════════════════════════════════════════════
//  Timer and Retransmission
// ═══════════════════════════════════════════════════════════════

/// Called periodically from the net-rx thread (~100 times/sec).
pub fn tcp_timer_tick() {
    let now = TICK_COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut conns = TCP_CONNECTIONS.lock();
    let mut to_remove = Vec::new();

    for (&sock_id, tcb) in conns.iter_mut() {
        // Clean up TIME_WAIT connections (after ~4 seconds)
        if tcb.state == TcpState::TimeWait {
            if now.wrapping_sub(tcb.time_wait_tick) > 400 {
                to_remove.push(sock_id);
            }
            continue;
        }

        // Clean up fully closed connections
        if tcb.state == TcpState::Closed {
            to_remove.push(sock_id);
            continue;
        }

        // Retransmission timeout
        if !tcb.retransmit_seg.is_empty()
            && now.wrapping_sub(tcb.retransmit_tick) > RETRANSMIT_TIMEOUT
        {
            if tcb.retransmit_count >= MAX_RETRANSMITS {
                // Connection timed out — reset
                send_tcp_raw(
                    tcb.local_port, tcb.remote_port,
                    tcb.snd_nxt, 0,
                    RST, 0, &[],
                    tcb.remote_ip,
                );
                tcb.state = TcpState::Closed;
                to_remove.push(sock_id);
                continue;
            }

            // Retransmit
            let seq = tcb.snd_una;
            let payload = tcb.retransmit_seg.clone();
            send_tcp_raw(
                tcb.local_port, tcb.remote_port,
                seq, tcb.rcv_nxt,
                ACK | PSH, WINDOW_SIZE,
                &payload,
                tcb.remote_ip,
            );
            tcb.retransmit_tick = now;
            tcb.retransmit_count += 1;
        }

        // Retransmit SYN if in SYN_SENT
        if tcb.state == TcpState::SynSent
            && now.wrapping_sub(tcb.retransmit_tick) > RETRANSMIT_TIMEOUT
        {
            if tcb.retransmit_count >= MAX_RETRANSMITS {
                tcb.state = TcpState::Closed;
                to_remove.push(sock_id);
                continue;
            }
            send_tcp_raw(
                tcb.local_port, tcb.remote_port,
                tcb.snd_una, 0,
                SYN, WINDOW_SIZE, &[],
                tcb.remote_ip,
            );
            tcb.retransmit_tick = now;
            tcb.retransmit_count += 1;
        }
    }

    for id in to_remove {
        conns.remove(&id);
    }
}

// ═══════════════════════════════════════════════════════════════
//  Helpers
// ═══════════════════════════════════════════════════════════════

/// Send a raw TCP segment over IPv4/Ethernet.
fn send_tcp_raw(
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    payload: &[u8],
    dst_ip: [u8; 4],
) {
    let lip = super::local_ip();
    let segment = build_segment(src_port, dst_port, seq, ack, flags, window, payload, lip, dst_ip);
    let ip_packet = ipv4::build(lip, dst_ip, ipv4::PROTO_TCP, &segment);

    let our_mac = match crate::net::mac_address() {
        Some(m) => m,
        None => return,
    };

    let dst_mac = if is_local(dst_ip) {
        resolve_or_broadcast(dst_ip)
    } else {
        resolve_or_broadcast(super::gateway_ip())
    };

    let frame = ethernet::build(dst_mac, our_mac, ethernet::ETHERTYPE_IPV4, &ip_packet);
    crate::net::send_frame(&frame).ok();
}

/// Check if an IP is on the local subnet.
fn is_local(ip: [u8; 4]) -> bool {
    let local = super::local_ip();
    let mask  = super::subnet_mask();
    for i in 0..4 {
        if (ip[i] & mask[i]) != (local[i] & mask[i]) { return false; }
    }
    true
}

/// Resolve IP to MAC or return broadcast.
fn resolve_or_broadcast(ip: [u8; 4]) -> [u8; 6] {
    if let Some(mac) = arp::resolve(ip) {
        mac
    } else {
        arp::send_arp_request(ip);
        BROADCAST_MAC
    }
}

/// Sequence number comparison: is `a` after `b`?
fn seq_after(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

/// Allocate an ephemeral port.
pub fn alloc_ephemeral_port() -> u16 {
    let port = NEXT_EPHEMERAL.fetch_add(1, Ordering::Relaxed);
    ((port % 16384) + 49152) as u16
}

// ═══════════════════════════════════════════════════════════════
//  Diagnostics
// ═══════════════════════════════════════════════════════════════

/// Get count of active TCP connections.
pub fn connection_count() -> usize {
    TCP_CONNECTIONS.lock().len()
}

/// Get summary of all connections: (socket_id, state, local_port, remote_ip, remote_port).
pub fn connection_list() -> Vec<(TcpSocketId, TcpState, u16, [u8; 4], u16)> {
    let conns = TCP_CONNECTIONS.lock();
    conns.iter().map(|(&id, tcb)| {
        (id, tcb.state, tcb.local_port, tcb.remote_ip, tcb.remote_port)
    }).collect()
}

/// Get listener count.
pub fn listener_count() -> usize {
    TCP_LISTENERS.lock().len()
}

/// Check if a TCP connection has data available to read (for select/poll).
pub fn has_recv_data(conn_id: TcpSocketId) -> bool {
    let conns = TCP_CONNECTIONS.lock();
    conns.get(&conn_id).map(|tcb| !tcb.recv_buf.is_empty()).unwrap_or(false)
}

/// Returns true if the connection has reached ESTABLISHED state.
pub fn is_established(sock_id: TcpSocketId) -> bool {
    TCP_CONNECTIONS.lock()
        .get(&sock_id)
        .map(|tcb| tcb.state == TcpState::Established)
        .unwrap_or(false)
}

/// Returns true if the connection is closed/removed (connect failed or peer reset).
pub fn is_gone(sock_id: TcpSocketId) -> bool {
    let conns = TCP_CONNECTIONS.lock();
    match conns.get(&sock_id) {
        None => true,
        Some(tcb) => tcb.state == TcpState::Closed,
    }
}

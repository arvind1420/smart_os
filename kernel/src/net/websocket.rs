//! WebSocket Client — Phase 42 for Smart OS.
//!
//! Implements RFC 6455:
//!  • HTTP/1.1 Upgrade handshake (Sec-WebSocket-Key / Accept)
//!  • Frame codec: FIN, RSV, opcode, masking, 7/16/64-bit payload length
//!  • Opcodes: Text(0x1), Binary(0x2), Close(0x8), Ping(0x9), Pong(0xA),
//!             Continuation(0x0)
//!  • Client-side masking (4-byte XOR key, MUST mask client→server)
//!  • Fragmented message reassembly
//!  • Close handshake (status code + reason)
//!  • Extension/subprotocol negotiation stubs
//!
//! Built on top of the kernel TCP stack (net::tcp).

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::format;
use alloc::collections::VecDeque;

// ─────────────────────────────────────────────────────────────────────────────
//  WebSocket frame
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Opcode {
    Continuation,
    Text,
    Binary,
    Close,
    Ping,
    Pong,
    Reserved(u8),
}

impl Opcode {
    pub fn from_u8(v: u8) -> Self {
        match v & 0x0F {
            0x0 => Opcode::Continuation,
            0x1 => Opcode::Text,
            0x2 => Opcode::Binary,
            0x8 => Opcode::Close,
            0x9 => Opcode::Ping,
            0xA => Opcode::Pong,
            n   => Opcode::Reserved(n),
        }
    }
    pub fn to_u8(&self) -> u8 {
        match self {
            Opcode::Continuation => 0x0,
            Opcode::Text         => 0x1,
            Opcode::Binary       => 0x2,
            Opcode::Close        => 0x8,
            Opcode::Ping         => 0x9,
            Opcode::Pong         => 0xA,
            Opcode::Reserved(n)  => *n,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WsFrame {
    pub fin:     bool,
    pub opcode:  Opcode,
    pub payload: Vec<u8>,
}

impl WsFrame {
    /// Encode a frame with optional client masking.
    pub fn encode(&self, mask: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let fin_bit  = if self.fin { 0x80u8 } else { 0x00 };
        out.push(fin_bit | self.opcode.to_u8());

        let mask_bit = if mask { 0x80u8 } else { 0x00 };
        let len = self.payload.len();
        if len < 126 {
            out.push(mask_bit | len as u8);
        } else if len < 65536 {
            out.push(mask_bit | 126);
            out.push((len >> 8) as u8);
            out.push(len as u8);
        } else {
            out.push(mask_bit | 127);
            for i in (0..8).rev() { out.push((len >> (i * 8)) as u8); }
        }

        if mask {
            // Simple mask key derived from payload length (deterministic for testing)
            let mk = mask_key(len as u32);
            out.extend_from_slice(&mk);
            for (i, &b) in self.payload.iter().enumerate() {
                out.push(b ^ mk[i & 3]);
            }
        } else {
            out.extend_from_slice(&self.payload);
        }
        out
    }

    /// Try to parse one frame from a byte slice; returns (frame, bytes_consumed).
    pub fn decode(data: &[u8]) -> Option<(WsFrame, usize)> {
        if data.len() < 2 { return None; }
        let b0 = data[0]; let b1 = data[1];
        let fin    = b0 & 0x80 != 0;
        let opcode = Opcode::from_u8(b0);
        let masked  = b1 & 0x80 != 0;
        let raw_len = (b1 & 0x7F) as usize;

        let mut pos = 2;
        let payload_len: usize = if raw_len == 126 {
            if data.len() < pos + 2 { return None; }
            let l = u16::from_be_bytes([data[pos], data[pos+1]]) as usize;
            pos += 2; l
        } else if raw_len == 127 {
            if data.len() < pos + 8 { return None; }
            let l = u64::from_be_bytes([data[pos],data[pos+1],data[pos+2],data[pos+3],
                                        data[pos+4],data[pos+5],data[pos+6],data[pos+7]]) as usize;
            pos += 8; l
        } else {
            raw_len
        };

        let mask_key: Option<[u8;4]> = if masked {
            if data.len() < pos + 4 { return None; }
            let k = [data[pos],data[pos+1],data[pos+2],data[pos+3]];
            pos += 4; Some(k)
        } else { None };

        if data.len() < pos + payload_len { return None; }
        let raw = &data[pos..pos+payload_len];
        let payload: Vec<u8> = if let Some(mk) = mask_key {
            raw.iter().enumerate().map(|(i,&b)| b ^ mk[i&3]).collect()
        } else {
            raw.to_vec()
        };
        pos += payload_len;

        Some((WsFrame { fin, opcode, payload }, pos))
    }
}

fn mask_key(seed: u32) -> [u8; 4] {
    // LCG-derived mask key
    let v = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    [(v>>24) as u8, (v>>16) as u8, (v>>8) as u8, v as u8]
}

// ─────────────────────────────────────────────────────────────────────────────
//  WebSocket handshake helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Generate a random 16-byte base64-encoded Sec-WebSocket-Key.
fn generate_ws_key(nonce: u32) -> String {
    // Deterministic 16-byte key from nonce
    let mut key = [0u8; 16];
    for i in 0..16 {
        key[i] = ((nonce.wrapping_mul(i as u32 + 1).wrapping_add(0xDEAD)) & 0xFF) as u8;
    }
    base64_encode(&key)
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let combined = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((combined >> 18) & 63) as usize] as char);
        out.push(TABLE[((combined >> 12) & 63) as usize] as char);
        if chunk.len() > 1 { out.push(TABLE[((combined >> 6) & 63) as usize] as char); } else { out.push('='); }
        if chunk.len() > 2 { out.push(TABLE[(combined & 63) as usize] as char); } else { out.push('='); }
    }
    out
}

/// Compute Sec-WebSocket-Accept = base64(SHA-1(key + GUID)).
/// We implement a lightweight SHA-1 inline.
fn ws_accept(key: &str) -> String {
    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut input = String::new();
    input.push_str(key);
    input.push_str(GUID);
    let hash = sha1(input.as_bytes());
    base64_encode(&hash)
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301,0xEFCDAB89,0x98BADCFE,0x10325476,0xC3D2E1F0];
    // Pre-processing: pad message
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg: Vec<u8> = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    for i in (0..8).rev() { msg.push((bit_len >> (i*8)) as u8); }

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i*4],chunk[i*4+1],chunk[i*4+2],chunk[i*4+3]]);
        }
        for i in 16..80 {
            let v = w[i-3] ^ w[i-8] ^ w[i-14] ^ w[i-16];
            w[i] = v.rotate_left(1);
        }
        let (mut a,mut b,mut c,mut d,mut e) = (h[0],h[1],h[2],h[3],h[4]);
        for i in 0..80 {
            let (f, k) = if i < 20 { ((b&c)|((!b)&d), 0x5A827999u32) }
                else if i < 40 { (b^c^d, 0x6ED9EBA1) }
                else if i < 60 { ((b&c)|(b&d)|(c&d), 0x8F1BBCDC) }
                else            { (b^c^d, 0xCA62C1D6) };
            let temp = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(w[i]);
            e=d; d=c; c=b.rotate_left(30); b=a; a=temp;
        }
        h[0]=h[0].wrapping_add(a); h[1]=h[1].wrapping_add(b);
        h[2]=h[2].wrapping_add(c); h[3]=h[3].wrapping_add(d); h[4]=h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for i in 0..5 {
        out[i*4..(i+1)*4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
//  Close codes
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CloseCode {
    Normal         = 1000,
    GoingAway      = 1001,
    ProtocolError  = 1002,
    UnsupportedData= 1003,
    NoStatus       = 1005,
    AbnormalClose  = 1006,
    InvalidData    = 1007,
    PolicyViolation= 1008,
    MessageTooBig  = 1009,
    MandatoryExt   = 1010,
    InternalError  = 1011,
    TlsHandshake   = 1015,
}

impl CloseCode {
    pub fn from_u16(v: u16) -> Self {
        match v {
            1000 => Self::Normal,          1001 => Self::GoingAway,
            1002 => Self::ProtocolError,   1003 => Self::UnsupportedData,
            1007 => Self::InvalidData,     1008 => Self::PolicyViolation,
            1009 => Self::MessageTooBig,   1011 => Self::InternalError,
            _    => Self::ProtocolError,
        }
    }
    pub fn to_u16(self) -> u16 { self as u16 }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Messages (fully reassembled)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close { code: CloseCode, reason: String },
}

impl WsMessage {
    pub fn is_text(&self) -> bool { matches!(self, WsMessage::Text(_)) }
    pub fn text(&self) -> Option<&str> {
        if let WsMessage::Text(s) = self { Some(s) } else { None }
    }
    pub fn binary(&self) -> Option<&[u8]> {
        if let WsMessage::Binary(b) = self { Some(b) } else { None }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  WebSocket connection state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum WsState { Connecting, Open, Closing, Closed }

// ─────────────────────────────────────────────────────────────────────────────
//  WebSocket client
// ─────────────────────────────────────────────────────────────────────────────

pub struct WebSocket {
    pub url:        String,
    pub state:      WsState,
    conn_id:        u64,         // kernel TCP connection id
    recv_buf:       Vec<u8>,     // raw receive buffer
    frag_buf:       Vec<u8>,     // fragmented message accumulator
    frag_opcode:    Opcode,      // opcode of first fragment
    pub messages:   VecDeque<WsMessage>,
    nonce:          u32,
    ws_key:         String,
    pub subprotocol: Option<String>,
    pub extensions:  Vec<String>,
    send_seq:       u32,
}

impl WebSocket {
    /// Create and connect a WebSocket to `url` (ws:// or wss://).
    pub fn connect(url: &str) -> Result<Self, &'static str> {
        let (host, port, path, _secure) = parse_ws_url(url)?;

        let local_port = 49200u16 + (port % 1000) as u16;
        let ip = crate::net::dns::resolve(&host).map_err(|_| "DNS failed")?;
        let conn_id = crate::net::tcp::connect(ip, port, local_port)
            .map_err(|_| "TCP connect failed")?;

        let nonce = conn_id as u32 ^ 0xBEEF_CAFE;
        let ws_key = generate_ws_key(nonce);

        // Send HTTP Upgrade request
        let req = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
            path, host, ws_key
        );
        crate::net::tcp::send(conn_id, req.as_bytes())
            .map_err(|_| "HTTP upgrade send failed")?;

        // Read HTTP 101 response
        let mut resp_buf = Vec::new();
        let mut tmp = [0u8; 1024];
        for _ in 0..200 {
            match crate::net::tcp::recv(conn_id, &mut tmp) {
                Ok(n) if n > 0 => {
                    resp_buf.extend_from_slice(&tmp[..n]);
                    if resp_buf.windows(4).any(|w| w == b"\r\n\r\n") { break; }
                }
                _ => {}
            }
        }
        let resp_str = core::str::from_utf8(&resp_buf).unwrap_or("");
        if !resp_str.contains("101") { return Err("WebSocket upgrade rejected"); }

        // Verify accept key
        let expected = ws_accept(&ws_key);
        if !resp_str.contains(&expected) {
            return Err("Sec-WebSocket-Accept mismatch");
        }

        // Parse subprotocol
        let subprotocol = resp_str.lines()
            .find(|l| l.to_ascii_lowercase().starts_with("sec-websocket-protocol:"))
            .map(|l| l[l.find(':').unwrap_or(0)+1..].trim().to_string());

        crate::serial_println!("[ws] Connected to {}", url);

        Ok(WebSocket {
            url:        url.to_string(),
            state:      WsState::Open,
            conn_id,
            recv_buf:   Vec::new(),
            frag_buf:   Vec::new(),
            frag_opcode: Opcode::Continuation,
            messages:   VecDeque::new(),
            nonce,
            ws_key,
            subprotocol,
            extensions: Vec::new(),
            send_seq:   0,
        })
    }

    // ── Sending ──────────────────────────────────────────────────────────────

    pub fn send_text(&mut self, text: &str) -> Result<(), &'static str> {
        self.send_msg(Opcode::Text, text.as_bytes())
    }

    pub fn send_binary(&mut self, data: &[u8]) -> Result<(), &'static str> {
        self.send_msg(Opcode::Binary, data)
    }

    pub fn send_ping(&mut self, data: &[u8]) -> Result<(), &'static str> {
        self.send_msg(Opcode::Ping, data)
    }

    pub fn send_pong(&mut self, data: &[u8]) -> Result<(), &'static str> {
        self.send_msg(Opcode::Pong, data)
    }

    pub fn close(&mut self, code: CloseCode, reason: &str) -> Result<(), &'static str> {
        if self.state != WsState::Open { return Ok(()); }
        self.state = WsState::Closing;
        let code_u16 = code.to_u16();
        let mut payload = vec![(code_u16 >> 8) as u8, code_u16 as u8];
        payload.extend_from_slice(reason.as_bytes());
        self.send_msg(Opcode::Close, &payload)?;
        Ok(())
    }

    fn send_msg(&mut self, opcode: Opcode, payload: &[u8]) -> Result<(), &'static str> {
        if self.state != WsState::Open && opcode != Opcode::Close { return Err("Not open"); }
        self.send_seq = self.send_seq.wrapping_add(1);
        let frame = WsFrame { fin: true, opcode, payload: payload.to_vec() };
        let encoded = frame.encode(true); // clients MUST mask
        crate::net::tcp::send(self.conn_id, &encoded).map(|_| ()).map_err(|_| "TCP send failed")
    }

    // ── Fragmented send ──────────────────────────────────────────────────────

    /// Send a message in `chunk_size`-byte fragments.
    pub fn send_fragmented(&mut self, opcode: Opcode, data: &[u8], chunk_size: usize)
        -> Result<(), &'static str> {
        if data.is_empty() { return self.send_msg(opcode, data); }
        let chunks: Vec<&[u8]> = data.chunks(chunk_size).collect();
        for (i, chunk) in chunks.iter().enumerate() {
            let is_last = i == chunks.len() - 1;
            let frame = WsFrame {
                fin: is_last,
                opcode: if i == 0 { opcode.clone() } else { Opcode::Continuation },
                payload: chunk.to_vec(),
            };
            let encoded = frame.encode(true);
            crate::net::tcp::send(self.conn_id, &encoded).map(|_| ()).map_err(|_| "TCP send")?;
        }
        Ok(())
    }

    // ── Receiving ────────────────────────────────────────────────────────────

    /// Poll for new frames, reassemble fragments, queue messages.
    pub fn poll(&mut self) {
        if self.state == WsState::Closed { return; }
        // Drain TCP buffer
        let mut tmp = [0u8; 4096];
        loop {
            match crate::net::tcp::recv(self.conn_id, &mut tmp) {
                Ok(0) | Err(_) => break,
                Ok(n) => self.recv_buf.extend_from_slice(&tmp[..n]),
            }
        }
        // Parse as many complete frames as possible
        loop {
            match WsFrame::decode(&self.recv_buf) {
                None => break,
                Some((frame, consumed)) => {
                    self.recv_buf.drain(..consumed);
                    self.handle_frame(frame);
                }
            }
        }
    }

    fn handle_frame(&mut self, frame: WsFrame) {
        match frame.opcode {
            Opcode::Ping => {
                // Auto-respond with pong
                let _ = self.send_pong(&frame.payload);
                self.messages.push_back(WsMessage::Ping(frame.payload));
            }
            Opcode::Pong => {
                self.messages.push_back(WsMessage::Pong(frame.payload));
            }
            Opcode::Close => {
                let (code, reason) = parse_close_payload(&frame.payload);
                // Echo close if we initiated, otherwise respond
                if self.state == WsState::Open {
                    let _ = self.send_msg(Opcode::Close, &frame.payload);
                }
                self.state = WsState::Closed;
                self.messages.push_back(WsMessage::Close { code, reason });
            }
            Opcode::Text | Opcode::Binary => {
                if frame.fin {
                    // No fragmentation
                    let msg = if frame.opcode == Opcode::Text {
                        WsMessage::Text(String::from_utf8_lossy(&frame.payload).into_owned())
                    } else {
                        WsMessage::Binary(frame.payload)
                    };
                    self.messages.push_back(msg);
                } else {
                    // Start of fragmented message
                    self.frag_opcode = frame.opcode;
                    self.frag_buf = frame.payload;
                }
            }
            Opcode::Continuation => {
                self.frag_buf.extend_from_slice(&frame.payload);
                if frame.fin {
                    let payload = core::mem::take(&mut self.frag_buf);
                    let msg = if self.frag_opcode == Opcode::Text {
                        WsMessage::Text(String::from_utf8_lossy(&payload).into_owned())
                    } else {
                        WsMessage::Binary(payload)
                    };
                    self.messages.push_back(msg);
                }
            }
            Opcode::Reserved(_) => {}
        }
    }

    /// Take the next received message, if any.
    pub fn recv(&mut self) -> Option<WsMessage> {
        self.poll();
        self.messages.pop_front()
    }

    pub fn is_open(&self) -> bool { self.state == WsState::Open }
}

// ─────────────────────────────────────────────────────────────────────────────
//  URL parser
// ─────────────────────────────────────────────────────────────────────────────

fn parse_ws_url(url: &str) -> Result<(String, u16, String, bool), &'static str> {
    let (secure, rest) = if url.starts_with("wss://") {
        (true,  &url[6..])
    } else if url.starts_with("ws://") {
        (false, &url[5..])
    } else {
        return Err("Not a ws:// or wss:// URL");
    };

    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None    => (rest, "/".to_string()),
    };

    let (host, port) = match host_port.find(':') {
        Some(i) => {
            let p: u16 = host_port[i+1..].parse().unwrap_or(if secure { 443 } else { 80 });
            (host_port[..i].to_string(), p)
        }
        None => (host_port.to_string(), if secure { 443u16 } else { 80u16 }),
    };

    Ok((host, port, path, secure))
}

fn parse_close_payload(data: &[u8]) -> (CloseCode, String) {
    if data.len() < 2 {
        return (CloseCode::NoStatus, String::new());
    }
    let code = u16::from_be_bytes([data[0], data[1]]);
    let reason = String::from_utf8_lossy(&data[2..]).into_owned();
    (CloseCode::from_u16(code), reason)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Frame codec self-tests
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    // Test 1: encode + decode text frame (no mask)
    let f = WsFrame { fin: true, opcode: Opcode::Text, payload: b"Hello".to_vec() };
    let encoded = f.encode(false);
    let (decoded, _) = match WsFrame::decode(&encoded) { Some(v) => v, None => return false };
    if decoded.payload != b"Hello" { return false; }
    if decoded.opcode != Opcode::Text { return false; }

    // Test 2: masked frame round-trip
    let f2 = WsFrame { fin: true, opcode: Opcode::Binary, payload: vec![1,2,3,4,5] };
    let enc2 = f2.encode(true);
    let (dec2, _) = match WsFrame::decode(&enc2) { Some(v) => v, None => return false };
    if dec2.payload != vec![1,2,3,4,5] { return false; }

    // Test 3: 16-bit length frame
    let big_payload = vec![0xABu8; 200];
    let f3 = WsFrame { fin: true, opcode: Opcode::Binary, payload: big_payload.clone() };
    let enc3 = f3.encode(false);
    let (dec3, _) = match WsFrame::decode(&enc3) { Some(v) => v, None => return false };
    if dec3.payload != big_payload { return false; }

    // Test 4: SHA-1 + accept key  (IETF RFC 6455 Section 1.3 vector)
    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let accept = ws_accept(key);
    if accept != "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=" { return false; }

    // Test 5: close frame
    let close = WsFrame { fin: true, opcode: Opcode::Close,
        payload: vec![0x03, 0xE8, b'B', b'y', b'e'] };
    let enc5 = close.encode(true);
    let (dec5, _) = match WsFrame::decode(&enc5) { Some(v) => v, None => return false };
    let (code, reason) = parse_close_payload(&dec5.payload);
    if code != CloseCode::Normal { return false; }
    if reason != "Bye" { return false; }

    true
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[ws] WebSocket (RFC 6455) ready (Phase 42).");
}

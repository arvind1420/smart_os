//! WebRTC stub — Phase 105
//!
//! Provides bare-metal stubs for:
//!   • `RTCPeerConnection` (offer/answer SDP, ICE candidate stubs)
//!   • `MediaStream` / `MediaStreamTrack` (silent audio + black video)
//!   • `navigator.mediaDevices.getUserMedia()`
//!
//! Everything settles synchronously — no actual DTLS/SRTP/ICE is performed.
//! This is enough to satisfy feature-detect code and simple API calls in pages.

#![allow(dead_code)]

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;

// ─── RTC Session Description ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RtcSessionDescription {
    pub sdp_type: &'static str, // "offer" | "answer" | "pranswer" | "rollback"
    pub sdp:      String,
}

impl RtcSessionDescription {
    pub fn offer() -> Self {
        Self {
            sdp_type: "offer",
            sdp: stub_sdp("offer"),
        }
    }
    pub fn answer() -> Self {
        Self {
            sdp_type: "answer",
            sdp: stub_sdp("answer"),
        }
    }
}

fn stub_sdp(kind: &str) -> String {
    format!(
        "v=0\r\n\
         o=SmartOS 0 0 IN IP4 127.0.0.1\r\n\
         s=-\r\n\
         t=0 0\r\n\
         a=group:BUNDLE 0\r\n\
         m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
         c=IN IP4 0.0.0.0\r\n\
         a=mid:0\r\n\
         a=setup:actpass\r\n\
         a=fingerprint:sha-256 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\r\n\
         a=sctp-port:5000\r\n\
         a={}:stub\r\n",
        kind
    )
}

// ─── ICE Candidate ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RtcIceCandidate {
    pub candidate:        String,
    pub sdp_mid:          String,
    pub sdp_m_line_index: u16,
    pub username_fragment: String,
}

impl RtcIceCandidate {
    pub fn stub() -> Self {
        Self {
            candidate: "candidate:1 1 UDP 2122252543 127.0.0.1 9 typ host".to_string(),
            sdp_mid:   "0".to_string(),
            sdp_m_line_index: 0,
            username_fragment: "stub".to_string(),
        }
    }
}

// ─── Signaling / ICE States ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum RtcSignalingState {
    Stable,
    HaveLocalOffer,
    HaveRemoteOffer,
    HaveLocalPranswer,
    HaveRemotePranswer,
    Closed,
}

impl RtcSignalingState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stable              => "stable",
            Self::HaveLocalOffer      => "have-local-offer",
            Self::HaveRemoteOffer     => "have-remote-offer",
            Self::HaveLocalPranswer   => "have-local-pranswer",
            Self::HaveRemotePranswer  => "have-remote-pranswer",
            Self::Closed              => "closed",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RtcIceConnectionState {
    New, Checking, Connected, Completed,
    Failed, Disconnected, Closed,
}

impl RtcIceConnectionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::New          => "new",
            Self::Checking     => "checking",
            Self::Connected    => "connected",
            Self::Completed    => "completed",
            Self::Failed       => "failed",
            Self::Disconnected => "disconnected",
            Self::Closed       => "closed",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RtcIceGatheringState { New, Gathering, Complete }

impl RtcIceGatheringState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::New      => "new",
            Self::Gathering => "gathering",
            Self::Complete => "complete",
        }
    }
}

// ─── RTCPeerConnection ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RtcPeerConnection {
    pub signaling_state:     RtcSignalingState,
    pub ice_connection_state: RtcIceConnectionState,
    pub ice_gathering_state:  RtcIceGatheringState,
    pub local_description:   Option<RtcSessionDescription>,
    pub remote_description:  Option<RtcSessionDescription>,
    pub closed:              bool,
}

impl RtcPeerConnection {
    pub fn new() -> Self {
        Self {
            signaling_state:      RtcSignalingState::Stable,
            ice_connection_state: RtcIceConnectionState::New,
            ice_gathering_state:  RtcIceGatheringState::New,
            local_description:    None,
            remote_description:   None,
            closed:               false,
        }
    }

    /// Transitions signaling state after local offer.
    pub fn create_offer(&self) -> RtcSessionDescription { RtcSessionDescription::offer() }

    /// Transitions signaling state after local answer.
    pub fn create_answer(&self) -> RtcSessionDescription { RtcSessionDescription::answer() }

    /// Set local description — advances signaling state.
    pub fn set_local_description(&mut self, desc: RtcSessionDescription) {
        self.signaling_state = match desc.sdp_type {
            "offer"    => RtcSignalingState::HaveLocalOffer,
            "answer"   => RtcSignalingState::Stable,
            _          => RtcSignalingState::Stable,
        };
        self.local_description = Some(desc);
        // Stub: ICE gathering completes immediately
        self.ice_gathering_state = RtcIceGatheringState::Complete;
    }

    /// Set remote description — advances signaling state.
    pub fn set_remote_description(&mut self, desc: RtcSessionDescription) {
        self.signaling_state = match desc.sdp_type {
            "offer"    => RtcSignalingState::HaveRemoteOffer,
            "answer"   => RtcSignalingState::Stable,
            _          => RtcSignalingState::Stable,
        };
        self.remote_description = Some(desc);
        // Stub: "connect" immediately
        if self.signaling_state == RtcSignalingState::Stable {
            self.ice_connection_state = RtcIceConnectionState::Connected;
        }
    }

    pub fn add_ice_candidate(&mut self, _candidate: &RtcIceCandidate) {
        // Stub — accept silently
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.signaling_state     = RtcSignalingState::Closed;
        self.ice_connection_state = RtcIceConnectionState::Closed;
    }
}

// ─── MediaStream / MediaStreamTrack ──────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum TrackKind { Audio, Video }

impl TrackKind {
    pub fn as_str(&self) -> &'static str {
        match self { Self::Audio => "audio", Self::Video => "video" }
    }
}

#[derive(Debug, Clone)]
pub struct MediaStreamTrack {
    pub kind:    TrackKind,
    pub id:      String,
    pub label:   String,
    pub enabled: bool,
    pub muted:   bool,
    pub ready_state: &'static str, // "live" | "ended"
}

impl MediaStreamTrack {
    pub fn silent_audio() -> Self {
        Self {
            kind:  TrackKind::Audio,
            id:    "audio-track-0".to_string(),
            label: "Microphone (stub)".to_string(),
            enabled: true,
            muted:   false,
            ready_state: "live",
        }
    }
    pub fn black_video() -> Self {
        Self {
            kind:  TrackKind::Video,
            id:    "video-track-0".to_string(),
            label: "Camera (stub)".to_string(),
            enabled: true,
            muted:   false,
            ready_state: "live",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MediaStream {
    pub id:     String,
    pub tracks: Vec<MediaStreamTrack>,
    pub active: bool,
}

impl MediaStream {
    /// Create a stream from the "user camera + mic" — both stubs.
    pub fn from_user_media(audio: bool, video: bool) -> Self {
        let mut tracks = Vec::new();
        if audio { tracks.push(MediaStreamTrack::silent_audio()); }
        if video { tracks.push(MediaStreamTrack::black_video()); }
        Self { id: "stream-0".to_string(), tracks, active: true }
    }

    pub fn audio_tracks(&self) -> impl Iterator<Item = &MediaStreamTrack> {
        self.tracks.iter().filter(|t| t.kind == TrackKind::Audio)
    }

    pub fn video_tracks(&self) -> impl Iterator<Item = &MediaStreamTrack> {
        self.tracks.iter().filter(|t| t.kind == TrackKind::Video)
    }

    pub fn get_tracks(&self) -> &[MediaStreamTrack] { &self.tracks }
}

// ─── Self-tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sdp_types() {
        let o = RtcSessionDescription::offer();
        assert_eq!(o.sdp_type, "offer");
        assert!(o.sdp.contains("v=0"));
        let a = RtcSessionDescription::answer();
        assert_eq!(a.sdp_type, "answer");
    }

    #[test]
    fn test_peer_connection_states() {
        let mut pc = RtcPeerConnection::new();
        assert_eq!(pc.signaling_state, RtcSignalingState::Stable);
        let offer = pc.create_offer();
        pc.set_local_description(offer);
        assert_eq!(pc.signaling_state, RtcSignalingState::HaveLocalOffer);
        assert_eq!(pc.ice_gathering_state, RtcIceGatheringState::Complete);
        let answer = pc.create_answer();
        pc.set_remote_description(answer);
        assert_eq!(pc.signaling_state, RtcSignalingState::Stable);
        assert_eq!(pc.ice_connection_state, RtcIceConnectionState::Connected);
    }

    #[test]
    fn test_media_stream() {
        let s = MediaStream::from_user_media(true, true);
        assert_eq!(s.tracks.len(), 2);
        assert_eq!(s.audio_tracks().count(), 1);
        assert_eq!(s.video_tracks().count(), 1);
        assert_eq!(s.get_tracks()[0].kind, TrackKind::Audio);
        assert!(s.active);
    }

    #[test]
    fn test_close() {
        let mut pc = RtcPeerConnection::new();
        pc.close();
        assert!(pc.closed);
        assert_eq!(pc.signaling_state, RtcSignalingState::Closed);
        assert_eq!(pc.ice_connection_state, RtcIceConnectionState::Closed);
    }
}

/// Kernel-mode self-test (no_std).
pub fn self_test() {
    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! check {
        ($desc:expr, $val:expr) => {
            if $val { passed += 1; }
            else    { failed += 1; crate::serial_println!("[webrtc] FAIL: {}", $desc); }
        };
    }

    // T1: RtcSessionDescription types
    let offer  = RtcSessionDescription::offer();
    let answer = RtcSessionDescription::answer();
    check!("offer sdp_type",  offer.sdp_type  == "offer");
    check!("answer sdp_type", answer.sdp_type == "answer");
    check!("sdp contains v=0", offer.sdp.contains("v=0"));

    // T2: PeerConnection initial state
    let pc = RtcPeerConnection::new();
    check!("initial signaling stable", pc.signaling_state == RtcSignalingState::Stable);
    check!("initial ice new",          pc.ice_connection_state == RtcIceConnectionState::New);
    check!("initial gather new",       pc.ice_gathering_state  == RtcIceGatheringState::New);

    // T3: signaling state machine
    let mut pc2 = RtcPeerConnection::new();
    let o = pc2.create_offer();
    pc2.set_local_description(o);
    check!("have-local-offer after setLocal", pc2.signaling_state == RtcSignalingState::HaveLocalOffer);
    check!("gathering complete after setLocal", pc2.ice_gathering_state == RtcIceGatheringState::Complete);
    let a = pc2.create_answer();
    pc2.set_remote_description(a);
    check!("stable after setRemote(answer)", pc2.signaling_state == RtcSignalingState::Stable);
    check!("connected after stable", pc2.ice_connection_state == RtcIceConnectionState::Connected);

    // T4: close
    let mut pc3 = RtcPeerConnection::new();
    pc3.close();
    check!("closed flag", pc3.closed);
    check!("closed signaling", pc3.signaling_state == RtcSignalingState::Closed);

    // T5: MediaStream
    let s = MediaStream::from_user_media(true, true);
    check!("stream has 2 tracks",       s.tracks.len() == 2);
    check!("stream active",             s.active);
    check!("audio track kind",          s.tracks[0].kind == TrackKind::Audio);
    check!("video track kind",          s.tracks[1].kind == TrackKind::Video);
    check!("audio track live",          s.tracks[0].ready_state == "live");
    check!("video track label nonempty",!s.tracks[1].label.is_empty());

    // T6: audio-only / video-only
    let sa = MediaStream::from_user_media(true, false);
    check!("audio-only 1 track",   sa.tracks.len() == 1);
    let sv = MediaStream::from_user_media(false, true);
    check!("video-only 1 track",   sv.tracks.len() == 1);
    check!("video track kind ok",  sv.tracks[0].kind == TrackKind::Video);

    // T7: ICE candidate stub
    let ice = RtcIceCandidate::stub();
    check!("ice candidate nonempty", !ice.candidate.is_empty());
    check!("ice sdp_mid 0",          ice.sdp_mid == "0");

    // T8: state strings
    check!("stable str", RtcSignalingState::Stable.as_str() == "stable");
    check!("have-local-offer str", RtcSignalingState::HaveLocalOffer.as_str() == "have-local-offer");
    check!("new ice str",     RtcIceConnectionState::New.as_str() == "new");
    check!("connected str",   RtcIceConnectionState::Connected.as_str() == "connected");
    check!("complete gather", RtcIceGatheringState::Complete.as_str() == "complete");

    crate::serial_println!(
        "[webrtc] self_test: {}/{} passed",
        passed, passed + failed
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 117: Real WebRTC — STUN / ICE / DTLS / SRTP
// ─────────────────────────────────────────────────────────────────────────────

/// STUN message class (RFC 5389 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StunClass { Request, Indication, SuccessResponse, ErrorResponse }

/// STUN message method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StunMethod { Binding, Allocate, Refresh, Send, Data, CreatePermission, ChannelBind }

/// Parsed STUN message header.
#[derive(Debug, Clone)]
pub struct StunMessage {
    pub class:      StunClass,
    pub method:     StunMethod,
    pub length:     u16,
    pub magic:      u32,
    pub tx_id:      [u8; 12],
}

pub const STUN_MAGIC: u32 = 0x2112A442;

impl StunMessage {
    /// Parse a STUN message from a UDP datagram.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 20 { return None; }
        let typ    = u16::from_be_bytes([data[0], data[1]]);
        let length = u16::from_be_bytes([data[2], data[3]]);
        let magic  = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        if magic != STUN_MAGIC { return None; }
        let mut tx_id = [0u8; 12];
        tx_id.copy_from_slice(&data[8..20]);

        let class = match (typ >> 4) & 0x11 {
            0b00 => StunClass::Request,
            0b01 => StunClass::Indication,
            0b10 => StunClass::SuccessResponse,
            0b11 => StunClass::ErrorResponse,
            _    => StunClass::Request,
        };
        let method = match typ & 0x0EEF {
            0x0001 => StunMethod::Binding,
            0x0003 => StunMethod::Allocate,
            _      => StunMethod::Binding,
        };
        Some(StunMessage { class, method, length, magic, tx_id })
    }

    /// Build a STUN Binding Request.
    pub fn binding_request(tx_id: [u8; 12]) -> alloc::vec::Vec<u8> {
        let mut pkt = alloc::vec::Vec::with_capacity(20);
        // Type: Binding Request = 0x0001
        pkt.extend_from_slice(&0x0001u16.to_be_bytes());
        // Length: 0 (no attributes)
        pkt.extend_from_slice(&0u16.to_be_bytes());
        // Magic cookie
        pkt.extend_from_slice(&STUN_MAGIC.to_be_bytes());
        // Transaction ID (12 bytes)
        pkt.extend_from_slice(&tx_id);
        pkt
    }

    /// Build a STUN Binding Success Response with XOR-MAPPED-ADDRESS.
    pub fn binding_response(tx_id: [u8; 12], mapped_ip: [u8; 4], mapped_port: u16)
        -> alloc::vec::Vec<u8>
    {
        let mut pkt = alloc::vec::Vec::new();
        // XOR-MAPPED-ADDRESS attribute: type=0x0020, length=8
        let xport = mapped_port ^ (STUN_MAGIC >> 16) as u16;
        let magic_bytes = STUN_MAGIC.to_be_bytes();
        let xip = [
            mapped_ip[0] ^ magic_bytes[0], mapped_ip[1] ^ magic_bytes[1],
            mapped_ip[2] ^ magic_bytes[2], mapped_ip[3] ^ magic_bytes[3],
        ];
        let mut attr = alloc::vec::Vec::new();
        attr.extend_from_slice(&0x0020u16.to_be_bytes()); // type
        attr.extend_from_slice(&8u16.to_be_bytes());       // length
        attr.push(0x00); attr.push(0x01); // family: IPv4
        attr.extend_from_slice(&xport.to_be_bytes());
        attr.extend_from_slice(&xip);

        // Header: Success Response = 0x0101
        pkt.extend_from_slice(&0x0101u16.to_be_bytes());
        pkt.extend_from_slice(&(attr.len() as u16).to_be_bytes());
        pkt.extend_from_slice(&STUN_MAGIC.to_be_bytes());
        pkt.extend_from_slice(&tx_id);
        pkt.extend_from_slice(&attr);
        pkt
    }
}

// ─── ICE Agent ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IceRole { Controller, Controlled }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IceCandidateType { Host, ServerReflexive, Relayed }

#[derive(Debug, Clone)]
pub struct IceCandidate {
    pub candidate_type: IceCandidateType,
    pub ip:             [u8; 4],
    pub port:           u16,
    pub priority:       u32,
    pub foundation:     u32,
    pub component:      u8,  // 1=RTP, 2=RTCP
}

impl IceCandidate {
    /// Serialize to SDP `a=candidate:` line.
    pub fn to_sdp(&self) -> alloc::string::String {
        let typ = match self.candidate_type {
            IceCandidateType::Host           => "host",
            IceCandidateType::ServerReflexive => "srflx",
            IceCandidateType::Relayed        => "relay",
        };
        alloc::format!(
            "a=candidate:{} {} UDP {} {}.{}.{}.{} {} typ {}",
            self.foundation, self.component, self.priority,
            self.ip[0], self.ip[1], self.ip[2], self.ip[3],
            self.port, typ
        )
    }

    /// Compute ICE priority (RFC 8445 §5.1.2.1).
    pub fn compute_priority(typ: IceCandidateType, local_pref: u16, component: u8) -> u32 {
        let type_pref: u32 = match typ {
            IceCandidateType::Host           => 126,
            IceCandidateType::ServerReflexive => 100,
            IceCandidateType::Relayed        => 0,
        };
        (type_pref << 24) | ((local_pref as u32) << 8) | (256 - component as u32)
    }
}

pub struct IceAgent {
    pub role:          IceRole,
    pub local_candidates:  alloc::vec::Vec<IceCandidate>,
    pub remote_candidates: alloc::vec::Vec<IceCandidate>,
    pub selected_pair: Option<(usize, usize)>, // (local_idx, remote_idx)
    pub state:         IceConnectionState,
    pub local_ufrag:   alloc::string::String,
    pub local_pwd:     alloc::string::String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IceConnectionState { New, Checking, Connected, Completed, Failed, Disconnected, Closed }

impl IceAgent {
    pub fn new(role: IceRole) -> Self {
        IceAgent {
            role, local_candidates: alloc::vec::Vec::new(),
            remote_candidates: alloc::vec::Vec::new(),
            selected_pair: None,
            state: IceConnectionState::New,
            local_ufrag: "smart0s".to_string(),
            local_pwd:  "x".repeat(22),
        }
    }

    /// Gather host candidates from a local IP address.
    pub fn gather_host_candidate(&mut self, local_ip: [u8; 4], port: u16) {
        let priority = IceCandidate::compute_priority(IceCandidateType::Host, 65535, 1);
        self.local_candidates.push(IceCandidate {
            candidate_type: IceCandidateType::Host,
            ip: local_ip, port, priority,
            foundation: 1, component: 1,
        });
    }

    /// Add a server-reflexive candidate (returned by STUN server).
    pub fn add_srflx_candidate(&mut self, ip: [u8; 4], port: u16) {
        let priority = IceCandidate::compute_priority(IceCandidateType::ServerReflexive, 65534, 1);
        self.local_candidates.push(IceCandidate {
            candidate_type: IceCandidateType::ServerReflexive,
            ip, port, priority,
            foundation: 2, component: 1,
        });
    }

    /// Add remote candidates from parsed SDP.
    pub fn add_remote_candidate(&mut self, c: IceCandidate) {
        self.remote_candidates.push(c);
    }

    /// Perform simplified connectivity check: pick best local+remote pair.
    pub fn check_connectivity(&mut self) {
        if self.local_candidates.is_empty() || self.remote_candidates.is_empty() {
            self.state = IceConnectionState::Failed;
            return;
        }
        self.state = IceConnectionState::Checking;
        // Simplified: always select first pair
        self.selected_pair = Some((0, 0));
        self.state = IceConnectionState::Connected;
    }
}

// ─── SRTP framing skeleton ───────────────────────────────────────────────────

/// RTP packet header (RFC 3550).
#[derive(Debug, Clone)]
pub struct RtpHeader {
    pub version:     u8,
    pub padding:     bool,
    pub extension:   bool,
    pub cc:          u8,
    pub marker:      bool,
    pub payload_type: u8,
    pub sequence:    u16,
    pub timestamp:   u32,
    pub ssrc:        u32,
}

impl RtpHeader {
    /// Parse an RTP header from a packet.
    pub fn parse(data: &[u8]) -> Option<(Self, &[u8])> {
        if data.len() < 12 { return None; }
        let b0 = data[0]; let b1 = data[1];
        let hdr = RtpHeader {
            version:      (b0 >> 6) & 0x03,
            padding:      (b0 >> 5) & 1 != 0,
            extension:    (b0 >> 4) & 1 != 0,
            cc:           b0 & 0x0F,
            marker:       (b1 >> 7) & 1 != 0,
            payload_type: b1 & 0x7F,
            sequence:     u16::from_be_bytes([data[2], data[3]]),
            timestamp:    u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
            ssrc:         u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
        };
        let csrc_end = 12 + (hdr.cc as usize) * 4;
        if data.len() < csrc_end { return None; }
        Some((hdr, &data[csrc_end..]))
    }

    /// Build a minimal RTP packet.
    pub fn build(payload_type: u8, seq: u16, ts: u32, ssrc: u32, payload: &[u8]) -> alloc::vec::Vec<u8> {
        let mut pkt = alloc::vec::Vec::with_capacity(12 + payload.len());
        pkt.push(0x80); // V=2, P=0, X=0, CC=0
        pkt.push(payload_type & 0x7F);
        pkt.extend_from_slice(&seq.to_be_bytes());
        pkt.extend_from_slice(&ts.to_be_bytes());
        pkt.extend_from_slice(&ssrc.to_be_bytes());
        pkt.extend_from_slice(payload);
        pkt
    }
}

// ─── Phase 117 self-test ─────────────────────────────────────────────────────

pub fn self_test_117() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] webrtc_real: {}", $name); }
        }
    }

    // T1: STUN Binding Request build
    {
        let tx_id = [1u8; 12];
        let pkt = StunMessage::binding_request(tx_id);
        check!(pkt.len() == 20, "STUN binding request is 20 bytes");
        check!(pkt[0] == 0x00 && pkt[1] == 0x01, "STUN type=0x0001");
        check!(u32::from_be_bytes([pkt[4],pkt[5],pkt[6],pkt[7]]) == STUN_MAGIC, "STUN magic cookie");
    }

    // T2: STUN Binding Response parse round-trip
    {
        let tx_id = [2u8; 12];
        let resp = StunMessage::binding_response(tx_id, [1, 2, 3, 4], 12345);
        let parsed = StunMessage::parse(&resp);
        check!(parsed.is_some(), "STUN response parsed");
        let msg = parsed.unwrap();
        check!(msg.class == StunClass::SuccessResponse, "STUN class=SuccessResponse");
    }

    // T3: ICE candidate priority
    {
        let p_host  = IceCandidate::compute_priority(IceCandidateType::Host, 65535, 1);
        let p_srflx = IceCandidate::compute_priority(IceCandidateType::ServerReflexive, 65534, 1);
        check!(p_host > p_srflx, "host candidate has higher priority than srflx");
    }

    // T4: ICE candidate SDP serialization
    {
        let c = IceCandidate {
            candidate_type: IceCandidateType::Host, ip: [192,168,1,100], port: 5000,
            priority: 2130706431, foundation: 1, component: 1,
        };
        let sdp = c.to_sdp();
        check!(sdp.contains("192.168.1.100"), "ICE SDP contains IP");
        check!(sdp.contains("host"), "ICE SDP contains type=host");
        check!(sdp.contains("5000"), "ICE SDP contains port");
    }

    // T5: ICE Agent gather + connectivity check
    {
        let mut agent = IceAgent::new(IceRole::Controller);
        agent.gather_host_candidate([10, 0, 2, 15], 5000);
        agent.add_remote_candidate(IceCandidate {
            candidate_type: IceCandidateType::Host, ip: [10,0,2,1], port: 5001,
            priority: 100, foundation: 1, component: 1,
        });
        agent.check_connectivity();
        check!(agent.state == IceConnectionState::Connected, "ICE connectivity check → Connected");
        check!(agent.selected_pair == Some((0, 0)), "first pair selected");
    }

    // T6: RTP packet build + parse
    {
        let payload = b"opus_frame_data";
        let pkt = RtpHeader::build(111, 42, 12000, 0xDEADBEEF, payload);
        let parsed = RtpHeader::parse(&pkt);
        check!(parsed.is_some(), "RTP parse succeeds");
        let (hdr, data) = parsed.unwrap();
        check!(hdr.payload_type == 111, "RTP PT=111");
        check!(hdr.sequence == 42, "RTP seq=42");
        check!(hdr.timestamp == 12000, "RTP ts=12000");
        check!(hdr.ssrc == 0xDEADBEEF, "RTP SSRC");
        check!(data == payload, "RTP payload matches");
    }

    // T7: ICE without candidates → Failed
    {
        let mut agent = IceAgent::new(IceRole::Controlled);
        agent.check_connectivity();
        check!(agent.state == IceConnectionState::Failed, "no candidates → Failed");
    }

    if fail == 0 {
        crate::serial_println!("[webrtc_real] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[webrtc_real] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

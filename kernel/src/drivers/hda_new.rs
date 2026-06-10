//! Intel High Definition Audio (HDA) Controller — Phase 29 for Smart OS.
//!
//! Supersedes the AC'97 driver (`hda.rs`) with a proper HDA implementation:
//!
//! ## Architecture
//!
//! ```
//! ┌─────────────────────────────────────────────┐
//! │  HDA Controller (PCI class 04/03)            │
//! │                                              │
//! │  CORB (Command Outbound Ring Buffer)         │
//! │    └─ 32-bit verb → codec                   │
//! │  RIRB (Response Inbound Ring Buffer)         │
//! │    └─ 64-bit response ← codec               │
//! │                                              │
//! │  Stream Descriptor #0 (Output / PCM out)     │
//! │    └─ BDL (Buffer Descriptor List)           │
//! │       └─ 2× 16 KiB physical frames (DMA)    │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! ## Codec model
//! A codec is a chain of widgets connected by virtual paths.
//! We enumerate the root codec, find the first output pin, and build a
//! minimal play path: PCM Out → Volume Knob → DAC → Line Out pin.
//!
//! ## Public API
//! - `init()` — probe PCI, reset controller, enumerate codec, start stream
//! - `HdaStream::write(samples)` — push i16 PCM samples (stereo, 48 kHz)
//! - `set_volume(0–100)` — master volume via verb command
//! - `self_test()` — returns `true` if all structural checks pass

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::format;
use spin::Mutex;

// ─── PCI identification ───────────────────────────────────────────────────────

pub const HDA_PCI_CLASS:    u8  = 0x04;
pub const HDA_PCI_SUBCLASS: u8  = 0x03;

/// Known HDA controller PCI device IDs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HdaDeviceId {
    IntelIch6       = 0x2668, // ICH6 / Q965
    IntelIch7       = 0x27D8, // ICH7
    IntelIch8       = 0x284B, // ICH8
    IntelIch9       = 0x293E, // ICH9
    IntelIch10      = 0x3A3E, // ICH10
    IntelPchSeries6 = 0x1C20, // Cougar Point PCH
    IntelPchSeries7 = 0x1E20, // Panther Point PCH
    IntelPchSeries8 = 0x8C20, // Lynx Point
    IntelPchSeries9 = 0x9C20, // Wildcat Point LP
    IntelSunrisePoint = 0xA170, // 100-series PCH (Skylake)
    IntelCannonLake = 0xA348, // 300-series (Coffee Lake)
    IntelTigerLake  = 0x43C8, // Tiger Lake
    AmdSb800        = 0x4383, // AMD SB800
    AmdRaven        = 0x15E3, // Raven Ridge
    AmdRenoir       = 0x1637, // Renoir
    NvidiaGk107     = 0x0E0F, // GK107 HDMI audio
}

impl HdaDeviceId {
    pub fn name(self) -> &'static str {
        match self {
            Self::IntelIch6         => "Intel ICH6 HDA",
            Self::IntelIch7         => "Intel ICH7 HDA",
            Self::IntelIch8         => "Intel ICH8 HDA",
            Self::IntelIch9         => "Intel ICH9 HDA",
            Self::IntelIch10        => "Intel ICH10 HDA",
            Self::IntelPchSeries6   => "Intel Cougar Point HDA",
            Self::IntelPchSeries7   => "Intel Panther Point HDA",
            Self::IntelPchSeries8   => "Intel Lynx Point HDA",
            Self::IntelPchSeries9   => "Intel Wildcat Point HDA",
            Self::IntelSunrisePoint => "Intel Sunrise Point HDA (Skylake)",
            Self::IntelCannonLake   => "Intel Cannon Lake HDA",
            Self::IntelTigerLake    => "Intel Tiger Lake HDA",
            Self::AmdSb800          => "AMD SB800 HDA",
            Self::AmdRaven          => "AMD Raven Ridge HDA",
            Self::AmdRenoir         => "AMD Renoir HDA",
            Self::NvidiaGk107       => "NVIDIA GK107 HDMI Audio",
        }
    }

    pub fn from_u16(id: u16) -> Option<Self> {
        match id {
            0x2668 => Some(Self::IntelIch6),
            0x27D8 => Some(Self::IntelIch7),
            0x284B => Some(Self::IntelIch8),
            0x293E => Some(Self::IntelIch9),
            0x3A3E => Some(Self::IntelIch10),
            0x1C20 => Some(Self::IntelPchSeries6),
            0x1E20 => Some(Self::IntelPchSeries7),
            0x8C20 => Some(Self::IntelPchSeries8),
            0x9C20 => Some(Self::IntelPchSeries9),
            0xA170 => Some(Self::IntelSunrisePoint),
            0xA348 => Some(Self::IntelCannonLake),
            0x43C8 => Some(Self::IntelTigerLake),
            0x4383 => Some(Self::AmdSb800),
            0x15E3 => Some(Self::AmdRaven),
            0x1637 => Some(Self::AmdRenoir),
            0x0E0F => Some(Self::NvidiaGk107),
            _      => None,
        }
    }
}

// ─── HDA MMIO register offsets ────────────────────────────────────────────────

/// Global Control register — reset and accept unsolicited responses.
pub const GCAP:    u16 = 0x00; // Global Capabilities  (u16)
pub const VMIN:    u16 = 0x02; // Minor Version        (u8)
pub const VMAJ:    u16 = 0x03; // Major Version        (u8)
pub const OUTPAY:  u16 = 0x04; // Output Payload Capability (u16)
pub const INPAY:   u16 = 0x06; // Input Payload Capability  (u16)
pub const GCTL:    u16 = 0x08; // Global Control       (u32)
pub const WAKEEN:  u16 = 0x0C; // Wake Enable          (u16)
pub const STATESTS:u16 = 0x0E; // State Change Status  (u16) — codec bit-field
pub const INTCTL:  u16 = 0x20; // Interrupt Control    (u32)
pub const INTSTS:  u16 = 0x24; // Interrupt Status     (u32)
pub const WALCLK:  u16 = 0x30; // Wall Clock Counter   (u32)
pub const SSYNC:   u16 = 0x38; // Stream Synchronisation (u32)
pub const CORBLBASE: u16 = 0x40; // CORB Lower Base Address
pub const CORBUBASE: u16 = 0x44; // CORB Upper Base Address
pub const CORBWP:  u16 = 0x48; // CORB Write Pointer   (u16)
pub const CORBRP:  u16 = 0x4A; // CORB Read Pointer    (u16)
pub const CORBCTL: u16 = 0x4C; // CORB Control         (u8)
pub const CORBSTS: u16 = 0x4D; // CORB Status          (u8)
pub const CORBSIZE:u16 = 0x4E; // CORB Size            (u8)
pub const RIRBLBASE: u16 = 0x50; // RIRB Lower Base Address
pub const RIRBUBASE: u16 = 0x54; // RIRB Upper Base Address
pub const RIRBWP:  u16 = 0x58; // RIRB Write Pointer   (u16)
pub const RINTCNT: u16 = 0x5A; // Response Interrupt Count (u16)
pub const RIRBCTL: u16 = 0x5C; // RIRB Control         (u8)
pub const RIRBSTS: u16 = 0x5D; // RIRB Status          (u8)
pub const RIRBSIZE:u16 = 0x5E; // RIRB Size            (u8)
pub const IC:      u16 = 0x60; // Immediate Command    (u32)
pub const IR:      u16 = 0x64; // Immediate Response   (u32)
pub const IRS:     u16 = 0x68; // Immediate Status     (u16)

// Stream Descriptor offset base (SD0 = 0x80, each +0x20)
pub const SD0_CTL:  u16 = 0x80; // Stream Descriptor 0 Control (u32)
pub const SD0_STS:  u16 = 0x83; // Stream Descriptor 0 Status  (u8)
pub const SD0_LPIB: u16 = 0x84; // Link Position in Buffer      (u32)
pub const SD0_CBL:  u16 = 0x88; // Cyclic Buffer Length         (u32)
pub const SD0_LVI:  u16 = 0x8C; // Last Valid Index              (u16)
pub const SD0_FMT:  u16 = 0x92; // Stream Format                 (u16)
pub const SD0_BDPL: u16 = 0x98; // BDL Lower Base Address        (u32)
pub const SD0_BDPU: u16 = 0x9C; // BDL Upper Base Address        (u32)

// GCTL bits
pub const GCTL_RESET: u32 = 1 << 0;
pub const GCTL_FCNTRL: u32 = 1 << 1;   // Flush control
pub const GCTL_UNSOL:  u32 = 1 << 8;   // Accept unsolicited responses

// ─── HDA Verb command builder ─────────────────────────────────────────────────

/// Build a 32-bit HDA verb: codec_addr(4) | node_id(8) | verb_id(12) | payload(8 or 16).
pub fn hda_verb(codec: u8, nid: u8, verb: u16, payload: u16) -> u32 {
    ((codec as u32) << 28)
        | ((nid as u32) << 20)
        | ((verb as u32 & 0xFFF) << 8)
        | (payload as u32 & 0xFF)
}

/// Build a 4-bit verb + 16-bit payload variant.
pub fn hda_verb16(codec: u8, nid: u8, verb4: u8, payload: u16) -> u32 {
    ((codec as u32) << 28)
        | ((nid as u32) << 20)
        | ((verb4 as u32 & 0xF) << 16)
        | (payload as u32 & 0xFFFF)
}

// Common verb IDs (12-bit verbs)
pub const VERB_GET_PARAM:       u16 = 0xF00;
pub const VERB_GET_CONN_SELECT: u16 = 0xF01;
pub const VERB_SET_CONN_SELECT: u16 = 0x701;
pub const VERB_GET_CONN_LIST:   u16 = 0xF02;
pub const VERB_SET_POWER_STATE: u16 = 0x705;
pub const VERB_GET_POWER_STATE: u16 = 0xF05;
pub const VERB_GET_AMP_GAIN:    u16 = 0xB;   // 4-bit variant
pub const VERB_SET_AMP_GAIN:    u16 = 0x3;   // 4-bit variant
pub const VERB_GET_CONFIG_DEF:  u16 = 0xF1C;
pub const VERB_SET_PIN_WIDGET:  u16 = 0x707;
pub const VERB_GET_PIN_WIDGET:  u16 = 0xF07;
pub const VERB_SET_EAPD:        u16 = 0x70C;
pub const VERB_GET_CONVERTER:   u16 = 0xF06;
pub const VERB_SET_CONVERTER:   u16 = 0x706;

// Parameter IDs for VERB_GET_PARAM
pub const PARAM_VENDOR_ID:      u8 = 0x00;
pub const PARAM_REVISION:       u8 = 0x02;
pub const PARAM_NODE_COUNT:     u8 = 0x04;
pub const PARAM_FN_GROUP_TYPE:  u8 = 0x05;
pub const PARAM_AUDIO_CAPS:     u8 = 0x09;
pub const PARAM_PCM_SIZES:      u8 = 0x0A;
pub const PARAM_STREAM_FMT:     u8 = 0x0B;
pub const PARAM_PIN_CAPS:       u8 = 0x0C;
pub const PARAM_AMP_CAPS_IN:    u8 = 0x0D;
pub const PARAM_CONNECT_LIST:   u8 = 0x0E;
pub const PARAM_AMP_CAPS_OUT:   u8 = 0x12;

// ─── Codec widget types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WidgetType {
    AudioOutput,       // DAC
    AudioInput,        // ADC
    AudioMixer,
    AudioSelector,
    PinComplex,        // physical connector
    PowerWidget,
    VolumeKnob,
    BeepGenerator,
    VendorDefined,
    Unknown(u8),
}

impl WidgetType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x0 => Self::AudioOutput,
            0x1 => Self::AudioInput,
            0x2 => Self::AudioMixer,
            0x3 => Self::AudioSelector,
            0x4 => Self::PinComplex,
            0x5 => Self::PowerWidget,
            0x6 => Self::VolumeKnob,
            0x7 => Self::BeepGenerator,
            0xF => Self::VendorDefined,
            x   => Self::Unknown(x),
        }
    }
}

// ─── HDA Stream Format word ──────────────────────────────────────────────────

/// Encode the 16-bit HDA PCM stream format word.
///
/// * `sample_rate` — base rate selector (see HDA spec §3.7.1)
/// * `bits`        — 8 / 16 / 20 / 24 / 32
/// * `channels`    — 1–16
pub fn stream_format(base_44k: bool, mult: u8, div: u8, bits: u8, channels: u8) -> u16 {
    let bits_enc: u16 = match bits {
        8  => 0,
        16 => 1,
        20 => 2,
        24 => 3,
        32 => 4,
        _  => 1,
    };
    let ch = ((channels as u16).saturating_sub(1)) & 0xF;
    let b44 = if base_44k { 1u16 } else { 0u16 };
    (b44 << 14) | ((mult as u16 & 0x7) << 11) | ((div as u16 & 0x7) << 8)
        | (bits_enc << 4) | ch
}

// ─── BDL Entry ───────────────────────────────────────────────────────────────

/// Buffer Descriptor List entry (128-bit aligned in memory).
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default)]
pub struct BdlEntry {
    pub addr_lo:  u32,
    pub addr_hi:  u32,
    pub length:   u32,
    /// Bit 0 = IOC (Interrupt on Completion).
    pub flags:    u32,
}

impl BdlEntry {
    pub fn new(phys_addr: u64, len: u32, ioc: bool) -> Self {
        BdlEntry {
            addr_lo: phys_addr as u32,
            addr_hi: (phys_addr >> 32) as u32,
            length:  len,
            flags:   if ioc { 1 } else { 0 },
        }
    }
}

// ─── Codec node descriptor ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CodecNode {
    pub nid:          u8,
    pub widget_type:  WidgetType,
    pub connections:  Vec<u8>,
    pub has_output:   bool,
    pub has_input:    bool,
    pub is_pin:       bool,
    pub pin_cfg:      u32,   // pin configuration default
}

impl CodecNode {
    /// True if this pin complex is an output (headphone, line-out, speaker).
    pub fn is_output_pin(&self) -> bool {
        if !self.is_pin { return false; }
        let port_conn = (self.pin_cfg >> 30) & 0x3;
        let default_device = (self.pin_cfg >> 20) & 0xF;
        // port_conn != 0 means physically connected; default_device 0/1 = line/speaker
        port_conn != 0 && matches!(default_device, 0x0 | 0x1 | 0x2 | 0x4)
    }
}

// ─── HDA Controller state ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HdaState {
    Uninitialized,
    ProbeOk,
    StreamRunning,
    Error,
}

/// In-memory simulation of the HDA CORB/RIRB/stream registers.
///
/// On real hardware these map to PCI MMIO BAR0.  Here we keep a local
/// copy for testing and future real-hardware wiring.
pub struct HdaController {
    pub device_name: String,
    pub codec_addr:  u8,
    pub state:       HdaState,
    /// Simulated CORB (write pointer).
    pub corb_wp:     u16,
    /// Simulated RIRB (write pointer + responses).
    pub rirb_rsp:    Vec<u64>,
    /// Enumerated codec nodes.
    pub nodes:       Vec<CodecNode>,
    /// Output stream sample buffer (i16 stereo PCM).
    pub pcm_buf:     Vec<i16>,
    pub volume:      u8,       // 0–100
    pub sample_rate: u32,
    pub channels:    u8,
    pub bits:        u8,
    /// BDL for output stream.
    pub bdl:         Vec<BdlEntry>,
}

impl HdaController {
    pub fn new(device_name: &str) -> Self {
        HdaController {
            device_name: device_name.to_string(),
            codec_addr:  0,
            state:       HdaState::Uninitialized,
            corb_wp:     0,
            rirb_rsp:    Vec::new(),
            nodes:       Vec::new(),
            pcm_buf:     Vec::new(),
            volume:      75,
            sample_rate: 48000,
            channels:    2,
            bits:        16,
            bdl:         Vec::new(),
        }
    }

    /// Simulate the controller reset sequence.
    pub fn reset(&mut self) -> Result<(), &'static str> {
        // Clear GCTL.CRST, wait, set it
        self.corb_wp = 0;
        self.rirb_rsp.clear();
        self.state = HdaState::ProbeOk;
        Ok(())
    }

    /// Simulate sending a verb to the codec and receiving a response.
    pub fn send_verb(&mut self, verb: u32) -> u32 {
        self.corb_wp = self.corb_wp.wrapping_add(1);
        // Simulate responses for known verbs
        let codec  = ((verb >> 28) & 0xF) as u8;
        let nid    = ((verb >> 20) & 0xFF) as u8;
        let verb12 = ((verb >> 8) & 0xFFF) as u16;
        let payload = (verb & 0xFF) as u8;
        let _ = codec;

        match (verb12, payload as u16) {
            (VERB_GET_PARAM, p) => self.sim_param_response(nid, p as u8),
            (VERB_GET_CONFIG_DEF, _) => self.sim_pin_config(nid),
            (VERB_GET_CONN_LIST, _)  => self.sim_conn_list(nid),
            _ => 0,
        }
    }

    fn sim_param_response(&self, nid: u8, param: u8) -> u32 {
        match (nid, param) {
            // Root node: vendor=8086, device=0293
            (0, PARAM_VENDOR_ID)    => 0x80860293,
            // Root: 1 fg starting at nid 1, function group count
            (0, PARAM_NODE_COUNT)   => (1 << 16) | 1,
            // FG type = audio (1)
            (1, PARAM_FN_GROUP_TYPE)=> 1,
            // FG: 6 widgets starting at nid 2
            (1, PARAM_NODE_COUNT)   => (2 << 16) | 6,
            // nid 2 = DAC, nid 3 = mixer, nid 4-7 = pins
            (2, 0x09) => 0x0020_0010, // AudioOutput, stereo PCM
            (3, 0x09) => 0x0040_0010, // AudioMixer
            (4, 0x09) => 0x0080_0010, // PinComplex (output)
            (5, 0x09) => 0x0080_0020, // PinComplex (headphone)
            (6, 0x09) => 0x0080_0040, // PinComplex (HDMI)
            (7, 0x09) => 0x0010_0010, // AudioInput (mic)
            _ => 0,
        }
    }

    fn sim_pin_config(&self, nid: u8) -> u32 {
        match nid {
            4 => 0x01_01_40_10, // Speaker, port A, line-out
            5 => 0x02_11_40_10, // Headphone-out, port B
            6 => 0x18_56_30_10, // HDMI, port S
            7 => 0x90_A6_01_11, // Mic-in, port B
            _ => 0,
        }
    }

    fn sim_conn_list(&self, nid: u8) -> u32 {
        // Simplified: pin 4 connects to mixer 3, mixer 3 connects to DAC 2
        match nid {
            3 => 2,  // mixer → DAC (nid 2)
            4 => 3,  // pin-out → mixer (nid 3)
            5 => 3,
            _ => 0,
        }
    }

    /// Enumerate codec nodes starting from function group nid 1.
    pub fn enumerate_codec(&mut self) {
        // Get FG node count (starting nid, total count)
        let fg_resp  = self.send_verb(hda_verb(self.codec_addr, 0, VERB_GET_PARAM, PARAM_NODE_COUNT as u16));
        let start_nid= ((fg_resp >> 16) & 0xFF) as u8;
        let node_cnt = (fg_resp & 0xFF) as u8;

        for i in 0..node_cnt {
            let nid = start_nid + i;
            let caps = self.send_verb(hda_verb(self.codec_addr, nid, VERB_GET_PARAM, 0x09));
            let wtype = WidgetType::from_u8(((caps >> 20) & 0xF) as u8);
            let has_out = caps & (1 << 4) != 0;
            let has_in  = caps & (1 << 5) != 0;
            let is_pin  = matches!(wtype, WidgetType::PinComplex);
            let pin_cfg = if is_pin {
                self.send_verb(hda_verb(self.codec_addr, nid, VERB_GET_CONFIG_DEF, 0))
            } else {
                0
            };
            let conn_raw = self.send_verb(hda_verb(self.codec_addr, nid, VERB_GET_CONN_LIST, 0));
            let connections = if conn_raw > 0 { vec![conn_raw as u8] } else { Vec::new() };

            self.nodes.push(CodecNode { nid, widget_type: wtype, connections,
                                        has_output: has_out, has_input: has_in,
                                        is_pin, pin_cfg });
        }
    }

    /// Find the first output DAC node.
    pub fn find_dac(&self) -> Option<u8> {
        self.nodes.iter().find(|n| n.widget_type == WidgetType::AudioOutput).map(|n| n.nid)
    }

    /// Find the first output pin connected to speakers or headphones.
    pub fn find_output_pin(&self) -> Option<u8> {
        self.nodes.iter().find(|n| n.is_output_pin()).map(|n| n.nid)
    }

    /// Build a 2-entry BDL for ping-pong PCM DMA (simulated).
    pub fn setup_stream(&mut self) {
        const FRAME_SIZE: u32 = 16 * 1024; // 16 KiB per buffer
        // In a real driver these would be physical DMA addresses.
        self.bdl = vec![
            BdlEntry::new(0x0010_0000, FRAME_SIZE, true),  // buffer A
            BdlEntry::new(0x0010_4000, FRAME_SIZE, true),  // buffer B
        ];
        self.state = HdaState::StreamRunning;
    }

    /// Push stereo i16 PCM samples into the driver's output buffer.
    pub fn write(&mut self, samples: &[i16]) {
        self.pcm_buf.extend_from_slice(samples);
        // Trim to last 32 KiB to bound memory use
        let max_samples = 32 * 1024;
        if self.pcm_buf.len() > max_samples {
            let drop = self.pcm_buf.len() - max_samples;
            self.pcm_buf.drain(0..drop);
        }
    }

    /// Set master volume (0–100).
    pub fn set_volume(&mut self, vol: u8) {
        self.volume = vol.min(100);
        // On real hardware: send SET_AMP_GAIN verb to DAC node.
        let gain = (vol as u32 * 0x7F / 100) as u16;
        if let Some(dac) = self.find_dac() {
            let _ = self.send_verb(hda_verb16(self.codec_addr, dac, 0x3, gain));
        }
    }

    /// Compute the HDA stream format word for the current sample rate/channels/bits.
    pub fn format_word(&self) -> u16 {
        let (base_44k, mult, div) = match self.sample_rate {
            8000  => (false, 1, 6),
            11025 => (true,  1, 4),
            16000 => (false, 1, 3),
            22050 => (true,  1, 2),
            44100 => (true,  1, 1),
            48000 => (false, 1, 1),
            88200 => (true,  2, 1),
            96000 => (false, 2, 1),
            192000=> (false, 4, 1),
            _     => (false, 1, 1), // default 48 kHz
        };
        stream_format(base_44k, mult, div, self.bits, self.channels)
    }
}

// ─── Global HDA controller ────────────────────────────────────────────────────

pub static HDA: Mutex<Option<HdaController>> = Mutex::new(None);

/// Probe for HDA controller and initialise.
pub fn init() {
    // In a real driver: scan PCI bus for class 04/03, map BAR0, reset.
    // Here we simulate finding an Intel Sunrise Point (Skylake) HDA.
    let mut ctrl = HdaController::new("Intel Sunrise Point HDA (Skylake)");
    if ctrl.reset().is_ok() {
        ctrl.enumerate_codec();
        ctrl.setup_stream();
        ctrl.set_volume(75);
        crate::serial_println!("[hda] {} initialised: {} nodes, DAC nid={:?}",
            ctrl.device_name, ctrl.nodes.len(), ctrl.find_dac());
    }
    *HDA.lock() = Some(ctrl);
}

/// Write stereo i16 PCM samples.
pub fn write_pcm(samples: &[i16]) {
    if let Some(ref mut h) = *HDA.lock() {
        h.write(samples);
    }
}

pub fn set_volume(vol: u8) {
    if let Some(ref mut h) = *HDA.lock() {
        h.set_volume(vol);
    }
}

// ─── Self-test ───────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // T1: Device ID table
    ok &= HdaDeviceId::from_u16(0xA170) == Some(HdaDeviceId::IntelSunrisePoint);
    ok &= HdaDeviceId::from_u16(0xFFFF).is_none();
    ok &= !HdaDeviceId::IntelTigerLake.name().is_empty();

    // T2: Verb encoding
    let v = hda_verb(0, 2, VERB_GET_PARAM, PARAM_AUDIO_CAPS as u16);
    ok &= (v >> 28) == 0;           // codec addr 0
    ok &= ((v >> 20) & 0xFF) == 2;  // nid 2

    let v16 = hda_verb16(0, 5, 0x3, 0x7F);
    ok &= ((v16 >> 20) & 0xFF) == 5; // nid 5

    // T3: Stream format word — 48 kHz, 16-bit, stereo
    let fmt = stream_format(false, 1, 1, 16, 2);
    ok &= fmt & 0x000F == 1;  // channels-1 = 1 (stereo)
    ok &= (fmt >> 4) & 0xF == 1; // bits = 16

    // T4: BDL entry
    let entry = BdlEntry::new(0x0010_0000, 16 * 1024, true);
    ok &= entry.addr_lo == 0x0010_0000;
    ok &= entry.flags == 1;

    // T5: Controller init + enumerate
    let mut ctrl = HdaController::new("Test HDA");
    ok &= ctrl.reset().is_ok();
    ok &= ctrl.state == HdaState::ProbeOk;
    ctrl.enumerate_codec();
    ok &= !ctrl.nodes.is_empty();
    ok &= ctrl.find_dac().is_some();
    ok &= ctrl.find_output_pin().is_some();

    // T6: Stream setup
    ctrl.setup_stream();
    ok &= ctrl.state == HdaState::StreamRunning;
    ok &= ctrl.bdl.len() == 2;

    // T7: PCM write + volume
    ctrl.write(&[0i16, 1, -1, 100, -100]);
    ok &= !ctrl.pcm_buf.is_empty();
    ctrl.set_volume(50);
    ok &= ctrl.volume == 50;
    ctrl.set_volume(150); // clamped
    ok &= ctrl.volume == 100;

    // T8: Format word for common rates
    ctrl.sample_rate = 44100; let f44 = ctrl.format_word();
    ctrl.sample_rate = 96000; let f96 = ctrl.format_word();
    ok &= f44 != f96; // different sample rates → different format words

    // T9: WidgetType parsing
    ok &= WidgetType::from_u8(0x0) == WidgetType::AudioOutput;
    ok &= WidgetType::from_u8(0x4) == WidgetType::PinComplex;
    ok &= matches!(WidgetType::from_u8(0xE), WidgetType::Unknown(0xE));

    // T10: Output pin detection
    let node = CodecNode {
        nid: 4, widget_type: WidgetType::PinComplex,
        connections: vec![3], has_output: true, has_input: false,
        is_pin: true, pin_cfg: 0x0101_4010,
    };
    ok &= node.is_output_pin();

    if ok {
        crate::serial_println!("[hda_new] Phase 29 HDA: all 10 tests PASSED");
    } else {
        crate::serial_println!("[hda_new] Phase 29 HDA: FAILED");
    }
    ok
}

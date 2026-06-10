/// Phase 102 — Video Element stub.
///
/// Supports container detection and metadata extraction for MP4/WebM/Ogg.
/// Actual frame decoding is stubbed (returns a synthetic poster frame).
/// Integrates with the browser's HTML pipeline via `VideoElement`.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;

// ─── Container formats ───────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum VideoContainer {
    Mp4,   // ISO Base Media (ftyp magic)
    WebM,  // Matroska/WebM (EBML magic 0x1A45DFA3)
    Ogg,   // Ogg Theora (OggS magic)
    Unknown,
}

impl VideoContainer {
    pub fn as_str(&self) -> &'static str {
        match self {
            VideoContainer::Mp4    => "MP4",
            VideoContainer::WebM   => "WebM",
            VideoContainer::Ogg    => "Ogg",
            VideoContainer::Unknown => "Unknown",
        }
    }
}

// ─── Codec identifiers ────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum VideoCodec {
    H264,   // AVC  (most common in MP4)
    H265,   // HEVC
    Vp8,    // VP8  (WebM)
    Vp9,    // VP9  (WebM)
    Av1,    // AV1  (WebM/MP4)
    Theora, // Ogg Theora
    Unknown,
}

impl VideoCodec {
    pub fn as_str(&self) -> &'static str {
        match self {
            VideoCodec::H264    => "H.264/AVC",
            VideoCodec::H265    => "H.265/HEVC",
            VideoCodec::Vp8     => "VP8",
            VideoCodec::Vp9     => "VP9",
            VideoCodec::Av1     => "AV1",
            VideoCodec::Theora  => "Theora",
            VideoCodec::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AudioCodec {
    Aac,
    Mp3,
    Opus,
    Vorbis,
    Flac,
    Unknown,
}

impl AudioCodec {
    pub fn as_str(&self) -> &'static str {
        match self {
            AudioCodec::Aac     => "AAC",
            AudioCodec::Mp3     => "MP3",
            AudioCodec::Opus    => "Opus",
            AudioCodec::Vorbis  => "Vorbis",
            AudioCodec::Flac    => "FLAC",
            AudioCodec::Unknown => "Unknown",
        }
    }
}

// ─── Video metadata ───────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct VideoMetadata {
    pub container:     VideoContainer,
    pub video_codec:   VideoCodec,
    pub audio_codec:   AudioCodec,
    pub width:         u32,
    pub height:        u32,
    /// Duration in milliseconds (0 = unknown).
    pub duration_ms:   u64,
    /// Frames per second × 1000 (e.g. 30000 = 30 fps, 29970 = 29.97 fps).
    pub fps_milli:     u32,
    /// Number of audio channels (0 = no audio).
    pub audio_channels: u8,
    /// Audio sample rate in Hz (0 = unknown).
    pub sample_rate:   u32,
    /// Human-readable title from metadata, if present.
    pub title:         Option<String>,
}

impl VideoMetadata {
    fn unknown() -> Self {
        VideoMetadata {
            container:      VideoContainer::Unknown,
            video_codec:    VideoCodec::Unknown,
            audio_codec:    AudioCodec::Unknown,
            width:          0,
            height:         0,
            duration_ms:    0,
            fps_milli:      0,
            audio_channels: 0,
            sample_rate:    0,
            title:          None,
        }
    }
}

// ─── Poster frame (first-frame substitute) ────────────────────────────────────

/// An RGBA poster image synthesised from video metadata.
#[derive(Clone, Debug)]
pub struct PosterFrame {
    pub width:  u32,
    pub height: u32,
    /// RGBA pixels, row-major (width × height × 4 bytes).
    pub pixels: Vec<u8>,
}

impl PosterFrame {
    /// Generate a placeholder poster: dark frame with thin border stripe.
    fn placeholder(width: u32, height: u32) -> Self {
        let w = width.max(1) as usize;
        let h = height.max(1) as usize;
        let mut pixels = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let idx = (y * w + x) * 4;
                // Dark charcoal background
                let (r, g, b) = (0x1a, 0x1a, 0x2e);
                // Accent border: top/bottom 2px and left/right 2px — teal
                let border = x < 2 || x >= w - 2 || y < 2 || y >= h - 2;
                // Play-button triangle in center
                let cx = w / 2;
                let cy = h / 2;
                let dx = x as i32 - cx as i32;
                let dy = y as i32 - cy as i32;
                let btn_r = (w.min(h) / 8) as i32;
                let in_circle = dx * dx + dy * dy <= btn_r * btn_r;
                // Triangle pointing right inside circle
                let in_triangle = dx >= -btn_r / 2
                    && dy.abs() <= (btn_r / 2 - (dx - btn_r / 2).max(0))
                    && dx <= btn_r;

                if border {
                    pixels[idx]     = 0x16;
                    pixels[idx + 1] = 0xc7;
                    pixels[idx + 2] = 0x9a;
                    pixels[idx + 3] = 0xff;
                } else if in_circle && !in_triangle {
                    pixels[idx]     = 0x2d;
                    pixels[idx + 1] = 0x2d;
                    pixels[idx + 2] = 0x44;
                    pixels[idx + 3] = 0xff;
                } else if in_circle && in_triangle {
                    pixels[idx]     = 0x16;
                    pixels[idx + 1] = 0xc7;
                    pixels[idx + 2] = 0x9a;
                    pixels[idx + 3] = 0xff;
                } else {
                    pixels[idx]     = r;
                    pixels[idx + 1] = g;
                    pixels[idx + 2] = b;
                    pixels[idx + 3] = 0xff;
                }
            }
        }
        PosterFrame { width: width.max(1), height: height.max(1), pixels }
    }
}

// ─── Error type ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum VideoError {
    TooShort,
    UnknownFormat,
    CorruptContainer(String),
    UnsupportedCodec(String),
    NoVideoTrack,
}

impl VideoError {
    pub fn as_str(&self) -> &'static str {
        match self {
            VideoError::TooShort            => "data too short",
            VideoError::UnknownFormat       => "unknown container format",
            VideoError::CorruptContainer(_) => "corrupt container",
            VideoError::UnsupportedCodec(_) => "unsupported codec",
            VideoError::NoVideoTrack        => "no video track found",
        }
    }
}

// ─── Container detection ─────────────────────────────────────────────────────

pub fn detect_container(data: &[u8]) -> VideoContainer {
    if data.len() < 12 { return VideoContainer::Unknown; }

    // WebM / Matroska: EBML header starts with 0x1A 0x45 0xDF 0xA3
    if data[0] == 0x1a && data[1] == 0x45 && data[2] == 0xdf && data[3] == 0xa3 {
        return VideoContainer::WebM;
    }

    // Ogg: magic bytes "OggS"
    if &data[0..4] == b"OggS" {
        return VideoContainer::Ogg;
    }

    // MP4 / ISO Base Media: box at offset 0 or 4.
    // The ftyp box has the word "ftyp" at bytes [4..8].
    if data.len() >= 8 && &data[4..8] == b"ftyp" {
        return VideoContainer::Mp4;
    }
    // QuickTime MOV uses "wide" + "mdat" / "moov" — treat as MP4-compatible.
    if data.len() >= 8 && (&data[4..8] == b"moov" || &data[4..8] == b"mdat") {
        return VideoContainer::Mp4;
    }

    VideoContainer::Unknown
}

// ─── MP4 box reader ───────────────────────────────────────────────────────────

/// Iterator over ISO Base Media boxes (type, payload slice).
struct BoxIter<'a> {
    data: &'a [u8],
    pos:  usize,
}

impl<'a> BoxIter<'a> {
    fn new(data: &'a [u8]) -> Self { BoxIter { data, pos: 0 } }
}

impl<'a> Iterator for BoxIter<'a> {
    type Item = ([u8; 4], &'a [u8]);
    fn next(&mut self) -> Option<Self::Item> {
        if self.pos + 8 > self.data.len() { return None; }
        let size = u32::from_be_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]) as usize;
        if size < 8 || self.pos + size > self.data.len() { return None; }
        let btype: [u8; 4] = [
            self.data[self.pos + 4],
            self.data[self.pos + 5],
            self.data[self.pos + 6],
            self.data[self.pos + 7],
        ];
        let payload = &self.data[self.pos + 8 .. self.pos + size];
        self.pos += size;
        Some((btype, payload))
    }
}

/// Find a top-level box by four-character code.
fn find_box<'a>(data: &'a [u8], code: &[u8; 4]) -> Option<&'a [u8]> {
    BoxIter::new(data).find(|(t, _)| t == code).map(|(_, p)| p)
}

/// Find a nested box recursively (depth ≤ 4).
fn find_box_deep<'a>(data: &'a [u8], path: &[&[u8; 4]]) -> Option<&'a [u8]> {
    if path.is_empty() { return Some(data); }
    let payload = find_box(data, path[0])?;
    find_box_deep(payload, &path[1..])
}

// ─── MP4 metadata extraction ─────────────────────────────────────────────────

fn mp4_extract_metadata(data: &[u8]) -> VideoMetadata {
    let mut meta = VideoMetadata::unknown();
    meta.container = VideoContainer::Mp4;

    // Read moov box
    let moov = match find_box(data, b"moov") { Some(b) => b, None => return meta };

    // mvhd: movie header — duration and timescale at fixed offsets
    // Full box: version(1) + flags(3) → then either 32-bit or 64-bit fields
    if let Some(mvhd) = find_box(moov, b"mvhd") {
        if mvhd.len() >= 20 {
            let version = mvhd[0];
            if version == 0 && mvhd.len() >= 20 {
                // v0: creation(4) + modification(4) + timescale(4) + duration(4)
                let timescale = u32::from_be_bytes([mvhd[12], mvhd[13], mvhd[14], mvhd[15]]) as u64;
                let duration  = u32::from_be_bytes([mvhd[16], mvhd[17], mvhd[18], mvhd[19]]) as u64;
                if timescale > 0 {
                    meta.duration_ms = duration * 1000 / timescale;
                }
            } else if version == 1 && mvhd.len() >= 32 {
                // v1: creation(8) + modification(8) + timescale(4) + duration(8)
                let timescale = u32::from_be_bytes([mvhd[24], mvhd[25], mvhd[26], mvhd[27]]) as u64;
                let duration  = u64::from_be_bytes([
                    mvhd[28], mvhd[29], mvhd[30], mvhd[31],
                    mvhd[32], mvhd[33], mvhd[34], mvhd[35],
                ]);
                if timescale > 0 {
                    meta.duration_ms = duration * 1000 / timescale;
                }
            }
        }
    }

    // Walk trak boxes looking for video and audio tracks
    let mut trak_pos = 0usize;
    while let Some(remaining) = moov.get(trak_pos..) {
        let mut iter = BoxIter::new(remaining);
        let item = iter.next();
        match item {
            Some((btype, payload)) if &btype == b"trak" => {
                parse_mp4_trak(payload, &mut meta);
                // Advance by box size (payload.len() + 8)
                trak_pos += payload.len() + 8;
            }
            Some((_, payload)) => { trak_pos += payload.len() + 8; }
            None => break,
        }
    }

    meta
}

fn parse_mp4_trak(trak: &[u8], meta: &mut VideoMetadata) {
    // mdia → hdlr → handler_type tells us if it's vide or soun
    let mdia = match find_box(trak, b"mdia") { Some(b) => b, None => return };
    let hdlr = match find_box(mdia, b"hdlr") { Some(b) => b, None => return };

    if hdlr.len() < 12 { return; }
    // hdlr: version(1) + flags(3) + pre_defined(4) + handler_type(4)
    let handler_type = &hdlr[8..12];

    if handler_type == b"vide" {
        // Video track — extract from stsd
        if let Some(minf) = find_box(mdia, b"minf") {
            if let Some(stbl) = find_box(minf, b"stbl") {
                if let Some(stsd) = find_box(stbl, b"stsd") {
                    parse_mp4_stsd_video(stsd, meta);
                }
            }
        }
    } else if handler_type == b"soun" {
        if let Some(minf) = find_box(mdia, b"minf") {
            if let Some(stbl) = find_box(minf, b"stbl") {
                if let Some(stsd) = find_box(stbl, b"stsd") {
                    parse_mp4_stsd_audio(stsd, meta);
                }
            }
        }
    }
}

fn parse_mp4_stsd_video(stsd: &[u8], meta: &mut VideoMetadata) {
    // stsd: version(1)+flags(3)+entry_count(4) then sample entries
    if stsd.len() < 8 { return; }
    // Skip version/flags/entry_count, then first sample entry
    let entry = &stsd[8..];
    if entry.len() < 8 { return; }
    // Sample entry box: size(4) + format(4)
    let fmt = &entry[4..8];
    meta.video_codec = match fmt {
        b"avc1" | b"avc2" | b"avc3" | b"avc4" => VideoCodec::H264,
        b"hvc1" | b"hev1" | b"dvhe"             => VideoCodec::H265,
        b"vp08"                                  => VideoCodec::Vp8,
        b"vp09"                                  => VideoCodec::Vp9,
        b"av01"                                  => VideoCodec::Av1,
        _ => VideoCodec::Unknown,
    };
    // VisualSampleEntry: reserved(6)+data_ref_index(2)+reserved(16)+width(2)+height(2)
    if entry.len() >= 32 {
        // offset 8: data_ref_index, then 16 bytes reserved, then width/height
        let w_off = 8 + 2 + 16;
        let h_off = w_off + 2;
        if h_off + 2 <= entry.len() {
            meta.width  = u16::from_be_bytes([entry[w_off],     entry[w_off + 1]]) as u32;
            meta.height = u16::from_be_bytes([entry[h_off],     entry[h_off + 1]]) as u32;
        }
    }
}

fn parse_mp4_stsd_audio(stsd: &[u8], meta: &mut VideoMetadata) {
    if stsd.len() < 8 { return; }
    let entry = &stsd[8..];
    if entry.len() < 8 { return; }
    let fmt = &entry[4..8];
    meta.audio_codec = match fmt {
        b"mp4a" => AudioCodec::Aac,
        b"mp3 " | b".mp3" => AudioCodec::Mp3,
        b"Opus" | b"opus" => AudioCodec::Opus,
        b"fLaC" => AudioCodec::Flac,
        _ => AudioCodec::Unknown,
    };
    // AudioSampleEntry: reserved(6)+data_ref_index(2)+reserved(8)+channel_count(2)+sample_size(2)+pre_defined(2)+reserved(2)+sample_rate(4)
    if entry.len() >= 28 {
        let ch_off  = 8 + 2 + 8;          // = 18
        let sr_off  = ch_off + 2 + 2 + 2 + 2; // = 26
        if sr_off + 4 <= entry.len() {
            meta.audio_channels = u16::from_be_bytes([entry[ch_off], entry[ch_off + 1]]) as u8;
            let sr_raw = u32::from_be_bytes([entry[sr_off], entry[sr_off+1], entry[sr_off+2], entry[sr_off+3]]);
            meta.sample_rate = sr_raw >> 16; // 16.16 fixed point
        }
    }
}

// ─── WebM metadata extraction ─────────────────────────────────────────────────

/// Minimal EBML variable-length integer reader.
fn ebml_varint(data: &[u8], pos: usize) -> Option<(u64, usize)> {
    if pos >= data.len() { return None; }
    let first = data[pos] as u64;
    if first == 0 { return None; } // reserved
    let len = first.leading_zeros() as usize + 1;
    if pos + len > data.len() { return None; }
    let mask = (1u64 << (8 * len - len)) - 1;
    let mut val = first & mask;
    for i in 1..len {
        val = (val << 8) | data[pos + i] as u64;
    }
    Some((val, len))
}

/// Read EBML element ID (variable-length, no leading-bit stripping).
fn ebml_id(data: &[u8], pos: usize) -> Option<(u32, usize)> {
    if pos >= data.len() { return None; }
    let first = data[pos];
    let len = first.leading_zeros() as usize + 1;
    if pos + len > data.len() { return None; }
    let mut id = first as u32;
    for i in 1..len { id = (id << 8) | data[pos + i] as u32; }
    Some((id, len))
}

fn webm_extract_metadata(data: &[u8]) -> VideoMetadata {
    let mut meta = VideoMetadata::unknown();
    meta.container = VideoContainer::WebM;

    // Scan top-level EBML elements looking for Segment (0x18538067)
    let mut pos = 0usize;
    while pos < data.len() {
        let (id, id_len) = match ebml_id(data, pos) { Some(v) => v, None => break };
        pos += id_len;
        let (size, sz_len) = match ebml_varint(data, pos) { Some(v) => v, None => break };
        pos += sz_len;
        let elem_end = pos + size as usize;

        if id == 0x18538067 {
            // Segment — parse children
            webm_parse_segment(&data[pos..elem_end.min(data.len())], &mut meta);
        }
        pos = elem_end.min(data.len());
    }

    meta
}

fn webm_parse_segment(seg: &[u8], meta: &mut VideoMetadata) {
    let mut pos = 0usize;
    while pos < seg.len() {
        let (id, id_len) = match ebml_id(seg, pos) { Some(v) => v, None => break };
        pos += id_len;
        let (size, sz_len) = match ebml_varint(seg, pos) { Some(v) => v, None => break };
        pos += sz_len;
        let elem_end = (pos + size as usize).min(seg.len());

        match id {
            0x1549A966 => { // Info
                webm_parse_info(&seg[pos..elem_end], meta);
            }
            0x1654AE6B => { // Tracks
                webm_parse_tracks(&seg[pos..elem_end], meta);
            }
            _ => {}
        }
        pos = elem_end;
    }
}

fn webm_parse_info(info: &[u8], meta: &mut VideoMetadata) {
    let mut pos = 0usize;
    let mut timescale_ns = 1_000_000u64; // default: 1ms
    let mut duration_ticks = 0.0f64;

    while pos < info.len() {
        let (id, id_len) = match ebml_id(info, pos) { Some(v) => v, None => break };
        pos += id_len;
        let (size, sz_len) = match ebml_varint(info, pos) { Some(v) => v, None => break };
        pos += sz_len;
        let elem_end = (pos + size as usize).min(info.len());

        match id {
            0x2AD7B1 => { // TimestampScale (nanoseconds per tick)
                let mut v = 0u64;
                for b in &info[pos..elem_end] { v = (v << 8) | *b as u64; }
                timescale_ns = v;
            }
            0x4489 => { // Duration (float64)
                if elem_end - pos == 8 {
                    let bits = u64::from_be_bytes([
                        info[pos], info[pos+1], info[pos+2], info[pos+3],
                        info[pos+4], info[pos+5], info[pos+6], info[pos+7],
                    ]);
                    duration_ticks = f64::from_bits(bits);
                } else if elem_end - pos == 4 {
                    let bits = u32::from_be_bytes([info[pos], info[pos+1], info[pos+2], info[pos+3]]);
                    duration_ticks = f32::from_bits(bits) as f64;
                }
            }
            _ => {}
        }
        pos = elem_end;
    }

    if timescale_ns > 0 && duration_ticks > 0.0 {
        meta.duration_ms = (duration_ticks * timescale_ns as f64 / 1_000_000.0) as u64;
    }
}

fn webm_parse_tracks(tracks: &[u8], meta: &mut VideoMetadata) {
    let mut pos = 0usize;
    while pos < tracks.len() {
        let (id, id_len) = match ebml_id(tracks, pos) { Some(v) => v, None => break };
        pos += id_len;
        let (size, sz_len) = match ebml_varint(tracks, pos) { Some(v) => v, None => break };
        pos += sz_len;
        let elem_end = (pos + size as usize).min(tracks.len());

        if id == 0xAE { // TrackEntry
            webm_parse_track_entry(&tracks[pos..elem_end], meta);
        }
        pos = elem_end;
    }
}

fn webm_parse_track_entry(entry: &[u8], meta: &mut VideoMetadata) {
    let mut pos = 0usize;
    let mut track_type = 0u8;
    let mut codec_id = String::new();

    // First pass: collect track type and codec ID
    while pos < entry.len() {
        let (id, id_len) = match ebml_id(entry, pos) { Some(v) => v, None => break };
        pos += id_len;
        let (size, sz_len) = match ebml_varint(entry, pos) { Some(v) => v, None => break };
        pos += sz_len;
        let elem_end = (pos + size as usize).min(entry.len());

        match id {
            0x83 => { // TrackType: 1=video, 2=audio, 3=complex, 16=logo, 17=subtitle
                if pos < elem_end { track_type = entry[pos]; }
            }
            0x86 => { // CodecID
                if let Ok(s) = core::str::from_utf8(&entry[pos..elem_end]) {
                    codec_id = s.trim_end_matches('\0').to_string();
                }
            }
            0xE0 if track_type == 1 => { // Video element
                webm_parse_video_elem(&entry[pos..elem_end], meta);
            }
            0xE1 if track_type == 2 => { // Audio element
                webm_parse_audio_elem(&entry[pos..elem_end], meta);
            }
            _ => {}
        }
        pos = elem_end;
    }

    if track_type == 1 {
        meta.video_codec = match codec_id.as_str() {
            "V_VP8"  => VideoCodec::Vp8,
            "V_VP9"  => VideoCodec::Vp9,
            "V_AV1"  => VideoCodec::Av1,
            "V_MPEG4/ISO/AVC" | "V_MPEG4/ISO/ASP" => VideoCodec::H264,
            _ => VideoCodec::Unknown,
        };
    } else if track_type == 2 {
        meta.audio_codec = match codec_id.as_str() {
            "A_VORBIS" => AudioCodec::Vorbis,
            "A_OPUS"   => AudioCodec::Opus,
            "A_AAC"    => AudioCodec::Aac,
            "A_FLAC"   => AudioCodec::Flac,
            "A_MPEG/L3" => AudioCodec::Mp3,
            _ => AudioCodec::Unknown,
        };
    }
}

fn webm_parse_video_elem(video: &[u8], meta: &mut VideoMetadata) {
    let mut pos = 0usize;
    while pos < video.len() {
        let (id, id_len) = match ebml_id(video, pos) { Some(v) => v, None => break };
        pos += id_len;
        let (size, sz_len) = match ebml_varint(video, pos) { Some(v) => v, None => break };
        pos += sz_len;
        let elem_end = (pos + size as usize).min(video.len());

        match id {
            0xB0 => { // PixelWidth
                let mut v = 0u32;
                for b in &video[pos..elem_end] { v = (v << 8) | *b as u32; }
                meta.width = v;
            }
            0xBA => { // PixelHeight
                let mut v = 0u32;
                for b in &video[pos..elem_end] { v = (v << 8) | *b as u32; }
                meta.height = v;
            }
            _ => {}
        }
        pos = elem_end;
    }
}

fn webm_parse_audio_elem(audio: &[u8], meta: &mut VideoMetadata) {
    let mut pos = 0usize;
    while pos < audio.len() {
        let (id, id_len) = match ebml_id(audio, pos) { Some(v) => v, None => break };
        pos += id_len;
        let (size, sz_len) = match ebml_varint(audio, pos) { Some(v) => v, None => break };
        pos += sz_len;
        let elem_end = (pos + size as usize).min(audio.len());

        match id {
            0x9F => { // Channels
                let mut v = 0u8;
                if pos < elem_end { v = audio[pos]; }
                meta.audio_channels = v;
            }
            0xB5 => { // SamplingFrequency (float)
                if elem_end - pos == 8 {
                    let bits = u64::from_be_bytes([
                        audio[pos], audio[pos+1], audio[pos+2], audio[pos+3],
                        audio[pos+4], audio[pos+5], audio[pos+6], audio[pos+7],
                    ]);
                    meta.sample_rate = f64::from_bits(bits) as u32;
                } else if elem_end - pos == 4 {
                    let bits = u32::from_be_bytes([audio[pos], audio[pos+1], audio[pos+2], audio[pos+3]]);
                    meta.sample_rate = f32::from_bits(bits) as u32;
                }
            }
            _ => {}
        }
        pos = elem_end;
    }
}

// ─── Ogg metadata extraction ──────────────────────────────────────────────────

fn ogg_extract_metadata(data: &[u8]) -> VideoMetadata {
    let mut meta = VideoMetadata::unknown();
    meta.container = VideoContainer::Ogg;

    // Walk Ogg pages: each starts with "OggS" + fixed header
    // Page structure: capture(4) + version(1) + header_type(1) + granule(8) + serial(4) + seq(4) + crc(4) + page_segs(1) + seg_table(page_segs)
    let mut pos = 0usize;
    while pos + 27 <= data.len() {
        if &data[pos..pos+4] != b"OggS" { pos += 1; continue; }
        let page_segs = data[pos + 26] as usize;
        if pos + 27 + page_segs > data.len() { break; }
        let seg_table = &data[pos + 27 .. pos + 27 + page_segs];
        let payload_size: usize = seg_table.iter().map(|&s| s as usize).sum();
        let payload_start = pos + 27 + page_segs;
        if payload_start + payload_size > data.len() { break; }
        let payload = &data[payload_start .. payload_start + payload_size];

        // Theora identification header: starts with 0x80 "theora"
        if payload.len() >= 7 && payload[0] == 0x80 && &payload[1..7] == b"theora" {
            meta.video_codec = VideoCodec::Theora;
            // Theora ident header: type(1)+id(6)+vmaj(1)+vmin(1)+vrev(1)+fmbw(2)+fmbh(2)+...+fps_num(4)+fps_den(4)
            if payload.len() >= 22 {
                let fmbw = u16::from_be_bytes([payload[7], payload[8]]) as u32;
                let fmbh = u16::from_be_bytes([payload[9], payload[10]]) as u32;
                meta.width  = fmbw * 16;
                meta.height = fmbh * 16;
                // fps at offsets 14-21
                if payload.len() >= 22 {
                    let fps_num = u32::from_be_bytes([payload[14], payload[15], payload[16], payload[17]]);
                    let fps_den = u32::from_be_bytes([payload[18], payload[19], payload[20], payload[21]]);
                    if fps_den > 0 {
                        meta.fps_milli = fps_num * 1000 / fps_den;
                    }
                }
            }
        }
        // Vorbis identification header: starts with 0x01 "vorbis"
        if payload.len() >= 7 && payload[0] == 0x01 && &payload[1..7] == b"vorbis" {
            meta.audio_codec = AudioCodec::Vorbis;
            if payload.len() >= 28 {
                meta.audio_channels = payload[11];
                meta.sample_rate = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
            }
        }

        pos = payload_start + payload_size;
    }

    meta
}

// ─── Public parse entry point ─────────────────────────────────────────────────

pub fn parse_video(data: &[u8]) -> Result<VideoMetadata, VideoError> {
    if data.len() < 12 { return Err(VideoError::TooShort); }
    match detect_container(data) {
        VideoContainer::Mp4  => {
            let m = mp4_extract_metadata(data);
            if m.video_codec == VideoCodec::Unknown && m.width == 0 {
                return Err(VideoError::NoVideoTrack);
            }
            Ok(m)
        }
        VideoContainer::WebM => {
            let m = webm_extract_metadata(data);
            Ok(m)
        }
        VideoContainer::Ogg  => Ok(ogg_extract_metadata(data)),
        VideoContainer::Unknown => Err(VideoError::UnknownFormat),
    }
}

// ─── Video Element (HTML <video>) ─────────────────────────────────────────────

/// Playback state.
#[derive(Clone, Debug, PartialEq)]
pub enum VideoReadyState {
    HaveNothing,
    HaveMetadata,
    HaveEnoughData,
}

/// A `<video>` element — holds metadata, poster, and playback state.
#[derive(Clone, Debug)]
pub struct VideoElement {
    pub src:         String,
    pub width:       u32,    // element layout width (CSS pixels)
    pub height:      u32,    // element layout height
    pub autoplay:    bool,
    pub controls:    bool,
    pub muted:       bool,
    pub loop_play:   bool,
    pub poster_url:  Option<String>,
    pub ready_state: VideoReadyState,
    pub metadata:    Option<VideoMetadata>,
    pub poster:      Option<PosterFrame>,
    /// Current playback position in milliseconds.
    pub current_time_ms: u64,
    pub paused:      bool,
    pub ended:       bool,
    pub volume:      f32, // 0.0–1.0
}

impl VideoElement {
    pub fn new(src: String, width: u32, height: u32) -> Self {
        VideoElement {
            src,
            width,
            height,
            autoplay:        false,
            controls:        true,
            muted:           false,
            loop_play:       false,
            poster_url:      None,
            ready_state:     VideoReadyState::HaveNothing,
            metadata:        None,
            poster:          None,
            current_time_ms: 0,
            paused:          true,
            ended:           false,
            volume:          1.0,
        }
    }

    /// Feed raw video data to the element (e.g. from a fetch response body).
    pub fn load(&mut self, data: &[u8]) {
        match parse_video(data) {
            Ok(meta) => {
                // Use video dimensions for layout if not set
                if self.width == 0  { self.width  = meta.width; }
                if self.height == 0 { self.height = meta.height; }
                let w = self.width.max(meta.width).max(320);
                let h = self.height.max(meta.height).max(240);
                self.poster      = Some(PosterFrame::placeholder(w, h));
                self.metadata    = Some(meta);
                self.ready_state = VideoReadyState::HaveMetadata;
            }
            Err(_) => {
                self.ready_state = VideoReadyState::HaveNothing;
            }
        }
    }

    /// Start playback (stub — no actual decoding).
    pub fn play(&mut self) {
        if self.ready_state != VideoReadyState::HaveNothing {
            self.paused = false;
            self.ended  = false;
            self.ready_state = VideoReadyState::HaveEnoughData;
        }
    }

    pub fn pause(&mut self) { self.paused = true; }

    pub fn seek(&mut self, ms: u64) {
        let max = self.metadata.as_ref().map(|m| m.duration_ms).unwrap_or(0);
        self.current_time_ms = ms.min(max);
        self.ended = false;
    }

    /// Advance playback clock by `delta_ms`.  Returns true when the video ends.
    pub fn tick(&mut self, delta_ms: u64) -> bool {
        if self.paused || self.ended { return self.ended; }
        self.current_time_ms += delta_ms;
        let dur = self.metadata.as_ref().map(|m| m.duration_ms).unwrap_or(u64::MAX);
        if self.current_time_ms >= dur {
            if self.loop_play {
                self.current_time_ms = 0;
            } else {
                self.current_time_ms = dur;
                self.ended  = true;
                self.paused = true;
            }
            true
        } else {
            false
        }
    }

    /// Current position as a fraction [0.0, 1.0].
    pub fn progress(&self) -> f32 {
        let dur = self.metadata.as_ref().map(|m| m.duration_ms).unwrap_or(0);
        if dur == 0 { return 0.0; }
        (self.current_time_ms as f32) / (dur as f32)
    }

    /// Returns the poster frame RGBA pixels (or a generated placeholder).
    pub fn poster_pixels(&self) -> Option<&PosterFrame> {
        self.poster.as_ref()
    }

    /// Human-readable status string for UI overlays.
    pub fn status_line(&self) -> String {
        if let Some(ref m) = self.metadata {
            let dur_s = m.duration_ms / 1000;
            let cur_s = self.current_time_ms / 1000;
            let state = if self.paused { "▶" } else { "⏸" };
            format!("{} {}x{}  {}/{}s  {} {}",
                state,
                m.width, m.height,
                cur_s, dur_s,
                m.video_codec.as_str(),
                m.container.as_str(),
            )
        } else {
            format!("⏳ Loading {}", self.src)
        }
    }
}

// ─── Self-test ────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;

    macro_rules! check {
        ($label:expr, $cond:expr) => {
            if $cond { pass += 1; } else {
                crate::serial_println!("[video] FAIL: {}", $label);
                fail += 1;
            }
        };
    }

    // 1. detect_container: WebM magic
    {
        let webm_hdr = [0x1au8, 0x45, 0xdf, 0xa3, 0, 0, 0, 0, 0, 0, 0, 0];
        check!("detect_container WebM", detect_container(&webm_hdr) == VideoContainer::WebM);
    }
    // 2. detect_container: Ogg
    {
        let ogg_hdr: &[u8] = b"OggS\x00\x02\x00\x00\x00\x00\x00\x00";
        check!("detect_container Ogg", detect_container(ogg_hdr) == VideoContainer::Ogg);
    }
    // 3. detect_container: MP4 ftyp
    {
        let mp4_hdr: &[u8] = b"\x00\x00\x00\x20ftypmp42\x00\x00\x00\x00mp42isom";
        check!("detect_container MP4", detect_container(mp4_hdr) == VideoContainer::Mp4);
    }
    // 4. detect_container: unknown
    {
        let unk: &[u8] = b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b";
        check!("detect_container unknown", detect_container(unk) == VideoContainer::Unknown);
    }
    // 5. parse_video on too-short data
    {
        check!("parse_video TooShort", matches!(parse_video(b"\x00\x01"), Err(VideoError::TooShort)));
    }
    // 6. parse_video on unknown container
    {
        let unk = [0xffu8; 12];
        check!("parse_video UnknownFormat", matches!(parse_video(&unk), Err(VideoError::UnknownFormat)));
    }
    // 7. PosterFrame placeholder: correct dimensions
    {
        let p = PosterFrame::placeholder(160, 90);
        check!("PosterFrame dimensions", p.width == 160 && p.height == 90);
        check!("PosterFrame pixel count", p.pixels.len() == 160 * 90 * 4);
    }
    // 8. VideoElement::new defaults
    {
        let el = VideoElement::new("test.mp4".to_string(), 640, 360);
        check!("VideoElement defaults", el.paused && el.controls && !el.autoplay);
        check!("VideoElement ready_state", el.ready_state == VideoReadyState::HaveNothing);
    }
    // 9. VideoElement::tick does not advance when paused
    {
        let mut el = VideoElement::new("x.webm".to_string(), 0, 0);
        el.tick(500);
        check!("VideoElement tick-while-paused", el.current_time_ms == 0);
    }
    // 10. VideoElement::progress on unknown duration
    {
        let el = VideoElement::new("x.mp4".to_string(), 0, 0);
        check!("VideoElement progress no duration", el.progress() == 0.0);
    }
    // 11. EBML varint single-byte
    {
        let data = [0x82u8, 0x00]; // 0x82 → leading zeros = 0, len=1, value = 0x82 & 0x7F = 2
        match ebml_varint(&data, 0) {
            Some((val, len)) => check!("ebml_varint single-byte", val == 2 && len == 1),
            None => { check!("ebml_varint single-byte", false); }
        }
    }
    // 12. BoxIter: empty slice
    {
        let mut iter = BoxIter::new(&[]);
        check!("BoxIter empty", iter.next().is_none());
    }
    // 13. VideoMetadata unknown defaults
    {
        let m = VideoMetadata::unknown();
        check!("VideoMetadata unknown", m.width == 0 && m.height == 0 && m.duration_ms == 0);
    }
    // 14. VideoElement status_line no metadata
    {
        let el = VideoElement::new("clip.mp4".to_string(), 640, 360);
        let s = el.status_line();
        check!("status_line no metadata", s.contains("clip.mp4"));
    }
    // 15. VideoElement seek clamps to 0 without metadata
    {
        let mut el = VideoElement::new("v.mp4".to_string(), 0, 0);
        el.seek(99999);
        check!("VideoElement seek clamp", el.current_time_ms == 0);
    }

    crate::serial_println!(
        "[test] video: {}/{} tests passed",
        pass, pass + fail
    );
    fail == 0
}

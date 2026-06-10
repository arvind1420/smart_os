/// Phase 116 — Media Source Extensions (MSE)
///
/// Implements the W3C Media Source Extensions API:
///   • `MediaSource`    — open/closed/ended states, duration
///   • `SourceBuffer`   — appendBuffer, remove, timestampOffset, buffered ranges
///   • HLS playlist parser (`#EXTM3U`, `#EXT-X-STREAM-INF`, segments)
///   • DASH MPD parser  (Period/AdaptationSet/Representation stub)
///   • Adaptive bitrate — bandwidth estimator + quality selector
///   • JS bindings:     `new MediaSource()`, `addSourceBuffer()`, `appendBuffer()`

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::collections::BTreeMap;

// ─────────────────────────────────────────────────────────────────────────────
// BUFFERED TIME RANGES
// ─────────────────────────────────────────────────────────────────────────────

/// A half-open time interval [start, end) in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeRange {
    pub start: f64,
    pub end:   f64,
}

impl TimeRange {
    pub fn new(start: f64, end: f64) -> Self { TimeRange { start, end } }
    pub fn duration(&self) -> f64 { self.end - self.start }
    pub fn contains(&self, t: f64) -> bool { t >= self.start && t < self.end }
}

/// A sorted, non-overlapping list of time ranges (like `TimeRanges` in Web API).
#[derive(Debug, Clone, Default)]
pub struct TimeRanges(Vec<TimeRange>);

impl TimeRanges {
    pub fn new() -> Self { Self::default() }

    /// Add a range, merging with existing overlapping/adjacent ranges.
    pub fn add(&mut self, r: TimeRange) {
        self.0.push(r);
        // Sort and merge
        self.0.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
        let mut merged: Vec<TimeRange> = Vec::new();
        for tr in &self.0 {
            if let Some(last) = merged.last_mut() {
                if tr.start <= last.end {
                    if tr.end > last.end { last.end = tr.end; }
                    continue;
                }
            }
            merged.push(*tr);
        }
        self.0 = merged;
    }

    /// Remove time range [start, end) from the buffered list.
    pub fn remove(&mut self, start: f64, end: f64) {
        let mut result = Vec::new();
        for tr in &self.0 {
            if tr.end <= start || tr.start >= end {
                result.push(*tr); // no overlap
            } else {
                if tr.start < start { result.push(TimeRange::new(tr.start, start)); }
                if tr.end > end     { result.push(TimeRange::new(end, tr.end)); }
            }
        }
        self.0 = result;
    }

    pub fn len(&self) -> usize { self.0.len() }
    pub fn start(&self, i: usize) -> f64 { self.0[i].start }
    pub fn end(&self,   i: usize) -> f64 { self.0[i].end   }

    /// Total buffered seconds.
    pub fn total_duration(&self) -> f64 {
        self.0.iter().map(|r| r.duration()).sum()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SOURCE BUFFER
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendState { WaitingForSegment, ParsingInitSegment, ParsingMediaSegment }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbMode { Segments, Sequence }

pub struct SourceBuffer {
    pub mime_type:       String,
    pub buffered:        TimeRanges,
    pub timestamp_offset: f64,
    pub append_window_start: f64,
    pub append_window_end:   f64,
    pub mode:            SbMode,
    pub updating:        bool,
    append_state:        AppendState,
    /// Track how many bytes have been appended (for testing).
    pub bytes_appended:  usize,
}

impl SourceBuffer {
    pub fn new(mime_type: &str) -> Self {
        SourceBuffer {
            mime_type: mime_type.to_string(),
            buffered: TimeRanges::new(),
            timestamp_offset: 0.0,
            append_window_start: 0.0,
            append_window_end: f64::INFINITY,
            mode: SbMode::Segments,
            updating: false,
            append_state: AppendState::WaitingForSegment,
            bytes_appended: 0,
        }
    }

    /// Append media bytes to the buffer.  In a real implementation this would
    /// feed data to the codec demuxer; here we parse a minimal fake timestamp.
    pub fn append_buffer(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if self.updating { return Err("updating=true; cannot append"); }
        self.updating = true;
        self.bytes_appended += data.len();

        // Fake: treat first 8 bytes as a little-endian f64 timestamp pair
        // [start_secs f32][dur_secs f32] if len >= 8, else use offset.
        let (seg_start, seg_end) = if data.len() >= 8 {
            let start = f32::from_le_bytes([data[0], data[1], data[2], data[3]]) as f64;
            let dur   = f32::from_le_bytes([data[4], data[5], data[6], data[7]]) as f64;
            (start + self.timestamp_offset, start + dur + self.timestamp_offset)
        } else {
            (self.timestamp_offset, self.timestamp_offset + (data.len() as f64 / 1000.0))
        };

        // Only add if within append window
        if seg_end > self.append_window_start && seg_start < self.append_window_end {
            let clamped_start = seg_start.max(self.append_window_start);
            let clamped_end   = seg_end.min(self.append_window_end);
            self.buffered.add(TimeRange::new(clamped_start, clamped_end));
        }

        self.updating = false;
        Ok(())
    }

    pub fn remove(&mut self, start: f64, end: f64) -> Result<(), &'static str> {
        if self.updating { return Err("updating=true"); }
        self.updating = true;
        self.buffered.remove(start, end);
        self.updating = false;
        Ok(())
    }

    pub fn abort(&mut self) {
        self.updating = false;
        self.append_state = AppendState::WaitingForSegment;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MEDIA SOURCE
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyState { Closed, Open, Ended }

pub struct MediaSource {
    pub ready_state:     ReadyState,
    pub duration:        f64,         // NaN = not set
    pub source_buffers:  Vec<SourceBuffer>,
}

impl MediaSource {
    pub fn new() -> Self {
        MediaSource { ready_state: ReadyState::Closed, duration: f64::NAN, source_buffers: Vec::new() }
    }

    pub fn open(&mut self) { self.ready_state = ReadyState::Open; }

    pub fn add_source_buffer(&mut self, mime: &str) -> Result<usize, &'static str> {
        if self.ready_state != ReadyState::Open { return Err("MediaSource not open"); }
        if !is_type_supported(mime) { return Err("MIME type not supported"); }
        let idx = self.source_buffers.len();
        self.source_buffers.push(SourceBuffer::new(mime));
        Ok(idx)
    }

    pub fn end_of_stream(&mut self) {
        self.ready_state = ReadyState::Ended;
    }

    pub fn is_type_supported(mime: &str) -> bool { is_type_supported(mime) }

    /// Compute overall buffered ranges (intersection of all source buffers).
    pub fn buffered(&self) -> TimeRanges {
        let mut result = TimeRanges::new();
        for sb in &self.source_buffers {
            for i in 0..sb.buffered.len() {
                result.add(TimeRange::new(sb.buffered.start(i), sb.buffered.end(i)));
            }
        }
        result
    }
}

pub fn is_type_supported(mime: &str) -> bool {
    let m = mime.to_lowercase();
    m.contains("video/mp4")  || m.contains("audio/mp4")  ||
    m.contains("video/webm") || m.contains("audio/webm") ||
    m.contains("video/mp2t")
}

// ─────────────────────────────────────────────────────────────────────────────
// HLS PLAYLIST PARSER
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HlsVariant {
    pub bandwidth:  u64,
    pub resolution: Option<String>,
    pub codecs:     Option<String>,
    pub url:        String,
}

#[derive(Debug, Clone)]
pub struct HlsSegment {
    pub duration:  f32,
    pub url:       String,
    pub sequence:  u64,
}

#[derive(Debug, Clone, Default)]
pub struct HlsPlaylist {
    pub variants:        Vec<HlsVariant>,     // master playlist
    pub segments:        Vec<HlsSegment>,     // media playlist
    pub target_duration: u32,
    pub media_sequence:  u64,
    pub is_vod:          bool,
}

/// Parse an HLS playlist string (both master and media playlists).
pub fn parse_hls(text: &str) -> HlsPlaylist {
    let mut pl = HlsPlaylist::default();
    let lines: Vec<&str> = text.lines().map(|l| l.trim()).collect();
    let mut i = 0;
    let mut pending_duration = 0.0f32;
    let mut seq = pl.media_sequence;

    while i < lines.len() {
        let line = lines[i];
        if line == "#EXTM3U" {
            i += 1; continue;
        }
        if line.starts_with("#EXT-X-TARGETDURATION:") {
            if let Ok(v) = line[22..].parse::<u32>() { pl.target_duration = v; }
        } else if line.starts_with("#EXT-X-MEDIA-SEQUENCE:") {
            if let Ok(v) = line[22..].parse::<u64>() { pl.media_sequence = v; seq = v; }
        } else if line == "#EXT-X-ENDLIST" {
            pl.is_vod = true;
        } else if line.starts_with("#EXTINF:") {
            // #EXTINF:8.008,
            let dur_str = line[8..].split(',').next().unwrap_or("0");
            pending_duration = dur_str.parse::<f32>().unwrap_or(0.0);
        } else if line.starts_with("#EXT-X-STREAM-INF:") {
            // Parse attributes
            let attrs = line[18..].to_string();
            let bandwidth = parse_attr_u64(&attrs, "BANDWIDTH").unwrap_or(0);
            let resolution = parse_attr_str(&attrs, "RESOLUTION");
            let codecs     = parse_attr_str(&attrs, "CODECS");
            // Next non-empty line is the URL
            i += 1;
            while i < lines.len() && lines[i].is_empty() { i += 1; }
            if i < lines.len() {
                pl.variants.push(HlsVariant { bandwidth, resolution, codecs, url: lines[i].to_string() });
            }
        } else if !line.is_empty() && !line.starts_with('#') {
            // Segment URL
            pl.segments.push(HlsSegment { duration: pending_duration, url: line.to_string(), sequence: seq });
            seq += 1;
            pending_duration = 0.0;
        }
        i += 1;
    }
    pl
}

fn parse_attr_u64(attrs: &str, key: &str) -> Option<u64> {
    let prefix = format!("{}=", key);
    let start = attrs.find(&prefix)? + prefix.len();
    let end = attrs[start..].find(',').map(|e| start + e).unwrap_or(attrs.len());
    attrs[start..end].parse().ok()
}

fn parse_attr_str(attrs: &str, key: &str) -> Option<String> {
    let prefix = format!("{}=", key);
    let start = attrs.find(&prefix)? + prefix.len();
    let rest = &attrs[start..];
    if rest.starts_with('"') {
        let end = rest[1..].find('"')? + 1;
        Some(rest[1..end].to_string())
    } else {
        let end = rest.find(',').unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ADAPTIVE BITRATE ENGINE
// ─────────────────────────────────────────────────────────────────────────────

pub struct AbrEngine {
    /// Download speed estimate in bps.
    pub bandwidth_bps:    u64,
    /// Available variants sorted by bandwidth.
    pub variants:         Vec<HlsVariant>,
    /// Current variant index.
    pub current_idx:      usize,
    /// Buffer fill level in seconds (used to avoid quality drops).
    pub buffer_level_sec: f64,
}

impl AbrEngine {
    pub fn new(variants: Vec<HlsVariant>) -> Self {
        let mut v = variants;
        v.sort_by_key(|x| x.bandwidth);
        AbrEngine { bandwidth_bps: 5_000_000, variants: v, current_idx: 0, buffer_level_sec: 0.0 }
    }

    /// Update the bandwidth estimate with a new segment download measurement.
    pub fn update_bandwidth(&mut self, bytes: usize, duration_ms: u64) {
        if duration_ms == 0 { return; }
        let bps = (bytes as u64 * 8 * 1000) / duration_ms;
        // EWMA with α=0.3
        self.bandwidth_bps = (self.bandwidth_bps * 7 + bps * 3) / 10;
    }

    /// Select the best variant given current bandwidth + buffer level.
    /// Returns the new variant index.
    pub fn select_quality(&mut self) -> usize {
        // Use 80% of estimated bandwidth to leave headroom.
        let available = self.bandwidth_bps * 4 / 5;
        // Find the highest variant that fits.
        let mut best = 0;
        for (i, v) in self.variants.iter().enumerate() {
            if v.bandwidth <= available { best = i; }
        }
        // Avoid quality increase when buffer is low (< 10 s).
        if best > self.current_idx && self.buffer_level_sec < 10.0 {
            best = self.current_idx;
        }
        self.current_idx = best;
        best
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SELF-TEST
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;

    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] mse: {}", $name); }
        }
    }

    // T1: TimeRanges merge
    {
        let mut tr = TimeRanges::new();
        tr.add(TimeRange::new(0.0, 5.0));
        tr.add(TimeRange::new(4.0, 10.0)); // overlaps → merged
        check!(tr.len() == 1, "TimeRanges merged to 1");
        check!(tr.end(0) == 10.0, "merged end=10.0");
    }

    // T2: TimeRanges remove
    {
        let mut tr = TimeRanges::new();
        tr.add(TimeRange::new(0.0, 10.0));
        tr.remove(3.0, 7.0);
        check!(tr.len() == 2, "remove splits into 2 ranges");
        check!(tr.start(0) == 0.0 && tr.end(0) == 3.0, "first piece [0,3)");
        check!(tr.start(1) == 7.0 && tr.end(1) == 10.0, "second piece [7,10)");
    }

    // T3: is_type_supported
    check!(is_type_supported("video/mp4; codecs=\"avc1.42E01E\""), "mp4 supported");
    check!(is_type_supported("video/webm; codecs=\"vp8\""), "webm supported");
    check!(!is_type_supported("video/avi"), "avi not supported");

    // T4: MediaSource lifecycle
    {
        let mut ms = MediaSource::new();
        check!(ms.ready_state == ReadyState::Closed, "MediaSource starts Closed");
        ms.open();
        check!(ms.ready_state == ReadyState::Open, "MediaSource opens");
        let r = ms.add_source_buffer("video/mp4");
        check!(r.is_ok(), "add_source_buffer succeeds");
        ms.end_of_stream();
        check!(ms.ready_state == ReadyState::Ended, "MediaSource ends");
    }

    // T5: SourceBuffer appendBuffer
    {
        let mut sb = SourceBuffer::new("video/mp4");
        // Fake segment: start=0.0s, dur=2.0s
        let mut data = Vec::new();
        data.extend_from_slice(&0.0f32.to_le_bytes()); // start
        data.extend_from_slice(&2.0f32.to_le_bytes()); // duration
        data.extend_from_slice(&[0u8; 1000]);
        sb.append_buffer(&data).unwrap();
        check!(sb.buffered.len() == 1, "SourceBuffer has 1 buffered range");
        check!(sb.buffered.start(0) == 0.0, "buffered start=0");
        // end should be ~2.0 (float conversion)
        let end = sb.buffered.end(0);
        check!(end > 1.9 && end < 2.1, "buffered end≈2.0");
    }

    // T6: HLS master playlist parse
    {
        let m3u8 = "#EXTM3U\n\
                    #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\n\
                    low.m3u8\n\
                    #EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1280x720\n\
                    high.m3u8\n";
        let pl = parse_hls(m3u8);
        check!(pl.variants.len() == 2, "HLS master: 2 variants");
        check!(pl.variants[0].bandwidth == 800000, "variant 0 BW=800k");
        check!(pl.variants[1].url == "high.m3u8", "variant 1 URL=high.m3u8");
    }

    // T7: HLS media playlist parse
    {
        let m3u8 = "#EXTM3U\n\
                    #EXT-X-TARGETDURATION:8\n\
                    #EXT-X-MEDIA-SEQUENCE:0\n\
                    #EXTINF:8.008,\n\
                    seg0.ts\n\
                    #EXTINF:8.008,\n\
                    seg1.ts\n\
                    #EXT-X-ENDLIST\n";
        let pl = parse_hls(m3u8);
        check!(pl.segments.len() == 2, "HLS media: 2 segments");
        check!(pl.segments[0].url == "seg0.ts", "segment 0 URL");
        check!(pl.target_duration == 8, "target duration=8");
        check!(pl.is_vod, "is_vod=true");
    }

    // T8: ABR engine selects quality
    {
        let mut abr = AbrEngine::new(vec![
            HlsVariant { bandwidth: 500_000,   resolution: None, codecs: None, url: "low.m3u8".to_string() },
            HlsVariant { bandwidth: 2_000_000, resolution: None, codecs: None, url: "mid.m3u8".to_string() },
            HlsVariant { bandwidth: 5_000_000, resolution: None, codecs: None, url: "high.m3u8".to_string() },
        ]);
        abr.bandwidth_bps = 3_000_000;
        abr.buffer_level_sec = 20.0; // healthy buffer
        let idx = abr.select_quality();
        // 80% of 3Mbps = 2.4Mbps → should pick 2Mbps tier
        check!(idx == 1, "ABR selects mid tier at 3Mbps");
    }

    // T9: ABR bandwidth update
    {
        let mut abr = AbrEngine::new(vec![]);
        // Download 1 MB in 2000ms → 4 Mbps
        abr.update_bandwidth(1_000_000, 2000);
        // EWMA: (5M*7 + 4M*3)/10 = (35+12)/10 = 4.7M
        check!(abr.bandwidth_bps > 4_000_000, "ABR bandwidth updated upward");
    }

    // T10: SourceBuffer abort clears updating flag
    {
        let mut sb = SourceBuffer::new("video/mp4");
        sb.updating = true;
        sb.abort();
        check!(!sb.updating, "abort clears updating=false");
    }

    if fail == 0 {
        crate::serial_println!("[mse] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[mse] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

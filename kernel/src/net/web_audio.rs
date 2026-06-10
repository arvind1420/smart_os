/// Phase 45 — Web Audio API
///
/// Implements the W3C Web Audio API on top of the Intel HDA software mixer.
///
/// # Architecture
///
/// Processing model: pull-based render graph.
///   • Each `AudioNode` exposes `render(quantum)` → fills a 128-sample f32 buffer.
///   • `AudioContext::render_frame()` traverses the graph from the destination
///     back to the sources, collecting rendered audio.
///   • The final PCM is converted to i16 and submitted to `drivers::hda::MIXER`.
///
/// Nodes implemented:
///   - `AudioDestinationNode`  — final sink, converts float→i16 → mixer
///   - `OscillatorNode`        — sine / square / sawtooth / triangle
///   - `GainNode`              — scalar gain (0.0–1.0+)
///   - `AudioBufferSourceNode` — plays a pre-loaded f32 PCM buffer
///   - `BiquadFilterNode`      — simple 1-pole lowpass / highpass
///   - `DelayNode`             — fixed-delay line
///   - `DynamicsCompressorNode`— soft-knee peak limiter
///   - `AnalyserNode`          — FFT/time-domain snapshot (stub)
///   - `StereoPannerNode`      — constant-power left/right pan
///   - `ChannelMergerNode`     — merge mono inputs to stereo
///
/// Sample rate: 44100 Hz (constant for now).
/// Quantum size: 128 samples.

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec,
    vec::Vec,
    format,
    rc::Rc,
};
use core::cell::RefCell;
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
//  Constants
// ─────────────────────────────────────────────────────────────────────────────

pub const SAMPLE_RATE: f32 = 44100.0;
pub const QUANTUM:     usize = 128;

// ─────────────────────────────────────────────────────────────────────────────
//  Inline float math (no libm)
// ─────────────────────────────────────────────────────────────────────────────

/// Sine via range reduction + Taylor series.
/// Step 1: reduce to [-π, π].
/// Step 2: reduce to [-π/2, π/2] using sin(π-x)=sin(x).
/// Step 3: for |x|>π/4, use sin(x)=cos(π/2-x) with 10th-deg cos Taylor
///         so that the argument is in [-π/4, π/4] — greatly improves accuracy.
///         sin(π/2) → cos(0) = 1.0 exactly in float.
/// Step 4: |x|≤π/4 → 11th-degree sin Taylor; error < 1e-10.
fn f32_sin(mut x: f32) -> f32 {
    const PI: f32 = core::f32::consts::PI;
    const TAU: f32 = PI * 2.0;
    // Reduce to [-π, π]
    x -= TAU * f32_floor(x / TAU + 0.5);
    // Reduce to [-π/2, π/2]
    if x > PI * 0.5 { x = PI - x; }
    else if x < -PI * 0.5 { x = -PI - x; }
    // Reduce to [-π/4, π/4] for precision
    if x > PI * 0.25 {
        // sin(x) = cos(π/2 - x);  arg ∈ [0, π/4]
        let y = PI * 0.5 - x;
        let y2 = y * y;
        return 1.0 - y2 * (0.5 - y2 * (1.0/24.0 - y2 * (1.0/720.0 - y2 * (1.0/40320.0 - y2/3628800.0))));
    }
    if x < -PI * 0.25 {
        // sin(x) = -cos(π/2 + x);  arg ∈ [0, π/4]
        let y = PI * 0.5 + x;
        let y2 = y * y;
        return -(1.0 - y2 * (0.5 - y2 * (1.0/24.0 - y2 * (1.0/720.0 - y2 * (1.0/40320.0 - y2/3628800.0)))));
    }
    // |x| ≤ π/4 → 11th-degree sin Taylor
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0 * (1.0 - x2 / 110.0)))))
}

fn f32_cos(x: f32) -> f32 {
    f32_sin(x + core::f32::consts::FRAC_PI_2)
}

fn f32_floor(x: f32) -> f32 {
    let xi = x as i64;
    if x < xi as f32 { xi as f32 - 1.0 } else { xi as f32 }
}

fn f32_abs(x: f32) -> f32 { if x < 0.0 { -x } else { x } }

fn f32_sqrt(x: f32) -> f32 {
    if x <= 0.0 { return 0.0; }
    let mut g = x * 0.5;
    for _ in 0..8 { g = 0.5 * (g + x / g); }
    g
}

fn f32_clamp(x: f32, lo: f32, hi: f32) -> f32 {
    if x < lo { lo } else if x > hi { hi } else { x }
}

// ─────────────────────────────────────────────────────────────────────────────
//  AudioBuffer (pre-loaded PCM)
// ─────────────────────────────────────────────────────────────────────────────

pub struct AudioBuffer {
    pub sample_rate:   f32,
    pub num_channels:  usize,
    /// Interleaved samples: [ch0_s0, ch1_s0, ch0_s1, ch1_s1, ...]
    pub data:          Vec<f32>,
}

impl AudioBuffer {
    pub fn new(num_channels: usize, num_samples: usize, sample_rate: f32) -> Self {
        Self {
            sample_rate,
            num_channels,
            data: vec![0.0; num_channels * num_samples],
        }
    }

    pub fn duration_secs(&self) -> f32 {
        if num_samples(self) == 0 { return 0.0; }
        num_samples(self) as f32 / self.sample_rate
    }

    pub fn get_channel_data(&self, ch: usize) -> Vec<f32> {
        let n = num_samples(self);
        (0..n).map(|i| self.data[i * self.num_channels + ch.min(self.num_channels - 1)]).collect()
    }

    pub fn copy_from_channel(&mut self, ch: usize, samples: &[f32]) {
        let n = num_samples(self).min(samples.len());
        for i in 0..n {
            let idx = i * self.num_channels + ch.min(self.num_channels - 1);
            if idx < self.data.len() { self.data[idx] = samples[i]; }
        }
    }
}

fn num_samples(buf: &AudioBuffer) -> usize {
    if buf.num_channels == 0 { return 0; }
    buf.data.len() / buf.num_channels
}

// ─────────────────────────────────────────────────────────────────────────────
//  AudioParam (automatable parameter)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct AudioParam {
    pub value:         f32,
    pub min_value:     f32,
    pub max_value:     f32,
    pub default_value: f32,
}

impl AudioParam {
    pub fn new(default: f32, min: f32, max: f32) -> Self {
        Self { value: default, default_value: default, min_value: min, max_value: max }
    }
    pub fn set(&mut self, v: f32) {
        self.value = f32_clamp(v, self.min_value, self.max_value);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Oscillator waveform
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OscillatorType {
    Sine,
    Square,
    Sawtooth,
    Triangle,
    Custom,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Biquad filter type
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BiquadFilterType {
    Lowpass,
    Highpass,
    Bandpass,
    Notch,
    Allpass,
    Peaking,
    LowShelf,
    HighShelf,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Node ID
// ─────────────────────────────────────────────────────────────────────────────

pub type NodeId = u32;

// ─────────────────────────────────────────────────────────────────────────────
//  Audio node variants
// ─────────────────────────────────────────────────────────────────────────────

pub enum AudioNodeKind {
    Destination,
    Oscillator {
        wave_type:  OscillatorType,
        frequency:  AudioParam,  // Hz
        detune:     AudioParam,  // cents
        phase_acc:  f32,
        started:    bool,
    },
    Gain {
        gain: AudioParam,
    },
    AudioBufferSource {
        buffer:    Option<Rc<RefCell<AudioBuffer>>>,
        loop_:     bool,
        play_pos:  f32,          // fractional sample position
        playback_rate: AudioParam,
        started:   bool,
        ended:     bool,
    },
    BiquadFilter {
        filter_type: BiquadFilterType,
        frequency:   AudioParam,
        q:           AudioParam,
        gain:        AudioParam,
        /// State: [x1, x2, y1, y2]
        state:       [f32; 4],
    },
    Delay {
        delay_time:  AudioParam,  // seconds
        buf:         Vec<f32>,
        write_pos:   usize,
    },
    DynamicsCompressor {
        threshold:   AudioParam,  // dBFS
        knee:        AudioParam,
        ratio:       AudioParam,
        attack:      AudioParam,
        release:     AudioParam,
        envelope:    f32,
    },
    Analyser {
        fft_size:    usize,
        /// Time-domain snapshot of last quantum
        time_domain: Vec<f32>,
    },
    StereoPanner {
        pan: AudioParam,  // -1.0 (left) .. 1.0 (right)
    },
    ChannelMerger {
        num_inputs: usize,
    },
}

pub struct AudioNode {
    pub id:       NodeId,
    pub kind:     AudioNodeKind,
    pub inputs:   Vec<NodeId>,   // nodes whose output feeds this node
    pub channel_count: usize,
}

impl AudioNode {
    /// Render one quantum (128 samples) into `out` (stereo interleaved).
    /// `input_mix` is the already-mixed signal from connected inputs.
    pub fn render(&mut self, input_mix: &[f32], out: &mut [f32], current_time: f64) {
        match &mut self.kind {
            AudioNodeKind::Destination => {
                let n = out.len().min(input_mix.len());
                out[..n].copy_from_slice(&input_mix[..n]);
            }

            AudioNodeKind::Oscillator { wave_type, frequency, detune, phase_acc, started, .. } => {
                if !*started { out.fill(0.0); return; }
                let freq_hz = frequency.value * f32_pow2(detune.value / 1200.0);
                let phase_inc = freq_hz / SAMPLE_RATE;
                let num_frames = out.len() / 2;
                for i in 0..num_frames {
                    let s = match wave_type {
                        OscillatorType::Sine => {
                            f32_sin(*phase_acc * 2.0 * core::f32::consts::PI)
                        }
                        OscillatorType::Square => {
                            if *phase_acc < 0.5 { 1.0 } else { -1.0 }
                        }
                        OscillatorType::Sawtooth => {
                            2.0 * *phase_acc - 1.0
                        }
                        OscillatorType::Triangle => {
                            1.0 - 4.0 * f32_abs(*phase_acc - 0.5)
                        }
                        OscillatorType::Custom => 0.0,
                    };
                    out[i * 2]     = s; // L
                    out[i * 2 + 1] = s; // R
                    *phase_acc += phase_inc;
                    if *phase_acc >= 1.0 { *phase_acc -= 1.0; }
                }
            }

            AudioNodeKind::Gain { gain } => {
                let g = gain.value;
                for i in 0..out.len() {
                    out[i] = input_mix.get(i).copied().unwrap_or(0.0) * g;
                }
            }

            AudioNodeKind::AudioBufferSource { buffer, loop_, play_pos, playback_rate, started, ended } => {
                if !*started || *ended { out.fill(0.0); return; }
                let rate = playback_rate.value;
                let num_frames = out.len() / 2;
                if let Some(buf_rc) = buffer.as_ref() {
                    let buf = buf_rc.borrow();
                    let total = num_samples(&buf) as f32;
                    for i in 0..num_frames {
                        let idx = *play_pos as usize;
                        let frac = *play_pos - idx as f32;
                        // Linear interpolation
                        let s0 = if idx < num_samples(&buf) { buf.data[idx] } else { 0.0 };
                        let s1 = if idx + 1 < num_samples(&buf) { buf.data[idx + 1] } else { 0.0 };
                        let s = s0 + (s1 - s0) * frac;
                        out[i * 2]     = s;
                        out[i * 2 + 1] = s;
                        *play_pos += rate;
                        if *play_pos >= total {
                            if *loop_ { *play_pos -= total; }
                            else { *ended = true; break; }
                        }
                    }
                } else {
                    out.fill(0.0);
                }
            }

            AudioNodeKind::BiquadFilter { filter_type, frequency, q, gain: g_param, state } => {
                // Compute biquad coefficients (direct form II transposed)
                let (b0, b1, b2, a1, a2) =
                    compute_biquad_coeffs(*filter_type, frequency.value, q.value, g_param.value);
                let num_frames = out.len() / 2;
                for i in 0..num_frames {
                    let x = input_mix.get(i * 2).copied().unwrap_or(0.0);
                    let y = b0 * x + state[0];
                    state[0] = b1 * x - a1 * y + state[1];
                    state[1] = b2 * x - a2 * y;
                    out[i * 2]     = y;
                    out[i * 2 + 1] = y;
                }
            }

            AudioNodeKind::Delay { delay_time, buf, write_pos } => {
                let delay_samples = (delay_time.value * SAMPLE_RATE) as usize;
                let delay_samples = delay_samples.max(1);
                // Ensure buffer is large enough
                if buf.len() < delay_samples * 2 { buf.resize(delay_samples * 2 + 2, 0.0); }
                let buf_len = buf.len() / 2;
                let num_frames = out.len() / 2;
                for i in 0..num_frames {
                    let in_l = input_mix.get(i * 2).copied().unwrap_or(0.0);
                    let in_r = input_mix.get(i * 2 + 1).copied().unwrap_or(0.0);
                    let read_pos = (*write_pos + buf_len - delay_samples) % buf_len;
                    out[i * 2]     = buf[read_pos * 2];
                    out[i * 2 + 1] = buf[read_pos * 2 + 1];
                    buf[*write_pos * 2]     = in_l;
                    buf[*write_pos * 2 + 1] = in_r;
                    *write_pos = (*write_pos + 1) % buf_len;
                }
            }

            AudioNodeKind::DynamicsCompressor { threshold, knee: _, ratio, attack, release, envelope } => {
                let thresh_lin = db_to_linear(threshold.value);
                let ratio_val = ratio.value.max(1.0);
                let att_coeff = f32_exp(-1.0 / (attack.value * SAMPLE_RATE));
                let rel_coeff = f32_exp(-1.0 / (release.value * SAMPLE_RATE));
                let num_frames = out.len() / 2;
                for i in 0..num_frames {
                    let in_l = input_mix.get(i * 2).copied().unwrap_or(0.0);
                    let in_r = input_mix.get(i * 2 + 1).copied().unwrap_or(0.0);
                    let peak = f32_abs(in_l).max(f32_abs(in_r));
                    // Envelope follower
                    if peak > *envelope {
                        *envelope = att_coeff * *envelope + (1.0 - att_coeff) * peak;
                    } else {
                        *envelope = rel_coeff * *envelope + (1.0 - rel_coeff) * peak;
                    }
                    // Gain computation
                    let gain = if *envelope > thresh_lin {
                        let excess = *envelope / thresh_lin;
                        thresh_lin * f32_pow(excess, 1.0 / ratio_val) / *envelope
                    } else {
                        1.0
                    };
                    out[i * 2]     = in_l * gain;
                    out[i * 2 + 1] = in_r * gain;
                }
            }

            AudioNodeKind::Analyser { fft_size, time_domain } => {
                // Just capture time-domain data; FFT is a stub
                let n = QUANTUM.min(*fft_size);
                time_domain.resize(n, 0.0);
                for i in 0..n {
                    time_domain[i] = input_mix.get(i * 2).copied().unwrap_or(0.0);
                }
                // Pass through unchanged
                let len = out.len().min(input_mix.len());
                out[..len].copy_from_slice(&input_mix[..len]);
            }

            AudioNodeKind::StereoPanner { pan } => {
                let p = f32_clamp(pan.value, -1.0, 1.0);
                // Constant-power pan law
                let angle = (p + 1.0) * core::f32::consts::FRAC_PI_4;
                let gain_l = f32_cos(angle);
                let gain_r = f32_sin(angle);
                let num_frames = out.len() / 2;
                for i in 0..num_frames {
                    let in_l = input_mix.get(i * 2).copied().unwrap_or(0.0);
                    let in_r = input_mix.get(i * 2 + 1).copied().unwrap_or(0.0);
                    let mono = (in_l + in_r) * 0.5;
                    out[i * 2]     = mono * gain_l;
                    out[i * 2 + 1] = mono * gain_r;
                }
            }

            AudioNodeKind::ChannelMerger { .. } => {
                // Pass through (inputs are already merged by the context)
                let len = out.len().min(input_mix.len());
                out[..len].copy_from_slice(&input_mix[..len]);
            }
        }
    }

    pub fn is_source(&self) -> bool {
        matches!(self.kind,
            AudioNodeKind::Oscillator { .. } | AudioNodeKind::AudioBufferSource { .. })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Biquad coefficient computation
// ─────────────────────────────────────────────────────────────────────────────

fn compute_biquad_coeffs(
    t: BiquadFilterType, freq: f32, q: f32, gain_db: f32,
) -> (f32, f32, f32, f32, f32) {
    let w0 = 2.0 * core::f32::consts::PI * freq / SAMPLE_RATE;
    let sin_w0 = f32_sin(w0);
    let cos_w0 = f32_cos(w0);
    let alpha = sin_w0 / (2.0 * q.max(0.001));
    let a = db_to_linear(gain_db / 2.0); // for shelving/peaking

    match t {
        BiquadFilterType::Lowpass => {
            let b0 = (1.0 - cos_w0) / 2.0;
            let b1 = 1.0 - cos_w0;
            let b2 = (1.0 - cos_w0) / 2.0;
            let a0 = 1.0 + alpha;
            let a1 = -2.0 * cos_w0;
            let a2 = 1.0 - alpha;
            normalise_biquad(b0, b1, b2, a0, a1, a2)
        }
        BiquadFilterType::Highpass => {
            let b0 = (1.0 + cos_w0) / 2.0;
            let b1 = -(1.0 + cos_w0);
            let b2 = (1.0 + cos_w0) / 2.0;
            let a0 = 1.0 + alpha;
            let a1 = -2.0 * cos_w0;
            let a2 = 1.0 - alpha;
            normalise_biquad(b0, b1, b2, a0, a1, a2)
        }
        BiquadFilterType::Bandpass => {
            let b0 = sin_w0 / 2.0;
            let b1 = 0.0;
            let b2 = -sin_w0 / 2.0;
            let a0 = 1.0 + alpha;
            let a1 = -2.0 * cos_w0;
            let a2 = 1.0 - alpha;
            normalise_biquad(b0, b1, b2, a0, a1, a2)
        }
        BiquadFilterType::Notch => {
            let b0 = 1.0;
            let b1 = -2.0 * cos_w0;
            let b2 = 1.0;
            let a0 = 1.0 + alpha;
            let a1 = -2.0 * cos_w0;
            let a2 = 1.0 - alpha;
            normalise_biquad(b0, b1, b2, a0, a1, a2)
        }
        _ => {
            // Allpass / peaking / shelves: return flat passthrough
            (1.0, 0.0, 0.0, 0.0, 0.0)
        }
    }
}

fn normalise_biquad(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32)
    -> (f32, f32, f32, f32, f32)
{
    let inv = 1.0 / a0;
    (b0 * inv, b1 * inv, b2 * inv, a1 * inv, a2 * inv)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Float helpers
// ─────────────────────────────────────────────────────────────────────────────

fn db_to_linear(db: f32) -> f32 { f32_pow(10.0, db / 20.0) }

/// 2^x via Taylor: 2^x = e^(x*ln2), approximated.
fn f32_pow2(x: f32) -> f32 {
    const LN2: f32 = 0.693_147_18;
    f32_exp(x * LN2)
}

/// e^x via Taylor, clamped.
fn f32_exp(x: f32) -> f32 {
    if x > 88.0 { return f32::MAX; }
    if x < -88.0 { return 0.0; }
    // Use range reduction: e^x = e^n * e^f, n=floor(x), f=frac
    let n = f32_floor(x);
    let f = x - n;
    // 6-term Horner: 1 + f + f²/2! + f³/3! + f⁴/4! + f⁵/5! + f⁶/6!
    // Reduces error at f≈0.93 from ~2.8 to ~0.05 (well within the 1.0 test threshold)
    let ef = 1.0 + f * (1.0 + f * (0.5 + f * (1.0/6.0 + f * (1.0/24.0 + f * (1.0/120.0 + f/720.0)))));
    // e^n via repeated squaring on integer
    let mut result = 1.0f32;
    let mut base = core::f32::consts::E;
    let mut exp_i = f32_abs(n) as u32;
    while exp_i > 0 {
        if exp_i & 1 != 0 { result *= base; }
        base *= base;
        exp_i >>= 1;
    }
    if n < 0.0 { ef / result } else { ef * result }
}

/// a^b = exp(b * ln(a))
fn f32_pow(a: f32, b: f32) -> f32 {
    if a <= 0.0 { return 0.0; }
    f32_exp(b * f32_ln(a))
}

/// Natural log via identity: ln(x) = 2 * atanh((x-1)/(x+1))
fn f32_ln(x: f32) -> f32 {
    if x <= 0.0 { return f32::NEG_INFINITY; }
    // Normalise to [1, 2) via exponent
    let mut mantissa = x;
    let mut exp2: i32 = 0;
    while mantissa >= 2.0 { mantissa *= 0.5; exp2 += 1; }
    while mantissa < 1.0  { mantissa *= 2.0; exp2 -= 1; }
    // ln(m * 2^n) = ln(m) + n*ln(2)
    const LN2: f32 = 0.693_147_18;
    let y = (mantissa - 1.0) / (mantissa + 1.0);
    let y2 = y * y;
    let ln_m = 2.0 * y * (1.0 + y2 / 3.0 + y2 * y2 / 5.0 + y2 * y2 * y2 / 7.0);
    ln_m + exp2 as f32 * LN2
}

// ─────────────────────────────────────────────────────────────────────────────
//  AudioContext
// ─────────────────────────────────────────────────────────────────────────────

pub struct AudioContext {
    pub sample_rate:   f32,
    pub current_time:  f64,

    nodes:     BTreeMap<NodeId, AudioNode>,
    dest_id:   NodeId,
    next_id:   NodeId,
}

impl AudioContext {
    pub fn new() -> Self {
        let mut ctx = AudioContext {
            sample_rate: SAMPLE_RATE,
            current_time: 0.0,
            nodes: BTreeMap::new(),
            dest_id: 0,
            next_id: 1,
        };
        // Create destination node
        let dest = AudioNode {
            id: 0,
            kind: AudioNodeKind::Destination,
            inputs: Vec::new(),
            channel_count: 2,
        };
        ctx.nodes.insert(0, dest);
        ctx
    }

    pub fn destination_id(&self) -> NodeId { self.dest_id }

    // ── Node creation ─────────────────────────────────────────────────────────

    pub fn create_oscillator(&mut self) -> NodeId {
        let id = self.alloc_id();
        self.nodes.insert(id, AudioNode {
            id, channel_count: 2, inputs: Vec::new(),
            kind: AudioNodeKind::Oscillator {
                wave_type: OscillatorType::Sine,
                frequency: AudioParam::new(440.0, 0.0, 24000.0),
                detune:    AudioParam::new(0.0, -153_600.0, 153_600.0),
                phase_acc: 0.0,
                started:   false,
            },
        });
        id
    }

    pub fn create_gain(&mut self) -> NodeId {
        let id = self.alloc_id();
        self.nodes.insert(id, AudioNode {
            id, channel_count: 2, inputs: Vec::new(),
            kind: AudioNodeKind::Gain {
                gain: AudioParam::new(1.0, f32::NEG_INFINITY, f32::MAX),
            },
        });
        id
    }

    pub fn create_buffer_source(&mut self) -> NodeId {
        let id = self.alloc_id();
        self.nodes.insert(id, AudioNode {
            id, channel_count: 2, inputs: Vec::new(),
            kind: AudioNodeKind::AudioBufferSource {
                buffer: None, loop_: false, play_pos: 0.0,
                playback_rate: AudioParam::new(1.0, -3.4028235e+38, 3.4028235e+38),
                started: false, ended: false,
            },
        });
        id
    }

    pub fn create_biquad_filter(&mut self) -> NodeId {
        let id = self.alloc_id();
        self.nodes.insert(id, AudioNode {
            id, channel_count: 2, inputs: Vec::new(),
            kind: AudioNodeKind::BiquadFilter {
                filter_type: BiquadFilterType::Lowpass,
                frequency:   AudioParam::new(350.0, 0.0, SAMPLE_RATE / 2.0),
                q:           AudioParam::new(1.0, 0.0001, 1000.0),
                gain:        AudioParam::new(0.0, -40.0, 40.0),
                state:       [0.0; 4],
            },
        });
        id
    }

    pub fn create_delay(&mut self, max_delay_secs: f32) -> NodeId {
        let max_samples = (max_delay_secs * SAMPLE_RATE) as usize * 2 + 2;
        let id = self.alloc_id();
        self.nodes.insert(id, AudioNode {
            id, channel_count: 2, inputs: Vec::new(),
            kind: AudioNodeKind::Delay {
                delay_time: AudioParam::new(0.0, 0.0, max_delay_secs),
                buf: vec![0.0; max_samples],
                write_pos: 0,
            },
        });
        id
    }

    pub fn create_dynamics_compressor(&mut self) -> NodeId {
        let id = self.alloc_id();
        self.nodes.insert(id, AudioNode {
            id, channel_count: 2, inputs: Vec::new(),
            kind: AudioNodeKind::DynamicsCompressor {
                threshold: AudioParam::new(-24.0, -100.0, 0.0),
                knee:      AudioParam::new(30.0, 0.0, 40.0),
                ratio:     AudioParam::new(12.0, 1.0, 20.0),
                attack:    AudioParam::new(0.003, 0.0, 1.0),
                release:   AudioParam::new(0.25, 0.0, 1.0),
                envelope:  0.0,
            },
        });
        id
    }

    pub fn create_analyser(&mut self) -> NodeId {
        let id = self.alloc_id();
        self.nodes.insert(id, AudioNode {
            id, channel_count: 2, inputs: Vec::new(),
            kind: AudioNodeKind::Analyser {
                fft_size: 2048,
                time_domain: Vec::new(),
            },
        });
        id
    }

    pub fn create_stereo_panner(&mut self) -> NodeId {
        let id = self.alloc_id();
        self.nodes.insert(id, AudioNode {
            id, channel_count: 2, inputs: Vec::new(),
            kind: AudioNodeKind::StereoPanner {
                pan: AudioParam::new(0.0, -1.0, 1.0),
            },
        });
        id
    }

    // ── Node parameter setters ─────────────────────────────────────────────────

    pub fn set_oscillator_type(&mut self, id: NodeId, wt: OscillatorType) {
        if let Some(n) = self.nodes.get_mut(&id) {
            if let AudioNodeKind::Oscillator { wave_type, .. } = &mut n.kind {
                *wave_type = wt;
            }
        }
    }

    pub fn set_frequency(&mut self, id: NodeId, hz: f32) {
        if let Some(n) = self.nodes.get_mut(&id) {
            if let AudioNodeKind::Oscillator { frequency, .. } = &mut n.kind {
                frequency.set(hz);
            }
        }
    }

    pub fn set_gain(&mut self, id: NodeId, g: f32) {
        if let Some(n) = self.nodes.get_mut(&id) {
            if let AudioNodeKind::Gain { gain } = &mut n.kind {
                gain.set(g);
            }
        }
    }

    pub fn set_pan(&mut self, id: NodeId, p: f32) {
        if let Some(n) = self.nodes.get_mut(&id) {
            if let AudioNodeKind::StereoPanner { pan } = &mut n.kind {
                pan.set(p);
            }
        }
    }

    pub fn start_oscillator(&mut self, id: NodeId) {
        if let Some(n) = self.nodes.get_mut(&id) {
            if let AudioNodeKind::Oscillator { started, .. } = &mut n.kind {
                *started = true;
            }
        }
    }

    pub fn stop_oscillator(&mut self, id: NodeId) {
        if let Some(n) = self.nodes.get_mut(&id) {
            if let AudioNodeKind::Oscillator { started, .. } = &mut n.kind {
                *started = false;
            }
        }
    }

    pub fn set_buffer(&mut self, id: NodeId, buf: Rc<RefCell<AudioBuffer>>) {
        if let Some(n) = self.nodes.get_mut(&id) {
            if let AudioNodeKind::AudioBufferSource { buffer, .. } = &mut n.kind {
                *buffer = Some(buf);
            }
        }
    }

    pub fn start_buffer_source(&mut self, id: NodeId) {
        if let Some(n) = self.nodes.get_mut(&id) {
            if let AudioNodeKind::AudioBufferSource { started, ended, play_pos, .. } = &mut n.kind {
                *started = true;
                *ended = false;
                *play_pos = 0.0;
            }
        }
    }

    // ── Connection ─────────────────────────────────────────────────────────────

    /// Connect source node's output to dest node's input.
    pub fn connect(&mut self, source: NodeId, dest: NodeId) {
        if let Some(dest_node) = self.nodes.get_mut(&dest) {
            if !dest_node.inputs.contains(&source) {
                dest_node.inputs.push(source);
            }
        }
    }

    pub fn disconnect(&mut self, source: NodeId, dest: NodeId) {
        if let Some(dest_node) = self.nodes.get_mut(&dest) {
            dest_node.inputs.retain(|&id| id != source);
        }
    }

    // ── Rendering ─────────────────────────────────────────────────────────────

    /// Render one quantum (QUANTUM frames) and submit to the HDA mixer.
    pub fn render_frame(&mut self) {
        // Topological render: collect all node IDs, then render leaves-first.
        // Simple approach: render in reverse BFS from destination.
        let mut rendered: BTreeMap<NodeId, Vec<f32>> = BTreeMap::new();
        self.render_node(self.dest_id, &mut rendered);

        // Extract destination output and push to HDA mixer
        if let Some(dest_buf) = rendered.get(&self.dest_id) {
            let samples: Vec<i16> = dest_buf.iter().map(|&s| {
                let clamped = f32_clamp(s, -1.0, 1.0);
                (clamped * 32767.0) as i16
            }).collect();
            // Submit to the mixer as a burst
            submit_to_mixer(&samples);
        }

        self.current_time += QUANTUM as f64 / SAMPLE_RATE as f64;
    }

    fn render_node(&mut self, id: NodeId, cache: &mut BTreeMap<NodeId, Vec<f32>>) {
        if cache.contains_key(&id) { return; }

        // First render all inputs
        let inputs: Vec<NodeId> = self.nodes.get(&id)
            .map(|n| n.inputs.clone())
            .unwrap_or_default();

        for inp_id in &inputs {
            self.render_node(*inp_id, cache);
        }

        // Mix all input buffers
        let mut mixed = vec![0.0f32; QUANTUM * 2];
        for inp_id in &inputs {
            if let Some(buf) = cache.get(inp_id) {
                for i in 0..mixed.len().min(buf.len()) {
                    mixed[i] += buf[i];
                }
            }
        }

        // Render this node
        let mut out = vec![0.0f32; QUANTUM * 2];
        let ct = self.current_time;
        if let Some(node) = self.nodes.get_mut(&id) {
            node.render(&mixed, &mut out, ct);
        }
        cache.insert(id, out);
    }

    fn alloc_id(&mut self) -> NodeId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Get the time-domain data from an AnalyserNode.
    pub fn get_time_domain_data(&self, id: NodeId) -> Vec<f32> {
        if let Some(n) = self.nodes.get(&id) {
            if let AudioNodeKind::Analyser { time_domain, .. } = &n.kind {
                return time_domain.clone();
            }
        }
        Vec::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  HDA mixer bridge
// ─────────────────────────────────────────────────────────────────────────────

fn submit_to_mixer(samples: &[i16]) {
    use crate::drivers::hda;
    let mut mixer = hda::MIXER.lock();
    // Find or create stream 0 for the Web Audio output
    if mixer.streams.is_empty() {
        mixer.add_stream(hda::AudioStream {
            id: 0,
            buffer: alloc::collections::VecDeque::new(),
            volume: 100,
        });
    }
    if let Some(stream) = mixer.streams.first_mut() {
        for &s in samples {
            stream.buffer.push_back(s);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Global AudioContext registry
// ─────────────────────────────────────────────────────────────────────────────

pub type ContextId = u32;

struct ContextRegistry {
    contexts: BTreeMap<ContextId, AudioContext>,
    next_id:  ContextId,
}

impl ContextRegistry {
    const fn new() -> Self { Self { contexts: BTreeMap::new(), next_id: 0 } }

    fn create(&mut self) -> ContextId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.contexts.insert(id, AudioContext::new());
        id
    }

    fn get(&mut self, id: ContextId) -> Option<&mut AudioContext> {
        self.contexts.get_mut(&id)
    }

    fn close(&mut self, id: ContextId) {
        self.contexts.remove(&id);
    }
}

// SAFETY: AudioContext contains Rc (for AudioBuffer sharing), but Smart OS
// is cooperatively scheduled on a single core.
struct SendRegistry(ContextRegistry);
unsafe impl Send for SendRegistry {}
unsafe impl Sync for SendRegistry {}

static CONTEXTS: Mutex<SendRegistry> = Mutex::new(SendRegistry(ContextRegistry::new()));

pub fn create_context() -> ContextId {
    CONTEXTS.lock().0.create()
}

pub fn with_context<F, R>(id: ContextId, f: F) -> Option<R>
    where F: FnOnce(&mut AudioContext) -> R
{
    let mut reg = CONTEXTS.lock();
    reg.0.get(id).map(f)
}

pub fn close_context(id: ContextId) {
    CONTEXTS.lock().0.close(id);
}

// ─────────────────────────────────────────────────────────────────────────────
//  Self-tests
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    // ── Test 1: OscillatorNode (sine 440 Hz) ─────────────────────────────────
    let mut ctx = AudioContext::new();
    let osc = ctx.create_oscillator();
    let gain = ctx.create_gain();
    ctx.set_frequency(osc, 440.0);
    ctx.set_gain(gain, 0.5);
    ctx.connect(osc, gain);
    ctx.connect(gain, ctx.destination_id());
    ctx.start_oscillator(osc);

    // Render one frame — output should have non-zero samples
    let dest_id = ctx.destination_id();
    let mut rendered: BTreeMap<NodeId, Vec<f32>> = BTreeMap::new();
    ctx.render_node(dest_id, &mut rendered);
    let dest_out = match rendered.get(&dest_id) {
        Some(v) => v.clone(),
        None => return false,
    };
    // First sample of a 440 Hz sine at 0 phase should be ~0 (sin(0)=0)
    // Third sample should be non-zero
    if dest_out.len() < 6 { return false; }
    // Samples 4..6 should be nonzero
    let nonzero = dest_out[4..6].iter().any(|&s| s.abs() > 1e-6);
    if !nonzero { return false; }

    // ── Test 2: GainNode at 0.0 → silence ────────────────────────────────────
    let mut ctx2 = AudioContext::new();
    let osc2 = ctx2.create_oscillator();
    let gain2 = ctx2.create_gain();
    ctx2.set_frequency(osc2, 1000.0);
    ctx2.set_gain(gain2, 0.0);
    ctx2.connect(osc2, gain2);
    ctx2.connect(gain2, ctx2.destination_id());
    ctx2.start_oscillator(osc2);
    let mut rendered2: BTreeMap<NodeId, Vec<f32>> = BTreeMap::new();
    let dest2 = ctx2.destination_id();
    ctx2.render_node(dest2, &mut rendered2);
    if let Some(out2) = rendered2.get(&dest2) {
        if out2.iter().any(|&s| s.abs() > 1e-9) { return false; }
    }

    // ── Test 3: AudioBuffer round-trip ────────────────────────────────────────
    let mut buf = AudioBuffer::new(1, 256, SAMPLE_RATE);
    let sine_data: Vec<f32> = (0..256)
        .map(|i| f32_sin(i as f32 * 2.0 * core::f32::consts::PI * 440.0 / SAMPLE_RATE))
        .collect();
    buf.copy_from_channel(0, &sine_data);
    let ch = buf.get_channel_data(0);
    if (ch[0] - sine_data[0]).abs() > 1e-6 { return false; }
    if (ch[128] - sine_data[128]).abs() > 1e-6 { return false; }

    // ── Test 4: BiquadFilter (lowpass) passes low freq ────────────────────────
    let mut ctx3 = AudioContext::new();
    let filt = ctx3.create_biquad_filter();
    // Low-frequency content should pass through a 1kHz lowpass
    // (simple smoke test: the node renders without panicking)
    let input_mix = vec![0.5f32; QUANTUM * 2];
    let mut out3 = vec![0.0f32; QUANTUM * 2];
    if let Some(n) = ctx3.nodes.get_mut(&filt) {
        n.render(&input_mix, &mut out3, 0.0);
    }
    // Output should be non-NaN
    if out3.iter().any(|s| s.is_nan()) { return false; }

    // ── Test 5: Float math helpers ────────────────────────────────────────────
    if (f32_sin(0.0) - 0.0).abs() > 1e-5 { return false; }
    if (f32_sin(core::f32::consts::FRAC_PI_2) - 1.0).abs() > 1e-4 { return false; }
    if (f32_exp(1.0) - core::f32::consts::E).abs() > 1e-4 { return false; }
    if (f32_ln(core::f32::consts::E) - 1.0).abs() > 1e-4 { return false; }
    if (f32_pow2(10.0) - 1024.0).abs() > 1.0 { return false; }

    true
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[web_audio] Web Audio API ready (Phase 45, 44100 Hz).");
}

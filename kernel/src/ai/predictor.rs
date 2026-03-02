/// Predictive Resource Scheduling for Smart OS.
///
/// Tracks user app launch patterns and predicts which apps will be
/// launched next. Uses a small FeedForwardModel (8→8→16) trained on
/// time-of-day patterns and app transition sequences.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use super::model::*;
use super::tensor::Tensor;

/// Maximum number of events in the history ring.
const MAX_HISTORY: usize = 256;
/// Number of time buckets (map uptime to periodic buckets for patterns).
const TIME_BUCKETS: usize = 16;
/// Number of recent apps tracked in the sequence feature.
const RECENT_SEQUENCE: usize = 4;
/// Maximum number of distinct apps tracked.
const MAX_APPS: usize = 16;

// ── Launch Event ────────────────────────────────────────────────────

/// A recorded app launch event.
#[derive(Debug, Clone)]
pub struct LaunchEvent {
    pub app_name: String,
    pub tick: u64,
    pub time_bucket: u8,
}

// ── Usage Tracker ───────────────────────────────────────────────────

/// Tracks app launch patterns for prediction.
pub struct UsageTracker {
    /// History of recent launch events (ring buffer).
    history: Vec<LaunchEvent>,
    /// Per-app launch count by time bucket.
    time_patterns: BTreeMap<String, [u32; TIME_BUCKETS]>,
    /// Transition matrix: app_i launched after app_j.
    transitions: [[u32; MAX_APPS]; MAX_APPS],
    /// App name → index mapping.
    app_indices: BTreeMap<String, usize>,
    /// Reverse index: index → app name.
    app_names: Vec<String>,
    /// Next available app index.
    next_app_idx: usize,
    /// Last launched app index (for transitions).
    last_app_idx: Option<usize>,
}

/// Global usage tracker.
pub static TRACKER: Mutex<Option<UsageTracker>> = Mutex::new(None);

impl UsageTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            time_patterns: BTreeMap::new(),
            transitions: [[0u32; MAX_APPS]; MAX_APPS],
            app_indices: BTreeMap::new(),
            app_names: Vec::new(),
            next_app_idx: 0,
            last_app_idx: None,
        }
    }

    /// Record an app launch event.
    pub fn record_launch(&mut self, app_name: &str) {
        let tick = crate::drivers::timer::ticks();
        let bucket = Self::current_time_bucket();
        let idx = self.get_or_create_index(app_name);

        // Update time pattern
        self.time_patterns
            .entry(String::from(app_name))
            .or_insert([0u32; TIME_BUCKETS])[bucket as usize] += 1;

        // Update transition matrix
        if let Some(prev_idx) = self.last_app_idx {
            if prev_idx < MAX_APPS && idx < MAX_APPS {
                self.transitions[prev_idx][idx] += 1;
            }
        }
        self.last_app_idx = Some(idx);

        // Add to history (ring)
        let event = LaunchEvent {
            app_name: String::from(app_name),
            tick,
            time_bucket: bucket,
        };
        if self.history.len() >= MAX_HISTORY {
            self.history.remove(0);
        }
        self.history.push(event);
    }

    /// Extract an 8-element INT8 feature vector for the predictor model.
    ///
    /// Features:
    ///   [0]: Time bucket (sin component, scaled to INT8)
    ///   [1]: Time bucket (cos component, scaled to INT8)
    ///   [2..5]: Last 4 app indices (scaled to INT8)
    ///   [6]: Total launch count (normalized)
    ///   [7]: History diversity (unique apps / total apps)
    pub fn extract_features(&self) -> Vec<i8> {
        let mut features = vec![0i8; 8];
        let bucket = Self::current_time_bucket() as f32;

        // Time features: sin/cos encoding of time bucket
        let angle = bucket / TIME_BUCKETS as f32 * core::f32::consts::PI * 2.0;
        features[0] = (sin_approx(angle) * 127.0) as i8;
        features[1] = (cos_approx(angle) * 127.0) as i8;

        // Recent app sequence (last 4 apps)
        let history_len = self.history.len();
        for i in 0..RECENT_SEQUENCE {
            if i < history_len {
                let event = &self.history[history_len - 1 - i];
                let idx = self.app_indices.get(&event.app_name).copied().unwrap_or(0);
                features[2 + i] = ((idx as f32 / MAX_APPS as f32) * 254.0 - 127.0)
                    .clamp(-128.0, 127.0) as i8;
            }
        }

        // Total launch count (normalized, capped at 256)
        let total = self.history.len().min(256) as f32 / 256.0;
        features[6] = (total * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;

        // History diversity
        let unique: usize = {
            let mut seen = alloc::collections::BTreeSet::new();
            for event in &self.history {
                seen.insert(event.app_name.clone());
            }
            seen.len()
        };
        let diversity = if self.history.is_empty() {
            0.0
        } else {
            unique as f32 / self.history.len().min(MAX_APPS) as f32
        };
        features[7] = (diversity * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;

        features
    }

    /// Get the app index for a name, allocating a new index if needed.
    fn get_or_create_index(&mut self, name: &str) -> usize {
        if let Some(&idx) = self.app_indices.get(name) {
            return idx;
        }
        if self.next_app_idx >= MAX_APPS {
            return MAX_APPS - 1; // Saturate
        }
        let idx = self.next_app_idx;
        self.app_indices.insert(String::from(name), idx);
        self.app_names.push(String::from(name));
        self.next_app_idx += 1;
        idx
    }

    /// Get the app name for an index.
    pub fn app_name_for_index(&self, idx: usize) -> Option<&str> {
        self.app_names.get(idx).map(|s| s.as_str())
    }

    /// Get current time bucket from uptime.
    fn current_time_bucket() -> u8 {
        let secs = crate::drivers::timer::ticks() / 100; // ~100Hz timer
        ((secs / 60) % TIME_BUCKETS as u64) as u8
    }

    /// Number of tracked events.
    pub fn event_count(&self) -> usize {
        self.history.len()
    }

    /// Number of tracked apps.
    pub fn app_count(&self) -> usize {
        self.next_app_idx
    }
}

// ── Predictor Model ─────────────────────────────────────────────────

/// App launch predictor: 8→8→MAX_APPS FeedForwardModel.
pub struct AppPredictor {
    model: FeedForwardModel,
}

/// Global predictor instance.
pub static PREDICTOR: Mutex<Option<AppPredictor>> = Mutex::new(None);

impl AppPredictor {
    /// Create a new predictor with hand-tuned initial weights.
    pub fn new() -> Self {
        let mut model = FeedForwardModel::new("app-predictor", 8);

        // Generate labels for MAX_APPS slots
        let labels: Vec<String> = (0..MAX_APPS)
            .map(|i| alloc::format!("app_{}", i))
            .collect();
        model.set_labels(labels);

        // Layer 1: 8→8, ReLU
        // Weights emphasize time patterns and recent app sequence
        #[rustfmt::skip]
        let w1_data: Vec<i8> = vec![
            // Neuron 0: time-sensitive (sin/cos)
             40,  30,  10,   5,   5,   5,  10,   5,
            // Neuron 1: time-sensitive (cos emphasis)
             30,  40,   5,  10,   5,   5,   5,  10,
            // Neuron 2: recent app 0 emphasis
             10,  10,  40,  15,  10,   5,   5,   5,
            // Neuron 3: recent app 1 emphasis
              5,   5,  15,  40,  15,  10,   5,   5,
            // Neuron 4: recent apps 2-3
              5,   5,  10,  15,  35,  25,   5,   5,
            // Neuron 5: recent app 3 + diversity
              5,   5,   5,  10,  25,  35,   5,  15,
            // Neuron 6: volume (total count)
             10,  10,   5,   5,   5,   5,  40,  10,
            // Neuron 7: diversity-weighted
              5,   5,  10,  10,  10,  10,  10,  40,
        ];
        let w1 = Tensor::from_data(w1_data, 8, 8, 0.02, 0);
        let b1 = vec![0i32; 8];
        let layer1 = DenseLayer::new(w1, b1, Activation::ReLU, 0.02, 0);

        // Layer 2: 8→16, None (softmax applied in inference)
        // Each output neuron corresponds to one app slot
        let mut w2_data = vec![5i8; 8 * MAX_APPS];
        // Make each app slot responsive to different feature combinations
        for i in 0..MAX_APPS.min(8) {
            w2_data[i * 8 + (i % 8)] = 30; // Diagonal emphasis
            w2_data[i * 8 + 2] = 20;       // Recent app 0 always relevant
        }
        let w2 = Tensor::from_data(w2_data, MAX_APPS, 8, 0.02, 0);
        let b2 = vec![0i32; MAX_APPS];
        let layer2 = DenseLayer::new(w2, b2, Activation::None, 0.02, 0);

        model.add_layer(layer1);
        model.add_layer(layer2);

        Self { model }
    }

    /// Run prediction: returns ranked list of (app_index, probability).
    pub fn predict(&self, features: &[i8]) -> Vec<(usize, f32)> {
        let tensor = Tensor::from_vec(features.to_vec(), 0.01, 0);

        let engine = super::inference::ENGINE.lock();
        if let Some(eng) = engine.as_ref() {
            if let Ok(result) = eng.infer("app-predictor", &tensor) {
                let mut scores: Vec<(usize, f32)> = result
                    .all_scores
                    .iter()
                    .enumerate()
                    .map(|(i, (_, score))| (i, *score))
                    .collect();
                scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
                return scores;
            }
        }
        Vec::new()
    }
}

// ── Public API ──────────────────────────────────────────────────────

/// Initialize the predictor system.
pub fn init() {
    *TRACKER.lock() = Some(UsageTracker::new());

    // Create and register the predictor model
    let predictor = AppPredictor::new();
    {
        let mut engine = super::inference::ENGINE.lock();
        if let Some(eng) = engine.as_mut() {
            eng.register_model(predictor.model.clone());
        }
    }
    *PREDICTOR.lock() = Some(predictor);

    crate::serial_println!("[predictor] Predictive resource scheduler initialized.");
}

/// Record an app launch event (called from scheduler::spawn).
pub fn record_app_launch(app_name: &str) {
    if let Some(tracker) = TRACKER.lock().as_mut() {
        tracker.record_launch(app_name);
    }
}

/// Get top-N predicted next apps.
/// Returns Vec<(app_name, probability)>.
pub fn predict_next(n: usize) -> Vec<(String, f32)> {
    let tracker = TRACKER.lock();
    let predictor = PREDICTOR.lock();

    if let (Some(t), Some(p)) = (tracker.as_ref(), predictor.as_ref()) {
        let features = t.extract_features();
        let scores = p.predict(&features);

        let mut results = Vec::new();
        for (idx, score) in scores.iter().take(n) {
            if let Some(name) = t.app_name_for_index(*idx) {
                results.push((String::from(name), *score));
            }
        }
        results
    } else {
        Vec::new()
    }
}

// ── Math helpers (no_std) ───────────────────────────────────────────

/// Approximate sine for no_std (Bhaskara I approximation).
fn sin_approx(x: f32) -> f32 {
    let pi = core::f32::consts::PI;
    // Normalize to [0, 2pi]
    let x = x % (2.0 * pi);
    let x = if x < 0.0 { x + 2.0 * pi } else { x };

    // Use symmetry: sin(x) for x in [0, pi], -sin(2pi-x) for x in [pi, 2pi]
    let (x_norm, sign) = if x > pi { (2.0 * pi - x, -1.0) } else { (x, 1.0) };

    // Bhaskara approximation: sin(x) ≈ 16x(π-x) / (5π²-4x(π-x))
    let num = 16.0 * x_norm * (pi - x_norm);
    let den = 5.0 * pi * pi - 4.0 * x_norm * (pi - x_norm);
    if den.abs() < 1e-10 {
        0.0
    } else {
        sign * num / den
    }
}

/// Approximate cosine for no_std.
fn cos_approx(x: f32) -> f32 {
    sin_approx(x + core::f32::consts::FRAC_PI_2)
}

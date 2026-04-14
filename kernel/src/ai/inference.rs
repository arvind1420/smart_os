/// High-level AI inference API and built-in models.
///
/// Provides a simple interface: feed bytes in, get classification out.
/// The built-in file classifier uses hand-tuned INT8 weights to categorize
/// file contents by type (text, source code, config, binary, etc.).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use super::tensor::*;
use super::model::*;

/// Result of running inference.
#[derive(Debug, Clone)]
pub struct InferenceResult {
    /// Predicted class label.
    pub label: String,
    /// Confidence score (0.0 - 1.0).
    pub confidence: f32,
    /// All class scores.
    pub all_scores: Vec<(String, f32)>,
}

/// Mock inference call for security/auth.
pub fn run_inference(_data: &[u8]) -> Result<(), &'static str> {
    Ok(())
}

/// The AI inference engine.
pub struct InferenceEngine {
    models: BTreeMap<String, FeedForwardModel>,
}

pub static ENGINE: Mutex<Option<InferenceEngine>> = Mutex::new(None);

impl InferenceEngine {
    fn new() -> Self {
        Self {
            models: BTreeMap::new(),
        }
    }

    /// Register a model.
    pub fn register_model(&mut self, model: FeedForwardModel) {
        crate::serial_println!("[ai] Registered model '{}' ({} params, {} layers)",
            model.name, model.param_count(), model.layers.len());
        self.models.insert(model.name.clone(), model);
    }

    /// Run inference on a model.
    pub fn infer(&self, model_name: &str, input: &Tensor) -> Result<InferenceResult, &'static str> {
        let model = self.models.get(model_name).ok_or("Model not found")?;

        // Forward pass through all layers
        let mut current = input.clone();

        for layer in &model.layers {
            // Matrix multiply: input * weights^T
            // input: [1 x in_size], weights: [out_size x in_size]
            // We need to multiply: input[1 x in] * weights_transposed[in x out] = [1 x out]
            // But our weights are stored as [out x in], so we do: output[1 x out] where
            // output[0][k] = sum_j(input[0][j] * weights[k][j])

            let mut acc = AccTensor::zeros(1, layer.output_size());
            for k in 0..layer.output_size() {
                let mut sum: i32 = 0;
                for j in 0..layer.input_size() {
                    sum += current.get(0, j) as i32 * layer.weights.get(k, j) as i32;
                }
                acc.set(0, k, sum);
            }

            // Add bias
            add_bias_i32(&mut acc, &layer.bias);

            // Requantize
            let combined_scale = (current.scale * layer.weights.scale) / layer.output_scale;
            current = requantize(&acc, combined_scale, layer.output_zero_point, layer.output_scale);

            // Apply activation
            match layer.activation {
                Activation::ReLU => relu_i8(&mut current),
                Activation::Sigmoid => current = sigmoid_i8(&current),
                Activation::Softmax | Activation::None => {} // Softmax handled separately
            }
        }

        // Get probabilities from the final layer output
        let probabilities = softmax_f32(&current);

        // Build results
        let mut all_scores: Vec<(String, f32)> = Vec::new();
        let mut best_idx = 0;
        let mut best_score = 0.0f32;

        for (i, &prob) in probabilities.iter().enumerate() {
            let label = if i < model.labels.len() {
                model.labels[i].clone()
            } else {
                alloc::format!("class_{}", i)
            };
            if prob > best_score {
                best_score = prob;
                best_idx = i;
            }
            all_scores.push((label, prob));
        }

        let label = if best_idx < model.labels.len() {
            model.labels[best_idx].clone()
        } else {
            String::from("unknown")
        };

        Ok(InferenceResult {
            label,
            confidence: best_score,
            all_scores,
        })
    }

    /// List all registered models.
    pub fn list_models(&self) -> Vec<&str> {
        self.models.keys().map(|k| k.as_str()).collect()
    }
}

/// Initialize the inference engine with built-in models.
pub fn init() {
    let mut engine = InferenceEngine::new();
    engine.register_model(create_builtin_file_classifier());
    *ENGINE.lock() = Some(engine);
}

/// Classify file content using the built-in file classifier.
pub fn classify_file_content(data: &[u8]) -> Result<InferenceResult, &'static str> {
    let features = extract_file_features(data);
    let engine = ENGINE.lock();
    let eng = engine.as_ref().ok_or("AI engine not initialized")?;
    eng.infer("file-classifier", &features)
}

/// Extract a 32-element feature vector from file bytes.
///
/// Features:
///   [0..8]:   Byte class ratios (printable, whitespace, null, high, control, digit, lower, upper)
///   [8..16]:  Magic byte checks (ELF, PNG, ZIP, PDF, JPEG, SmartPack, shebang, XML)
///   [16..24]: Statistical features (entropy-like, mean, variance proxy, etc.)
///   [24..32]: Byte frequency peaks (most common byte ranges)
pub fn extract_file_features(data: &[u8]) -> Tensor {
    let len = data.len().max(1);
    let sample = &data[..len.min(512)]; // Use first 512 bytes

    // Byte frequency count
    let mut freq = [0u32; 256];
    for &b in sample {
        freq[b as usize] += 1;
    }
    let n = sample.len() as f32;

    // Byte class counts
    let mut printable = 0u32;
    let mut whitespace = 0u32;
    let mut null_bytes = 0u32;
    let mut high_bytes = 0u32;
    let mut control = 0u32;
    let mut digits = 0u32;
    let mut lowercase = 0u32;
    let mut uppercase = 0u32;

    for &b in sample {
        match b {
            0 => null_bytes += 1,
            1..=8 | 14..=31 => control += 1,
            9..=13 => whitespace += 1, // tab, LF, VT, FF, CR
            b'0'..=b'9' => { digits += 1; printable += 1; }
            b'a'..=b'z' => { lowercase += 1; printable += 1; }
            b'A'..=b'Z' => { uppercase += 1; printable += 1; }
            32..=126 => printable += 1,
            128..=255 => high_bytes += 1,
            _ => {}
        }
    }

    let mut features = vec![0i8; 32];

    // [0..8]: Byte class ratios → scaled to [-128, 127]
    let to_q = |ratio: f32| -> i8 { (ratio * 200.0 - 100.0).clamp(-128.0, 127.0) as i8 };
    features[0] = to_q(printable as f32 / n);
    features[1] = to_q(whitespace as f32 / n);
    features[2] = to_q(null_bytes as f32 / n);
    features[3] = to_q(high_bytes as f32 / n);
    features[4] = to_q(control as f32 / n);
    features[5] = to_q(digits as f32 / n);
    features[6] = to_q(lowercase as f32 / n);
    features[7] = to_q(uppercase as f32 / n);

    // [8..16]: Magic byte checks → 127 if match, -128 if not
    let magic_check = |pattern: &[u8]| -> i8 {
        if data.len() >= pattern.len() && &data[..pattern.len()] == pattern { 127 } else { -128 }
    };
    features[8] = magic_check(&[0x7F, b'E', b'L', b'F']);  // ELF
    features[9] = magic_check(&[0x89, b'P', b'N', b'G']);  // PNG
    features[10] = magic_check(&[b'P', b'K', 0x03, 0x04]); // ZIP
    features[11] = magic_check(&[b'%', b'P', b'D', b'F']); // PDF
    features[12] = if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8 { 127 } else { -128 }; // JPEG
    features[13] = magic_check(&[0x53, 0x50]); // SmartPack "SP"
    features[14] = magic_check(&[b'#', b'!']); // Shebang
    features[15] = if data.len() >= 1 && (data[0] == b'<' || data[0] == b'{') { 127 } else { -128 }; // XML/JSON

    // [16..24]: Statistical features
    // Entropy estimate (using byte frequency)
    let mut entropy_sum: f32 = 0.0;
    for &f in freq.iter() {
        if f > 0 {
            let p = f as f32 / n;
            entropy_sum -= p * approx_log2(p);
        }
    }
    features[16] = ((entropy_sum / 8.0) * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;

    // Mean byte value
    let mean: f32 = sample.iter().map(|&b| b as f32).sum::<f32>() / n;
    features[17] = ((mean / 255.0) * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;

    // Longest printable ASCII run
    let mut max_ascii_run = 0u32;
    let mut current_run = 0u32;
    for &b in sample {
        if b >= 32 && b < 127 {
            current_run += 1;
            max_ascii_run = max_ascii_run.max(current_run);
        } else {
            current_run = 0;
        }
    }
    let ascii_run_ratio = (max_ascii_run as f32 / n).min(1.0);
    features[18] = (ascii_run_ratio * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;

    // Longest null run
    let mut max_null_run = 0u32;
    current_run = 0;
    for &b in sample {
        if b == 0 {
            current_run += 1;
            max_null_run = max_null_run.max(current_run);
        } else {
            current_run = 0;
        }
    }
    features[19] = ((max_null_run.min(128) as f32 / 128.0) * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;

    // Unique byte count
    let unique = freq.iter().filter(|&&f| f > 0).count();
    features[20] = ((unique as f32 / 256.0) * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;

    // File size bucket (log2 scale)
    let size_log = approx_log2(data.len().max(1) as f32);
    features[21] = ((size_log / 20.0) * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;

    // First byte features
    features[22] = if data.is_empty() { 0 } else { data[0] as i8 };
    features[23] = if data.len() >= 2 { data[1] as i8 } else { 0 };

    // [24..32]: Byte frequency peaks (peaks in 32-byte ranges)
    for i in 0..8 {
        let range_start = i * 32;
        let range_end = range_start + 32;
        let peak: u32 = freq[range_start..range_end].iter().sum();
        let ratio = peak as f32 / n;
        features[24 + i] = (ratio * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;
    }

    Tensor::from_vec(features, 0.01, 0)
}

/// Approximate log2 for no_std.
fn approx_log2(x: f32) -> f32 {
    if x <= 0.0 { return 0.0; }
    // Use the trick: log2(x) ≈ exponent + mantissa approximation
    // For our purposes, a rough estimate is fine
    let bits = x.to_bits();
    let exp = ((bits >> 23) & 0xFF) as f32 - 127.0;
    let mantissa = f32::from_bits((bits & 0x007FFFFF) | 0x3F800000) - 1.0;
    exp + mantissa * (1.0 - mantissa * 0.333)
}

/// Create the built-in file classifier model.
///
/// Architecture: 32 → 16 → 8 (3 layers, ~696 parameters)
/// Categories: text, source, config, binary, image, archive, data, unknown
pub fn create_builtin_file_classifier() -> FeedForwardModel {
    let mut model = FeedForwardModel::new("file-classifier", 32);

    model.set_labels(vec![
        String::from("text"),
        String::from("source"),
        String::from("config"),
        String::from("binary"),
        String::from("image"),
        String::from("archive"),
        String::from("data"),
        String::from("unknown"),
    ]);

    // Layer 1: 32 → 16, ReLU
    // Hand-tuned weights: emphasize printable ratio for text, magic bytes for binary types
    #[rustfmt::skip]
    let w1_data: Vec<i8> = vec![
        // Neuron 0: high printable + whitespace → text-like
         40, 30, -20, -30, -20,  10,  20,  20,  -5,  -5, -5, -5, -5, -5,  10,  -5,  20, -10,  30, -20,  10,  0,  0,  0,  10,  5,  5, -5, -5, -5, -5, -5,
        // Neuron 1: high printable + lowercase + some structure → source code
         30, 10, -10, -20, -10,  15,  30,  10,  -5,  -5, -5, -5, -5, -5,  20,  -5,  15,  -5,  20, -15,  20,  5, 10,  5,   5,  5, 10, -5, -5, -5, -5, -5,
        // Neuron 2: config-like (text + special first byte)
         20, 10, -10, -20,  -5,  10,  15,  10,  -5,  -5, -5, -5, -5, -5,  -5,  30,  10,  -5,  15, -10,  15,  0,  5,  5,   5,  5,  5,  5, -5, -5, -5, -5,
        // Neuron 3: binary (high bytes, null bytes, low printable)
        -30, -10,  20,  30,  20, -10, -20, -10,  20,  10, 10, 10, 10, 10, -10, -10, -10,  15, -20,  20,  20,  5,  5,  5, -10, -5, -5, 10, 10, 10, 10,  5,
        // Neuron 4: image (magic byte PNG/JPEG)
        -10,  -5,  -5,  10,  -5,  -5,  -5,  -5,  -5,  40, -5, -5, 40, -5,  -5,  -5,  -5,  10, -10,  -5,  15,  5,  5,  5, -10, -5, -5, 10,  5,  5,  5,  5,
        // Neuron 5: archive (ZIP magic)
        -10,  -5,  -5,  10,  -5,  -5,  -5,  -5,  -5,  -5, 40, -5, -5, -5,  -5,  -5,  30,  10, -10,  -5,  10,  5,  5,  5, -10, -5, -5,  5, 10,  5,  5,  5,
        // Neuron 6: data (high entropy, many unique bytes)
        -10,  -5,  -5,  -5,   5,  15,  -5,  -5,  -5,  -5, -5, -5, -5, -5,  -5,  -5,  30,   5,  -5,  -5,  30, 10,  5,  5,  -5,  5,  5,  5,  5,  5,  5,  5,
        // Neuron 7: ELF binary
        -20,  -5,  -5,  20,  -5,  -5,  -5,  -5,  40,  -5, -5, -5, -5, -5,  -5,  -5,  -5,  10, -15,   5,  15,  5,  5,  5, -10, -5, -5,  5, 10,  5,  5,  5,
        // Neurons 8-15: mixed feature detectors
        20,  15, -10, -15, -15,   5,  15,  15,  -5,  -5, -5, -5, -5, -5,   5,  -5,  10,  -5,  15, -10,   5,  0,  0,  0,   5,  5,  5, -5, -5, -5, -5, -5,
        10,  20,  -5, -10, -10,   5,  10,  10,  -5,  -5, -5, -5, -5, -5,  10,  -5,   5,   0,  10,  -5,  10,  0,  0,  0,   0,  5,  5, -5, -5, -5, -5, -5,
        -5,  -5,  10,  20,  10,  -5, -10,  -5,  10,  10, 10, 10, 10,  5,  -5, -10,   5,  10, -10,  10,  15,  5,  5,  5,  -5, -5, -5,  5, 10,  5,  5,  5,
        15,   5,  -5, -10,  -5,  20,  10,   5,  -5,  -5, -5, -5, -5, -5,  -5,  10,   5,  -5,  10,  -5,  10,  0,  0,  0,   5,  5,  5, -5, -5, -5, -5, -5,
        -5,  -5,  -5,  -5,  -5,  -5,  -5,  -5,  -5,  -5, -5, -5, -5, -5,  -5,  -5,  20,   5,  -5,  -5,  20, 10,  5,  5,  -5,  5,  5,  5,  5,  5,  5,  5,
        25,  10, -15, -20, -15,  10,  20,  15,  -5,  -5, -5, -5, -5, -5,  15,  -5,  15, -10,  25, -15,  10,  0,  0,  0,   5,  5, 10, -5, -5, -5, -5, -5,
        -15,  -5,  15,  15,  15,  -5, -15,  -5,  15,   5,  5,  5,  5,  5, -10, -10, -10,  10, -15,  10,  10,  5,  5,  5, -10, -5, -5, 10,  5,  5,  5,  5,
         5,   5,   5,   5,   5,   5,   5,   5,  -5,  -5, -5, -5, -5, -5,  -5,  -5,   5,   0,   5,  -5,   5,  0,  0,  0,   5,  5,  5,  5,  5,  5,  5,  5,
    ];
    let w1 = Tensor::from_data(w1_data, 16, 32, 0.02, 0);
    let b1: Vec<i32> = vec![5, 5, 5, 5, 5, 5, 5, 5, 0, 0, 0, 0, 0, 0, 0, 0];
    let layer1 = DenseLayer::new(w1, b1, Activation::ReLU, 0.02, 0);

    // Layer 2: 16 → 8, None (softmax applied externally)
    #[rustfmt::skip]
    let w2_data: Vec<i8> = vec![
        // text: strong from text-like neurons (0, 8, 9, 13)
        40, 10,  5, -20, -10, -10,  -5, -10,  30,  20, -10,  10, -5,  30, -15,   5,
        // source: from code neurons (1, 11, 13)
        10, 40,  5, -10,  -5,  -5,  -5, -10,  15,  10, -10,  30, -5,  25, -10,   5,
        // config: from config neurons (2, 15)
         5, 10, 40, -10,  -5,  -5,  -5,  -5,   5,  10,  -5,  15, -5,   5,  -5,  30,
        // binary: from binary neurons (3, 7, 10, 14)
       -20,-10,-10,  40,  10,  10,   5,  30, -15, -10,  30,  -5,  5, -15,  25,  -5,
        // image: from image neuron (4)
       -10, -5, -5,  10,  40,  -5,  -5,  10, -10,  -5,  10,  -5,  5, -10,   5,  -5,
        // archive: from archive neuron (5)
       -10, -5, -5,  10,  -5,  40,  -5,  -5, -10,  -5,  10,  -5, 10, -10,   5,  -5,
        // data: from data neurons (6, 12)
        -5, -5, -5,   5,  -5,  -5,  40,  -5,  -5,  -5,   5, -10, 30,  -5,   5,  10,
        // unknown: mild from everywhere (fallback)
         5,  5,  5,   5,   5,   5,   5,   5,   5,   5,   5,   5,  5,   5,   5,   5,
    ];
    let w2 = Tensor::from_data(w2_data, 8, 16, 0.02, 0);
    let b2: Vec<i32> = vec![0, 0, 0, 0, 0, 0, 0, -10]; // Slight bias against "unknown"
    let layer2 = DenseLayer::new(w2, b2, Activation::None, 0.02, 0);

    model.add_layer(layer1);
    model.add_layer(layer2);

    model
}

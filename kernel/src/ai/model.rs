/// Neural network model definition and SmartPack serialization.
///
/// Models are sequential feed-forward networks with dense (fully-connected) layers.
/// All weights are INT8 quantized for minimal memory usage.

use alloc::string::String;
use alloc::vec::Vec;
use super::tensor::Tensor;

/// Activation function type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    None,
    ReLU,
    Sigmoid,
    Softmax,
}

impl Activation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Activation::None => "none",
            Activation::ReLU => "relu",
            Activation::Sigmoid => "sigmoid",
            Activation::Softmax => "softmax",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "relu" => Activation::ReLU,
            "sigmoid" => Activation::Sigmoid,
            "softmax" => Activation::Softmax,
            _ => Activation::None,
        }
    }
}

/// A single dense (fully-connected) layer.
#[derive(Debug, Clone)]
pub struct DenseLayer {
    /// Weight matrix [output_size x input_size], INT8 quantized.
    pub weights: Tensor,
    /// Bias vector [output_size], INT32 (pre-scaled to accumulator domain).
    pub bias: Vec<i32>,
    /// Activation function applied after this layer.
    pub activation: Activation,
    /// Output quantization scale.
    pub output_scale: f32,
    /// Output quantization zero point.
    pub output_zero_point: i8,
}

impl DenseLayer {
    /// Create a new dense layer with given weights and bias.
    pub fn new(
        weights: Tensor,
        bias: Vec<i32>,
        activation: Activation,
        output_scale: f32,
        output_zero_point: i8,
    ) -> Self {
        Self { weights, bias, activation, output_scale, output_zero_point }
    }

    /// Number of input features.
    pub fn input_size(&self) -> usize {
        self.weights.cols
    }

    /// Number of output features.
    pub fn output_size(&self) -> usize {
        self.weights.rows
    }

    /// Number of parameters (weights + biases).
    pub fn param_count(&self) -> usize {
        self.weights.numel() + self.bias.len()
    }
}

/// A feed-forward neural network model (sequential dense layers).
#[derive(Debug, Clone)]
pub struct FeedForwardModel {
    /// Model name.
    pub name: String,
    /// Sequential layers.
    pub layers: Vec<DenseLayer>,
    /// Input feature size.
    pub input_size: usize,
    /// Output class labels.
    pub labels: Vec<String>,
}

impl FeedForwardModel {
    /// Create a new empty model.
    pub fn new(name: &str, input_size: usize) -> Self {
        Self {
            name: String::from(name),
            layers: Vec::new(),
            input_size,
            labels: Vec::new(),
        }
    }

    /// Add a layer to the model.
    pub fn add_layer(&mut self, layer: DenseLayer) {
        self.layers.push(layer);
    }

    /// Set output class labels.
    pub fn set_labels(&mut self, labels: Vec<String>) {
        self.labels = labels;
    }

    /// Get the output size (number of classes).
    pub fn output_size(&self) -> usize {
        self.layers.last().map(|l| l.output_size()).unwrap_or(0)
    }

    /// Total parameter count across all layers.
    pub fn param_count(&self) -> usize {
        self.layers.iter().map(|l| l.param_count()).sum()
    }

    /// Serialize model metadata to SmartPack Value (excluding weights for display).
    pub fn to_smartpack_info(&self) -> smartpack::Value {
        use smartpack::Value;

        let mut map = Vec::new();
        map.push((
            Value::String(String::from("name")),
            Value::String(self.name.clone()),
        ));
        map.push((
            Value::String(String::from("input_size")),
            Value::UInt32(self.input_size as u32),
        ));
        map.push((
            Value::String(String::from("output_size")),
            Value::UInt32(self.output_size() as u32),
        ));
        map.push((
            Value::String(String::from("layers")),
            Value::UInt8(self.layers.len() as u8),
        ));
        map.push((
            Value::String(String::from("params")),
            Value::UInt32(self.param_count() as u32),
        ));

        let label_values: Vec<Value> = self.labels.iter()
            .map(|l| Value::String(l.clone()))
            .collect();
        map.push((
            Value::String(String::from("labels")),
            Value::Array(label_values),
        ));

        Value::Map(map)
    }
}

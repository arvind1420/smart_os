/// AI Inference Engine for Smart OS.
///
/// Pure Rust, no_std, INT8 quantized feed-forward neural network inference.
/// Models are stored in SmartPack format. Used for file classification,
/// content-aware features, and future smart OS capabilities.

pub mod tensor;
pub mod model;
pub mod inference;
pub mod npu;
pub mod predictor;
pub mod prefetch;

/// Initialize the AI inference engine, NPU, and predictive models.
pub fn init() {
    inference::init();
    npu::init();
    predictor::init();
    crate::serial_println!("[ai] AI inference engine initialized (NPU: {}).", npu::npu_name());
}

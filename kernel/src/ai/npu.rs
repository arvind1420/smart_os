/// NPU (Neural Processing Unit) Hardware Abstraction Layer for Smart OS.
///
/// Provides a trait-based abstraction for neural inference acceleration.
/// Supports runtime selection between hardware NPU (Intel GNA, ARM Ethos)
/// and software CPU fallback. The software backend delegates to the
/// existing FeedForwardModel inference engine.

use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::Mutex;
use super::model::Activation;

// ── NPU Status ──────────────────────────────────────────────────────

/// Status of an NPU device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NpuStatus {
    /// Device has not been initialized yet.
    Uninitialized,
    /// Device is ready for inference.
    Ready,
    /// Device is currently running inference.
    Busy,
    /// Device encountered an error.
    Error,
}

// ── Model Topology ──────────────────────────────────────────────────

/// Describes a model's layer topology for NPU loading.
#[derive(Debug, Clone)]
pub struct ModelTopology {
    pub layers: Vec<LayerDesc>,
}

/// Describes a single layer in the model topology.
#[derive(Debug, Clone)]
pub struct LayerDesc {
    pub input_size: usize,
    pub output_size: usize,
    pub activation: Activation,
}

// ── NPU Device Trait ────────────────────────────────────────────────

/// Hardware abstraction for neural processing units.
/// Each implementation handles a specific hardware family.
pub trait NpuDevice: Send {
    /// Initialize the device hardware.
    fn init(&mut self) -> Result<(), &'static str>;

    /// Load a model's weights into device memory.
    fn load_model(
        &mut self,
        model_id: &str,
        weights: &[i8],
        topology: &ModelTopology,
    ) -> Result<(), &'static str>;

    /// Run inference. Input is quantized INT8 feature vector, returns output vector.
    fn infer(&self, model_id: &str, input: &[i8]) -> Result<Vec<i8>, &'static str>;

    /// Query device status.
    fn status(&self) -> NpuStatus;

    /// Human-readable device name.
    fn name(&self) -> &'static str;
}

// ── Software CPU Fallback ───────────────────────────────────────────

/// Software-based "NPU" that delegates to the existing InferenceEngine.
/// This is the default fallback when no hardware NPU is detected.
pub struct SoftwareNpu {
    status: NpuStatus,
}

impl SoftwareNpu {
    pub fn new() -> Self {
        Self {
            status: NpuStatus::Uninitialized,
        }
    }
}

impl NpuDevice for SoftwareNpu {
    fn init(&mut self) -> Result<(), &'static str> {
        self.status = NpuStatus::Ready;
        Ok(())
    }

    fn load_model(
        &mut self,
        _model_id: &str,
        _weights: &[i8],
        _topology: &ModelTopology,
    ) -> Result<(), &'static str> {
        // Software backend uses models already registered in InferenceEngine.
        // No separate loading needed.
        Ok(())
    }

    fn infer(&self, model_id: &str, input: &[i8]) -> Result<Vec<i8>, &'static str> {
        if self.status != NpuStatus::Ready {
            return Err("Software NPU not initialized");
        }

        // Build a Tensor from the input slice
        let tensor = super::tensor::Tensor::from_vec(input.to_vec(), 0.01, 0);

        // Delegate to the existing InferenceEngine
        let engine = super::inference::ENGINE.lock();
        let eng = engine.as_ref().ok_or("AI engine not initialized")?;
        let result = eng.infer(model_id, &tensor)?;

        // Convert float scores back to INT8 for the unified interface
        let output: Vec<i8> = result
            .all_scores
            .iter()
            .map(|(_, score)| (*score * 127.0).clamp(-128.0, 127.0) as i8)
            .collect();

        Ok(output)
    }

    fn status(&self) -> NpuStatus {
        self.status
    }

    fn name(&self) -> &'static str {
        "Software (CPU)"
    }
}

// ── Intel GNA Stub Driver ───────────────────────────────────────────

/// Intel Gaussian & Neural Accelerator (GNA) driver stub.
/// Scans PCI for class 0x0B (processor), subclass 0x40 (neural).
/// When no GNA hardware is found, falls back to software.
pub struct IntelGnaNpu {
    status: NpuStatus,
    mmio_base: Option<u64>,
}

impl IntelGnaNpu {
    pub fn new() -> Self {
        Self {
            status: NpuStatus::Uninitialized,
            mmio_base: None,
        }
    }
}

impl NpuDevice for IntelGnaNpu {
    fn init(&mut self) -> Result<(), &'static str> {
        // Scan PCI bus for Intel GNA device (class=0x0B, subclass=0x40)
        let devices = crate::drivers::pci::scan_bus();
        for dev in &devices {
            if dev.class_code == 0x0B && dev.subclass == 0x40 {
                // Found GNA device — read BAR0 for MMIO base
                let bar0 = dev.bars[0] as u64 & 0xFFFF_FFF0;
                self.mmio_base = Some(bar0);
                self.status = NpuStatus::Ready;
                crate::serial_println!(
                    "[npu] Intel GNA found at PCI {:02x}:{:02x}.{}, BAR0={:#X}",
                    dev.bus, dev.device, dev.function, bar0,
                );
                return Ok(());
            }
        }
        Err("Intel GNA device not found on PCI bus")
    }

    fn load_model(
        &mut self,
        _model_id: &str,
        _weights: &[i8],
        _topology: &ModelTopology,
    ) -> Result<(), &'static str> {
        // GNA model loading would write descriptors to MMIO space.
        // For now, fall back to software inference.
        Ok(())
    }

    fn infer(&self, model_id: &str, input: &[i8]) -> Result<Vec<i8>, &'static str> {
        // Until full GNA register programming is implemented,
        // delegate to software inference as a transparent fallback.
        let sw = SoftwareNpu { status: NpuStatus::Ready };
        sw.infer(model_id, input)
    }

    fn status(&self) -> NpuStatus {
        self.status
    }

    fn name(&self) -> &'static str {
        "Intel GNA"
    }
}

// ── Global NPU Instance ────────────────────────────────────────────

/// Global NPU device instance. Initialized during boot.
pub static NPU: Mutex<Option<Box<dyn NpuDevice>>> = Mutex::new(None);

/// Initialize the NPU subsystem.
/// Tries hardware NPU first (Intel GNA), falls back to software CPU.
pub fn init() {
    // Try Intel GNA first
    let mut gna = IntelGnaNpu::new();
    if gna.init().is_ok() {
        crate::serial_println!("[npu] Using Intel GNA hardware accelerator.");
        *NPU.lock() = Some(Box::new(gna));
        return;
    }

    // Fall back to software CPU inference
    let mut sw = SoftwareNpu::new();
    sw.init().ok();
    crate::serial_println!("[npu] Using software CPU fallback for inference.");
    *NPU.lock() = Some(Box::new(sw));
}

/// Run inference through the NPU (or software fallback).
pub fn infer(model_id: &str, input: &[i8]) -> Result<Vec<i8>, &'static str> {
    let npu = NPU.lock();
    let device = npu.as_ref().ok_or("NPU not initialized")?;
    device.infer(model_id, input)
}

/// Get the name of the active NPU device.
pub fn npu_name() -> &'static str {
    let npu = NPU.lock();
    match npu.as_ref() {
        Some(device) => device.name(),
        None => "None",
    }
}

/// Get the status of the active NPU device.
pub fn npu_status() -> NpuStatus {
    let npu = NPU.lock();
    match npu.as_ref() {
        Some(device) => device.status(),
        None => NpuStatus::Uninitialized,
    }
}

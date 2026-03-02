/// Tensor operations for INT8 quantized neural network inference.
///
/// All computation uses INT8 weights/activations with INT32 accumulators.
/// Quantization: real_value = (int8_value - zero_point) * scale

use alloc::vec;
use alloc::vec::Vec;

/// A 2D tensor of INT8 quantized values.
#[derive(Debug, Clone)]
pub struct Tensor {
    /// INT8 quantized data (row-major).
    pub data: Vec<i8>,
    /// Shape: [rows, cols].
    pub rows: usize,
    pub cols: usize,
    /// Quantization scale: real = (q - zero_point) * scale.
    pub scale: f32,
    /// Quantization zero point.
    pub zero_point: i8,
}

/// A 2D tensor of INT32 accumulators (intermediate results).
#[derive(Debug, Clone)]
pub struct AccTensor {
    pub data: Vec<i32>,
    pub rows: usize,
    pub cols: usize,
}

impl Tensor {
    /// Create a zero-filled tensor.
    pub fn zeros(rows: usize, cols: usize, scale: f32, zero_point: i8) -> Self {
        Self {
            data: vec![zero_point; rows * cols],
            rows,
            cols,
            scale,
            zero_point,
        }
    }

    /// Create a tensor from existing data.
    pub fn from_data(data: Vec<i8>, rows: usize, cols: usize, scale: f32, zero_point: i8) -> Self {
        debug_assert_eq!(data.len(), rows * cols);
        Self { data, rows, cols, scale, zero_point }
    }

    /// Create a 1D tensor (single row).
    pub fn from_vec(data: Vec<i8>, scale: f32, zero_point: i8) -> Self {
        let cols = data.len();
        Self { data, rows: 1, cols, scale, zero_point }
    }

    /// Get element at (row, col).
    #[inline]
    pub fn get(&self, row: usize, col: usize) -> i8 {
        self.data[row * self.cols + col]
    }

    /// Set element at (row, col).
    #[inline]
    pub fn set(&mut self, row: usize, col: usize, val: i8) {
        self.data[row * self.cols + col] = val;
    }

    /// Total number of elements.
    pub fn numel(&self) -> usize {
        self.rows * self.cols
    }

    /// Dequantize a single value to f32.
    #[inline]
    pub fn dequantize_val(&self, q: i8) -> f32 {
        (q as f32 - self.zero_point as f32) * self.scale
    }

    /// Dequantize the entire tensor to f32 values.
    pub fn dequantize(&self) -> Vec<f32> {
        self.data.iter().map(|&q| self.dequantize_val(q)).collect()
    }
}

impl AccTensor {
    /// Create a zero-filled accumulator tensor.
    pub fn zeros(rows: usize, cols: usize) -> Self {
        Self {
            data: vec![0i32; rows * cols],
            rows,
            cols,
        }
    }

    /// Get element at (row, col).
    #[inline]
    pub fn get(&self, row: usize, col: usize) -> i32 {
        self.data[row * self.cols + col]
    }

    /// Set element at (row, col).
    #[inline]
    pub fn set(&mut self, row: usize, col: usize, val: i32) {
        self.data[row * self.cols + col] = val;
    }
}

/// INT8 matrix multiply: C[i][k] = sum_j(A[i][j] * B[j][k])
///
/// A: [M x N], B: [N x K] → C: [M x K] (accumulated in i32)
pub fn matmul_i8(a: &Tensor, b: &Tensor) -> AccTensor {
    debug_assert_eq!(a.cols, b.rows, "matmul dimension mismatch");
    let m = a.rows;
    let n = a.cols;
    let k = b.cols;

    let mut c = AccTensor::zeros(m, k);

    for i in 0..m {
        for j in 0..n {
            let a_val = a.get(i, j) as i32;
            for kk in 0..k {
                let b_val = b.get(j, kk) as i32;
                let idx = i * k + kk;
                c.data[idx] += a_val * b_val;
            }
        }
    }

    c
}

/// Add bias vector to each row of an accumulator tensor.
/// bias.len() must equal acc.cols.
pub fn add_bias_i32(acc: &mut AccTensor, bias: &[i32]) {
    debug_assert_eq!(bias.len(), acc.cols);
    for i in 0..acc.rows {
        for j in 0..acc.cols {
            let idx = i * acc.cols + j;
            acc.data[idx] += bias[j];
        }
    }
}

/// Requantize INT32 accumulators back to INT8.
///
/// output_q = clamp(round(acc * combined_scale) + output_zp, -128, 127)
/// where combined_scale = (input_scale * weight_scale) / output_scale
pub fn requantize(acc: &AccTensor, combined_scale: f32, output_zp: i8, output_scale: f32) -> Tensor {
    let mut data = vec![0i8; acc.data.len()];

    for (i, &val) in acc.data.iter().enumerate() {
        let scaled = val as f32 * combined_scale;
        let q = (scaled + output_zp as f32).clamp(-128.0, 127.0) as i8;
        data[i] = q;
    }

    Tensor::from_data(data, acc.rows, acc.cols, output_scale, output_zp)
}

/// ReLU activation: clamp values below zero_point to zero_point.
pub fn relu_i8(t: &mut Tensor) {
    let zp = t.zero_point;
    for val in t.data.iter_mut() {
        if *val < zp {
            *val = zp;
        }
    }
}

/// Sigmoid approximation using a lookup table.
/// Maps INT8 input [-128, 127] → INT8 output [0, 127] (sigmoid is 0..1).
pub fn sigmoid_i8(t: &Tensor) -> Tensor {
    let mut out = t.clone();
    out.scale = 1.0 / 127.0; // Output range 0..1 mapped to 0..127
    out.zero_point = 0;

    for val in out.data.iter_mut() {
        *val = SIGMOID_LUT[(*val as i16 + 128) as usize];
    }
    out
}

/// Precomputed sigmoid lookup table for INT8 inputs.
/// sigmoid_lut[i+128] = round(sigmoid(dequant(i)) * 127)
/// Using a generic scale of ~0.05 so input range covers -6.4 to +6.35.
static SIGMOID_LUT: [i8; 256] = {
    let mut lut = [0i8; 256];
    let scale = 0.05_f32;
    let mut i: i16 = -128;
    while i < 128 {
        // Approximate sigmoid: 1 / (1 + exp(-x))
        // Using piecewise linear approximation since const fn can't do exp:
        let x = i as f32 * scale;
        let sig = if x < -4.0 {
            0.0
        } else if x < -1.0 {
            // Linear ramp from 0 to 0.27 in range -4 to -1
            0.27 * (x + 4.0) / 3.0
        } else if x < 1.0 {
            // Linear ramp from 0.27 to 0.73 in range -1 to 1
            0.27 + 0.46 * (x + 1.0) / 2.0
        } else if x < 4.0 {
            // Linear ramp from 0.73 to 1.0 in range 1 to 4
            0.73 + 0.27 * (x - 1.0) / 3.0
        } else {
            1.0
        };
        let q = (sig * 127.0) as i8;
        lut[(i + 128) as usize] = q;
        i += 1;
    }
    lut
};

/// Softmax: dequantize to f32, compute softmax probabilities.
/// Only used on the final output vector (typically small, <64 elements).
pub fn softmax_f32(t: &Tensor) -> Vec<f32> {
    let values = t.dequantize();
    let max_val = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

    // exp(x - max) for numerical stability
    let mut exps: Vec<f32> = values.iter().map(|&x| approx_exp(x - max_val)).collect();
    let sum: f32 = exps.iter().sum();

    if sum > 0.0 {
        for v in exps.iter_mut() {
            *v /= sum;
        }
    }
    exps
}

/// Fast exponential approximation using a 6th-degree polynomial.
/// Good enough for softmax in no_std.
fn approx_exp(x: f32) -> f32 {
    // Clamp to prevent overflow
    let x = x.clamp(-20.0, 20.0);

    // exp(x) ≈ (1 + x/n)^n approach using repeated squaring
    // We use the identity: exp(x) = exp(x/8)^8
    let x8 = x / 8.0;
    let mut r = 1.0 + x8 + x8 * x8 * 0.5 + x8 * x8 * x8 / 6.0;
    r = r * r; // ^2
    r = r * r; // ^4
    r = r * r; // ^8
    if r < 0.0 { 0.0 } else { r }
}

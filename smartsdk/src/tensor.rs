/// Tensor API for Smart OS (GPU-Accelerated AI Workspace)
///
/// Phase 18: Exposes the Ring-0 NPU and iGPU compute capabilities directly
/// to user-space applications for running local models (like LLMs) 
/// securely and without external dependencies.

use crate::syscall::{syscall3, syscall4};
use alloc::vec::Vec;

pub const SYS_TENSOR_CREATE: u64 = 80;
pub const SYS_TENSOR_OP: u64 = 81;
pub const SYS_TENSOR_DESTROY: u64 = 82;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TensorOp {
    MatMul = 1,
    Relu = 2,
    Softmax = 3,
}

pub struct Tensor {
    handle: u64,
    pub shape: [usize; 4], // NCHW
}

impl Tensor {
    /// Creates a new tensor in the kernel's AI engine memory.
    pub fn new(shape: [usize; 4], data: &[f32]) -> Result<Self, &'static str> {
        // We pack the shape into a 64-bit value for the syscall MVP
        let shape_packed = ((shape[0] as u64) << 48) | ((shape[1] as u64) << 32) | ((shape[2] as u64) << 16) | (shape[3] as u64);
        
        let handle = syscall3(SYS_TENSOR_CREATE, shape_packed, data.as_ptr() as u64, (data.len() * 4) as u64);
        if handle == u64::MAX {
            Err("Failed to allocate tensor on device")
        } else {
            Ok(Self { handle, shape })
        }
    }

    /// Performs an operation on the device, returning a new Tensor.
    pub fn operate(&self, op: TensorOp, other: Option<&Tensor>) -> Result<Tensor, &'static str> {
        let other_handle = other.map(|t| t.handle).unwrap_or(0);
        let res_handle = syscall4(SYS_TENSOR_OP, self.handle, other_handle, op as u64, 0);
        
        if res_handle == u64::MAX {
            Err("Tensor operation failed")
        } else {
            // For MVP, assume shape doesn't change or calculate it properly.
            Ok(Tensor { handle: res_handle, shape: self.shape })
        }
    }

    /// Reads the tensor data back to the CPU.
    pub fn read(&self) -> Result<Vec<f32>, &'static str> {
        let size = self.shape[0] * self.shape[1] * self.shape[2] * self.shape[3];
        let mut buf = alloc::vec![0.0f32; size];
        
        // Overloading TENSOR_OP with 0 as a read command for MVP
        let res = syscall4(SYS_TENSOR_OP, self.handle, 0, 0, buf.as_mut_ptr() as u64);
        if res == 0 {
            Ok(buf)
        } else {
            Err("Failed to read tensor")
        }
    }
}

impl Drop for Tensor {
    fn drop(&mut self) {
        syscall3(SYS_TENSOR_DESTROY, self.handle, 0, 0);
    }
}

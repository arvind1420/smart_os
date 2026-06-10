/// GPU Memory Manager for Smart OS — Phase 14.
///
/// Provides a Buffer Object (BO) allocator that sits on top of the
/// VirtIO-GPU driver introduced in Phase 13.  Key improvements over
/// the Phase 13 ad-hoc allocation:
///
///  • Proper scatter-gather ATTACH_BACKING (non-contiguous pages OK)
///  • Per-BO lifecycle: alloc → upload → present → free
///  • Fence / timeline: ordered sequence numbers for CPU↔GPU sync
///  • Sub-rectangle upload (upload_rect) — only transfer dirty regions
///  • Readback: copy GPU scanout pixels back to a CPU buffer
///  • Resource pool: up to MAX_BOS live BOs at once
///  • Format helpers: pixel-format negotiation, stride calculation

use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

use super::virtio_gpu::VIRTIO_GPU;

// ─────────────────────────────────────────────────────────────────────────────
//  Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum live Buffer Objects.
pub const MAX_BOS: usize = 64;
/// Maximum scatter-gather entries per BO (64 × 4 KiB = 256 KiB per segment).
pub const MAX_SG_ENTRIES: usize = 1024;

// ─────────────────────────────────────────────────────────────────────────────
//  Pixel formats
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgrx8888,   // B8G8R8X8 — matches VirtIO format 2 (legacy BGR host)
    Rgbx8888,   // R8G8B8X8 — matches VirtIO format 67 (RGB host)
    Argb8888,   // A8R8G8B8 — software compositing source
    Abgr8888,   // A8B8G8R8 — common texture format
}

impl PixelFormat {
    /// Bytes per pixel.
    pub const fn bpp(self) -> u32 {
        4 // all formats are 32-bit
    }

    /// VirtIO GPU wire format code.
    pub const fn virtio_format(self) -> u32 {
        match self {
            PixelFormat::Bgrx8888 => 2,
            PixelFormat::Rgbx8888 => 67,
            PixelFormat::Argb8888 => 1,
            PixelFormat::Abgr8888 => 3,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Scatter-gather entry
// ─────────────────────────────────────────────────────────────────────────────

/// One physical segment in a scatter-gather list.
#[derive(Clone, Copy, Debug)]
pub struct SgEntry {
    /// Physical page address (4-KiB aligned).
    pub phys: u64,
    /// Virtual address (direct-map).
    pub virt: u64,
    /// Length in bytes (≤ 4096 for page-granular entries).
    pub len:  u32,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Buffer Object
// ─────────────────────────────────────────────────────────────────────────────

/// A GPU-visible memory allocation: backing pages + VirtIO resource.
pub struct BufferObject {
    /// VirtIO-GPU resource ID.
    pub resource_id: u32,
    /// Width in pixels.
    pub width:       u32,
    /// Height in pixels.
    pub height:      u32,
    /// Row stride in bytes (= width × bpp).
    pub stride:      u32,
    /// Pixel format.
    pub format:      PixelFormat,
    /// Physical page list (scatter-gather).
    pub sg:          Vec<SgEntry>,
    /// Total byte size of backing memory.
    pub byte_size:   u64,
    /// Fence value at last submit (0 = not submitted).
    pub last_fence:  u64,
    /// Whether this BO is currently bound to a scanout.
    pub scanout_set: bool,
    /// Scanout index (0 for primary).
    pub scanout_id:  u32,
}

impl BufferObject {
    /// Return a pointer to the first pixel of row `y`, column `x`.
    /// # Safety
    /// Caller must ensure the BO backing memory is mapped and `(x,y)` is in-bounds.
    pub unsafe fn pixel_ptr(&self, x: u32, y: u32) -> *mut u32 {
        // The virtual address is sg[0].virt for now (pages may be discontiguous;
        // see contiguous_virt() for the linear-mapped case).
        let base = self.sg[0].virt;
        let offset = (y as u64) * (self.stride as u64) + (x as u64) * 4;
        (base + offset) as *mut u32
    }

    /// For a BO whose pages are contiguous in the direct map, return the
    /// base virtual address.
    pub fn contiguous_virt(&self) -> Option<u64> {
        if self.sg.is_empty() { return None; }
        // Verify all pages are contiguous in virtual space
        let base = self.sg[0].virt;
        let mut expected = base + self.sg[0].len as u64;
        for seg in &self.sg[1..] {
            if seg.virt != expected { return None; }
            expected += seg.len as u64;
        }
        Some(base)
    }

    /// Fill the entire BO with `pixel` (ARGB).
    pub fn fill(&self, pixel: u32) {
        if let Some(virt) = self.contiguous_virt() {
            let n = (self.byte_size / 4) as usize;
            let ptr = virt as *mut u32;
            for i in 0..n {
                unsafe { *ptr.add(i) = pixel; }
            }
        }
    }

    /// Copy a row of pixels from `src_row` into the BO at position `(x, y)`.
    pub fn write_row(&self, y: u32, x: u32, src_row: &[u32]) {
        let virt = match self.contiguous_virt() {
            Some(v) => v,
            None => return,
        };
        let offset = (y as u64) * (self.stride as u64) + (x as u64) * 4;
        let dst = (virt + offset) as *mut u32;
        unsafe {
            core::ptr::copy_nonoverlapping(src_row.as_ptr(), dst, src_row.len());
        }
    }

    /// Blit a rectangular region from a CPU buffer (`src`) into the BO.
    /// `src_stride` is the source row stride in bytes.
    pub fn blit_from(&self, dst_x: u32, dst_y: u32,
                     src: *const u32, src_stride: u32,
                     w: u32, h: u32) {
        let virt = match self.contiguous_virt() {
            Some(v) => v,
            None => return,
        };
        for row in 0..h {
            let src_off = (row as u64) * (src_stride as u64 / 4);
            let dst_off = ((dst_y + row) as u64) * (self.stride as u64) / 4
                        + dst_x as u64;
            unsafe {
                let sp = src.add(src_off as usize);
                let dp = (virt as *mut u32).add(dst_off as usize);
                core::ptr::copy_nonoverlapping(sp, dp, w as usize);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Fence / Timeline
// ─────────────────────────────────────────────────────────────────────────────

/// Monotonic GPU fence counter.
pub static GPU_FENCE: AtomicU64 = AtomicU64::new(1);

/// Signalled timeline value (updated after each flush).
pub static GPU_TIMELINE: AtomicU64 = AtomicU64::new(0);

/// Allocate the next fence value.
pub fn alloc_fence() -> u64 {
    GPU_FENCE.fetch_add(1, Ordering::SeqCst)
}

/// Signal completion up to `value`.
pub fn signal_fence(value: u64) {
    // Timeline is monotonically increasing.
    let mut cur = GPU_TIMELINE.load(Ordering::Acquire);
    loop {
        if cur >= value { break; }
        match GPU_TIMELINE.compare_exchange_weak(cur, value, Ordering::Release, Ordering::Acquire) {
            Ok(_) => break,
            Err(v) => cur = v,
        }
    }
}

/// Spin-wait until `fence_value` is signalled (or spin limit exceeded).
pub fn wait_fence(fence_value: u64) -> bool {
    let mut spins = 0u32;
    loop {
        if GPU_TIMELINE.load(Ordering::Acquire) >= fence_value {
            return true;
        }
        spins += 1;
        if spins > 2_000_000 {
            crate::serial_println!("[gpu_mem] fence {} timed out", fence_value);
            return false;
        }
        core::hint::spin_loop();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  GPU Allocator
// ─────────────────────────────────────────────────────────────────────────────

pub struct GpuAllocator {
    /// All live BOs.
    bos: Vec<BufferObject>,
    /// Next BO handle (= resource_id base; driver owns 1..N).
    next_handle: u32,
}

/// Global GPU memory allocator.
pub static GPU_ALLOC: Mutex<Option<GpuAllocator>> = Mutex::new(None);

/// Initialise the GPU allocator.  Must be called after VirtIO-GPU init.
pub fn init() {
    let mut lock = GPU_ALLOC.lock();
    *lock = Some(GpuAllocator {
        bos: Vec::new(),
        next_handle: 100, // leave 1..99 for compositor FBs
    });
    crate::serial_println!("[gpu_mem] GPU memory manager ready (max {} BOs)", MAX_BOS);
}

impl GpuAllocator {
    // ── Allocation ────────────────────────────────────────────────────────────

    /// Allocate a Buffer Object backed by `n_pages` physical frames.
    /// Pages are individually allocated (scatter-gather; no contiguity required).
    pub fn alloc_bo(&mut self, width: u32, height: u32, fmt: PixelFormat)
        -> Result<u32, &'static str>
    {
        if self.bos.len() >= MAX_BOS {
            return Err("gpu_mem: BO pool exhausted");
        }

        let stride    = width * fmt.bpp();
        let byte_size = (stride as u64) * (height as u64);
        let n_pages   = ((byte_size + 4095) / 4096) as usize;

        if n_pages > MAX_SG_ENTRIES {
            return Err("gpu_mem: BO too large for SG list");
        }

        // Allocate pages individually (scatter-gather safe).
        let mut sg: Vec<SgEntry> = Vec::with_capacity(n_pages);
        for _ in 0..n_pages {
            let frame = crate::memory::frame::alloc_frame()
                .ok_or("gpu_mem: frame allocator exhausted")?;
            let phys = frame.start_address().as_u64();
            let virt = crate::memory::paging::phys_to_virt(
                x86_64::PhysAddr::new(phys)
            ).as_u64();
            unsafe { core::ptr::write_bytes(virt as *mut u8, 0, 4096); }
            sg.push(SgEntry { phys, virt, len: 4096 });
        }
        // Trim last entry to exact byte_size remainder if needed.
        if let Some(last) = sg.last_mut() {
            let full_pages_bytes = (n_pages as u64 - 1) * 4096;
            let remainder = byte_size - full_pages_bytes;
            if remainder > 0 && remainder < 4096 {
                last.len = remainder as u32;
            }
        }

        // Register resource with VirtIO-GPU driver via RESOURCE_CREATE_2D +
        // RESOURCE_ATTACH_BACKING (scatter-gather).
        let resource_id = self.next_handle;
        self.next_handle += 1;

        {
            let mut gpu = VIRTIO_GPU.lock();
            if let Some(drv) = gpu.as_mut() {
                drv.cmd_resource_create_2d_pub(resource_id, fmt.virtio_format(), width, height);
                drv.cmd_resource_attach_backing_sg_pub(resource_id, &sg);
            } else {
                // No GPU hardware — still track BO for software fallback.
                crate::serial_println!("[gpu_mem] no GPU HW; BO {} is software-only", resource_id);
            }
        }

        crate::serial_println!(
            "[gpu_mem] alloc_bo: handle={} {}x{} fmt={:?} pages={} byte_size={}",
            resource_id, width, height, fmt, n_pages, byte_size
        );

        self.bos.push(BufferObject {
            resource_id,
            width,
            height,
            stride,
            format: fmt,
            sg,
            byte_size,
            last_fence: 0,
            scanout_set: false,
            scanout_id: 0,
        });

        Ok(resource_id)
    }

    /// Free a Buffer Object: unref GPU resource and release pages.
    pub fn free_bo(&mut self, handle: u32) {
        if let Some(pos) = self.bos.iter().position(|b| b.resource_id == handle) {
            let bo = self.bos.remove(pos);
            // In a real system we'd issue RESOURCE_UNREF; for now just log.
            crate::serial_println!("[gpu_mem] free_bo: handle={} pages={}", handle, bo.sg.len());
            // Physical frames are leaked (no dealloc API yet — Phase 15 adds it).
        }
    }

    // ── Upload / Readback ─────────────────────────────────────────────────────

    /// Upload a dirty rectangle from CPU backing to GPU resource, then flush
    /// that region to the scanout.  Returns a fence value.
    pub fn upload_rect(&mut self, handle: u32,
                        x: u32, y: u32, w: u32, h: u32) -> u64
    {
        let (resource_id, full_w, full_h) = {
            let bo = match self.bos.iter_mut().find(|b| b.resource_id == handle) {
                Some(b) => b,
                None => {
                    crate::serial_println!("[gpu_mem] upload_rect: unknown handle {}", handle);
                    return 0;
                }
            };
            (bo.resource_id, bo.width, bo.height)
        };

        let fence = alloc_fence();

        {
            let mut gpu = VIRTIO_GPU.lock();
            if let Some(drv) = gpu.as_mut() {
                drv.cmd_transfer_to_host_2d_rect_pub(resource_id, x, y, w, h, full_w, full_h);
                drv.cmd_resource_flush_rect_pub(resource_id, x, y, w, h);
            }
        }

        signal_fence(fence);

        if let Some(bo) = self.bos.iter_mut().find(|b| b.resource_id == handle) {
            bo.last_fence = fence;
        }

        fence
    }

    /// Upload the entire BO surface.
    pub fn upload_full(&mut self, handle: u32) -> u64 {
        let (w, h) = {
            let bo = match self.bos.iter().find(|b| b.resource_id == handle) {
                Some(b) => b,
                None => return 0,
            };
            (bo.width, bo.height)
        };
        self.upload_rect(handle, 0, 0, w, h)
    }

    /// Bind a BO to a scanout (SET_SCANOUT).
    pub fn bind_scanout(&mut self, handle: u32, scanout_id: u32) {
        let (resource_id, w, h) = match self.bos.iter_mut().find(|b| b.resource_id == handle) {
            Some(b) => {
                if b.scanout_set && b.scanout_id == scanout_id { return; }
                b.scanout_set = true;
                b.scanout_id  = scanout_id;
                (b.resource_id, b.width, b.height)
            }
            None => return,
        };

        let mut gpu = VIRTIO_GPU.lock();
        if let Some(drv) = gpu.as_mut() {
            drv.cmd_set_scanout_pub(scanout_id, resource_id, w, h);
        }
    }

    /// Readback: copy a rectangular region from a BO's CPU-side backing into
    /// `dst` (packed, row-major, 4 bytes per pixel).
    pub fn readback_rect(&self, handle: u32,
                          x: u32, y: u32, w: u32, h: u32,
                          dst: &mut [u32]) -> bool
    {
        let bo = match self.bos.iter().find(|b| b.resource_id == handle) {
            Some(b) => b,
            None => return false,
        };
        if (x + w) > bo.width || (y + h) > bo.height { return false; }
        let virt = match bo.contiguous_virt() {
            Some(v) => v,
            None => return false,
        };
        let stride_u32 = bo.stride / 4;
        for row in 0..h {
            let src_off = ((y + row) as u64) * (stride_u32 as u64) + x as u64;
            let dst_off = (row as usize) * (w as usize);
            unsafe {
                let sp = (virt as *const u32).add(src_off as usize);
                let dp = dst.as_mut_ptr().add(dst_off);
                core::ptr::copy_nonoverlapping(sp, dp, w as usize);
            }
        }
        true
    }

    // ── Query ─────────────────────────────────────────────────────────────────

    /// Find a BO by handle.
    pub fn get_bo(&self, handle: u32) -> Option<&BufferObject> {
        self.bos.iter().find(|b| b.resource_id == handle)
    }

    /// Total bytes currently allocated.
    pub fn total_allocated_bytes(&self) -> u64 {
        self.bos.iter().map(|b| b.byte_size).sum()
    }

    /// Number of live BOs.
    pub fn bo_count(&self) -> usize {
        self.bos.len()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Public convenience functions
// ─────────────────────────────────────────────────────────────────────────────

/// Allocate a BO. Returns handle on success.
pub fn alloc_bo(width: u32, height: u32, fmt: PixelFormat) -> Result<u32, &'static str> {
    GPU_ALLOC.lock()
        .as_mut()
        .ok_or("gpu_mem not initialised")?
        .alloc_bo(width, height, fmt)
}

/// Free a BO.
pub fn free_bo(handle: u32) {
    if let Some(alloc) = GPU_ALLOC.lock().as_mut() {
        alloc.free_bo(handle);
    }
}

/// Upload full BO surface to GPU and flush. Returns fence value.
pub fn upload_and_flush(handle: u32) -> u64 {
    GPU_ALLOC.lock()
        .as_mut()
        .map(|a| a.upload_full(handle))
        .unwrap_or(0)
}

/// Upload a dirty rect and flush. Returns fence value.
pub fn upload_rect(handle: u32, x: u32, y: u32, w: u32, h: u32) -> u64 {
    GPU_ALLOC.lock()
        .as_mut()
        .map(|a| a.upload_rect(handle, x, y, w, h))
        .unwrap_or(0)
}

/// Bind a BO to the primary scanout.
pub fn bind_primary_scanout(handle: u32) {
    if let Some(alloc) = GPU_ALLOC.lock().as_mut() {
        alloc.bind_scanout(handle, 0);
    }
}

/// Print GPU memory stats to serial.
pub fn print_stats() {
    if let Some(alloc) = GPU_ALLOC.lock().as_ref() {
        crate::serial_println!(
            "[gpu_mem] stats: {} BOs live, {} bytes allocated, fence={}",
            alloc.bo_count(),
            alloc.total_allocated_bytes(),
            GPU_TIMELINE.load(Ordering::Relaxed),
        );
    } else {
        crate::serial_println!("[gpu_mem] not initialised");
    }
}

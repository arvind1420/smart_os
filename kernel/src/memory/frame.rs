/// Bitmap-based physical frame allocator for Smart OS.
///
/// Tracks every 4 KiB physical page frame using a compact bitmap.
/// Each bit represents one frame: 1 = free, 0 = used.
/// For 128 MiB of RAM this needs only ~4 KB of bitmap.

use alloc::vec::Vec;
use spin::Mutex;
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};
use x86_64::PhysAddr;
use crate::serial_println;

const FRAME_SIZE: u64 = 4096;

/// Global frame allocator instance.
pub static FRAME_ALLOCATOR: Mutex<Option<BitmapFrameAllocator>> = Mutex::new(None);

/// Bitmap-based physical frame allocator.
pub struct BitmapFrameAllocator {
    /// Each bit represents one 4 KiB frame. 1 = free, 0 = used.
    bitmap: Vec<u64>,
    /// Physical address of the first frame tracked (always 0 for simplicity).
    base_addr: u64,
    /// Total number of frames tracked.
    total_frames: usize,
    /// Number of free frames remaining.
    free_count: usize,
}

impl BitmapFrameAllocator {
    /// Create from bootloader memory regions.
    ///
    /// Scans all usable regions, marks their frames as free in the bitmap.
    /// Frames below `reserved_end` are never marked free (covers kernel,
    /// bootloader structures, and the heap).
    pub fn new(
        memory_regions: &[bootloader_api::info::MemoryRegion],
        reserved_end: u64,
    ) -> Self {
        use bootloader_api::info::MemoryRegionKind;

        // Find the highest physical address to determine bitmap size.
        let max_addr = memory_regions
            .iter()
            .map(|r| r.end)
            .max()
            .unwrap_or(0);

        let total_frames = (max_addr / FRAME_SIZE) as usize;
        let bitmap_words = (total_frames + 63) / 64; // round up

        // Start with all bits 0 (all frames used/reserved).
        let mut bitmap = alloc::vec![0u64; bitmap_words];
        let mut free_count = 0usize;

        // Mark usable frames above reserved_end as free.
        for region in memory_regions {
            if region.kind != MemoryRegionKind::Usable {
                continue;
            }

            // Align region start up to frame boundary, end down.
            let start = (region.start + FRAME_SIZE - 1) & !(FRAME_SIZE - 1);
            let end = region.end & !(FRAME_SIZE - 1);

            if start >= end {
                continue;
            }

            let mut addr = start;
            while addr < end {
                if addr >= reserved_end {
                    let frame_idx = (addr / FRAME_SIZE) as usize;
                    let word = frame_idx / 64;
                    let bit = frame_idx % 64;
                    if word < bitmap.len() {
                        bitmap[word] |= 1u64 << bit;
                        free_count += 1;
                    }
                }
                addr += FRAME_SIZE;
            }
        }

        Self {
            bitmap,
            base_addr: 0,
            total_frames,
            free_count,
        }
    }

    /// Allocate a single physical frame.
    pub fn alloc_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        if self.free_count == 0 {
            return None;
        }

        // Scan bitmap for a word with at least one free bit.
        for (word_idx, word) in self.bitmap.iter_mut().enumerate() {
            if *word == 0 {
                continue; // No free bits in this word
            }

            // Find the first set bit (free frame).
            let bit = word.trailing_zeros() as usize;
            let frame_idx = word_idx * 64 + bit;

            if frame_idx >= self.total_frames {
                continue;
            }

            // Clear the bit (mark as used).
            *word &= !(1u64 << bit);
            self.free_count -= 1;

            let phys_addr = self.base_addr + (frame_idx as u64) * FRAME_SIZE;
            return Some(PhysFrame::containing_address(PhysAddr::new(phys_addr)));
        }

        None
    }

    /// Deallocate a physical frame (mark it as free again).
    pub fn dealloc_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        let addr = frame.start_address().as_u64();
        let frame_idx = ((addr - self.base_addr) / FRAME_SIZE) as usize;
        let word = frame_idx / 64;
        let bit = frame_idx % 64;

        if word < self.bitmap.len() {
            // Only increment free_count if the bit was previously 0 (used).
            if self.bitmap[word] & (1u64 << bit) == 0 {
                self.bitmap[word] |= 1u64 << bit;
                self.free_count += 1;
            }
        }
    }

    /// Number of free frames.
    pub fn free_count(&self) -> usize {
        self.free_count
    }

    /// Total tracked frames.
    pub fn total_count(&self) -> usize {
        self.total_frames
    }
}

/// Implement the x86_64 crate's FrameAllocator trait so we can pass it
/// to OffsetPageTable::map_to().
unsafe impl FrameAllocator<Size4KiB> for BitmapFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.alloc_frame()
    }
}

/// Initialize the frame allocator from bootloader memory regions.
///
/// `reserved_end` should be the physical address past the end of the
/// kernel heap — everything below is off-limits.
pub fn init(memory_regions: &[bootloader_api::info::MemoryRegion], reserved_end: u64) {
    let allocator = BitmapFrameAllocator::new(memory_regions, reserved_end);
    let free = allocator.free_count();
    let total = allocator.total_count();
    serial_println!(
        "[frame] Frame allocator: {} free / {} total frames ({} MiB usable)",
        free,
        total,
        (free as u64 * FRAME_SIZE) / (1024 * 1024),
    );
    *FRAME_ALLOCATOR.lock() = Some(allocator);
}

/// Convenience: allocate one physical frame (locks the global allocator).
pub fn alloc_frame() -> Option<PhysFrame<Size4KiB>> {
    FRAME_ALLOCATOR.lock().as_mut()?.alloc_frame()
}

/// Convenience: deallocate one physical frame.
pub fn dealloc_frame(frame: PhysFrame<Size4KiB>) {
    if let Some(alloc) = FRAME_ALLOCATOR.lock().as_mut() {
        alloc.dealloc_frame(frame);
    }
}

/// Get frame allocator stats: (free_count, total_count).
pub fn frame_stats() -> (usize, usize) {
    match FRAME_ALLOCATOR.lock().as_ref() {
        Some(alloc) => (alloc.free_count(), alloc.total_count()),
        None => (0, 0),
    }
}

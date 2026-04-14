/// Kernel heap allocator.
///
/// Provides a global allocator so the kernel can use `alloc` types
/// (Vec, String, Box, etc.).

use core::sync::atomic::{AtomicU64, Ordering};
use linked_list_allocator::LockedHeap;
use crate::serial_println;

/// Initial kernel heap size: 32 MiB.
const HEAP_SIZE: u64 = 32 * 1024 * 1024;

/// Physical end address of the heap region (set during init).
/// The frame allocator uses this to know which frames are reserved.
pub static HEAP_PHYS_END: AtomicU64 = AtomicU64::new(0);

/// Returns true once the heap allocator has been initialized.
#[inline]
pub fn is_heap_ready() -> bool {
    HEAP_PHYS_END.load(core::sync::atomic::Ordering::Relaxed) != 0
}

/// The global allocator used by `alloc::` types in the kernel.
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Initialize the kernel heap.
///
/// Takes the physical memory offset and finds a usable memory region
/// to place the heap in.
pub fn init_heap(phys_offset: u64, memory_regions: &[bootloader_api::info::MemoryRegion]) {
    use bootloader_api::info::MemoryRegionKind;

    let heap_region = memory_regions
        .iter()
        .filter(|r| r.kind == MemoryRegionKind::Usable)
        .find(|r| (r.end - r.start) >= HEAP_SIZE);

    if let Some(region) = heap_region {
        let heap_phys_start = region.start;
        let heap_virt_start = phys_offset + heap_phys_start;

        // Record the physical end of the heap so the frame allocator
        // knows not to hand out these frames.
        HEAP_PHYS_END.store(heap_phys_start + HEAP_SIZE, Ordering::Relaxed);

        serial_println!(
            "[heap] Initializing {} KiB heap at virt={:#X} (phys={:#X})",
            HEAP_SIZE / 1024,
            heap_virt_start,
            heap_phys_start
        );

        unsafe {
            ALLOCATOR
                .lock()
                .init(heap_virt_start as *mut u8, HEAP_SIZE as usize);
        }

        serial_println!("[heap] Allocator ready.");
    } else {
        serial_println!("[heap] ERROR: No usable memory region >= {} KiB found!", HEAP_SIZE / 1024);
        panic!("Cannot initialize kernel heap: no suitable memory region");
    }
}

/// Returns (used, free) heap bytes.
pub fn heap_stats() -> (usize, usize) {
    let allocator = ALLOCATOR.lock();
    (allocator.used(), allocator.free())
}

pub fn kmalloc(size: usize, align: usize) -> *mut u8 {
    let layout = core::alloc::Layout::from_size_align(size, align).unwrap();
    unsafe { alloc::alloc::alloc(layout) }
}

pub fn kfree(ptr: *mut u8, size: usize, align: usize) {
    let layout = core::alloc::Layout::from_size_align(size, align).unwrap();
    unsafe { alloc::alloc::dealloc(ptr, layout) }
}

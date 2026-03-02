/// Page Fault Handler + Swap Layer for Smart OS.
///
/// Phase 10: Implements disk-backed virtual memory swapping.
/// When physical memory runs low, evict least-recently-used pages to
/// the VirtIO-blk device. On page fault, load them back.
///
/// Uses a simple FIFO eviction policy with a swap table tracking
/// which virtual pages are swapped out and their disk locations.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use spin::Mutex;
use x86_64::structures::paging::{PhysFrame, Size4KiB, PageTableFlags, Page};
use x86_64::VirtAddr;
use crate::serial_println;

/// Swap slot: a sector range on the block device.
#[derive(Debug, Clone, Copy)]
pub struct SwapSlot {
    pub sector_start: u64,
    pub sector_count: u64, // 4096/512 = 8 sectors per page
}

const SECTORS_PER_PAGE: u64 = 8; // 4096 bytes / 512 bytes per sector

/// Swap entry: maps a (pid, virtual_page) → swap slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SwapKey {
    pid: u64,
    vpage: u64, // virtual page number (addr >> 12)
}

/// The swap table: tracks all swapped-out pages.
static SWAP_TABLE: Mutex<BTreeMap<SwapKey, SwapSlot>> = Mutex::new(BTreeMap::new());

/// Next available swap sector on disk.
static SWAP_SECTOR_NEXT: AtomicU64 = AtomicU64::new(1024); // Start at sector 1024 (past filesystem)

/// Free swap slots for reuse.
static SWAP_FREE_SLOTS: Mutex<Vec<SwapSlot>> = Mutex::new(Vec::new());

/// Whether swap is available (VirtIO-blk present).
static SWAP_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Total pages swapped out.
static SWAP_OUT_COUNT: AtomicU64 = AtomicU64::new(0);
/// Total pages swapped in.
static SWAP_IN_COUNT: AtomicU64 = AtomicU64::new(0);

/// Page fault statistics.
static PAGE_FAULT_COUNT: AtomicU64 = AtomicU64::new(0);
static COW_FAULT_COUNT: AtomicU64 = AtomicU64::new(0);
static SWAP_FAULT_COUNT: AtomicU64 = AtomicU64::new(0);

/// Initialize the swap subsystem.
pub fn init() {
    // Check if VirtIO-blk is available for swap
    if crate::drivers::virtio_blk::is_available() {
        SWAP_AVAILABLE.store(true, Ordering::Relaxed);
        serial_println!("[swap] Swap subsystem initialized (VirtIO-blk backed).");
    } else {
        serial_println!("[swap] Swap subsystem initialized (no swap device).");
    }
}

/// Allocate a swap slot (8 sectors for one 4KiB page).
fn alloc_swap_slot() -> Option<SwapSlot> {
    // Try to reuse a freed slot first
    if let Some(slot) = SWAP_FREE_SLOTS.lock().pop() {
        return Some(slot);
    }

    // Allocate new sectors
    let sector = SWAP_SECTOR_NEXT.fetch_add(SECTORS_PER_PAGE, Ordering::Relaxed);
    Some(SwapSlot {
        sector_start: sector,
        sector_count: SECTORS_PER_PAGE,
    })
}

/// Free a swap slot for reuse.
fn free_swap_slot(slot: SwapSlot) {
    SWAP_FREE_SLOTS.lock().push(slot);
}

/// Swap out a page to disk.
///
/// Writes the 4KiB page data to the swap device and unmaps it
/// from the page table.
pub fn swap_out(pid: u64, vaddr: u64, frame: PhysFrame<Size4KiB>) -> bool {
    if !SWAP_AVAILABLE.load(Ordering::Relaxed) {
        return false;
    }

    let slot = match alloc_swap_slot() {
        Some(s) => s,
        None => return false,
    };

    // Read page data from physical frame
    let phys_virt = super::paging::phys_to_virt(frame.start_address());
    let data: &[u8] = unsafe { core::slice::from_raw_parts(phys_virt.as_ptr(), 4096) };

    // Write to disk in 512-byte sectors
    let mut success = true;
    for i in 0..8u64 {
        let sector = slot.sector_start + i;
        let offset = (i as usize) * 512;
        let mut sector_buf = [0u8; 512];
        sector_buf.copy_from_slice(&data[offset..offset + 512]);
        if crate::drivers::virtio_blk::write_sector(sector, &sector_buf).is_err() {
            success = false;
            break;
        }
    }

    if !success {
        free_swap_slot(slot);
        return false;
    }

    // Record the swap entry
    let key = SwapKey { pid, vpage: vaddr >> 12 };
    SWAP_TABLE.lock().insert(key, slot);

    // Free the physical frame
    super::frame::dealloc_frame(frame);

    SWAP_OUT_COUNT.fetch_add(1, Ordering::Relaxed);
    true
}

/// Swap in a page from disk.
///
/// Reads the 4KiB page data from the swap device into a new physical frame.
/// Returns the new physical frame.
pub fn swap_in(pid: u64, vaddr: u64) -> Option<PhysFrame<Size4KiB>> {
    let key = SwapKey { pid, vpage: vaddr >> 12 };

    let slot = SWAP_TABLE.lock().remove(&key)?;

    // Allocate a new physical frame
    let frame = super::frame::alloc_frame()?;
    let phys_virt = super::paging::phys_to_virt(frame.start_address());
    let data: &mut [u8] = unsafe { core::slice::from_raw_parts_mut(phys_virt.as_mut_ptr(), 4096) };

    // Read from disk
    for i in 0..8u64 {
        let sector = slot.sector_start + i;
        let offset = (i as usize) * 512;
        let mut sector_buf = [0u8; 512];
        if crate::drivers::virtio_blk::read_sector(sector, &mut sector_buf).is_err() {
            super::frame::dealloc_frame(frame);
            free_swap_slot(slot);
            return None;
        }
        data[offset..offset + 512].copy_from_slice(&sector_buf);
    }

    // Return the swap slot for reuse
    free_swap_slot(slot);

    SWAP_IN_COUNT.fetch_add(1, Ordering::Relaxed);
    Some(frame)
}

/// Check if a page is swapped out.
pub fn is_swapped(pid: u64, vaddr: u64) -> bool {
    let key = SwapKey { pid, vpage: vaddr >> 12 };
    SWAP_TABLE.lock().contains_key(&key)
}

/// Handle a page fault — the unified entry point.
///
/// Tries CoW first, then swap-in. Returns true if handled.
pub fn handle_page_fault(fault_addr: u64, error_code: u64) -> bool {
    PAGE_FAULT_COUNT.fetch_add(1, Ordering::Relaxed);

    let vaddr = VirtAddr::new(fault_addr);
    let is_write = error_code & 0x2 != 0;
    let is_user = error_code & 0x4 != 0;
    let is_present = error_code & 0x1 != 0;

    // Get current process CR3
    let cr3 = x86_64::registers::control::Cr3::read().0;

    // 1. Try CoW fault handling (write to present read-only page)
    if is_write && is_present {
        if super::cow::handle_cow_fault(cr3, vaddr) {
            COW_FAULT_COUNT.fetch_add(1, Ordering::Relaxed);
            return true;
        }
    }

    // 2. Try swap-in (page not present, was swapped out)
    if !is_present && is_user {
        let pid = crate::process::scheduler::current_pid().unwrap_or(0);
        if is_swapped(pid, fault_addr) {
            SWAP_FAULT_COUNT.fetch_add(1, Ordering::Relaxed);
            if let Some(frame) = swap_in(pid, fault_addr) {
                let page = Page::<Size4KiB>::containing_address(vaddr);
                let flags = PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE;
                if super::paging::map_page(cr3, page, frame, flags).is_ok() {
                    serial_println!("[swap] Swapped in page at {:#X} for pid={}", fault_addr, pid);
                    return true;
                }
            }
        }
    }

    false
}

/// Get swap statistics.
pub fn swap_stats() -> (u64, u64, u64, u64, u64, bool) {
    (
        PAGE_FAULT_COUNT.load(Ordering::Relaxed),
        COW_FAULT_COUNT.load(Ordering::Relaxed),
        SWAP_FAULT_COUNT.load(Ordering::Relaxed),
        SWAP_OUT_COUNT.load(Ordering::Relaxed),
        SWAP_IN_COUNT.load(Ordering::Relaxed),
        SWAP_AVAILABLE.load(Ordering::Relaxed),
    )
}

/// Number of pages currently swapped out.
pub fn swapped_page_count() -> usize {
    SWAP_TABLE.lock().len()
}

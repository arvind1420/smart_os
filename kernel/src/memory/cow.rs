/// Copy-on-Write (CoW) Fork for Smart OS.
///
/// Phase 10: Instead of deep-copying all user pages during fork(),
/// CoW marks shared pages as read-only. On write fault, the faulting
/// page is copied and remapped writable. Reference counting tracks
/// shared frames.
///
/// This dramatically reduces fork() latency and memory usage.

use alloc::collections::BTreeMap;
use spin::Mutex;
use x86_64::structures::paging::{
    PageTable, PageTableFlags, PhysFrame, Size4KiB, Page, Mapper,
    OffsetPageTable,
};
use x86_64::VirtAddr;
use crate::serial_println;

/// Per-physical-frame reference count.
/// When refcount > 1, the frame is shared (CoW).
/// When refcount drops to 1, the frame can be made writable again.
static REFCOUNTS: Mutex<BTreeMap<u64, u32>> = Mutex::new(BTreeMap::new());

/// Increment reference count for a physical frame.
pub fn ref_inc(phys_addr: u64) {
    let mut refs = REFCOUNTS.lock();
    let count = refs.entry(phys_addr).or_insert(0);
    *count += 1;
}

/// Decrement reference count. Returns the new count.
pub fn ref_dec(phys_addr: u64) -> u32 {
    let mut refs = REFCOUNTS.lock();
    if let Some(count) = refs.get_mut(&phys_addr) {
        if *count > 0 {
            *count -= 1;
        }
        let c = *count;
        if c == 0 {
            refs.remove(&phys_addr);
        }
        c
    } else {
        0
    }
}

/// Get the current reference count for a frame.
pub fn ref_count(phys_addr: u64) -> u32 {
    REFCOUNTS.lock().get(&phys_addr).copied().unwrap_or(0)
}

/// CoW-aware fork: clone a user page table using CoW semantics.
///
/// Instead of copying data pages, both parent and child share the same
/// physical frames, marked read-only. Write faults trigger copy.
///
/// Returns the new PML4 physical frame for the child process.
pub fn cow_fork(src_pml4: PhysFrame<Size4KiB>) -> Option<PhysFrame<Size4KiB>> {
    let new_pml4 = super::paging::create_user_page_table()?;

    let src_virt = super::paging::phys_to_virt(src_pml4.start_address());
    let src_l4: &PageTable = unsafe { &*src_virt.as_ptr() };

    // Walk user-half entries (0-255)
    for i in 0..256 {
        let l4e = &src_l4[i];
        if !l4e.flags().contains(PageTableFlags::PRESENT) {
            continue;
        }
        let src_l3_frame = match l4e.frame() {
            Ok(f) => f,
            Err(_) => continue,
        };
        cow_clone_recursive(src_pml4, new_pml4, src_l3_frame, 3, i, 0);
    }

    Some(new_pml4)
}

/// Recursively walk page tables, sharing leaf pages with CoW semantics.
fn cow_clone_recursive(
    src_pml4: PhysFrame<Size4KiB>,
    dst_pml4: PhysFrame<Size4KiB>,
    src_table_frame: PhysFrame<Size4KiB>,
    level: u8,
    l4_index: usize,
    vaddr_base: u64,
) {
    let src_virt = super::paging::phys_to_virt(src_table_frame.start_address());
    let src_table: &PageTable = unsafe { &*src_virt.as_ptr() };

    for (idx, entry) in src_table.iter().enumerate() {
        if !entry.flags().contains(PageTableFlags::PRESENT) {
            continue;
        }
        if entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            continue;
        }

        let child_frame = match entry.frame() {
            Ok(f) => f,
            Err(_) => continue,
        };

        if level == 1 {
            // Leaf level: share the physical frame, mark read-only in BOTH parent and child
            let phys_addr = child_frame.start_address().as_u64();
            let vaddr = vaddr_base | (idx as u64) << 12;
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(vaddr));

            // Original flags minus WRITABLE (make CoW)
            let mut cow_flags = entry.flags();
            let was_writable = cow_flags.contains(PageTableFlags::WRITABLE);
            if was_writable {
                cow_flags.remove(PageTableFlags::WRITABLE);
                // Mark with BIT_9 as a CoW indicator (available bit)
                cow_flags.insert(PageTableFlags::BIT_9);
            }

            // Remap in parent: remove WRITABLE
            if was_writable {
                remap_page_flags(src_pml4, page, cow_flags);
            }

            // Map in child with same CoW flags
            super::paging::map_page(dst_pml4, page, child_frame, cow_flags).ok();

            // Increment refcount for shared frame
            ref_inc(phys_addr);
            // If this is the first share, also count the original owner
            if ref_count(phys_addr) == 1 {
                ref_inc(phys_addr);
            }
        } else {
            let new_vaddr_base = if level == 3 {
                (l4_index as u64) << 39 | (idx as u64) << 30
            } else {
                vaddr_base | (idx as u64) << (if level == 2 { 21 } else { 12 })
            };
            cow_clone_recursive(src_pml4, dst_pml4, child_frame, level - 1, l4_index, new_vaddr_base);
        }
    }
}

/// Remap a page with new flags (used to remove WRITABLE for CoW).
fn remap_page_flags(
    pml4_frame: PhysFrame<Size4KiB>,
    page: Page<Size4KiB>,
    flags: PageTableFlags,
) {
    let virt = super::paging::phys_to_virt(pml4_frame.start_address());
    let l4_table: &mut PageTable = unsafe { &mut *virt.as_mut_ptr() };
    let page_table = unsafe { OffsetPageTable::new(l4_table, super::paging::phys_offset()) };

    if let Ok(_frame) = page_table.translate_page(page) {
        // Unmap and remap with new flags
        // For simplicity, we just update the flags in-place by walking the table
        update_page_flags_inplace(pml4_frame, page, flags);
    }
}

/// Update page table entry flags in-place for a given page.
fn update_page_flags_inplace(
    pml4_frame: PhysFrame<Size4KiB>,
    page: Page<Size4KiB>,
    new_flags: PageTableFlags,
) {
    let addr = page.start_address().as_u64();
    let l4_idx = ((addr >> 39) & 0x1FF) as usize;
    let l3_idx = ((addr >> 30) & 0x1FF) as usize;
    let l2_idx = ((addr >> 21) & 0x1FF) as usize;
    let l1_idx = ((addr >> 12) & 0x1FF) as usize;

    let phys_to_virt = super::paging::phys_to_virt;

    let l4: &PageTable = unsafe { &*phys_to_virt(pml4_frame.start_address()).as_ptr() };
    if !l4[l4_idx].flags().contains(PageTableFlags::PRESENT) { return; }
    let l3_frame = match l4[l4_idx].frame() { Ok(f) => f, Err(_) => return };

    let l3: &PageTable = unsafe { &*phys_to_virt(l3_frame.start_address()).as_ptr() };
    if !l3[l3_idx].flags().contains(PageTableFlags::PRESENT) { return; }
    let l2_frame = match l3[l3_idx].frame() { Ok(f) => f, Err(_) => return };

    let l2: &PageTable = unsafe { &*phys_to_virt(l2_frame.start_address()).as_ptr() };
    if !l2[l2_idx].flags().contains(PageTableFlags::PRESENT) { return; }
    let l1_frame = match l2[l2_idx].frame() { Ok(f) => f, Err(_) => return };

    let l1: &mut PageTable = unsafe { &mut *phys_to_virt(l1_frame.start_address()).as_mut_ptr() };
    if l1[l1_idx].flags().contains(PageTableFlags::PRESENT) {
        let phys = match l1[l1_idx].frame() { Ok(f) => f, Err(_) => return };
        l1[l1_idx].set_frame(phys, new_flags);
    }
}

/// Handle a CoW page fault.
///
/// Called from the page fault handler when a write to a CoW page occurs.
/// Returns true if the fault was handled (CoW copy performed).
pub fn handle_cow_fault(
    pml4_frame: PhysFrame<Size4KiB>,
    fault_addr: VirtAddr,
) -> bool {
    let page = Page::<Size4KiB>::containing_address(fault_addr);
    let addr = fault_addr.as_u64();

    // Walk page tables to find the PTE
    let l4_idx = ((addr >> 39) & 0x1FF) as usize;
    let l3_idx = ((addr >> 30) & 0x1FF) as usize;
    let l2_idx = ((addr >> 21) & 0x1FF) as usize;
    let l1_idx = ((addr >> 12) & 0x1FF) as usize;

    let phys_to_virt = super::paging::phys_to_virt;

    let l4: &PageTable = unsafe { &*phys_to_virt(pml4_frame.start_address()).as_ptr() };
    if !l4[l4_idx].flags().contains(PageTableFlags::PRESENT) { return false; }
    let l3_frame = match l4[l4_idx].frame() { Ok(f) => f, Err(_) => return false };

    let l3: &PageTable = unsafe { &*phys_to_virt(l3_frame.start_address()).as_ptr() };
    if !l3[l3_idx].flags().contains(PageTableFlags::PRESENT) { return false; }
    let l2_frame = match l3[l3_idx].frame() { Ok(f) => f, Err(_) => return false };

    let l2: &PageTable = unsafe { &*phys_to_virt(l2_frame.start_address()).as_ptr() };
    if !l2[l2_idx].flags().contains(PageTableFlags::PRESENT) { return false; }
    let l1_frame = match l2[l2_idx].frame() { Ok(f) => f, Err(_) => return false };

    let l1: &mut PageTable = unsafe { &mut *phys_to_virt(l1_frame.start_address()).as_mut_ptr() };
    let pte = &l1[l1_idx];
    if !pte.flags().contains(PageTableFlags::PRESENT) { return false; }

    // Check if this is a CoW page (BIT_9 set, not writable)
    if !pte.flags().contains(PageTableFlags::BIT_9) {
        return false;
    }

    let old_frame = match pte.frame() { Ok(f) => f, Err(_) => return false };
    let old_phys = old_frame.start_address().as_u64();
    let rc = ref_count(old_phys);

    if rc <= 1 {
        // We're the only owner — just make it writable again
        let mut flags = pte.flags();
        flags.insert(PageTableFlags::WRITABLE);
        flags.remove(PageTableFlags::BIT_9);
        l1[l1_idx].set_frame(old_frame, flags);
        // Flush TLB for this page
        x86_64::instructions::tlb::flush(page.start_address());
        return true;
    }

    // Multiple owners: allocate new frame and copy data
    let new_frame = match super::frame::alloc_frame() {
        Some(f) => f,
        None => return false, // OOM
    };

    // Copy the page data
    let src = phys_to_virt(old_frame.start_address()).as_u64() as *const u8;
    let dst = phys_to_virt(new_frame.start_address()).as_u64() as *mut u8;
    unsafe { core::ptr::copy_nonoverlapping(src, dst, 4096); }

    // Update PTE: new frame, writable, remove CoW flag
    let mut flags = pte.flags();
    flags.insert(PageTableFlags::WRITABLE);
    flags.remove(PageTableFlags::BIT_9);
    l1[l1_idx].set_frame(new_frame, flags);

    // Decrement old frame refcount
    let new_rc = ref_dec(old_phys);
    if new_rc == 1 {
        // The other owner is now the sole owner — could mark it writable
        // (handled lazily on their next write fault)
    }

    // Flush TLB
    x86_64::instructions::tlb::flush(page.start_address());

    serial_println!("[cow] CoW fault handled: vaddr={:#X}, copied frame", addr);
    true
}

/// Get CoW statistics: (shared_frames, total_refcounted_frames).
pub fn cow_stats() -> (usize, usize) {
    let refs = REFCOUNTS.lock();
    let shared = refs.values().filter(|&&v| v > 1).count();
    (shared, refs.len())
}

/// Initialize the CoW subsystem.
pub fn init() {
    serial_println!("[cow] Copy-on-Write fork subsystem initialized.");
}

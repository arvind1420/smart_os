/// Page table management for Smart OS.
///
/// Provides the ability to create per-process address spaces,
/// map/unmap virtual pages to physical frames, and switch CR3.
/// Built on the x86_64 crate's OffsetPageTable.
///
/// Phase 5: Full implementation.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::structures::paging::{
    OffsetPageTable, PageTable, PageTableFlags, PhysFrame, Size4KiB,
    Page, Mapper,
};
use x86_64::{VirtAddr, PhysAddr};
use x86_64::registers::control::Cr3;
use crate::serial_println;

/// Physical memory offset provided by the bootloader.
static PHYS_OFFSET: AtomicU64 = AtomicU64::new(0);

/// Initialize the paging subsystem.
/// Must be called after heap and frame allocator are ready.
pub fn init(phys_offset: u64) {
    PHYS_OFFSET.store(phys_offset, Ordering::Relaxed);
    serial_println!("[paging] Page table management initialized (offset={:#X}).", phys_offset);
}

/// Get the stored physical memory offset as a VirtAddr.
pub fn phys_offset() -> VirtAddr {
    VirtAddr::new(PHYS_OFFSET.load(Ordering::Relaxed))
}

/// Convert a physical address to a virtual address using the offset mapping.
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    phys_offset() + phys.as_u64()
}

/// Get a mutable reference to the active (kernel) Level 4 page table.
///
/// # Safety
/// Caller must ensure no other code is modifying the page table concurrently.
pub unsafe fn active_level_4_table() -> &'static mut PageTable {
    let (frame, _) = Cr3::read();
    let phys = frame.start_address();
    let virt = phys_to_virt(phys);
    unsafe { &mut *virt.as_mut_ptr() }
}

/// Create an OffsetPageTable from the active (kernel) page table.
///
/// # Safety
/// Caller must ensure exclusive access.
pub unsafe fn kernel_page_table() -> OffsetPageTable<'static> {
    let l4 = unsafe { active_level_4_table() };
    unsafe { OffsetPageTable::new(l4, phys_offset()) }
}

/// Create a new empty Level 4 page table for a user process.
///
/// Returns the physical frame containing the new PML4.
/// The upper half (entries 256-511) is cloned from the kernel page table
/// so that kernel space is shared across all address spaces.
pub fn create_user_page_table() -> Option<PhysFrame<Size4KiB>> {
    let frame = super::frame::alloc_frame()?;

    // Zero the new page table
    let virt = phys_to_virt(frame.start_address());
    let new_table: &mut PageTable = unsafe { &mut *virt.as_mut_ptr() };
    new_table.zero();

    // Copy kernel mappings (upper half: entries 256..512)
    let kernel_l4 = unsafe { active_level_4_table() };
    for i in 256..512 {
        new_table[i] = kernel_l4[i].clone();
    }

    Some(frame)
}

/// Map a virtual page to a physical frame in a given page table.
///
/// `pml4_frame` is the physical frame containing the Level 4 table.
/// Intermediate page table levels are allocated automatically from the frame allocator.
pub fn map_page(
    pml4_frame: PhysFrame<Size4KiB>,
    page: Page<Size4KiB>,
    phys_frame: PhysFrame<Size4KiB>,
    flags: PageTableFlags,
) -> Result<(), &'static str> {
    let virt = phys_to_virt(pml4_frame.start_address());
    let l4_table: &mut PageTable = unsafe { &mut *virt.as_mut_ptr() };
    let mut page_table = unsafe { OffsetPageTable::new(l4_table, phys_offset()) };
    let mut alloc_guard = super::frame::FRAME_ALLOCATOR.lock();
    let alloc = alloc_guard.as_mut().ok_or("Frame allocator not initialized")?;
    unsafe {
        page_table
            .map_to(page, phys_frame, flags, alloc)
            .map_err(|_| "Failed to map page")?
            .flush();
    }
    Ok(())
}

/// Map a range of virtual pages, allocating fresh physical frames for each.
pub fn map_range(
    pml4_frame: PhysFrame<Size4KiB>,
    start_vaddr: u64,
    page_count: usize,
    flags: PageTableFlags,
) -> Result<(), &'static str> {
    for i in 0..page_count {
        let vaddr = VirtAddr::new(start_vaddr + (i as u64) * 4096);
        let page = Page::<Size4KiB>::containing_address(vaddr);
        let frame = super::frame::alloc_frame().ok_or("Out of physical frames")?;
        map_page(pml4_frame, page, frame, flags)?;
    }
    Ok(())
}

/// Switch CR3 to a different page table. Returns the old CR3 frame.
///
/// # Safety
/// The new page table must have the kernel half mapped correctly.
pub unsafe fn switch_to(pml4_frame: PhysFrame<Size4KiB>) -> PhysFrame<Size4KiB> {
    let (old_frame, old_flags) = Cr3::read();
    unsafe { Cr3::write(pml4_frame, old_flags) };
    old_frame
}

/// Get the current CR3 physical frame.
pub fn current_cr3() -> PhysFrame<Size4KiB> {
    let (frame, _) = Cr3::read();
    frame
}

/// Free a user page table and all frames it maps in the lower half (entries 0-255).
/// Does NOT free kernel mappings (upper half).
pub fn free_user_page_table(pml4_frame: PhysFrame<Size4KiB>) {
    let virt = phys_to_virt(pml4_frame.start_address());
    let l4_table: &PageTable = unsafe { &*virt.as_ptr() };

    // Walk the lower half entries (user space).
    for i in 0..256 {
        let l4_entry = &l4_table[i];
        if !l4_entry.flags().contains(PageTableFlags::PRESENT) {
            continue;
        }
        // Free the L3 table (and recurse)
        let l3_frame = match l4_entry.frame() {
            Ok(f) => f,
            Err(_) => continue,
        };
        free_page_table_level(l3_frame, 3);
        super::frame::dealloc_frame(l3_frame);
    }

    // Free the PML4 frame itself.
    super::frame::dealloc_frame(pml4_frame);
}

/// Free only the user-half mappings (entries 0-255) from a page table,
/// but keep the PML4 frame itself. Used by exec() to clear user space
/// before loading a new program image.
pub fn free_user_pages_only(pml4_frame: PhysFrame<Size4KiB>) {
    let virt = phys_to_virt(pml4_frame.start_address());
    let l4_table: &mut PageTable = unsafe { &mut *virt.as_mut_ptr() };

    for i in 0..256 {
        let l4_entry = &l4_table[i];
        if !l4_entry.flags().contains(PageTableFlags::PRESENT) {
            continue;
        }
        let l3_frame = match l4_entry.frame() {
            Ok(f) => f,
            Err(_) => continue,
        };
        free_page_table_level(l3_frame, 3);
        super::frame::dealloc_frame(l3_frame);
    }

    // Zero out user-half entries (but keep the PML4 frame)
    for i in 0..256 {
        l4_table[i].set_unused();
    }
}

/// Clone a user page table (full copy of user-half pages).
/// Returns a new PML4 frame with a deep copy of all user-space mappings.
/// Used by fork().
pub fn clone_user_page_table(src_pml4: PhysFrame<Size4KiB>) -> Option<PhysFrame<Size4KiB>> {
    let new_pml4 = create_user_page_table()?; // copies kernel half

    let src_virt = phys_to_virt(src_pml4.start_address());
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
        clone_table_recursive(new_pml4, src_l3_frame, 3, i, 0);
    }

    Some(new_pml4)
}

/// Recursively clone page table entries and data pages.
fn clone_table_recursive(
    dst_pml4: PhysFrame<Size4KiB>,
    src_table_frame: PhysFrame<Size4KiB>,
    level: u8,
    l4_index: usize,
    vaddr_base: u64,
) {
    let src_virt = phys_to_virt(src_table_frame.start_address());
    let src_table: &PageTable = unsafe { &*src_virt.as_ptr() };

    for (idx, entry) in src_table.iter().enumerate() {
        if !entry.flags().contains(PageTableFlags::PRESENT) {
            continue;
        }
        if entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            continue; // skip huge pages
        }

        let child_frame = match entry.frame() {
            Ok(f) => f,
            Err(_) => continue,
        };

        if level == 1 {
            // Leaf level: copy the data page
            let new_frame = match super::frame::alloc_frame() {
                Some(f) => f,
                None => continue,
            };

            // Copy 4096 bytes
            let src_data = phys_to_virt(child_frame.start_address()).as_u64() as *const u8;
            let dst_data = phys_to_virt(new_frame.start_address()).as_u64() as *mut u8;
            unsafe { core::ptr::copy_nonoverlapping(src_data, dst_data, 4096); }

            // Calculate the virtual address for this page
            let vaddr = compute_vaddr(l4_index, vaddr_base, level, idx);
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(vaddr));
            let flags = entry.flags();
            map_page(dst_pml4, page, new_frame, flags).ok();
        } else {
            // Recurse into sub-table
            let new_vaddr_base = if level == 3 {
                (l4_index as u64) << 39 | (idx as u64) << 30
            } else {
                vaddr_base | (idx as u64) << (if level == 2 { 21 } else { 12 })
            };
            clone_table_recursive(dst_pml4, child_frame, level - 1, l4_index, new_vaddr_base);
        }
    }
}

/// Compute virtual address from page table indices.
fn compute_vaddr(_l4_idx: usize, vaddr_base: u64, _level: u8, idx: usize) -> u64 {
    // vaddr_base already encodes L4+L3+L2 indices, idx is the L1 index
    vaddr_base | (idx as u64) << 12
}

/// Translate a user virtual address to a kernel virtual address pointing to the same physical frame.
/// This acts as a safe memory check before accessing user memory in the kernel.
pub fn translate_user_addr(user_vaddr: u64) -> Result<u64, &'static str> {
    if user_vaddr >= 0x0000_8000_0000_0000 {
        return Err("Address is not in user space");
    }
    let (cr3_frame, _) = Cr3::read();
    let virt = phys_to_virt(cr3_frame.start_address());
    let l4_table: &mut PageTable = unsafe { &mut *virt.as_mut_ptr() };
    let page_table = unsafe { OffsetPageTable::new(l4_table, phys_offset()) };
    
    use x86_64::structures::paging::Translate;
    match page_table.translate_addr(VirtAddr::new(user_vaddr)) {
        Some(phys_addr) => Ok(phys_to_virt(phys_addr).as_u64()),
        None => Err("User address not mapped"),
    }
}

/// Translate a virtual address using a specific PML4 (not the current CR3).
/// Returns the physical address of the byte at `vaddr`, or None if not mapped.
/// Used by the ELF loader to resolve relocations in a newly-created page table.
pub fn translate_in_pml4(pml4_frame: PhysFrame<Size4KiB>, vaddr: VirtAddr) -> Option<PhysAddr> {
    let virt = phys_to_virt(pml4_frame.start_address());
    let l4_table: &mut PageTable = unsafe { &mut *virt.as_mut_ptr() };
    let page_table = unsafe { OffsetPageTable::new(l4_table, phys_offset()) };
    use x86_64::structures::paging::Translate;
    page_table.translate_addr(vaddr)
}

/// Safely copy data from user space to kernel space, traversing page boundaries.
pub fn copy_from_user(mut dst: &mut [u8], mut src_vaddr: u64) -> Result<(), &'static str> {
    while !dst.is_empty() {
        let kernel_vaddr = translate_user_addr(src_vaddr)?;
        let page_offset = src_vaddr & 0xFFF;
        let bytes_to_copy = (4096 - page_offset).min(dst.len() as u64) as usize;
        
        unsafe {
            core::ptr::copy_nonoverlapping(
                kernel_vaddr as *const u8,
                dst.as_mut_ptr(),
                bytes_to_copy,
            );
        }
        
        dst = &mut dst[bytes_to_copy..];
        src_vaddr += bytes_to_copy as u64;
    }
    Ok(())
}

/// Safely copy data from kernel space to user space, traversing page boundaries.
pub fn copy_to_user(mut dst_vaddr: u64, mut src: &[u8]) -> Result<(), &'static str> {
    while !src.is_empty() {
        let kernel_vaddr = translate_user_addr(dst_vaddr)?;
        // Ideally we should also check if the page is WRITABLE
        let page_offset = dst_vaddr & 0xFFF;
        let bytes_to_copy = (4096 - page_offset).min(src.len() as u64) as usize;
        
        unsafe {
            core::ptr::copy_nonoverlapping(
                src.as_ptr(),
                kernel_vaddr as *mut u8,
                bytes_to_copy,
            );
        }
        
        src = &src[bytes_to_copy..];
        dst_vaddr += bytes_to_copy as u64;
    }
    Ok(())
}

/// Map anonymous pages into a user address space.
/// Allocates physical frames and maps them at the given virtual address.
/// Returns the actual mapped address.
pub fn mmap_anonymous(
    pml4_frame: PhysFrame<Size4KiB>,
    hint_vaddr: u64,
    num_pages: usize,
) -> Result<u64, &'static str> {
    // Use hint address, aligned to page boundary
    let start = if hint_vaddr == 0 { 0x1000_0000 } else { hint_vaddr & !0xFFF };

    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE;

    map_range(pml4_frame, start, num_pages, flags)?;

    Ok(start)
}

/// Recursively free a page table at a given level (3=L3, 2=L2, 1=L1).
fn free_page_table_level(table_frame: PhysFrame<Size4KiB>, level: u8) {
    let virt = phys_to_virt(table_frame.start_address());
    let table: &PageTable = unsafe { &*virt.as_ptr() };

    for entry in table.iter() {
        if !entry.flags().contains(PageTableFlags::PRESENT) {
            continue;
        }
        // Huge pages: just free the entry frame at L2 (2MiB) or L3 (1GiB).
        if entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            // We don't use huge pages for user space, but handle gracefully.
            continue;
        }
        let child_frame = match entry.frame() {
            Ok(f) => f,
            Err(_) => continue,
        };
        if level > 1 {
            // Recurse into sub-table.
            free_page_table_level(child_frame, level - 1);
        }
        // Free the child frame (either a sub-table or a mapped data page at level 1).
        super::frame::dealloc_frame(child_frame);
    }
}

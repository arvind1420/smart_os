/// Shared Memory (shmem) for Smart OS.
///
/// Phase 10: High-performance IPC via shared physical pages mapped into
/// multiple process address spaces. Supports create/attach/detach/destroy.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::structures::paging::{PhysFrame, Size4KiB, PageTableFlags, Page};
use x86_64::VirtAddr;
use crate::serial_println;

/// Shared memory segment ID.
pub type ShmId = u64;

static NEXT_SHM_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

/// A shared memory segment.
pub struct SharedMemSegment {
    pub id: ShmId,
    pub name: String,
    /// Physical frames backing this segment.
    pub frames: Vec<PhysFrame<Size4KiB>>,
    /// Number of 4KiB pages.
    pub page_count: usize,
    /// PIDs that have attached this segment.
    pub attached_pids: Vec<u64>,
    /// Creator PID.
    pub creator_pid: u64,
}

/// Global shared memory table.
static SHMEM_TABLE: Mutex<BTreeMap<ShmId, SharedMemSegment>> = Mutex::new(BTreeMap::new());
/// Name → ID lookup.
static SHMEM_NAMES: Mutex<BTreeMap<String, ShmId>> = Mutex::new(BTreeMap::new());

/// Create a new shared memory segment.
///
/// Allocates `page_count` physical frames.
/// Returns the segment ID.
pub fn create(name: &str, page_count: usize, creator_pid: u64) -> Option<ShmId> {
    if page_count == 0 || page_count > 256 {
        return None; // Max 1MiB shared segment
    }

    let mut frames = Vec::with_capacity(page_count);
    for _ in 0..page_count {
        let frame = super::frame::alloc_frame()?;
        // Zero the frame
        let virt = super::paging::phys_to_virt(frame.start_address());
        unsafe { core::ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, 4096); }
        frames.push(frame);
    }

    let id = NEXT_SHM_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let segment = SharedMemSegment {
        id,
        name: String::from(name),
        frames,
        page_count,
        attached_pids: Vec::new(),
        creator_pid,
    };

    SHMEM_TABLE.lock().insert(id, segment);
    SHMEM_NAMES.lock().insert(String::from(name), id);

    serial_println!("[shmem] Created segment '{}' (id={}, {} pages)", name, id, page_count);
    Some(id)
}

/// Attach a shared memory segment to a process address space.
///
/// Maps the segment's physical frames into the process's page table
/// at the given virtual address.
///
/// Returns the actual mapped virtual address.
pub fn attach(
    shm_id: ShmId,
    pml4_frame: PhysFrame<Size4KiB>,
    hint_vaddr: u64,
    pid: u64,
) -> Option<u64> {
    let mut table = SHMEM_TABLE.lock();
    let segment = table.get_mut(&shm_id)?;

    let start_vaddr = if hint_vaddr == 0 { 0x2000_0000 } else { hint_vaddr & !0xFFF };

    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE;

    for (i, &frame) in segment.frames.iter().enumerate() {
        let vaddr = VirtAddr::new(start_vaddr + (i as u64) * 4096);
        let page = Page::<Size4KiB>::containing_address(vaddr);
        if super::paging::map_page(pml4_frame, page, frame, flags).is_err() {
            return None;
        }
    }

    if !segment.attached_pids.contains(&pid) {
        segment.attached_pids.push(pid);
    }

    serial_println!("[shmem] Process {} attached segment {} at {:#X}", pid, shm_id, start_vaddr);
    Some(start_vaddr)
}

/// Detach a shared memory segment from a process.
pub fn detach(shm_id: ShmId, pid: u64) {
    let mut table = SHMEM_TABLE.lock();
    if let Some(segment) = table.get_mut(&shm_id) {
        segment.attached_pids.retain(|&p| p != pid);
        serial_println!("[shmem] Process {} detached from segment {}", pid, shm_id);
    }
}

/// Destroy a shared memory segment (frees frames if no attachers).
pub fn destroy(shm_id: ShmId) -> bool {
    let mut table = SHMEM_TABLE.lock();
    if let Some(segment) = table.get(&shm_id) {
        if !segment.attached_pids.is_empty() {
            return false; // Still in use
        }
        let name = segment.name.clone();
        // Free physical frames
        for &frame in &segment.frames {
            super::frame::dealloc_frame(frame);
        }
        table.remove(&shm_id);
        SHMEM_NAMES.lock().remove(&name);
        serial_println!("[shmem] Destroyed segment {}", shm_id);
        true
    } else {
        false
    }
}

/// Look up a shared memory segment by name.
pub fn lookup(name: &str) -> Option<ShmId> {
    SHMEM_NAMES.lock().get(name).copied()
}

/// Get shmem statistics: (segment_count, total_pages).
pub fn shmem_stats() -> (usize, usize) {
    let table = SHMEM_TABLE.lock();
    let count = table.len();
    let pages: usize = table.values().map(|s| s.page_count).sum();
    (count, pages)
}

/// List all shared memory segments: (id, name, pages, attached_count).
pub fn list_segments() -> Vec<(ShmId, String, usize, usize)> {
    let table = SHMEM_TABLE.lock();
    table.values()
        .map(|s| (s.id, s.name.clone(), s.page_count, s.attached_pids.len()))
        .collect()
}

/// Initialize the shared memory subsystem.
pub fn init() {
    serial_println!("[shmem] Shared memory subsystem initialized.");
}

/// VirtIO split virtqueue abstraction for Smart OS.
///
/// Shared transport layer used by both VirtIO-blk and VirtIO-net drivers.
/// Uses the legacy PCI I/O port interface (BAR0).

use alloc::vec::Vec;
use x86_64::instructions::port::Port;
use x86_64::structures::paging::FrameAllocator;

// ── Legacy VirtIO PCI register offsets (from BAR0) ──
pub const VIRTIO_DEVICE_FEATURES: u16 = 0x00;
pub const VIRTIO_GUEST_FEATURES: u16 = 0x04;
pub const VIRTIO_QUEUE_ADDR: u16 = 0x08;
pub const VIRTIO_QUEUE_SIZE: u16 = 0x0C;
pub const VIRTIO_QUEUE_SELECT: u16 = 0x0E;
pub const VIRTIO_QUEUE_NOTIFY: u16 = 0x10;
pub const VIRTIO_DEVICE_STATUS: u16 = 0x12;
pub const VIRTIO_ISR_STATUS: u16 = 0x13;
/// Device-specific config starts here (legacy).
pub const VIRTIO_DEVICE_CONFIG: u16 = 0x14;

// ── VirtIO device status bits ──
pub const STATUS_ACKNOWLEDGE: u8 = 1;
pub const STATUS_DRIVER: u8 = 2;
pub const STATUS_DRIVER_OK: u8 = 4;
pub const STATUS_FEATURES_OK: u8 = 8;

// ── Descriptor flags ──
pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

/// Maximum queue size we support.
pub const MAX_QUEUE_SIZE: usize = 128;

/// VirtQueue descriptor (16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

/// VirtQueue available ring header.
#[repr(C)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; MAX_QUEUE_SIZE],
}

/// VirtQueue used ring element.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtqUsedElem {
    pub id: u32,
    pub len: u32,
}

/// VirtQueue used ring header.
#[repr(C)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: u16,
    pub ring: [VirtqUsedElem; MAX_QUEUE_SIZE],
}

/// A complete VirtIO split virtqueue.
pub struct Virtqueue {
    /// Pointer to descriptor table (physically contiguous).
    desc_ptr: *mut VirtqDesc,
    /// Pointer to available ring.
    avail_ptr: *mut VirtqAvail,
    /// Pointer to used ring.
    used_ptr: *mut VirtqUsed,
    /// Actual queue size from device.
    pub queue_size: u16,
    /// Head of free descriptor chain.
    free_head: u16,
    /// Number of free descriptors.
    num_free: u16,
    /// Last seen used ring index (for polling).
    last_used_idx: u16,
    /// I/O base port (BAR0).
    io_base: u16,
    /// Which queue index (0, 1, ...).
    queue_index: u16,
}

// Safety: Virtqueue is used behind Mutex.
unsafe impl Send for Virtqueue {}

impl Virtqueue {
    /// Allocate and initialize a virtqueue.
    ///
    /// Reads queue_size from device, allocates physically contiguous memory,
    /// sets queue address, and builds free descriptor chain.
    pub fn new(io_base: u16, queue_index: u16) -> Option<Self> {
        // Select this queue
        unsafe { Port::<u16>::new(io_base + VIRTIO_QUEUE_SELECT).write(queue_index); }

        // Read queue size from device
        let queue_size = unsafe { Port::<u16>::new(io_base + VIRTIO_QUEUE_SIZE).read() };
        if queue_size == 0 || queue_size > MAX_QUEUE_SIZE as u16 {
            return None;
        }
        let qs = queue_size as usize;

        // Calculate memory layout sizes
        let desc_size = qs * core::mem::size_of::<VirtqDesc>();
        let avail_size = 4 + 2 * qs; // flags(2) + idx(2) + ring entries
        let used_size = 4 + 8 * qs;  // flags(2) + idx(2) + used elements

        // Avail ring starts after descs, aligned to 2
        let avail_offset = (desc_size + 1) & !1;
        // Used ring starts after avail, aligned to 4096
        let used_offset = ((avail_offset + avail_size) + 4095) & !4095;
        let total_size = used_offset + used_size;

        // Allocate from frame allocator (enough 4K pages)
        let num_pages = (total_size + 4095) / 4096;
        let mut fa = crate::memory::frame::FRAME_ALLOCATOR.lock();
        let allocator = fa.as_mut()?;

        // Allocate first frame and use its physical address as base
        let first_frame = allocator.allocate_frame()?;
        let base_phys = first_frame.start_address().as_u64();

        // Allocate remaining pages (they may not be contiguous, but for small queues
        // they typically are from our bitmap allocator)
        let mut _extra_frames = Vec::new();
        for _ in 1..num_pages {
            if let Some(f) = allocator.allocate_frame() {
                _extra_frames.push(f);
            }
        }
        drop(fa);

        // Convert physical to virtual
        let base_virt = crate::memory::paging::phys_to_virt(
            x86_64::PhysAddr::new(base_phys)
        ).as_u64() as *mut u8;

        // Zero the memory
        unsafe {
            core::ptr::write_bytes(base_virt, 0, total_size.min(num_pages * 4096));
        }

        let desc_ptr = base_virt as *mut VirtqDesc;
        let avail_ptr = unsafe { base_virt.add(avail_offset) } as *mut VirtqAvail;
        let used_ptr = unsafe { base_virt.add(used_offset) } as *mut VirtqUsed;

        // Build free descriptor chain
        unsafe {
            for i in 0..qs {
                let desc = &mut *desc_ptr.add(i);
                desc.next = if i + 1 < qs { (i + 1) as u16 } else { 0 };
                desc.flags = 0;
            }
        }

        // Tell device the queue address (PFN = physical_addr / 4096)
        let pfn = (base_phys / 4096) as u32;
        unsafe {
            Port::<u16>::new(io_base + VIRTIO_QUEUE_SELECT).write(queue_index);
            Port::<u32>::new(io_base + VIRTIO_QUEUE_ADDR).write(pfn);
        }

        Some(Self {
            desc_ptr,
            avail_ptr,
            used_ptr,
            queue_size,
            free_head: 0,
            num_free: queue_size,
            last_used_idx: 0,
            io_base,
            queue_index,
        })
    }

    /// Allocate a single free descriptor, returning its index.
    fn alloc_desc(&mut self) -> Option<u16> {
        if self.num_free == 0 {
            return None;
        }
        let idx = self.free_head;
        let desc = unsafe { &*self.desc_ptr.add(idx as usize) };
        self.free_head = desc.next;
        self.num_free -= 1;
        Some(idx)
    }

    /// Free a single descriptor back to the free list.
    fn free_desc(&mut self, idx: u16) {
        let desc = unsafe { &mut *self.desc_ptr.add(idx as usize) };
        desc.addr = 0;
        desc.len = 0;
        desc.flags = 0;
        desc.next = self.free_head;
        self.free_head = idx;
        self.num_free += 1;
    }

    /// Free an entire descriptor chain starting at `head`.
    pub fn free_chain(&mut self, head: u16) {
        let mut idx = head;
        loop {
            let desc = unsafe { &*self.desc_ptr.add(idx as usize) };
            let has_next = desc.flags & VIRTQ_DESC_F_NEXT != 0;
            let next = desc.next;
            self.free_desc(idx);
            if has_next {
                idx = next;
            } else {
                break;
            }
        }
    }

    /// Add a buffer chain to the available ring.
    ///
    /// `bufs` is a slice of (physical_addr, length, flags) tuples.
    /// Returns the head descriptor index.
    pub fn add_buf(&mut self, bufs: &[(u64, u32, u16)]) -> Option<u16> {
        if bufs.is_empty() || bufs.len() as u16 > self.num_free {
            return None;
        }

        let head = self.alloc_desc()?;
        let mut prev = head;

        for (i, &(addr, len, flags)) in bufs.iter().enumerate() {
            let idx = if i == 0 { head } else { self.alloc_desc()? };

            let desc = unsafe { &mut *self.desc_ptr.add(idx as usize) };
            desc.addr = addr;
            desc.len = len;

            if i + 1 < bufs.len() {
                desc.flags = flags | VIRTQ_DESC_F_NEXT;
            } else {
                desc.flags = flags & !VIRTQ_DESC_F_NEXT;
            }

            if i > 0 {
                let prev_desc = unsafe { &mut *self.desc_ptr.add(prev as usize) };
                prev_desc.next = idx;
            }
            prev = idx;
        }

        // Add to available ring
        let avail = unsafe { &mut *self.avail_ptr };
        let avail_idx = avail.idx as usize % self.queue_size as usize;
        avail.ring[avail_idx] = head;
        // Memory barrier
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        avail.idx = avail.idx.wrapping_add(1);

        Some(head)
    }

    /// Notify the device that new buffers are available.
    pub fn notify(&self) {
        unsafe { Port::<u16>::new(self.io_base + VIRTIO_QUEUE_NOTIFY).write(self.queue_index); }
    }

    /// Poll the used ring for completed buffers.
    ///
    /// Returns `Some((head_desc_id, bytes_written))` if a buffer was consumed.
    pub fn poll_used(&mut self) -> Option<(u16, u32)> {
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
        let used = unsafe { &*self.used_ptr };
        if self.last_used_idx == used.idx {
            return None;
        }

        let idx = self.last_used_idx as usize % self.queue_size as usize;
        let elem = used.ring[idx];
        self.last_used_idx = self.last_used_idx.wrapping_add(1);

        Some((elem.id as u16, elem.len))
    }
}

// ── Device lifecycle helpers ──

/// Reset a VirtIO device.
pub fn virtio_reset(io_base: u16) {
    unsafe { Port::<u8>::new(io_base + VIRTIO_DEVICE_STATUS).write(0); }
}

/// Set device status bits.
pub fn virtio_set_status(io_base: u16, status: u8) {
    let current = unsafe { Port::<u8>::new(io_base + VIRTIO_DEVICE_STATUS).read() };
    unsafe { Port::<u8>::new(io_base + VIRTIO_DEVICE_STATUS).write(current | status); }
}

/// Negotiate features: read device features, AND with wanted, write back.
pub fn virtio_negotiate_features(io_base: u16, wanted: u32) -> u32 {
    let device_features = unsafe { Port::<u32>::new(io_base + VIRTIO_DEVICE_FEATURES).read() };
    let negotiated = device_features & wanted;
    unsafe { Port::<u32>::new(io_base + VIRTIO_GUEST_FEATURES).write(negotiated); }
    negotiated
}

/// Read a byte from device-specific config space.
pub fn virtio_config_read8(io_base: u16, offset: u16) -> u8 {
    unsafe { Port::<u8>::new(io_base + VIRTIO_DEVICE_CONFIG + offset).read() }
}

/// Read a 32-bit value from device-specific config space.
pub fn virtio_config_read32(io_base: u16, offset: u16) -> u32 {
    unsafe { Port::<u32>::new(io_base + VIRTIO_DEVICE_CONFIG + offset).read() }
}

/// Read a 64-bit value from device-specific config space (two 32-bit reads).
pub fn virtio_config_read64(io_base: u16, offset: u16) -> u64 {
    let lo = virtio_config_read32(io_base, offset) as u64;
    let hi = virtio_config_read32(io_base, offset + 4) as u64;
    (hi << 32) | lo
}

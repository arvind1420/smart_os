/// VirtIO Block Device Driver for Smart OS.
///
/// Provides sector-level read/write access to a VirtIO block device.
/// Uses the legacy PCI transport with a single request virtqueue.

use spin::Mutex;
use super::pci;
use super::virtio::{self, Virtqueue, VIRTQ_DESC_F_WRITE};
use super::BlockDevice;

/// VirtIO block request types.
const VIRTIO_BLK_T_IN: u32 = 0;  // read
const VIRTIO_BLK_T_OUT: u32 = 1; // write

/// Block request status values.
const VIRTIO_BLK_S_OK: u8 = 0;

/// Sector size in bytes.
pub const SECTOR_SIZE: usize = 512;

/// VirtIO block request header (prepended to every request).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct BlkReqHeader {
    req_type: u32,
    reserved: u32,
    sector: u64,
}

/// Global block device instance.
pub static BLK_DEVICE: Mutex<Option<VirtioBlkDevice>> = Mutex::new(None);

pub struct VirtioBlkDevice {
    queue: Virtqueue,
    pub capacity_sectors: u64,
    io_base: u16,
    // Persistent buffers for request header and status (avoid stack allocation issues)
    req_header: BlkReqHeader,
    req_status: u8,
}

// Safety: used behind Mutex
unsafe impl Send for VirtioBlkDevice {}

impl BlockDevice for VirtioBlkDevice {
    fn read_blocks(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let count = buf.len() / SECTOR_SIZE;
        for i in 0..count {
            let offset = i * SECTOR_SIZE;
            let sector_buf: &mut [u8; SECTOR_SIZE] = (&mut buf[offset..offset + SECTOR_SIZE])
                .try_into().map_err(|_| "slice conversion failed")?;
            do_request(self, VIRTIO_BLK_T_IN, sector + i as u64, sector_buf)?;
        }
        Ok(())
    }

    fn write_blocks(&mut self, sector: u64, buf: &[u8]) -> Result<(), &'static str> {
        let count = buf.len() / SECTOR_SIZE;
        for i in 0..count {
            let offset = i * SECTOR_SIZE;
            let sector_buf: &[u8; SECTOR_SIZE] = (&buf[offset..offset + SECTOR_SIZE])
                .try_into().map_err(|_| "slice conversion failed")?;
            // cast to mut for do_request
            let buf_mut = unsafe { core::slice::from_raw_parts_mut(sector_buf.as_ptr() as *mut u8, SECTOR_SIZE) };
            do_request(self, VIRTIO_BLK_T_OUT, sector + i as u64, buf_mut)?;
        }
        Ok(())
    }

    fn capacity(&self) -> u64 {
        self.capacity_sectors
    }
}

/// Initialize the VirtIO block device.
pub fn init() -> Result<(), &'static str> {
    // Find the VirtIO block device on PCI bus
    let dev = pci::find_device(pci::VIRTIO_VENDOR, pci::VIRTIO_BLK_DEV)
        .ok_or("VirtIO block device not found on PCI bus")?;

    pci::enable_bus_master(&dev);
    pci::enable_io_space(&dev);

    let io_base = pci::bar0_io_base(&dev)
        .ok_or("VirtIO block BAR0 is not I/O space")?;

    // VirtIO device initialization sequence
    virtio::virtio_reset(io_base);
    virtio::virtio_set_status(io_base, virtio::STATUS_ACKNOWLEDGE);
    virtio::virtio_set_status(io_base, virtio::STATUS_DRIVER);

    // Negotiate features (we don't need any special features)
    virtio::virtio_negotiate_features(io_base, 0);
    virtio::virtio_set_status(io_base, virtio::STATUS_FEATURES_OK);

    // Read disk capacity from device config (offset 0 in device-specific config = 8 bytes)
    let capacity = virtio::virtio_config_read64(io_base, 0);

    // Initialize the request virtqueue (queue 0)
    let queue = Virtqueue::new(io_base, 0)
        .ok_or("Failed to initialize VirtIO block virtqueue")?;

    // Mark driver ready
    virtio::virtio_set_status(io_base, virtio::STATUS_DRIVER_OK);

    crate::serial_println!(
        "[virtio-blk] Device ready: {} sectors ({} MiB)",
        capacity, capacity * SECTOR_SIZE as u64 / (1024 * 1024)
    );

    *BLK_DEVICE.lock() = Some(VirtioBlkDevice {
        queue,
        capacity_sectors: capacity,
        io_base,
        req_header: BlkReqHeader { req_type: 0, reserved: 0, sector: 0 },
        req_status: 0,
    });

    Ok(())
}

/// Perform a block request (read or write).
fn do_request(dev: &mut VirtioBlkDevice, req_type: u32, lba: u64, data: &mut [u8]) -> Result<(), &'static str> {
    if lba >= dev.capacity_sectors {
        return Err("LBA out of range");
    }
    if data.len() != SECTOR_SIZE {
        return Err("Buffer must be exactly 512 bytes");
    }

    // Set up request header
    dev.req_header.req_type = req_type;
    dev.req_header.reserved = 0;
    dev.req_header.sector = lba;
    dev.req_status = 0xFF; // sentinel

    // Get physical addresses
    let phys_offset = crate::memory::paging::phys_offset();
    let header_virt = &dev.req_header as *const BlkReqHeader as u64;
    let header_phys = header_virt - phys_offset.as_u64();
    let data_virt = data.as_ptr() as u64;
    let data_phys = data_virt - phys_offset.as_u64();
    let status_virt = &dev.req_status as *const u8 as u64;
    let status_phys = status_virt - phys_offset.as_u64();

    // Build 3-descriptor chain:
    // 1. Header (device-readable)
    // 2. Data (device-readable for write, device-writable for read)
    // 3. Status (device-writable, 1 byte)
    let data_flags = if req_type == VIRTIO_BLK_T_IN { VIRTQ_DESC_F_WRITE } else { 0 };
    let bufs = [
        (header_phys, core::mem::size_of::<BlkReqHeader>() as u32, 0u16),
        (data_phys, SECTOR_SIZE as u32, data_flags),
        (status_phys, 1u32, VIRTQ_DESC_F_WRITE),
    ];

    let head = dev.queue.add_buf(&bufs).ok_or("VirtIO queue full")?;
    dev.queue.notify();

    // Poll for completion
    let mut timeout = 1_000_000u32;
    loop {
        if let Some((completed_id, _len)) = dev.queue.poll_used() {
            dev.queue.free_chain(completed_id);
            if completed_id == head {
                break;
            }
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("VirtIO block request timed out");
        }
        core::hint::spin_loop();
    }

    if dev.req_status != VIRTIO_BLK_S_OK {
        return Err("VirtIO block request failed");
    }

    Ok(())
}

/// Read a single 512-byte sector from the block device.
pub fn read_sector(lba: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), &'static str> {
    let mut dev = BLK_DEVICE.lock();
    let dev = dev.as_mut().ok_or("Block device not initialized")?;
    do_request(dev, VIRTIO_BLK_T_IN, lba, buf)
}

/// Write a single 512-byte sector to the block device.
pub fn write_sector(lba: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), &'static str> {
    let mut dev = BLK_DEVICE.lock();
    let dev = dev.as_mut().ok_or("Block device not initialized")?;
    // do_request takes &mut [u8], we cast from &[u8] — data is only read by device
    let buf_mut = unsafe { core::slice::from_raw_parts_mut(buf.as_ptr() as *mut u8, SECTOR_SIZE) };
    do_request(dev, VIRTIO_BLK_T_OUT, lba, buf_mut)
}

/// Read multiple contiguous sectors.
pub fn read_sectors(start_lba: u64, count: usize, buf: &mut [u8]) -> Result<(), &'static str> {
    if buf.len() < count * SECTOR_SIZE {
        return Err("Buffer too small");
    }
    for i in 0..count {
        let offset = i * SECTOR_SIZE;
        let sector_buf: &mut [u8; SECTOR_SIZE] = (&mut buf[offset..offset + SECTOR_SIZE])
            .try_into().map_err(|_| "slice conversion failed")?;
        read_sector(start_lba + i as u64, sector_buf)?;
    }
    Ok(())
}

/// Write multiple contiguous sectors.
pub fn write_sectors(start_lba: u64, count: usize, buf: &[u8]) -> Result<(), &'static str> {
    if buf.len() < count * SECTOR_SIZE {
        return Err("Buffer too small");
    }
    for i in 0..count {
        let offset = i * SECTOR_SIZE;
        let sector_buf: &[u8; SECTOR_SIZE] = (&buf[offset..offset + SECTOR_SIZE])
            .try_into().map_err(|_| "slice conversion failed")?;
        write_sector(start_lba + i as u64, sector_buf)?;
    }
    Ok(())
}

/// Get disk capacity in sectors.
pub fn capacity() -> u64 {
    BLK_DEVICE.lock().as_ref().map(|d| d.capacity_sectors).unwrap_or(0)
}

/// Check if block device is available.
pub fn is_available() -> bool {
    BLK_DEVICE.lock().is_some()
}

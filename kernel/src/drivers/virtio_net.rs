/// VirtIO Network Device Driver for Smart OS.
///
/// Provides raw Ethernet frame send/receive via a VirtIO network device.
/// Uses legacy PCI transport with two virtqueues: RX (queue 0) and TX (queue 1).

use spin::Mutex;
use alloc::vec;
use alloc::vec::Vec;
use super::pci;
use super::virtio::{self, Virtqueue, VIRTQ_DESC_F_WRITE};

/// VirtIO network header (prepended to every packet, 10 bytes for legacy).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtioNetHeader {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: u16,
    pub gso_size: u16,
    pub csum_start: u16,
    pub csum_offset: u16,
}

impl VirtioNetHeader {
    fn empty() -> Self {
        Self { flags: 0, gso_type: 0, hdr_len: 0, gso_size: 0, csum_start: 0, csum_offset: 0 }
    }
}

const NET_HDR_SIZE: usize = core::mem::size_of::<VirtioNetHeader>();
const MAX_FRAME_SIZE: usize = 1514; // Ethernet MTU + header
const RX_BUF_SIZE: usize = NET_HDR_SIZE + MAX_FRAME_SIZE;
const NUM_RX_BUFS: usize = 16;

/// Global network device instance.
pub static NET_DEVICE: Mutex<Option<VirtioNetDevice>> = Mutex::new(None);

pub struct VirtioNetDevice {
    rx_queue: Virtqueue,
    tx_queue: Virtqueue,
    pub mac: [u8; 6],
    io_base: u16,
    // Pre-allocated RX buffers (pinned in memory)
    rx_buffers: Vec<Vec<u8>>,
    // TX header buffer (reused)
    tx_header: VirtioNetHeader,
}

// Safety: used behind Mutex
unsafe impl Send for VirtioNetDevice {}

/// Initialize the VirtIO network device.
pub fn init() -> Result<(), &'static str> {
    let dev = pci::find_device(pci::VIRTIO_VENDOR, pci::VIRTIO_NET_DEV)
        .ok_or("VirtIO net device not found on PCI bus")?;

    pci::enable_bus_master(&dev);
    pci::enable_io_space(&dev);

    let io_base = pci::bar0_io_base(&dev)
        .ok_or("VirtIO net BAR0 is not I/O space")?;

    // VirtIO initialization
    virtio::virtio_reset(io_base);
    virtio::virtio_set_status(io_base, virtio::STATUS_ACKNOWLEDGE);
    virtio::virtio_set_status(io_base, virtio::STATUS_DRIVER);

    // Negotiate features (bit 5 = MAC address available)
    virtio::virtio_negotiate_features(io_base, 1 << 5);
    virtio::virtio_set_status(io_base, virtio::STATUS_FEATURES_OK);

    // Read MAC address from device config
    let mut mac = [0u8; 6];
    for i in 0..6 {
        mac[i] = virtio::virtio_config_read8(io_base, i as u16);
    }

    // Initialize RX queue (queue 0) and TX queue (queue 1)
    let rx_queue = Virtqueue::new(io_base, 0)
        .ok_or("Failed to init VirtIO net RX queue")?;
    let tx_queue = Virtqueue::new(io_base, 1)
        .ok_or("Failed to init VirtIO net TX queue")?;

    virtio::virtio_set_status(io_base, virtio::STATUS_DRIVER_OK);

    // Pre-allocate RX buffers
    let mut rx_buffers = Vec::with_capacity(NUM_RX_BUFS);
    for _ in 0..NUM_RX_BUFS {
        rx_buffers.push(vec![0u8; RX_BUF_SIZE]);
    }

    let mut net_dev = VirtioNetDevice {
        rx_queue,
        tx_queue,
        mac,
        io_base,
        rx_buffers,
        tx_header: VirtioNetHeader::empty(),
    };

    // Post RX buffers to the receive queue
    populate_rx_queue(&mut net_dev);

    crate::serial_println!(
        "[virtio-net] Device ready: MAC={:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
    );

    *NET_DEVICE.lock() = Some(net_dev);
    Ok(())
}

/// Pre-populate the RX queue with receive buffers.
fn populate_rx_queue(dev: &mut VirtioNetDevice) {
    let phys_offset = crate::memory::paging::phys_offset().as_u64();

    for buf in &mut dev.rx_buffers {
        let virt = buf.as_ptr() as u64;
        let phys = virt - phys_offset;

        // Single descriptor: device-writable buffer for header + data
        let bufs = [(phys, RX_BUF_SIZE as u32, VIRTQ_DESC_F_WRITE)];
        dev.rx_queue.add_buf(&bufs);
    }
    dev.rx_queue.notify();
}

/// Get this device's MAC address.
pub fn mac_address() -> Option<[u8; 6]> {
    NET_DEVICE.lock().as_ref().map(|d| d.mac)
}

/// Send a raw Ethernet frame.
pub fn send_frame(frame: &[u8]) -> Result<(), &'static str> {
    let mut dev_guard = NET_DEVICE.lock();
    let dev = dev_guard.as_mut().ok_or("Net device not initialized")?;

    if frame.len() > MAX_FRAME_SIZE {
        return Err("Frame too large");
    }

    let phys_offset = crate::memory::paging::phys_offset().as_u64();

    // Set up VirtIO net header (all zeros for simple send)
    dev.tx_header = VirtioNetHeader::empty();

    let header_virt = &dev.tx_header as *const VirtioNetHeader as u64;
    let header_phys = header_virt - phys_offset;

    let data_virt = frame.as_ptr() as u64;
    let data_phys = data_virt - phys_offset;

    // 2-descriptor chain: header (device-read) + data (device-read)
    let bufs = [
        (header_phys, NET_HDR_SIZE as u32, 0u16),
        (data_phys, frame.len() as u32, 0u16),
    ];

    let head = dev.tx_queue.add_buf(&bufs).ok_or("TX queue full")?;
    dev.tx_queue.notify();

    // Poll for completion
    let mut timeout = 500_000u32;
    loop {
        if let Some((completed_id, _)) = dev.tx_queue.poll_used() {
            dev.tx_queue.free_chain(completed_id);
            if completed_id == head {
                break;
            }
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("TX timed out");
        }
        core::hint::spin_loop();
    }

    Ok(())
}

/// Receive a raw Ethernet frame (non-blocking, polling).
///
/// Returns the number of bytes received, or 0 if no frame available.
pub fn recv_frame(buf: &mut [u8]) -> Result<usize, &'static str> {
    let mut dev_guard = NET_DEVICE.lock();
    let dev = dev_guard.as_mut().ok_or("Net device not initialized")?;

    if let Some((desc_id, bytes_written)) = dev.rx_queue.poll_used() {
        let total = bytes_written as usize;
        if total <= NET_HDR_SIZE {
            // Repost buffer and return empty
            dev.rx_queue.free_chain(desc_id);
            repost_rx_buf(dev, desc_id as usize);
            return Ok(0);
        }

        let frame_len = total - NET_HDR_SIZE;
        let copy_len = frame_len.min(buf.len());

        // Copy frame data (skip VirtIO header)
        let rx_idx = (desc_id as usize) % dev.rx_buffers.len();
        if rx_idx < dev.rx_buffers.len() {
            buf[..copy_len].copy_from_slice(&dev.rx_buffers[rx_idx][NET_HDR_SIZE..NET_HDR_SIZE + copy_len]);
        }

        // Free and repost the RX buffer
        dev.rx_queue.free_chain(desc_id);
        repost_rx_buf(dev, rx_idx);

        Ok(copy_len)
    } else {
        Ok(0) // No frame available
    }
}

/// Re-post a single RX buffer to the receive queue.
fn repost_rx_buf(dev: &mut VirtioNetDevice, idx: usize) {
    if idx >= dev.rx_buffers.len() {
        return;
    }
    let phys_offset = crate::memory::paging::phys_offset().as_u64();
    let virt = dev.rx_buffers[idx].as_ptr() as u64;
    let phys = virt - phys_offset;
    let bufs = [(phys, RX_BUF_SIZE as u32, VIRTQ_DESC_F_WRITE)];
    dev.rx_queue.add_buf(&bufs);
    dev.rx_queue.notify();
}

/// Check if network device is available.
pub fn is_available() -> bool {
    NET_DEVICE.lock().is_some()
}

/// Get the MAC address of the network device (or a default if none).
pub fn get_mac() -> [u8; 6] {
    // If we have a device with a MAC, return it; otherwise return a default
    // For QEMU SLIRP, the default MAC is 52:54:00:12:34:56
    [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]
}

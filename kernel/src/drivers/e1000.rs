/// Intel 8254x (e1000) Network Device Driver for Smart OS.
///
/// Full implementation for sending and receiving raw Ethernet frames
/// using DMA via Transmit and Receive Descriptor Rings.

use spin::Mutex;
use alloc::vec::Vec;
use super::pci;

// e1000 Register Offsets
const REG_CTRL: u32 = 0x0000;
const REG_STATUS: u32 = 0x0008;
const REG_IMC: u32 = 0x00D8;
const REG_RCTL: u32 = 0x0100;
const REG_TCTL: u32 = 0x0400;
const REG_TIPG: u32 = 0x0410;
const REG_RDBAL: u32 = 0x2800;
const REG_RDBAH: u32 = 0x2804;
const REG_RDLEN: u32 = 0x2808;
const REG_RDH: u32 = 0x2810;
const REG_RDT: u32 = 0x2818;
const REG_TDBAL: u32 = 0x3800;
const REG_TDBAH: u32 = 0x3804;
const REG_TDLEN: u32 = 0x3808;
const REG_TDH: u32 = 0x3810;
const REG_TDT: u32 = 0x3818;
const REG_RAL0: u32 = 0x5400;
const REG_RAH0: u32 = 0x5404;

// RCTL Flags
const RCTL_EN: u32 = 1 << 1;
const RCTL_SBP: u32 = 1 << 2;
const RCTL_BAM: u32 = 1 << 15;
const RCTL_BSIZE_2048: u32 = 0 << 16;
const RCTL_SECRC: u32 = 1 << 26;

// CTRL Flags
const CTRL_ASDE: u32 = 1 << 5;  // Auto-Speed Detection Enable
const CTRL_SLU:  u32 = 1 << 6;  // Set Link Up

// TCTL Flags
const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;
const TCTL_CT_SHIFT: u32 = 4;
const TCTL_COLD_SHIFT: u32 = 12;

// Descriptor Definitions
const NUM_TX_DESCRIPTORS: usize = 256;
const NUM_RX_DESCRIPTORS: usize = 256;

#[repr(C, packed)]
struct TxDescriptor {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

#[repr(C, packed)]
struct RxDescriptor {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

/// Global e1000 device instance.
pub static E1000_DEVICE: Mutex<Option<E1000Device>> = Mutex::new(None);

pub struct E1000Device {
    pub mac: [u8; 6],
    mmio_base: u64,
    
    // Rings
    tx_descriptors: *mut TxDescriptor,
    tx_next: usize,
    
    rx_descriptors: *mut RxDescriptor,
    rx_next: usize,
    
    // Buffer virtual addresses (for reading received data)
    rx_buffers_virt: Vec<u64>,
}

// Safety: descriptors and MMIO access are manually synchronized via Mutex.
unsafe impl Send for E1000Device {}

unsafe fn read_reg(base: u64, offset: u32) -> u32 {
    unsafe { core::ptr::read_volatile((base + offset as u64) as *const u32) }
}

unsafe fn write_reg(base: u64, offset: u32, value: u32) {
    unsafe { core::ptr::write_volatile((base + offset as u64) as *mut u32, value) }
}

/// Initialize the e1000 network device.
pub fn init() -> Result<(), &'static str> {
    let supported_devs = [
        0x100E, // E1000_DEV_82540EM
        0x10D3, // E1000_DEV_82574L
        0x10EA, // E1000_DEV_82577LM
    ];
    let mut found_dev = None;
    for &dev_id in &supported_devs {
        if let Some(dev) = pci::find_device(0x8086, dev_id) {
            found_dev = Some(dev);
            break;
        }
    }

    let dev = found_dev.ok_or("Intel e1000 device not found on PCI bus")?;
    pci::enable_bus_master(&dev);

    let mmio_phys = pci::bar0_mmio_base(&dev).ok_or("e1000 BAR0 is not MMIO")?;
    let phys_offset = crate::memory::paging::phys_offset().as_u64();
    let mmio_base = phys_offset + mmio_phys;

    // 1. Initialise device (no RST — VirtualBox provides a clean device at VM start)
    //
    // VirtualBox 82540EM quirk: issuing CTRL.RST leaves the emulation in an
    // intermediate state; a subsequent CTRL.SLU write is ignored until the
    // emulator's async EMT thread finishes processing the reset, which can
    // take longer than our spin-wait.  Skipping RST entirely lets us go
    // straight to setting SLU and avoids the race.
    unsafe {
        // Mask all device interrupts.
        write_reg(mmio_base, REG_IMC, 0xFFFF_FFFF);

        // Set Full-Duplex + LRST (link normal, not in reset) + SLU + ASDE.
        // FD=1, LRST=1(bit3), ASDE=1(bit5), SLU=1(bit6) → 0x69
        // We OR in these bits instead of overwriting so we preserve any
        // emulator-set bits (e.g. flow control, ILOS).
        let ctrl = read_reg(mmio_base, REG_CTRL) & !(1u32 << 26); // ensure RST=0
        write_reg(mmio_base, REG_CTRL, ctrl | CTRL_SLU | CTRL_ASDE | (1 << 3) | (1 << 0));

        // Give VirtualBox up to ~2 ms to raise STATUS.LU.
        let mut link_wait = 2_000_000u32;
        while read_reg(mmio_base, REG_STATUS) & 0x02 == 0 {
            link_wait -= 1;
            if link_wait == 0 { break; }
            core::hint::spin_loop();
        }
        let status = read_reg(mmio_base, REG_STATUS);
        let link_up = status & 0x02 != 0;
        crate::serial_println!(
            "[e1000] STATUS=0x{:08X} CTRL=0x{:08X} → Link {}",
            status,
            read_reg(mmio_base, REG_CTRL),
            if link_up { "UP" } else { "DOWN (continuing)" }
        );
    }

    // 2. Read MAC Address
    let mac_low = unsafe { read_reg(mmio_base, REG_RAL0) };
    let mac_high = unsafe { read_reg(mmio_base, REG_RAH0) };
    let mac = [
        (mac_low & 0xFF) as u8,
        ((mac_low >> 8) & 0xFF) as u8,
        ((mac_low >> 16) & 0xFF) as u8,
        ((mac_low >> 24) & 0xFF) as u8,
        (mac_high & 0xFF) as u8,
        ((mac_high >> 8) & 0xFF) as u8,
    ];

    // 3. Setup RX Ring
    let rx_ring_frame = crate::memory::frame::alloc_frame().ok_or("No frame for RX ring")?;
    let rx_ring_phys = rx_ring_frame.start_address().as_u64();
    let rx_descriptors = (phys_offset + rx_ring_phys) as *mut RxDescriptor;
    
    let mut rx_buffers_virt = Vec::with_capacity(NUM_RX_DESCRIPTORS);
    for i in 0..NUM_RX_DESCRIPTORS {
        let buf_frame = crate::memory::frame::alloc_frame().ok_or("No frame for RX buffer")?;
        let buf_phys = buf_frame.start_address().as_u64();
        let buf_virt = phys_offset + buf_phys;

        unsafe {
            let desc = rx_descriptors.add(i);
            (*desc).addr = buf_phys;
            (*desc).status = 0;
        }
        rx_buffers_virt.push(buf_virt);
    }

    unsafe {
        write_reg(mmio_base, REG_RDBAL, (rx_ring_phys & 0xFFFFFFFF) as u32);
        write_reg(mmio_base, REG_RDBAH, (rx_ring_phys >> 32) as u32);
        write_reg(mmio_base, REG_RDLEN, (NUM_RX_DESCRIPTORS * 16) as u32);
        write_reg(mmio_base, REG_RDH, 0);
        write_reg(mmio_base, REG_RDT, (NUM_RX_DESCRIPTORS - 1) as u32);
        write_reg(mmio_base, REG_RCTL, RCTL_EN | RCTL_SBP | RCTL_BAM | RCTL_BSIZE_2048 | RCTL_SECRC);
    }

    // 4. Setup TX Ring
    let tx_ring_frame = crate::memory::frame::alloc_frame().ok_or("No frame for TX ring")?;
    let tx_ring_phys = tx_ring_frame.start_address().as_u64();
    let tx_descriptors = (phys_offset + tx_ring_phys) as *mut TxDescriptor;
    
    for i in 0..NUM_TX_DESCRIPTORS {
        unsafe {
            let desc = tx_descriptors.add(i);
            (*desc).addr = 0;
            (*desc).cmd = 0;
            (*desc).status = 0;
        }
    }

    unsafe {
        write_reg(mmio_base, REG_TDBAL, (tx_ring_phys & 0xFFFFFFFF) as u32);
        write_reg(mmio_base, REG_TDBAH, (tx_ring_phys >> 32) as u32);
        write_reg(mmio_base, REG_TDLEN, (NUM_TX_DESCRIPTORS * 16) as u32);
        write_reg(mmio_base, REG_TDH, 0);
        write_reg(mmio_base, REG_TDT, 0);
        write_reg(mmio_base, REG_TCTL, TCTL_EN | TCTL_PSP | (0x10 << TCTL_CT_SHIFT) | (0x40 << TCTL_COLD_SHIFT));
        write_reg(mmio_base, REG_TIPG, 10 | (10 << 10) | (10 << 20)); // Standard IPG
    }

    crate::serial_println!(
        "[e1000] Device fully initialized: MAC={:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
    );

    let e1000_dev = E1000Device {
        mac,
        mmio_base,
        tx_descriptors,
        tx_next: 0,
        rx_descriptors,
        rx_next: 0,
        rx_buffers_virt,
    };

    *E1000_DEVICE.lock() = Some(e1000_dev);
    Ok(())
}

/// Check if e1000 network device is available.
pub fn is_available() -> bool {
    E1000_DEVICE.lock().is_some()
}

/// Get this device's MAC address.
pub fn mac_address() -> Option<[u8; 6]> {
    E1000_DEVICE.lock().as_ref().map(|d| d.mac)
}

/// Send a raw Ethernet frame.
pub fn send_frame(frame: &[u8]) -> Result<(), &'static str> {
    // ── Phase 1: set up descriptor and kick the hardware (lock held briefly) ──
    let status_ptr: *const u8;
    {
        let mut dev_guard = E1000_DEVICE.lock();
        let dev = dev_guard.as_mut().ok_or("e1000 not initialized")?;

        let tx_idx = dev.tx_next;
        let desc = unsafe { &mut *dev.tx_descriptors.add(tx_idx) };

        let phys_offset = crate::memory::paging::phys_offset().as_u64();
        let frame_phys = (frame.as_ptr() as u64) - phys_offset;

        desc.addr   = frame_phys;
        desc.length = frame.len() as u16;
        desc.cmd    = (1 << 0) | (1 << 1) | (1 << 3); // EOP | IFCS | RS
        desc.status = 0;

        dev.tx_next = (tx_idx + 1) % NUM_TX_DESCRIPTORS;
        unsafe { write_reg(dev.mmio_base, REG_TDT, dev.tx_next as u32); }

        // Capture the status byte address so we can poll WITHOUT holding the lock.
        // Safety: the TX ring is never freed; the pointer stays valid for the
        // lifetime of the kernel.
        status_ptr = &desc.status as *const u8;
    } // ── lock released here ──

    // ── Phase 2: poll DD without holding the lock so recv_frame can proceed ──
    // VirtualBox's 82540EM emulation writes DD synchronously when TDT is updated.
    // Allow up to ~1 M tight iterations (~10 ms on typical hardware) then soft-fail.
    let mut timeout = 1_000_000u32;
    loop {
        let status = unsafe { core::ptr::read_volatile(status_ptr) };
        if status & 0x01 != 0 { break; } // DD set → done
        timeout -= 1;
        if timeout == 0 {
            crate::serial_println!("[e1000] TX status writeback timeout (frame submitted)");
            return Ok(()); // soft-fail: frame was submitted, hardware may still send it
        }
        core::hint::spin_loop();
    }

    Ok(())
}

/// Receive a raw Ethernet frame.
pub fn recv_frame(buf: &mut [u8]) -> Result<usize, &'static str> {
    let mut dev_guard = E1000_DEVICE.lock();
    let dev = dev_guard.as_mut().ok_or("e1000 not initialized")?;

    let rx_idx = dev.rx_next;
    let desc = unsafe { &mut *dev.rx_descriptors.add(rx_idx) };

    if (unsafe { core::ptr::read_volatile(&desc.status) } & 0x01) == 0 {
        return Ok(0);
    }

    let len = desc.length as usize;
    let copy_len = len.min(buf.len());

    let buf_virt = dev.rx_buffers_virt[rx_idx];
    unsafe {
        core::ptr::copy_nonoverlapping(buf_virt as *const u8, buf.as_mut_ptr(), copy_len);
    }

    desc.status = 0;
    dev.rx_next = (rx_idx + 1) % NUM_RX_DESCRIPTORS;
    unsafe {
        write_reg(dev.mmio_base, REG_RDT, rx_idx as u32);
    }

    Ok(copy_len)
}

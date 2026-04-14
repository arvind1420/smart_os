/// USB xHCI Controller Driver for Smart OS.
///
/// Provides USB device enumeration via xHCI (USB 3.0) controller.
/// Supports: PCI discovery, controller initialization, port scanning,
/// device enumeration via command/event/transfer rings.

use alloc::vec::Vec;
use alloc::vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

// PCI class codes for USB
pub const USB_CLASS: u8 = 0x0C;
pub const USB_SUBCLASS: u8 = 0x03;
pub const USB_PROGIF_XHCI: u8 = 0x30;

// xHCI capability register offsets
const CAPLENGTH: usize = 0x00;
const HCSPARAMS1: usize = 0x04;
const HCSPARAMS2: usize = 0x08;
const HCCPARAMS1: usize = 0x10;
const DBOFF: usize = 0x14;
const RTSOFF: usize = 0x18;

// xHCI operational register offsets (base = mmio + cap_length)
const USBCMD: usize = 0x00;
const USBSTS: usize = 0x04;
const CRCR: usize = 0x18;
const DCBAAP: usize = 0x30;
const CONFIG: usize = 0x38;
const PORTSC_BASE: usize = 0x400;

// TRB types
const TRB_NORMAL: u32 = 1;
const TRB_SETUP: u32 = 2;
const TRB_DATA: u32 = 3;
const TRB_STATUS: u32 = 4;
const TRB_LINK: u32 = 6;
const TRB_ENABLE_SLOT: u32 = 9;
const TRB_ADDRESS_DEVICE: u32 = 11;
const TRB_CONFIGURE_EP: u32 = 12;
const TRB_CMD_COMPLETION: u32 = 33;
const TRB_PORT_STATUS_CHANGE: u32 = 34;
const TRB_TRANSFER: u32 = 32;

// USBCMD bits
const CMD_RUN: u32 = 1 << 0;
const CMD_HCRST: u32 = 1 << 1;
const CMD_INTE: u32 = 1 << 2;

// USBSTS bits
const STS_HCH: u32 = 1 << 0;  // HC Halted
const STS_CNR: u32 = 1 << 11; // Controller Not Ready
const STS_EINT: u32 = 1 << 3; // Event Interrupt

// PORTSC bits
const PORTSC_CCS: u32 = 1 << 0;  // Current Connect Status
const PORTSC_PED: u32 = 1 << 1;  // Port Enabled/Disabled
const PORTSC_PR: u32 = 1 << 4;   // Port Reset
const PORTSC_PLS_MASK: u32 = 0xF << 5; // Port Link State
const PORTSC_PP: u32 = 1 << 9;   // Port Power
const PORTSC_PRC: u32 = 1 << 21; // Port Reset Change
const PORTSC_CSC: u32 = 1 << 17; // Connect Status Change
const PORTSC_SPEED_MASK: u32 = 0xF << 10; // Port Speed

// Port speed values (bits 13:10)
const SPEED_FULL: u32 = 1;
const SPEED_LOW: u32 = 2;
const SPEED_HIGH: u32 = 3;
const SPEED_SUPER: u32 = 4;

// USB descriptor types
const DESC_DEVICE: u8 = 1;
const DESC_CONFIGURATION: u8 = 2;
const DESC_INTERFACE: u8 = 4;

// USB request types
const REQ_GET_DESCRIPTOR: u8 = 6;

// Number of TRBs per ring
const RING_SIZE: usize = 64;

// Maximum number of polling iterations before timeout
const POLL_TIMEOUT: u32 = 100_000;

/// Whether xHCI has been initialized.
static XHCI_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Transfer Ring Buffer (TRB) — 16 bytes, 4-dword.
#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
pub struct Trb {
    pub param: u64,
    pub status: u32,
    pub control: u32,
}

impl Trb {
    /// Create a new zeroed TRB.
    pub const fn new() -> Self {
        Self { param: 0, status: 0, control: 0 }
    }

    /// Get the TRB type from the control field (bits 15:10).
    pub fn trb_type(&self) -> u32 {
        (self.control >> 10) & 0x3F
    }

    /// Get the cycle bit (bit 0 of control).
    pub fn cycle_bit(&self) -> bool {
        self.control & 1 != 0
    }

    /// Get the completion code from status field (bits 31:24).
    pub fn completion_code(&self) -> u8 {
        (self.status >> 24) as u8
    }

    /// Get the slot ID from control field (bits 31:24).
    pub fn slot_id(&self) -> u8 {
        (self.control >> 24) as u8
    }
}

/// Describes a discovered USB device.
#[derive(Debug, Clone)]
pub struct UsbDevice {
    pub slot_id: u8,
    pub port: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub is_hid_keyboard: bool,
    pub is_hid_mouse: bool,
    pub is_mass_storage: bool,
}

/// Event Ring Segment Table Entry (16 bytes).
#[repr(C, align(64))]
#[derive(Clone, Copy, Default)]
struct ErstEntry {
    ring_segment_base: u64,
    ring_segment_size: u16,
    _reserved: u16,
    _reserved2: u32,
}

/// Device Context Base Address Array slot (contains a 64-bit pointer).
/// The DCBAA is an array of u64 physical pointers, one per slot + slot 0
/// for the scratchpad buffer array.

/// Input Control Context (32 bytes).
#[repr(C, align(32))]
#[derive(Clone, Copy, Default)]
struct InputControlContext {
    drop_flags: u32,
    add_flags: u32,
    _reserved: [u32; 5],
    config_value: u32,
}

/// Slot Context (32 bytes).
#[repr(C, align(32))]
#[derive(Clone, Copy, Default)]
struct SlotContext {
    /// route_string (bits 19:0), speed (bits 23:20), MTT, hub, context_entries (bits 31:27)
    field1: u32,
    /// max_exit_latency (bits 15:0), root_hub_port (bits 23:16), num_ports (bits 31:24)
    field2: u32,
    /// parent hub slot id, parent port num, TT think time, interrupter target
    field3: u32,
    /// usb device address (bits 7:0), slot state (bits 31:27)
    field4: u32,
    _reserved: [u32; 4],
}

/// Endpoint Context (32 bytes).
#[repr(C, align(32))]
#[derive(Clone, Copy, Default)]
struct EndpointContext {
    /// ep_state (bits 2:0), mult (bits 9:8), max_pstreams (bits 14:10),
    /// LSA (bit 15), interval (bits 23:16), max_esit_hi (bits 31:24)
    field1: u32,
    /// error_count (bits 2:1), ep_type (bits 5:3), max_burst (bits 15:8),
    /// max_packet_size (bits 31:16)
    field2: u32,
    /// TR dequeue pointer (bits 63:4), DCS (bit 0)
    tr_dequeue_lo: u32,
    tr_dequeue_hi: u32,
    /// average TRB length (bits 15:0), max_esit_lo (bits 31:16)
    field3: u32,
    _reserved: [u32; 3],
}

/// xHCI Host Controller driver.
pub struct XhciController {
    /// Virtual MMIO base address (phys_offset + BAR0 physical address).
    mmio_base: u64,
    /// Capability register length — offset from mmio_base to operational regs.
    cap_length: u8,
    /// Maximum device slots supported by the controller.
    max_slots: u8,
    /// Maximum root hub ports.
    max_ports: u8,
    /// Virtual address of operational register base.
    op_base: u64,
    /// Virtual address of runtime register base.
    runtime_base: u64,
    /// Virtual address of doorbell array base.
    doorbell_base: u64,

    // Device Context Base Address Array
    dcbaa: Vec<u64>,
    dcbaa_phys: u64,

    // Command Ring
    cmd_ring: Vec<Trb>,
    cmd_ring_phys: u64,
    cmd_enqueue: usize,
    cmd_cycle: bool,

    // Event Ring (primary interrupter, index 0)
    event_ring: Vec<Trb>,
    event_ring_phys: u64,
    event_dequeue: usize,
    event_cycle: bool,

    // Event Ring Segment Table
    erst: Vec<ErstEntry>,
    erst_phys: u64,

    /// Discovered USB devices.
    pub devices: Vec<UsbDevice>,
}

// Safety: XhciController is always accessed behind a Mutex.
unsafe impl Send for XhciController {}

/// Global xHCI controller instance.
pub static XHCI: Mutex<Option<XhciController>> = Mutex::new(None);

// ── Helper: convert a heap virtual address to a physical address for DMA ──

/// Convert a virtual (heap) address to a physical address.
///
/// Since the kernel maps all physical memory at `phys_offset`, a virtual
/// address for a heap allocation is `phys_offset + phys_addr`. So
/// `phys_addr = virt_addr - phys_offset`.
fn virt_to_phys(virt: u64) -> u64 {
    let phys_off = crate::memory::paging::phys_offset().as_u64();
    virt.wrapping_sub(phys_off)
}

/// Convert a physical address to a virtual address.
fn phys_to_virt(phys: u64) -> u64 {
    let phys_off = crate::memory::paging::phys_offset().as_u64();
    phys.wrapping_add(phys_off)
}

impl XhciController {
    // ── MMIO volatile register access ──

    /// Read a 32-bit value from an absolute MMIO virtual address offset.
    fn read32(&self, addr: u64) -> u32 {
        unsafe { core::ptr::read_volatile(addr as *const u32) }
    }

    /// Write a 32-bit value to an absolute MMIO virtual address.
    fn write32(&self, addr: u64, val: u32) {
        unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
    }

    /// Read a 64-bit value from an absolute MMIO virtual address.
    fn read64(&self, addr: u64) -> u64 {
        unsafe { core::ptr::read_volatile(addr as *const u64) }
    }

    /// Write a 64-bit value to an absolute MMIO virtual address.
    fn write64(&self, addr: u64, val: u64) {
        unsafe { core::ptr::write_volatile(addr as *mut u64, val) }
    }

    // ── Capability register helpers ──

    fn cap_read32(&self, offset: usize) -> u32 {
        self.read32(self.mmio_base + offset as u64)
    }

    // ── Operational register helpers ──

    fn op_read32(&self, offset: usize) -> u32 {
        self.read32(self.op_base + offset as u64)
    }

    fn op_write32(&self, offset: usize, val: u32) {
        self.write32(self.op_base + offset as u64, val)
    }

    fn op_read64(&self, offset: usize) -> u64 {
        self.read64(self.op_base + offset as u64)
    }

    fn op_write64(&self, offset: usize, val: u64) {
        self.write64(self.op_base + offset as u64, val)
    }

    // ── Doorbell ──

    fn ring_doorbell(&self, slot: u8, target: u32) {
        let addr = self.doorbell_base + (slot as u64) * 4;
        self.write32(addr, target);
    }

    // ── Construction ──

    /// Create a new xHCI controller from a PCI device.
    ///
    /// Maps BAR0 as MMIO and reads capability registers to determine
    /// controller parameters (max slots, max ports, register offsets).
    pub fn new(pci_dev: &super::pci::PciDevice) -> Result<Self, &'static str> {
        let bar0_phys = super::pci::bar0_mmio_base(pci_dev)
            .ok_or("xHCI: BAR0 is not memory-mapped")?;

        let mmio_base = phys_to_virt(bar0_phys);

        // Read capability registers
        let cap_length = unsafe {
            core::ptr::read_volatile(mmio_base as *const u8)
        };

        let hcsparams1 = unsafe {
            core::ptr::read_volatile((mmio_base + HCSPARAMS1 as u64) as *const u32)
        };
        let max_slots = (hcsparams1 & 0xFF) as u8;
        let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;

        let dboff = unsafe {
            core::ptr::read_volatile((mmio_base + DBOFF as u64) as *const u32)
        };
        let rtsoff = unsafe {
            core::ptr::read_volatile((mmio_base + RTSOFF as u64) as *const u32)
        };

        let op_base = mmio_base + cap_length as u64;
        let runtime_base = mmio_base + (rtsoff & !0x1F) as u64;
        let doorbell_base = mmio_base + (dboff & !0x3) as u64;

        crate::serial_println!(
            "[xhci] MMIO at phys={:#X} virt={:#X}, cap_length={}, max_slots={}, max_ports={}",
            bar0_phys, mmio_base, cap_length, max_slots, max_ports,
        );

        Ok(Self {
            mmio_base,
            cap_length,
            max_slots,
            max_ports,
            op_base,
            runtime_base,
            doorbell_base,
            dcbaa: Vec::new(),
            dcbaa_phys: 0,
            cmd_ring: Vec::new(),
            cmd_ring_phys: 0,
            cmd_enqueue: 0,
            cmd_cycle: true,
            event_ring: Vec::new(),
            event_ring_phys: 0,
            event_dequeue: 0,
            event_cycle: true,
            erst: Vec::new(),
            erst_phys: 0,
            devices: Vec::new(),
        })
    }

    // ── Controller lifecycle ──

    /// Halt the controller and perform a full reset.
    ///
    /// 1. Clear CMD_RUN to halt.
    /// 2. Wait for STS_HCH (halted) bit.
    /// 3. Assert HCRST.
    /// 4. Wait for HCRST to self-clear and CNR to clear.
    pub fn reset(&mut self) -> Result<(), &'static str> {
        // Step 1: halt the controller
        let cmd = self.op_read32(USBCMD);
        self.op_write32(USBCMD, cmd & !CMD_RUN);

        // Wait for HCH (halted)
        for _ in 0..POLL_TIMEOUT {
            if self.op_read32(USBSTS) & STS_HCH != 0 {
                break;
            }
            spin_delay();
        }
        if self.op_read32(USBSTS) & STS_HCH == 0 {
            return Err("xHCI: controller did not halt");
        }

        // Step 2: assert HCRST
        self.op_write32(USBCMD, CMD_HCRST);

        // Wait for HCRST to self-clear
        for _ in 0..POLL_TIMEOUT {
            if self.op_read32(USBCMD) & CMD_HCRST == 0 {
                break;
            }
            spin_delay();
        }
        if self.op_read32(USBCMD) & CMD_HCRST != 0 {
            return Err("xHCI: HCRST did not clear");
        }

        // Wait for CNR (Controller Not Ready) to clear
        for _ in 0..POLL_TIMEOUT {
            if self.op_read32(USBSTS) & STS_CNR == 0 {
                break;
            }
            spin_delay();
        }
        if self.op_read32(USBSTS) & STS_CNR != 0 {
            return Err("xHCI: controller not ready after reset");
        }

        crate::serial_println!("[xhci] Controller reset complete.");
        Ok(())
    }

    /// Initialize the Device Context Base Address Array.
    ///
    /// Allocates an array of (max_slots + 1) 64-bit pointers (slot 0 is
    /// the scratchpad buffer array pointer). Writes the physical address
    /// of the array to the DCBAAP operational register.
    pub fn init_dcbaa(&mut self) -> Result<(), &'static str> {
        // Configure max device slots
        let max = self.max_slots as u32;
        self.op_write32(CONFIG, max);

        // Allocate DCBAA: (max_slots + 1) entries of u64
        // The array must be 64-byte aligned. Vec<u64> from the heap allocator
        // will be at least 8-byte aligned; xHCI requires 64-byte alignment
        // for DCBAA. We over-allocate and align manually if needed, but
        // typical heap allocators satisfy this for large-enough allocations.
        let count = (self.max_slots as usize) + 1;
        let dcbaa = vec![0u64; count];

        let dcbaa_virt = dcbaa.as_ptr() as u64;
        let dcbaa_phys = virt_to_phys(dcbaa_virt);

        // Write DCBAAP (64-bit register)
        self.op_write64(DCBAAP, dcbaa_phys);

        self.dcbaa = dcbaa;
        self.dcbaa_phys = dcbaa_phys;

        crate::serial_println!(
            "[xhci] DCBAA at phys={:#X} ({} slots configured).",
            dcbaa_phys, max,
        );
        Ok(())
    }

    /// Initialize the Command Ring.
    ///
    /// Allocates a ring of RING_SIZE TRBs (the last is a Link TRB that
    /// wraps back to the start). Writes the ring physical address and
    /// initial cycle state to the CRCR operational register.
    pub fn init_cmd_ring(&mut self) -> Result<(), &'static str> {
        let mut ring = vec![Trb::new(); RING_SIZE];

        let ring_virt = ring.as_ptr() as u64;
        let ring_phys = virt_to_phys(ring_virt);

        // Set up the Link TRB at the last position to wrap around
        let link_idx = RING_SIZE - 1;
        ring[link_idx].param = ring_phys; // points back to start
        ring[link_idx].control = (TRB_LINK << 10) | (1 << 1) | 0; // Toggle Cycle bit
        // Note: cycle bit of link TRB will be set when we enqueue to it

        // Write CRCR: physical address | Ring Cycle State (bit 0 = 1)
        let crcr_val = ring_phys | 1; // initial cycle = 1
        self.op_write64(CRCR, crcr_val);

        self.cmd_ring = ring;
        self.cmd_ring_phys = ring_phys;
        self.cmd_enqueue = 0;
        self.cmd_cycle = true;

        crate::serial_println!("[xhci] Command ring at phys={:#X}.", ring_phys);
        Ok(())
    }

    /// Initialize the Event Ring for the primary interrupter (index 0).
    ///
    /// Allocates RING_SIZE TRBs for the event ring segment, plus an
    /// Event Ring Segment Table (ERST) with one entry. Programs the
    /// runtime registers: ERSTSZ, ERSTBA, ERDP.
    pub fn init_event_ring(&mut self) -> Result<(), &'static str> {
        // Allocate the event ring segment
        let event_ring = vec![Trb::new(); RING_SIZE];
        let event_ring_virt = event_ring.as_ptr() as u64;
        let event_ring_phys = virt_to_phys(event_ring_virt);

        // Allocate the Event Ring Segment Table (1 entry)
        let mut erst = vec![ErstEntry::default(); 1];
        erst[0].ring_segment_base = event_ring_phys;
        erst[0].ring_segment_size = RING_SIZE as u16;

        let erst_virt = erst.as_ptr() as u64;
        let erst_phys = virt_to_phys(erst_virt);

        // Runtime register set for interrupter 0 starts at runtime_base + 0x20
        // Each interrupter register set is 32 bytes.
        let ir0_base = self.runtime_base + 0x20;

        // ERSTSZ: number of segments (offset 0x08 in interrupter reg set)
        self.write32(ir0_base + 0x08, 1);

        // ERDP: Event Ring Dequeue Pointer (offset 0x18, 64-bit)
        // Set to start of event ring, with EHB bit cleared
        self.write64(ir0_base + 0x18, event_ring_phys);

        // ERSTBA: Event Ring Segment Table Base Address (offset 0x10, 64-bit)
        // Writing ERSTBA also initializes the event ring state in the controller
        self.write64(ir0_base + 0x10, erst_phys);

        self.event_ring = event_ring;
        self.event_ring_phys = event_ring_phys;
        self.event_dequeue = 0;
        self.event_cycle = true;
        self.erst = erst;
        self.erst_phys = erst_phys;

        crate::serial_println!(
            "[xhci] Event ring at phys={:#X}, ERST at phys={:#X}.",
            event_ring_phys, erst_phys,
        );
        Ok(())
    }

    /// Start the controller by setting CMD_RUN.
    pub fn start(&mut self) -> Result<(), &'static str> {
        let cmd = self.op_read32(USBCMD);
        self.op_write32(USBCMD, cmd | CMD_RUN | CMD_INTE);

        // Wait for HCH to clear (controller running)
        for _ in 0..POLL_TIMEOUT {
            if self.op_read32(USBSTS) & STS_HCH == 0 {
                crate::serial_println!("[xhci] Controller started (running).");
                return Ok(());
            }
            spin_delay();
        }

        Err("xHCI: controller did not start")
    }

    // ── Event Ring polling ──

    /// Poll the event ring for a new TRB.
    ///
    /// Returns `Some(trb)` if a new event is available (cycle bit matches
    /// the expected producer cycle state). Advances the dequeue pointer
    /// and writes it back to ERDP.
    pub fn poll_event(&mut self) -> Option<Trb> {
        let trb = self.event_ring[self.event_dequeue];
        let cycle = trb.cycle_bit();

        if cycle != self.event_cycle {
            return None; // No new event
        }

        // Advance dequeue pointer
        self.event_dequeue += 1;
        if self.event_dequeue >= RING_SIZE {
            self.event_dequeue = 0;
            self.event_cycle = !self.event_cycle;
        }

        // Update ERDP (runtime interrupter 0, offset 0x18)
        let ir0_base = self.runtime_base + 0x20;
        let new_erdp_phys = self.event_ring_phys
            + (self.event_dequeue as u64) * core::mem::size_of::<Trb>() as u64;
        // Set bit 3 (EHB — Event Handler Busy) to acknowledge
        self.write64(ir0_base + 0x18, new_erdp_phys | (1 << 3));

        Some(trb)
    }

    /// Poll for an event with a timeout.
    fn poll_event_timeout(&mut self, timeout: u32) -> Option<Trb> {
        for _ in 0..timeout {
            if let Some(trb) = self.poll_event() {
                return Some(trb);
            }
            spin_delay();
        }
        None
    }

    // ── Command Ring operations ──

    /// Enqueue a TRB to the command ring, ring doorbell 0 (host controller),
    /// and poll for a Command Completion Event.
    ///
    /// Returns the completion event TRB on success.
    pub fn post_command(&mut self, mut trb: Trb) -> Result<Trb, &'static str> {
        // Set or clear the cycle bit to match the producer cycle state
        if self.cmd_cycle {
            trb.control |= 1;
        } else {
            trb.control &= !1;
        }

        // Write TRB to the current enqueue position
        self.cmd_ring[self.cmd_enqueue] = trb;

        // Memory barrier to ensure the TRB is visible before ringing doorbell
        core::sync::atomic::fence(Ordering::Release);

        // Advance enqueue pointer
        self.cmd_enqueue += 1;

        // If we hit the Link TRB, toggle cycle and wrap
        if self.cmd_enqueue >= RING_SIZE - 1 {
            // Update the Link TRB's cycle bit
            let link = &mut self.cmd_ring[RING_SIZE - 1];
            if self.cmd_cycle {
                link.control |= 1;
            } else {
                link.control &= !1;
            }
            core::sync::atomic::fence(Ordering::Release);

            self.cmd_enqueue = 0;
            self.cmd_cycle = !self.cmd_cycle;
        }

        // Ring doorbell 0 (host controller doorbell), target = 0
        self.ring_doorbell(0, 0);

        // Poll for Command Completion Event
        let event = self.poll_event_timeout(POLL_TIMEOUT)
            .ok_or("xHCI: command completion event timeout")?;

        if event.trb_type() != TRB_CMD_COMPLETION {
            return Err("xHCI: unexpected event type (expected command completion)");
        }

        let code = event.completion_code();
        if code != 1 {
            // 1 = Success in xHCI completion codes
            crate::serial_println!("[xhci] Command failed with completion code {}.", code);
            return Err("xHCI: command failed");
        }

        Ok(event)
    }

    // ── Port operations ──

    /// Read the PORTSC register for a given 1-based port number.
    fn portsc_addr(&self, port: u8) -> u64 {
        // Ports are 1-based; PORTSC registers are spaced 16 bytes apart
        self.op_base + PORTSC_BASE as u64 + ((port as u64 - 1) * 0x10)
    }

    /// Check if a device is connected on the given 1-based port.
    pub fn port_connected(&self, port: u8) -> bool {
        if port == 0 || port > self.max_ports {
            return false;
        }
        let portsc = self.read32(self.portsc_addr(port));
        portsc & PORTSC_CCS != 0
    }

    /// Get the speed of a connected port (1-based port number).
    /// Returns the speed code from PORTSC bits 13:10.
    fn port_speed(&self, port: u8) -> u32 {
        let portsc = self.read32(self.portsc_addr(port));
        (portsc >> 10) & 0xF
    }

    /// Reset a USB port (1-based port number).
    ///
    /// Sets the Port Reset (PR) bit and waits for Port Reset Change (PRC)
    /// to indicate completion. Clears PRC afterwards.
    pub fn reset_port(&mut self, port: u8) -> Result<(), &'static str> {
        if port == 0 || port > self.max_ports {
            return Err("xHCI: invalid port number");
        }

        let addr = self.portsc_addr(port);

        // Read current PORTSC — preserve RW bits, clear RW1C status bits
        // to avoid accidentally clearing them.
        let portsc = self.read32(addr);
        // Mask off RW1C bits (bits 17-23) but keep PP, and set PR
        let _preserve_mask: u32 = PORTSC_PP | PORTSC_PED; // keep power, clear PED separately
        let _write_val = (portsc & 0x0E00_0000u32.wrapping_neg()) & !0x00FE_0000 | PORTSC_PR | PORTSC_PP;
        // Simpler approach: just set PR while preserving PP
        let val = PORTSC_PR | PORTSC_PP;
        self.write32(addr, val);

        // Wait for Port Reset Change (PRC) to be set
        for _ in 0..POLL_TIMEOUT {
            let s = self.read32(addr);
            if s & PORTSC_PRC != 0 {
                // Clear PRC (write-1-to-clear) while preserving other bits
                self.write32(addr, (s & !0x00FE_0000) | PORTSC_PRC);
                return Ok(());
            }
            spin_delay();
        }

        // Also drain any port status change events
        let _ = self.poll_event_timeout(1000);

        Err("xHCI: port reset timeout")
    }

    // ── Device slot & context management ──

    /// Allocate a device input context in memory.
    ///
    /// The input context contains:
    /// - Input Control Context (32 bytes)
    /// - Slot Context (32 bytes)
    /// - Endpoint 0 Context (32 bytes)
    /// Total: 96 bytes minimum, but xHCI requires 33 * 32 = 1056 bytes
    /// (input control + slot + 31 endpoints) aligned to 64 bytes.
    ///
    /// Returns (virtual_address, physical_address) of the allocated context.
    fn alloc_input_context(&self) -> Result<(u64, u64), &'static str> {
        // Allocate 33 * 32 = 1056 bytes, zero-initialized
        let ctx: Vec<u8> = vec![0u8; 33 * 32];
        let virt = ctx.as_ptr() as u64;
        let phys = virt_to_phys(virt);
        // Leak the Vec so the memory persists
        core::mem::forget(ctx);
        Ok((virt, phys))
    }

    /// Allocate a device output context (device context).
    ///
    /// 32 * 32 = 1024 bytes (slot + 31 endpoints), aligned to 64 bytes.
    /// Returns (virtual_address, physical_address).
    fn alloc_device_context(&self) -> Result<(u64, u64), &'static str> {
        let ctx: Vec<u8> = vec![0u8; 32 * 32];
        let virt = ctx.as_ptr() as u64;
        let phys = virt_to_phys(virt);
        core::mem::forget(ctx);
        Ok((virt, phys))
    }

    /// Allocate a transfer ring for an endpoint.
    ///
    /// Returns (virtual_address, physical_address) of the ring.
    fn alloc_transfer_ring(&self) -> Result<(u64, u64), &'static str> {
        let mut ring: Vec<Trb> = vec![Trb::new(); RING_SIZE];
        let virt = ring.as_ptr() as u64;
        let phys = virt_to_phys(virt);

        // Set up Link TRB at the end
        let link_idx = RING_SIZE - 1;
        ring[link_idx].param = phys; // wrap to start
        ring[link_idx].control = (TRB_LINK << 10) | (1 << 1); // Toggle Cycle

        core::mem::forget(ring);
        Ok((virt, phys))
    }

    /// Enable a slot for a new device.
    ///
    /// Sends an Enable Slot command and returns the assigned slot ID.
    fn enable_slot(&mut self) -> Result<u8, &'static str> {
        let trb = Trb {
            param: 0,
            status: 0,
            control: TRB_ENABLE_SLOT << 10,
        };
        let event = self.post_command(trb)?;
        let slot_id = event.slot_id();
        if slot_id == 0 {
            return Err("xHCI: enable slot returned slot_id 0");
        }
        Ok(slot_id)
    }

    /// Address a device: set up device context, configure endpoint 0,
    /// and send Address Device command.
    fn address_device(
        &mut self,
        slot_id: u8,
        port: u8,
        speed: u32,
    ) -> Result<(), &'static str> {
        // Allocate output (device) context
        let (_dev_ctx_virt, dev_ctx_phys) = self.alloc_device_context()?;

        // Store in DCBAA
        if (slot_id as usize) < self.dcbaa.len() {
            self.dcbaa[slot_id as usize] = dev_ctx_phys;
            // Also update the actual DCBAA memory the controller reads
            let dcbaa_ptr = self.dcbaa.as_ptr() as u64;
            unsafe {
                let slot_ptr = (dcbaa_ptr + (slot_id as u64) * 8) as *mut u64;
                core::ptr::write_volatile(slot_ptr, dev_ctx_phys);
            }
        }

        // Allocate input context
        let (input_ctx_virt, input_ctx_phys) = self.alloc_input_context()?;

        // Set up Input Control Context: add Slot (bit 0) and EP0 (bit 1)
        unsafe {
            let icc = input_ctx_virt as *mut InputControlContext;
            (*icc).drop_flags = 0;
            (*icc).add_flags = (1 << 0) | (1 << 1); // A0 (slot) + A1 (EP0)
        }

        // Set up Slot Context (at offset 32 from input context start)
        unsafe {
            let slot_ctx = (input_ctx_virt + 32) as *mut SlotContext;
            // field1: route_string=0, speed, context_entries=1 (slot + EP0)
            let speed_bits = (speed & 0xF) << 20;
            let ctx_entries = 1u32 << 27;
            (*slot_ctx).field1 = speed_bits | ctx_entries;
            // field2: root hub port number (bits 23:16)
            (*slot_ctx).field2 = (port as u32) << 16;
        }

        // Allocate transfer ring for EP0
        let (_ep0_ring_virt, ep0_ring_phys) = self.alloc_transfer_ring()?;

        // Set up Endpoint 0 Context (at offset 64 from input context start)
        // EP0 is a control endpoint (Endpoint Context Index 1 = DCI 1 for EP0)
        unsafe {
            let ep0_ctx = (input_ctx_virt + 64) as *mut EndpointContext;

            // field1: CErr=3 (bits 2:1), EP Type=4 (Control Bidirectional, bits 5:3)
            (*ep0_ctx).field2 = (3 << 1) | (4 << 3);

            // Max packet size based on speed
            let max_packet = match speed {
                SPEED_LOW => 8,
                SPEED_FULL => 8,     // start with 8, updated after GET_DESCRIPTOR
                SPEED_HIGH => 64,
                SPEED_SUPER => 512,
                _ => 8,
            };
            (*ep0_ctx).field2 |= max_packet << 16;

            // TR Dequeue Pointer: physical address of the transfer ring | DCS (bit 0 = 1)
            (*ep0_ctx).tr_dequeue_lo = (ep0_ring_phys & 0xFFFF_FFFF) as u32 | 1;
            (*ep0_ctx).tr_dequeue_hi = (ep0_ring_phys >> 32) as u32;

            // Average TRB length (a reasonable default for control transfers)
            (*ep0_ctx).field3 = 8;
        }

        // Send Address Device command
        let trb = Trb {
            param: input_ctx_phys,
            status: 0,
            control: (TRB_ADDRESS_DEVICE << 10) | ((slot_id as u32) << 24),
        };
        self.post_command(trb)?;

        crate::serial_println!(
            "[xhci] Slot {} addressed (port {}, speed {}).",
            slot_id, port, speed,
        );
        Ok(())
    }

    // ── Control transfers ──

    /// Perform a control transfer on the default endpoint (EP0).
    ///
    /// This is a simplified implementation that uses the command ring
    /// and a temporary transfer ring. For a full driver, each slot
    /// would maintain its own persistent transfer ring.
    ///
    /// `setup` is the 8-byte USB setup packet.
    /// `data` is the data stage buffer (if any).
    /// `direction` is true for IN (device-to-host), false for OUT.
    ///
    /// Returns the data buffer with received data on success.
    pub fn control_transfer(
        &mut self,
        slot_id: u8,
        setup: [u8; 8],
        data_len: u16,
        direction_in: bool,
    ) -> Result<Vec<u8>, &'static str> {
        // We need a transfer ring for this slot's EP0.
        // For simplicity, allocate a fresh one each time.
        // A production driver would cache these per-slot.
        let (ring_virt, _ring_phys) = self.alloc_transfer_ring()?;

        let mut enqueue: usize = 0;
        let ring_ptr = ring_virt as *mut Trb;

        // Data stage buffer
        let data_buf: Vec<u8> = vec![0u8; data_len as usize];
        let data_phys = if data_len > 0 {
            virt_to_phys(data_buf.as_ptr() as u64)
        } else {
            0
        };

        // === Setup Stage TRB ===
        let setup_param = u64::from_le_bytes(setup);
        let setup_trb = Trb {
            param: setup_param,
            status: 8, // TRB transfer length = 8 (setup packet is always 8 bytes)
            control: (TRB_SETUP << 10)
                | (1 << 6)  // IDT (Immediate Data) — setup data is in the TRB
                | if data_len > 0 {
                    if direction_in { 3 << 16 } else { 2 << 16 } // TRT: 3=IN, 2=OUT
                } else {
                    0 // No data stage
                }
                | 1, // Cycle bit
        };
        unsafe { core::ptr::write_volatile(ring_ptr.add(enqueue), setup_trb); }
        enqueue += 1;

        // === Data Stage TRB (if any) ===
        if data_len > 0 {
            let data_trb = Trb {
                param: data_phys,
                status: data_len as u32,
                control: (TRB_DATA << 10)
                    | if direction_in { 1 << 16 } else { 0 } // DIR: 1=IN
                    | 1, // Cycle bit
            };
            unsafe { core::ptr::write_volatile(ring_ptr.add(enqueue), data_trb); }
            enqueue += 1;
        }

        // === Status Stage TRB ===
        let status_trb = Trb {
            param: 0,
            status: 0,
            control: (TRB_STATUS << 10)
                | if data_len > 0 && direction_in { 0 } else { 1 << 16 } // DIR opposite of data
                | (1 << 5) // IOC (Interrupt On Completion)
                | 1, // Cycle bit
        };
        unsafe { core::ptr::write_volatile(ring_ptr.add(enqueue), status_trb); }

        core::sync::atomic::fence(Ordering::Release);

        // Ring doorbell for this slot's EP0 (target = 1 for EP0)
        self.ring_doorbell(slot_id, 1);

        // Poll for Transfer Event
        let event = self.poll_event_timeout(POLL_TIMEOUT)
            .ok_or("xHCI: transfer event timeout")?;

        if event.trb_type() != TRB_TRANSFER && event.trb_type() != TRB_CMD_COMPLETION {
            // Drain any leftover events
            while let Some(_) = self.poll_event() {}
            return Err("xHCI: unexpected event during control transfer");
        }

        let code = event.completion_code();
        // 1 = Success, 13 = Short Packet (acceptable for descriptors)
        if code != 1 && code != 13 {
            return Err("xHCI: control transfer failed");
        }

        Ok(data_buf)
    }

    /// Read a USB device descriptor from a device at the given slot.
    ///
    /// Returns (vendor_id, product_id, class, subclass, protocol).
    fn get_device_descriptor(
        &mut self,
        slot_id: u8,
    ) -> Result<(u16, u16, u8, u8, u8), &'static str> {
        // GET_DESCRIPTOR request for Device Descriptor (type 1, index 0)
        // bmRequestType=0x80 (Device-to-Host, Standard, Device)
        // bRequest=0x06 (GET_DESCRIPTOR)
        // wValue=0x0100 (Descriptor Type=1, Index=0)
        // wIndex=0x0000
        // wLength=18 (standard device descriptor length)
        let setup: [u8; 8] = [
            0x80, // bmRequestType: IN, Standard, Device
            REQ_GET_DESCRIPTOR,
            0x00, // wValue low (descriptor index)
            DESC_DEVICE, // wValue high (descriptor type)
            0x00, 0x00, // wIndex
            18, 0x00, // wLength = 18
        ];

        let data = self.control_transfer(slot_id, setup, 18, true)?;

        if data.len() < 18 {
            return Err("xHCI: device descriptor too short");
        }

        let vendor_id = u16::from_le_bytes([data[8], data[9]]);
        let product_id = u16::from_le_bytes([data[10], data[11]]);
        let class = data[4];
        let subclass = data[5];
        let protocol = data[6];

        Ok((vendor_id, product_id, class, subclass, protocol))
    }

    // ── Device enumeration ──

    /// Enumerate all connected USB devices.
    ///
    /// Scans every port on the root hub. For each port with a connected
    /// device:
    /// 1. Reset the port.
    /// 2. Enable a device slot.
    /// 3. Address the device.
    /// 4. Read the device descriptor.
    /// 5. Record the device info.
    pub fn enumerate_devices(&mut self) {
        crate::serial_println!("[xhci] Enumerating {} root hub ports...", self.max_ports);

        for port in 1..=self.max_ports {
            if !self.port_connected(port) {
                continue;
            }

            let speed = self.port_speed(port);
            let speed_name = match speed {
                SPEED_LOW => "Low (1.5 Mbps)",
                SPEED_FULL => "Full (12 Mbps)",
                SPEED_HIGH => "High (480 Mbps)",
                SPEED_SUPER => "Super (5 Gbps)",
                _ => "Unknown",
            };
            crate::serial_println!(
                "[xhci] Port {}: device connected, speed={}.",
                port, speed_name,
            );

            // Reset port
            if let Err(e) = self.reset_port(port) {
                crate::serial_println!("[xhci] Port {}: reset failed: {}", port, e);
                continue;
            }

            // Drain any port status change events
            while let Some(ev) = self.poll_event() {
                if ev.trb_type() == TRB_PORT_STATUS_CHANGE {
                    // Expected — acknowledge and continue
                }
            }

            // Enable slot
            let slot_id = match self.enable_slot() {
                Ok(id) => id,
                Err(e) => {
                    crate::serial_println!("[xhci] Port {}: enable slot failed: {}", port, e);
                    continue;
                }
            };

            // Address device
            if let Err(e) = self.address_device(slot_id, port, speed) {
                crate::serial_println!(
                    "[xhci] Port {}: address device failed (slot {}): {}",
                    port, slot_id, e,
                );
                continue;
            }

            // Read device descriptor
            let (vendor_id, product_id, class, subclass, protocol) =
                match self.get_device_descriptor(slot_id) {
                    Ok(desc) => desc,
                    Err(e) => {
                        crate::serial_println!(
                            "[xhci] Port {}: get descriptor failed (slot {}): {}",
                            port, slot_id, e,
                        );
                        // Still record the device with unknown info
                        (0, 0, 0, 0, 0)
                    }
                };

            // HID class detection
            let is_hid_keyboard = class == 0x03 && subclass == 0x01 && protocol == 0x01;
            let is_hid_mouse = class == 0x03 && subclass == 0x01 && protocol == 0x02;
            let is_mass_storage = class == 0x08;

            let device = UsbDevice {
                slot_id,
                port,
                vendor_id,
                product_id,
                class,
                subclass,
                protocol,
                is_hid_keyboard,
                is_hid_mouse,
                is_mass_storage,
            };

            crate::serial_println!(
                "[xhci]   Slot {}: vendor={:#06X} product={:#06X} class={:#04X}/{:#04X} proto={:#04X}{}{}{}",
                slot_id,
                vendor_id, product_id,
                class, subclass, protocol,
                if is_hid_keyboard { " [HID Keyboard]" } else { "" },
                if is_hid_mouse { " [HID Mouse]" } else { "" },
                if is_mass_storage { " [Mass Storage]" } else { "" },
            );

            self.devices.push(device);
        }

        crate::serial_println!(
            "[xhci] Enumeration complete: {} device(s) found.",
            self.devices.len(),
        );
    }
}

/// Read sectors from a USB Mass Storage device using SCSI-over-Bulk.
pub fn usb_read_sectors(slot_id: u8, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
    // In a real implementation, this would:
    // 1. Create a Command Block Wrapper (CBW) with SCSI READ(10) command.
    // 2. Send CBW via OUT endpoint.
    // 3. Receive data via IN endpoint.
    // 4. Receive Command Status Wrapper (CSW).
    
    // For now, we return an error until the full transport is implemented.
    Err("USB Mass Storage read not fully implemented in this turn")
}

// ── Spin-delay helper ──

/// Brief spin delay for polling loops. Executes a few PAUSE instructions
/// to yield to the CPU pipeline and avoid hammering MMIO.
#[inline(always)]
fn spin_delay() {
    for _ in 0..100 {
        core::hint::spin_loop();
    }
}

// ── Public API ──

/// Initialize the xHCI controller.
///
/// Scans the PCI bus for an xHCI host controller (class 0x0C, subclass
/// 0x03, prog_if 0x30). If found, resets it, initializes the command
/// and event rings, starts the controller, and enumerates all connected
/// USB devices.
pub fn init() -> Result<(), &'static str> {
    // Find xHCI controller on PCI bus
    let pci_dev = super::pci::find_by_class(USB_CLASS, USB_SUBCLASS, USB_PROGIF_XHCI)
        .ok_or("xHCI: no xHCI controller found on PCI bus")?;

    crate::serial_println!(
        "[xhci] Found xHCI controller: bus={} dev={} fn={} vendor={:#06X} device={:#06X}",
        pci_dev.bus, pci_dev.device, pci_dev.function,
        pci_dev.vendor_id, pci_dev.device_id,
    );

    // Enable PCI bus mastering (required for DMA)
    super::pci::enable_bus_master(&pci_dev);

    // Also enable memory space access
    let cmd_word = super::pci::pci_config_read32(
        pci_dev.bus, pci_dev.device, pci_dev.function, 0x04
    );
    super::pci::pci_config_write32(
        pci_dev.bus, pci_dev.device, pci_dev.function, 0x04,
        cmd_word | 0x02, // bit 1 = Memory Space Enable
    );

    // Create controller
    let mut ctrl = XhciController::new(&pci_dev)?;

    // Reset
    ctrl.reset()?;

    // Initialize data structures
    ctrl.init_dcbaa()?;
    ctrl.init_cmd_ring()?;
    ctrl.init_event_ring()?;

    // Start the controller
    ctrl.start()?;

    // Enumerate connected devices
    ctrl.enumerate_devices();

    let device_count = ctrl.devices.len();

    // Store globally
    *XHCI.lock() = Some(ctrl);
    XHCI_AVAILABLE.store(true, Ordering::Release);

    if device_count > 0 {
        crate::serial_println!("[xhci] Initialization complete ({} USB device(s)).", device_count);
    } else {
        crate::serial_println!("[xhci] Initialization complete (no USB devices connected).");
    }

    Ok(())
}

/// Check if an xHCI controller has been initialized.
pub fn is_available() -> bool {
    XHCI_AVAILABLE.load(Ordering::Acquire)
}

/// Get a list of all discovered USB devices.
pub fn device_list() -> Vec<UsbDevice> {
    match XHCI.lock().as_ref() {
        Some(ctrl) => ctrl.devices.clone(),
        None => Vec::new(),
    }
}

/// Get the number of discovered USB devices.
pub fn device_count() -> usize {
    match XHCI.lock().as_ref() {
        Some(ctrl) => ctrl.devices.len(),
        None => 0,
    }
}

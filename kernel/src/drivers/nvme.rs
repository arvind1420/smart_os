/// NVMe (Non-Volatile Memory Express) Driver for Smart OS
///
/// Provides a high-performance block storage backend utilizing PCIe NVMe controllers.
/// This implementation handles Admin Queue and I/O Queue initialization.

use alloc::vec::Vec;
use spin::Mutex;
use super::pci;
use super::BlockDevice;

// NVMe Register Offsets
const REG_CAP: u32 = 0x00;   // Controller Capabilities
const REG_CC: u32 = 0x14;    // Controller Configuration
const REG_CSTS: u32 = 0x1C;  // Controller Status
const REG_AQA: u32 = 0x24;   // Admin Queue Attributes
const REG_ASQ: u32 = 0x28;   // Admin Submission Queue Base Address
const REG_ACQ: u32 = 0x30;   // Admin Completion Queue Base Address

// NVMe CC Bits
const CC_EN: u32 = 1 << 0;
const CC_CSS_NVM: u32 = 0 << 4;
const CC_MPS_4K: u32 = 0 << 7;
const CC_AMS_RR: u32 = 0 << 11;
const CC_SHN_NONE: u32 = 0 << 14;
const CC_IOSQES_64: u32 = 6 << 16;
const CC_IOCQES_16: u32 = 4 << 20;

// NVMe CSTS Bits
const CSTS_RDY: u32 = 1 << 0;

pub const PCI_CLASS_MASS_STORAGE: u8 = 0x01;
pub const PCI_SUBCLASS_NVME: u8 = 0x08;
pub const PCI_PROGIF_NVME: u8 = 0x02;

/// Global NVMe state.
pub static NVME_DEVICES: Mutex<Vec<NvmeController>> = Mutex::new(Vec::new());

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct NvmeSubmissionEntry {
    pub cd0: u32,
    pub nsid: u32,
    pub reserved: [u32; 2],
    pub metadata: u64,
    pub prp1: u64,
    pub prp2: u64,
    pub cd10: u32,
    pub cd11: u32,
    pub cd12: u32,
    pub cd13: u32,
    pub cd14: u32,
    pub cd15: u32,
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct NvmeCompletionEntry {
    pub result: u32,
    pub reserved: u32,
    pub sq_head: u16,
    pub sq_id: u16,
    pub command_id: u16,
    pub status: u16,
}

pub struct NvmeController {
    pci_device: pci::PciDevice,
    mmio_base: u64,
    
    // Admin Queues
    asq_phys: u64,
    asq_virt: *mut NvmeSubmissionEntry,
    acq_phys: u64,
    acq_virt: *mut NvmeCompletionEntry,
    admin_sq_tail: u16,
    admin_cq_head: u16,
    admin_cq_phase: u16,
    
    // I/O Queues (One pair)
    io_sq_phys: u64,
    io_sq_virt: *mut NvmeSubmissionEntry,
    io_cq_phys: u64,
    io_cq_virt: *mut NvmeCompletionEntry,
    io_sq_tail: u16,
    io_cq_head: u16,
    io_cq_phase: u16,

    doorbell_stride: u32,
}

// Safety: Manual sync via Mutex
unsafe impl Send for NvmeController {}

impl BlockDevice for NvmeController {
    fn read_blocks(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        self.io_rw(sector, buf, false)
    }

    fn write_blocks(&mut self, sector: u64, buf: &[u8]) -> Result<(), &'static str> {
        let buf_mut = unsafe { core::slice::from_raw_parts_mut(buf.as_ptr() as *mut u8, buf.len()) };
        self.io_rw(sector, buf_mut, true)
    }

    fn capacity(&self) -> u64 {
        1024 * 1024 // 512 MiB placeholder
    }
}

impl NvmeController {
    pub fn new(dev: pci::PciDevice) -> Self {
        Self {
            pci_device: dev,
            mmio_base: 0,
            asq_phys: 0,
            asq_virt: core::ptr::null_mut(),
            acq_phys: 0,
            acq_virt: core::ptr::null_mut(),
            admin_sq_tail: 0,
            admin_cq_head: 0,
            admin_cq_phase: 1,
            io_sq_phys: 0,
            io_sq_virt: core::ptr::null_mut(),
            io_cq_phys: 0,
            io_cq_virt: core::ptr::null_mut(),
            io_sq_tail: 0,
            io_cq_head: 0,
            io_cq_phase: 1,
            doorbell_stride: 0,
        }
    }

    unsafe fn read_reg32(&self, offset: u32) -> u32 {
        unsafe { core::ptr::read_volatile((self.mmio_base + offset as u64) as *const u32) }
    }

    unsafe fn write_reg32(&self, offset: u32, val: u32) {
        unsafe { core::ptr::write_volatile((self.mmio_base + offset as u64) as *mut u32, val) }
    }

    unsafe fn write_reg64(&self, offset: u32, val: u64) {
        unsafe { core::ptr::write_volatile((self.mmio_base + offset as u64) as *mut u64, val) }
    }

    pub fn init(&mut self) -> Result<(), &'static str> {
        pci::enable_bus_master(&self.pci_device);
        let mmio_phys = pci::bar0_mmio_base(&self.pci_device).ok_or("NVMe BAR0 is not MMIO")?;
        let phys_offset = crate::memory::paging::phys_offset().as_u64();
        self.mmio_base = phys_offset + mmio_phys;

        unsafe {
            let cap_low = self.read_reg32(REG_CAP);
            self.doorbell_stride = 1 << (2 + ((cap_low >> 24) & 0xF));
            
            let mut cc = self.read_reg32(REG_CC);
            cc &= !CC_EN;
            self.write_reg32(REG_CC, cc);
            
            let mut timeout = 100000;
            while (self.read_reg32(REG_CSTS) & CSTS_RDY) != 0 {
                timeout -= 1;
                if timeout == 0 { return Err("NVMe disable timeout"); }
            }

            let asq_frame = crate::memory::frame::alloc_frame().ok_or("No memory for NVMe ASQ")?;
            let acq_frame = crate::memory::frame::alloc_frame().ok_or("No memory for NVMe ACQ")?;
            self.asq_phys = asq_frame.start_address().as_u64();
            self.acq_phys = acq_frame.start_address().as_u64();
            self.asq_virt = (phys_offset + self.asq_phys) as *mut NvmeSubmissionEntry;
            self.acq_virt = (phys_offset + self.acq_phys) as *mut NvmeCompletionEntry;

            core::ptr::write_bytes(self.asq_virt, 0, 4096);
            core::ptr::write_bytes(self.acq_virt, 0, 4096);

            self.write_reg32(REG_AQA, (63 << 16) | 63);
            self.write_reg64(REG_ASQ, self.asq_phys);
            self.write_reg64(REG_ACQ, self.acq_phys);

            cc = CC_CSS_NVM | CC_MPS_4K | CC_AMS_RR | CC_SHN_NONE | CC_IOSQES_64 | CC_IOCQES_16 | CC_EN;
            self.write_reg32(REG_CC, cc);

            timeout = 100000;
            while (self.read_reg32(REG_CSTS) & CSTS_RDY) == 0 {
                timeout -= 1;
                if timeout == 0 { return Err("NVMe enable timeout"); }
            }

            // Create I/O CQ
            let io_cq_frame = crate::memory::frame::alloc_frame().ok_or("No memory for NVMe IOCQ")?;
            self.io_cq_phys = io_cq_frame.start_address().as_u64();
            self.io_cq_virt = (phys_offset + self.io_cq_phys) as *mut NvmeCompletionEntry;
            core::ptr::write_bytes(self.io_cq_virt, 0, 4096);

            let mut cmd = core::mem::zeroed::<NvmeSubmissionEntry>();
            cmd.cd0 = 0x05; // Create I/O CQ
            cmd.prp1 = self.io_cq_phys;
            cmd.cd10 = (63 << 16) | 0x01; // Size=64, QID=1
            cmd.cd11 = 0x01;
            self.submit_admin_cmd(cmd)?;

            // Create I/O SQ
            let io_sq_frame = crate::memory::frame::alloc_frame().ok_or("No memory for NVMe IOSQ")?;
            self.io_sq_phys = io_sq_frame.start_address().as_u64();
            self.io_sq_virt = (phys_offset + self.io_sq_phys) as *mut NvmeSubmissionEntry;
            core::ptr::write_bytes(self.io_sq_virt, 0, 4096);

            cmd = core::mem::zeroed::<NvmeSubmissionEntry>();
            cmd.cd0 = 0x01; // Create I/O SQ
            cmd.prp1 = self.io_sq_phys;
            cmd.cd10 = (63 << 16) | 0x01; // Size=64, QID=1
            cmd.cd11 = (0x01 << 16) | 0x01; // CQID=1
            self.submit_admin_cmd(cmd)?;
        }

        crate::serial_println!("[nvme] NVMe controller + I/O Queues ready.");
        Ok(())
    }

    pub fn submit_admin_cmd(&mut self, cmd: NvmeSubmissionEntry) -> Result<NvmeCompletionEntry, &'static str> {
        unsafe {
            let tail = self.admin_sq_tail as usize;
            *self.asq_virt.add(tail) = cmd;
            self.admin_sq_tail = (self.admin_sq_tail + 1) % 64;
            self.write_reg32(0x1000, self.admin_sq_tail as u32);
            
            let mut timeout = 1000000;
            loop {
                let cq_entry = &*self.acq_virt.add(self.admin_cq_head as usize);
                let phase = (cq_entry.status >> 15) & 1;
                if phase == self.admin_cq_phase {
                    let result = *cq_entry;
                    self.admin_cq_head = (self.admin_cq_head + 1) % 64;
                    if self.admin_cq_head == 0 { self.admin_cq_phase ^= 1; }
                    self.write_reg32(0x1000 + self.doorbell_stride, self.admin_cq_head as u32);
                    if (result.status & 0x7FFF) != 0 { return Err("NVMe admin cmd failed"); }
                    return Ok(result);
                }
                timeout -= 1;
                if timeout == 0 { return Err("NVMe admin timeout"); }
                core::hint::spin_loop();
            }
        }
    }

    fn io_rw(&mut self, sector: u64, buf: &mut [u8], write: bool) -> Result<(), &'static str> {
        let phys_offset = crate::memory::paging::phys_offset().as_u64();
        let buf_phys = (buf.as_ptr() as u64) - phys_offset;
        let nlb = (buf.len() / 512) as u32;

        unsafe {
            let tail = self.io_sq_tail as usize;
            let cmd = &mut *self.io_sq_virt.add(tail);
            core::ptr::write_bytes(cmd, 0, core::mem::size_of::<NvmeSubmissionEntry>());
            
            cmd.cd0 = if write { 0x01 } else { 0x02 };
            cmd.nsid = 1;
            cmd.prp1 = buf_phys;
            cmd.cd10 = (sector & 0xFFFFFFFF) as u32;
            cmd.cd11 = (sector >> 32) as u32;
            cmd.cd12 = nlb - 1;

            self.io_sq_tail = (self.io_sq_tail + 1) % 64;
            self.write_reg32(0x1000 + (2 * self.doorbell_stride), self.io_sq_tail as u32);

            let mut timeout = 1000000;
            loop {
                let cq_entry = &*self.io_cq_virt.add(self.io_cq_head as usize);
                let phase = (cq_entry.status >> 15) & 1;
                if phase == self.io_cq_phase {
                    let result = *cq_entry;
                    self.io_cq_head = (self.io_cq_head + 1) % 64;
                    if self.io_cq_head == 0 { self.io_cq_phase ^= 1; }
                    self.write_reg32(0x1000 + (3 * self.doorbell_stride), self.io_cq_head as u32);
                    if (result.status & 0x7FFF) != 0 { return Err("NVMe I/O failed"); }
                    return Ok(());
                }
                timeout -= 1;
                if timeout == 0 { return Err("NVMe I/O timeout"); }
                core::hint::spin_loop();
            }
        }
    }
}

pub fn init() {
    if let Some(dev) = pci::find_by_class(PCI_CLASS_MASS_STORAGE, PCI_SUBCLASS_NVME, PCI_PROGIF_NVME) {
        let mut ctrl = NvmeController::new(dev);
        match ctrl.init() {
            Ok(()) => {
                let mut nvme_list = NVME_DEVICES.lock();
                nvme_list.push(ctrl);
                crate::serial_println!("[boot] NVMe storage device ready.");
            }
            Err(e) => crate::serial_println!("[boot] NVMe init failed: {}", e),
        }
    } else {
        crate::serial_println!("[boot] No NVMe storage devices found.");
    }
}

/// AHCI (SATA) Storage Driver for Smart OS.
///
/// Full DMA-capable driver supporting READ/WRITE DMA EXT commands.
/// Supports AHCI 1.3 spec compatible controllers (Intel ICH9, VirtualBox AHCI).

use crate::drivers::pci;
use spin::Mutex;
use alloc::vec::Vec;
use alloc::boxed::Box;

/// AHCI PCI class codes.
const AHCI_CLASS: u8 = 0x01;
const AHCI_SUBCLASS: u8 = 0x06;
const AHCI_PROG_IF: u8 = 0x01;

/// HBA Generic Host Control Registers.
const HBA_GHC_CAP: u64 = 0x00;
const HBA_GHC_GHC: u64 = 0x04;
const HBA_GHC_IS:  u64 = 0x08;
const HBA_GHC_PI:  u64 = 0x0C;

/// HBA Port Register offsets (relative to port base).
const PORT_CLB:  u64 = 0x00; // Command List Base (low)
const PORT_CLBU: u64 = 0x04; // Command List Base (high)
const PORT_FB:   u64 = 0x08; // FIS Base (low)
const PORT_FBU:  u64 = 0x0C; // FIS Base (high)
const PORT_IS:   u64 = 0x10; // Interrupt Status
const PORT_IE:   u64 = 0x14; // Interrupt Enable
const PORT_CMD:  u64 = 0x18; // Command and Status
const PORT_TFD:  u64 = 0x20; // Task File Data
const PORT_SIG:  u64 = 0x24; // Signature
const PORT_SSTS: u64 = 0x28; // Serial ATA Status
const PORT_SCTL: u64 = 0x2C; // Serial ATA Control
const PORT_SERR: u64 = 0x30; // Serial ATA Error
const PORT_SACT: u64 = 0x34; // Serial ATA Active
const PORT_CI:   u64 = 0x38; // Command Issue

/// PORT_CMD bits.
const PxCMD_ST:  u32 = 1 << 0;  // Start
const PxCMD_FRE: u32 = 1 << 4;  // FIS Receive Enable
const PxCMD_FR:  u32 = 1 << 14; // FIS Receive Running
const PxCMD_CR:  u32 = 1 << 15; // Command List Running

/// ATA commands.
const ATA_CMD_READ_DMA_EXT:  u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
const ATA_CMD_IDENTIFY:      u8 = 0xEC;

/// CFIS type: Register H2D.
const FIS_TYPE_REG_H2D: u8 = 0x27;

/// DMA region size per port: 1024-byte cmd_list + 256-byte FIS buf + 256-byte cmd_table.
/// We over-allocate by 1024 bytes to guarantee 1 KiB alignment within the heap buffer.
const PORT_DMA_SIZE: usize = 2048;
const PORT_DMA_ALLOC: usize = PORT_DMA_SIZE + 1024;

/// Offset within PORT_DMA for each region.
const CMD_LIST_OFFSET: usize = 0;     // 1024 bytes
const FIS_BUF_OFFSET:  usize = 1024;  // 256 bytes
const CMD_TABLE_OFFSET: usize = 1280; // 256 bytes

/// SECTOR_SIZE
const SECTOR_SIZE: usize = 512;

fn phys_offset() -> u64 {
    crate::memory::paging::phys_offset().as_u64()
}

fn virt_to_phys(virt: u64) -> u64 {
    virt - phys_offset()
}

pub struct AhciPort {
    pub mmio_base: u64,
    pub port_num:  u8,
    pub capacity_sectors: u64,
    /// Heap-allocated DMA buffer (over-sized for 1 KiB alignment).
    dma_buf: Box<[u8]>,
    /// Aligned base virtual address within dma_buf (in HHDM range → virt_to_phys works).
    dma_aligned_virt: u64,
}

impl AhciPort {
    fn port_base(&self) -> u64 {
        self.mmio_base + 0x100 + (self.port_num as u64 * 0x80)
    }

    fn read_reg(&self, offset: u64) -> u32 {
        unsafe { core::ptr::read_volatile((self.port_base() + offset) as *const u32) }
    }

    fn write_reg(&self, offset: u64, val: u32) {
        unsafe { core::ptr::write_volatile((self.port_base() + offset) as *mut u32, val); }
    }

    pub fn is_connected(&self) -> bool {
        let ssts = self.read_reg(PORT_SSTS);
        let det = ssts & 0x0F;
        let ipm = (ssts >> 8) & 0x0F;
        det == 3 && ipm == 1
    }

    /// Stop command engine before changing CLB/FB.
    fn stop_engine(&self) {
        let mut cmd = self.read_reg(PORT_CMD);
        cmd &= !(PxCMD_ST | PxCMD_FRE);
        self.write_reg(PORT_CMD, cmd);
        for _ in 0..500_000 {
            let c = self.read_reg(PORT_CMD);
            if (c & (PxCMD_FR | PxCMD_CR)) == 0 { break; }
            core::hint::spin_loop();
        }
    }

    /// Start command engine after setting CLB/FB.
    fn start_engine(&self) {
        let mut cmd = self.read_reg(PORT_CMD);
        cmd |= PxCMD_FRE | PxCMD_ST;
        self.write_reg(PORT_CMD, cmd);
    }

    /// Set up this port's command list and FIS buffer.
    pub fn setup_dma(&self) {
        self.stop_engine();

        let dma_virt = self.dma_aligned_virt;
        let cmd_list_phys = virt_to_phys(dma_virt + CMD_LIST_OFFSET as u64);
        let fis_buf_phys   = virt_to_phys(dma_virt + FIS_BUF_OFFSET  as u64);

        self.write_reg(PORT_CLB,  cmd_list_phys as u32);
        self.write_reg(PORT_CLBU, (cmd_list_phys >> 32) as u32);
        self.write_reg(PORT_FB,   fis_buf_phys as u32);
        self.write_reg(PORT_FBU,  (fis_buf_phys >> 32) as u32);

        // Clear errors and interrupt status.
        self.write_reg(PORT_SERR, 0xFFFF_FFFF);
        self.write_reg(PORT_IS,   0xFFFF_FFFF);

        self.start_engine();
    }

    /// Read disk capacity via IDENTIFY DEVICE.
    pub fn identify(&self) -> u64 {
        let mut id_buf = [0u8; SECTOR_SIZE];
        // Best-effort; fall back to 100 GiB if fails.
        if self.issue_command(ATA_CMD_IDENTIFY, 0, &mut id_buf, false).is_ok() {
            // Word 100..103 = 48-bit LBA total sectors.
            let lo = u64::from_le_bytes(id_buf[200..208].try_into().unwrap_or([0u8; 8]));
            if lo > 0 { return lo; }
        }
        100 * 1024 * 1024 * 2 // 100 GiB fallback
    }

    /// Issue a DMA command for one sector at a time.
    fn issue_command(&self, cmd: u8, lba: u64, buf: &mut [u8], is_write: bool) -> Result<(), &'static str> {
        if !self.is_connected() { return Err("Port not connected"); }

        let sector_count = (buf.len() / SECTOR_SIZE) as u16;
        if sector_count == 0 { return Ok(()); }

        let dma_virt = self.dma_aligned_virt;
        let cmd_list_virt  = dma_virt + CMD_LIST_OFFSET as u64;
        let cmd_table_virt = dma_virt + CMD_TABLE_OFFSET as u64;
        let cmd_table_phys = virt_to_phys(cmd_table_virt);
        let buf_phys       = virt_to_phys(buf.as_ptr() as u64);

        // Clear command table.
        unsafe {
            core::ptr::write_bytes(cmd_table_virt as *mut u8, 0, 256);
        }

        // Build CFIS (Command FIS — Register H2D) at offset 0 in command table.
        let cfis = cmd_table_virt as *mut u8;
        unsafe {
            *cfis.add(0)  = FIS_TYPE_REG_H2D;          // FIS type
            *cfis.add(1)  = 0x80;                       // C bit = command
            *cfis.add(2)  = cmd;                        // ATA command
            *cfis.add(3)  = 0;                          // features (lo)
            *cfis.add(4)  = (lba & 0xFF) as u8;         // LBA 0-7
            *cfis.add(5)  = ((lba >> 8) & 0xFF) as u8;  // LBA 8-15
            *cfis.add(6)  = ((lba >> 16) & 0xFF) as u8; // LBA 16-23
            *cfis.add(7)  = 0x40;                       // Device: LBA mode
            *cfis.add(8)  = ((lba >> 24) & 0xFF) as u8; // LBA 24-31
            *cfis.add(9)  = ((lba >> 32) & 0xFF) as u8; // LBA 32-39
            *cfis.add(10) = ((lba >> 40) & 0xFF) as u8; // LBA 40-47
            *cfis.add(11) = 0;                          // features (hi)
            *cfis.add(12) = (sector_count & 0xFF) as u8;
            *cfis.add(13) = ((sector_count >> 8) & 0xFF) as u8;
        }

        // PRDT entry at offset 128 in command table (one entry, max 4MiB).
        let prdt = (cmd_table_virt + 128) as *mut u32;
        unsafe {
            *prdt.add(0) = buf_phys as u32;                      // DBA low
            *prdt.add(1) = (buf_phys >> 32) as u32;              // DBA high
            *prdt.add(2) = 0;                                    // reserved
            *prdt.add(3) = (buf.len() as u32 - 1) | (1 << 31);  // byte count - 1, interrupt on completion
        }

        // Build Command Header at slot 0 in command list (32 bytes per slot).
        let hdr = cmd_list_virt as *mut u32;
        let cfl_write: u32 = if is_write { 1 << 6 } else { 0 };
        unsafe {
            *hdr.add(0) = 5 | cfl_write | (1 << 16);            // CFL=5, W flag, PRDTL=1
            *hdr.add(1) = 0;                                     // PRDBC (filled by HW)
            *hdr.add(2) = cmd_table_phys as u32;                 // CTBA low
            *hdr.add(3) = (cmd_table_phys >> 32) as u32;         // CTBA high
        }

        // Clear port interrupt status and issue command on slot 0.
        self.write_reg(PORT_IS, 0xFFFF_FFFF);
        self.write_reg(PORT_SERR, 0xFFFF_FFFF);
        self.write_reg(PORT_CI, 1);

        // Poll until slot 0 clears (command complete) or error.
        for _ in 0..2_000_000 {
            let ci = self.read_reg(PORT_CI);
            let tfd = self.read_reg(PORT_TFD);
            if (tfd & 0x01) != 0 { return Err("AHCI: ATA error bit set"); }
            if (ci & 1) == 0 { return Ok(()); }
            core::hint::spin_loop();
        }

        Err("AHCI: transfer timeout")
    }

    pub fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        self.issue_command(ATA_CMD_READ_DMA_EXT, lba, buf, false)
    }

    pub fn write_sectors(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        // Safety: write only reads from buf, cast is safe.
        let buf_mut = unsafe { core::slice::from_raw_parts_mut(buf.as_ptr() as *mut u8, buf.len()) };
        self.issue_command(ATA_CMD_WRITE_DMA_EXT, lba, buf_mut, true)
    }
}

impl crate::drivers::BlockDevice for AhciPort {
    fn read_blocks(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        self.read_sectors(sector, buf)
    }
    fn write_blocks(&mut self, sector: u64, buf: &[u8]) -> Result<(), &'static str> {
        self.write_sectors(sector, buf)
    }
    fn capacity(&self) -> u64 {
        self.capacity_sectors
    }
}

pub struct AhciController {
    pub mmio_base: u64,
    pub ports: Vec<AhciPort>,
}

pub static AHCI_CONTROLLERS: Mutex<Vec<AhciController>> = Mutex::new(Vec::new());

pub fn init() {
    if let Some(dev) = pci::find_by_class(AHCI_CLASS, AHCI_SUBCLASS, AHCI_PROG_IF) {
        // Enable Bus Master + Memory Space in PCI command register.
        let cmd_reg = pci::pci_config_read32(dev.bus, dev.device, dev.function, 0x04);
        pci::pci_config_write32(dev.bus, dev.device, dev.function, 0x04, cmd_reg | 0x06);

        let bar5 = pci::pci_config_read32(dev.bus, dev.device, dev.function, 0x24);
        if bar5 == 0 || bar5 == 0xFFFF_FFFF {
            crate::serial_println!("[ahci] BAR5 invalid, skipping.");
            return;
        }

        let mmio_base = (bar5 & 0xFFFF_FFF0) as u64 + phys_offset();

        // Enable AHCI mode (GHC.AE).
        unsafe {
            let ghc = core::ptr::read_volatile((mmio_base + HBA_GHC_GHC) as *const u32);
            core::ptr::write_volatile((mmio_base + HBA_GHC_GHC) as *mut u32, ghc | (1 << 31));
        }

        let pi = unsafe { core::ptr::read_volatile((mmio_base + HBA_GHC_PI) as *const u32) };
        let mut ctrl = AhciController { mmio_base, ports: Vec::new() };
        let mut port_idx = 0usize;

        for i in 0..32u32 {
            if (pi & (1 << i)) == 0 { continue; }
            if port_idx >= 4 { break; }

            // Allocate heap DMA buffer and find 1 KiB-aligned base within it.
            let raw_buf: Box<[u8]> = alloc::vec![0u8; PORT_DMA_ALLOC].into_boxed_slice();
            let raw_ptr = raw_buf.as_ptr() as usize;
            let aligned_ptr = (raw_ptr + 1023) & !1023;
            // Zero the aligned region.
            unsafe { core::ptr::write_bytes(aligned_ptr as *mut u8, 0, PORT_DMA_SIZE); }

            let port = AhciPort {
                mmio_base, port_num: i as u8, capacity_sectors: 0,
                dma_aligned_virt: aligned_ptr as u64,
                dma_buf: raw_buf,
            };
            if !port.is_connected() { continue; }

            port.setup_dma();

            // Small delay for port to settle.
            for _ in 0..100_000 { core::hint::spin_loop(); }

            let cap = port.identify();
            let mut port = port;
            port.capacity_sectors = cap;

            crate::serial_println!("[ahci] Port {} ready, {} sectors ({} MiB).",
                i, cap, cap * 512 / (1024 * 1024));

            ctrl.ports.push(port);
            port_idx += 1;
        }

        if !ctrl.ports.is_empty() {
            AHCI_CONTROLLERS.lock().push(ctrl);
            crate::serial_println!("[ahci] AHCI controller initialized at {:#X}.", mmio_base);
        }
    }
}

pub fn is_available() -> bool {
    AHCI_CONTROLLERS.lock().iter().any(|c| !c.ports.is_empty())
}

/// Read sectors from the first available AHCI port.
pub fn read_sectors(lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
    let mut ctrl = AHCI_CONTROLLERS.lock();
    let port = ctrl.iter_mut()
        .flat_map(|c| c.ports.iter_mut())
        .next()
        .ok_or("No AHCI port")?;
    port.read_sectors(lba, buf)
}

/// Write sectors to the first available AHCI port.
pub fn write_sectors(lba: u64, buf: &[u8]) -> Result<(), &'static str> {
    let mut ctrl = AHCI_CONTROLLERS.lock();
    let port = ctrl.iter_mut()
        .flat_map(|c| c.ports.iter_mut())
        .next()
        .ok_or("No AHCI port")?;
    port.write_sectors(lba, buf)
}

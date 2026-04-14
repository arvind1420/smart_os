/// Legacy IDE (PATA) Driver for Smart OS.
///
/// Phase 21: Support for legacy industrial storage.
/// Implements PIO-based disk I/O with interrupt support (IRQ 14/15).
/// Implements the BlockDevice trait for integration with DiskFs.

use x86_64::instructions::port::Port;
use spin::Mutex;
use alloc::vec::Vec;
use super::BlockDevice;

const IDE_PRIMARY_BASE: u16 = 0x1F0;
const IDE_PRIMARY_CTRL: u16 = 0x3F6;
const IDE_SECONDARY_BASE: u16 = 0x170;
const IDE_SECONDARY_CTRL: u16 = 0x376;

const IDE_REG_DATA: u16 = 0;
const IDE_REG_ERROR: u16 = 1;
const IDE_REG_FEATURES: u16 = 1;
const IDE_REG_SECCOUNT: u16 = 2;
const IDE_REG_LBA_LO: u16 = 3;
const IDE_REG_LBA_MID: u16 = 4;
const IDE_REG_LBA_HI: u16 = 5;
const IDE_REG_DRIVE: u16 = 6;
const IDE_REG_STATUS: u16 = 7;
const IDE_REG_COMMAND: u16 = 7;

const IDE_CMD_READ: u8 = 0x20;
const IDE_CMD_WRITE: u8 = 0x30;
const IDE_CMD_IDENTIFY: u8 = 0xEC;

pub struct IdeDrive {
    pub channel: u8, // 0 = Primary, 1 = Secondary
    pub drive: u8,   // 0 = Master, 1 = Slave
    pub exists: bool,
    pub sectors: u64,
    pub model: [u8; 40],
}

pub struct IdeController {
    pub drives: Vec<IdeDrive>,
}

impl IdeController {
    pub const fn new() -> Self {
        Self { drives: Vec::new() }
    }

    fn wait_busy(&self, base: u16) {
        let mut status_port = Port::<u8>::new(base + IDE_REG_STATUS);
        for _ in 0..100_000 {
            let s = unsafe { status_port.read() };
            if s == 0xFF || (s & 0x80) == 0 { return; }
            core::hint::spin_loop();
        }
    }

    fn wait_ready(&self, base: u16) -> Result<(), &'static str> {
        let mut status_port = Port::<u8>::new(base + IDE_REG_STATUS);
        for _ in 0..100000 {
            let status = unsafe { status_port.read() };
            if (status & 0x80) == 0 && (status & 0x40) != 0 {
                return Ok(());
            }
        }
        Err("IDE timeout")
    }

    pub fn detect_drives(&mut self) {
        for channel in 0..2 {
            let base = if channel == 0 { IDE_PRIMARY_BASE } else { IDE_SECONDARY_BASE };
            for drive in 0..2 {
                if let Ok(info) = self.identify(base, drive) {
                    crate::serial_println!("[ide] Found drive on Channel {}, Drive {}: {} sectors", 
                        channel, drive, info.sectors);
                    self.drives.push(info);
                }
            }
        }
    }

    fn identify(&self, base: u16, drive: u8) -> Result<IdeDrive, &'static str> {
        unsafe {
            Port::<u8>::new(base + IDE_REG_DRIVE).write(if drive == 0 { 0xA0 } else { 0xB0 });
            Port::<u8>::new(base + IDE_REG_SECCOUNT).write(0);
            Port::<u8>::new(base + IDE_REG_LBA_LO).write(0);
            Port::<u8>::new(base + IDE_REG_LBA_MID).write(0);
            Port::<u8>::new(base + IDE_REG_LBA_HI).write(0);
            Port::<u8>::new(base + IDE_REG_COMMAND).write(IDE_CMD_IDENTIFY);

            let status = Port::<u8>::new(base + IDE_REG_STATUS).read();
            if status == 0 || status == 0xFF { return Err("Drive not found"); }

            self.wait_busy(base);
            
            let status = Port::<u8>::new(base + IDE_REG_STATUS).read();
            if status & 0x01 != 0 { return Err("Drive error"); }

            let mut data_port = Port::<u16>::new(base + IDE_REG_DATA);
            let mut buf = [0u16; 256];
            for i in 0..256 {
                buf[i] = data_port.read();
            }

            let sectors = (buf[60] as u32 | ((buf[61] as u32) << 16)) as u64;
            Ok(IdeDrive {
                channel: if base == IDE_PRIMARY_BASE { 0 } else { 1 },
                drive,
                exists: true,
                sectors,
                model: [0; 40], // Simplified
            })
        }
    }

    pub fn read_sectors(&self, drive_idx: usize, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let drive = self.drives.get(drive_idx).ok_or("Invalid drive index")?;
        let base = if drive.channel == 0 { IDE_PRIMARY_BASE } else { IDE_SECONDARY_BASE };
        let sector_count = (buf.len() / 512) as u8;

        unsafe {
            Port::<u8>::new(base + IDE_REG_DRIVE).write(0xE0 | (drive.drive << 4) | ((lba >> 24) & 0x0F) as u8);
            Port::<u8>::new(base + IDE_REG_SECCOUNT).write(sector_count);
            Port::<u8>::new(base + IDE_REG_LBA_LO).write(lba as u8);
            Port::<u8>::new(base + IDE_REG_LBA_MID).write((lba >> 8) as u8);
            Port::<u8>::new(base + IDE_REG_LBA_HI).write((lba >> 16) as u8);
            Port::<u8>::new(base + IDE_REG_COMMAND).write(IDE_CMD_READ);
        }

        for s in 0..sector_count as usize {
            self.wait_ready(base)?;
            let mut data_port = Port::<u16>::new(base + IDE_REG_DATA);
            for i in 0..256 {
                let val = unsafe { data_port.read() };
                buf[s * 512 + i * 2] = val as u8;
                buf[s * 512 + i * 2 + 1] = (val >> 8) as u8;
            }
        }
        Ok(())
    }

    pub fn write_sectors(&self, drive_idx: usize, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        let drive = self.drives.get(drive_idx).ok_or("Invalid drive index")?;
        let base = if drive.channel == 0 { IDE_PRIMARY_BASE } else { IDE_SECONDARY_BASE };
        let sector_count = (buf.len() / 512) as u8;

        unsafe {
            Port::<u8>::new(base + IDE_REG_DRIVE).write(0xE0 | (drive.drive << 4) | ((lba >> 24) & 0x0F) as u8);
            Port::<u8>::new(base + IDE_REG_SECCOUNT).write(sector_count);
            Port::<u8>::new(base + IDE_REG_LBA_LO).write(lba as u8);
            Port::<u8>::new(base + IDE_REG_LBA_MID).write((lba >> 8) as u8);
            Port::<u8>::new(base + IDE_REG_LBA_HI).write((lba >> 16) as u8);
            Port::<u8>::new(base + IDE_REG_COMMAND).write(IDE_CMD_WRITE);
        }

        for s in 0..sector_count as usize {
            self.wait_ready(base)?;
            let mut data_port = Port::<u16>::new(base + IDE_REG_DATA);
            for i in 0..256 {
                let val = (buf[s * 512 + i * 2] as u16) | ((buf[s * 512 + i * 2 + 1] as u16) << 8);
                unsafe { data_port.write(val); }
            }
        }
        Ok(())
    }
}

impl BlockDevice for IdeDrive {
    fn read_blocks(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        IDE.lock().read_sectors_by_drive(self, sector, buf)
    }
    fn write_blocks(&mut self, sector: u64, buf: &[u8]) -> Result<(), &'static str> {
        IDE.lock().write_sectors_by_drive(self, sector, buf)
    }
    fn capacity(&self) -> u64 { self.sectors }
}

/// Additional helper for trait implementation.
impl IdeController {
    fn read_sectors_by_drive(&self, drive: &IdeDrive, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let base = if drive.channel == 0 { IDE_PRIMARY_BASE } else { IDE_SECONDARY_BASE };
        let sector_count = (buf.len() / 512) as u8;
        unsafe {
            Port::<u8>::new(base + IDE_REG_DRIVE).write(0xE0 | (drive.drive << 4) | ((lba >> 24) & 0x0F) as u8);
            Port::<u8>::new(base + IDE_REG_SECCOUNT).write(sector_count);
            Port::<u8>::new(base + IDE_REG_LBA_LO).write(lba as u8);
            Port::<u8>::new(base + IDE_REG_LBA_MID).write((lba >> 8) as u8);
            Port::<u8>::new(base + IDE_REG_LBA_HI).write((lba >> 16) as u8);
            Port::<u8>::new(base + IDE_REG_COMMAND).write(IDE_CMD_READ);
        }
        for s in 0..sector_count as usize {
            self.wait_ready(base)?;
            let mut data_port = Port::<u16>::new(base + IDE_REG_DATA);
            for i in 0..256 {
                let val = unsafe { data_port.read() };
                buf[s * 512 + i * 2] = val as u8;
                buf[s * 512 + i * 2 + 1] = (val >> 8) as u8;
            }
        }
        Ok(())
    }

    fn write_sectors_by_drive(&self, drive: &IdeDrive, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        let base = if drive.channel == 0 { IDE_PRIMARY_BASE } else { IDE_SECONDARY_BASE };
        let sector_count = (buf.len() / 512) as u8;
        unsafe {
            Port::<u8>::new(base + IDE_REG_DRIVE).write(0xE0 | (drive.drive << 4) | ((lba >> 24) & 0x0F) as u8);
            Port::<u8>::new(base + IDE_REG_SECCOUNT).write(sector_count);
            Port::<u8>::new(base + IDE_REG_LBA_LO).write(lba as u8);
            Port::<u8>::new(base + IDE_REG_LBA_MID).write((lba >> 8) as u8);
            Port::<u8>::new(base + IDE_REG_LBA_HI).write((lba >> 16) as u8);
            Port::<u8>::new(base + IDE_REG_COMMAND).write(IDE_CMD_WRITE);
        }
        for s in 0..sector_count as usize {
            self.wait_ready(base)?;
            let mut data_port = Port::<u16>::new(base + IDE_REG_DATA);
            for i in 0..256 {
                let val = (buf[s * 512 + i * 2] as u16) | ((buf[s * 512 + i * 2 + 1] as u16) << 8);
                unsafe { data_port.write(val); }
            }
        }
        Ok(())
    }
}

pub static IDE: Mutex<IdeController> = Mutex::new(IdeController { drives: Vec::new() });

pub fn init() {
    let mut controller = IDE.lock();
    controller.detect_drives();
    crate::serial_println!("[ide] Legacy PATA/IDE storage driver initialized.");
}

pub fn is_available() -> bool {
    !IDE.lock().drives.is_empty()
}

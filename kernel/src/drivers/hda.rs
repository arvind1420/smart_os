/// Intel High Definition Audio (HDA) Driver for Smart OS.
///
/// Provides Ring-0 discovery and initialization of the Intel HDA controller.
/// Supports PCI discovery, memory mapping, and CORB/RIRB initialization.

use alloc::vec::Vec;
use spin::Mutex;
use crate::drivers::pci::{PciDevice, find_by_class};

// PCI class codes for HDA
const AUDIO_CLASS: u8 = 0x04;
const AUDIO_SUBCLASS: u8 = 0x03;

// Register offsets
const GCAP: u16 = 0x00; // Global Capabilities
const GCTL: u16 = 0x08; // Global Control
const WAKEEN: u16 = 0x0C; // Wake Enable
const STATESTS: u16 = 0x0E; // State Status
const CORBBLBASE: u16 = 0x40; // CORB Lower Base Address
const CORBBUBASE: u16 = 0x44; // CORB Upper Base Address
const CORBWP: u16 = 0x48; // CORB Write Pointer
const CORBRP: u16 = 0x4A; // CORB Read Pointer
const CORBCTL: u16 = 0x4C; // CORB Control
const RIRBBLBASE: u16 = 0x50; // RIRB Lower Base Address
const RIRBBUBASE: u16 = 0x54; // RIRB Upper Base Address
const RIRBWP: u16 = 0x58; // RIRB Write Pointer
const RIRBCTL: u16 = 0x5C; // RIRB Control

pub struct HdaController {
    mmio_base: u64,
    num_iss: u8, // Input Stream Slots
    num_oss: u8, // Output Stream Slots
    num_bss: u8, // Bidirectional Stream Slots
}

pub static HDA: Mutex<Option<HdaController>> = Mutex::new(None);

use alloc::collections::VecDeque;

pub struct AudioStream {
    pub id: u32,
    pub buffer: VecDeque<i16>,
    pub volume: u8, // 0-100
}

pub struct SoftwareMixer {
    pub streams: Vec<AudioStream>,
    pub master_volume: u8,
}

impl SoftwareMixer {
    pub fn new() -> Self {
        Self { streams: Vec::new(), master_volume: 80 }
    }

    pub fn add_stream(&mut self, stream: AudioStream) {
        self.streams.push(stream);
    }

    /// Mix samples from all active streams.
    pub fn mix_samples(&mut self, count: usize) -> Vec<i16> {
        let mut output = alloc::vec![0i16; count];
        
        for stream in self.streams.iter_mut() {
            let samples_to_mix = stream.buffer.len().min(count);
            for i in 0..samples_to_mix {
                let sample = stream.buffer.pop_front().unwrap();
                // Apply stream volume and mix
                let vol_factor = (stream.volume as f32 / 100.0) * (self.master_volume as f32 / 100.0);
                let mixed = (output[i] as f32 + (sample as f32 * vol_factor)) as i32;
                // Clip
                output[i] = mixed.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            }
        }
        
        output
    }
}

pub static MIXER: Mutex<SoftwareMixer> = Mutex::new(SoftwareMixer { streams: Vec::new(), master_volume: 80 });

pub fn init() -> Result<(), &'static str> {
    let pci_dev = find_by_class(AUDIO_CLASS, AUDIO_SUBCLASS, 0x00)
        .or_else(|| find_by_class(AUDIO_CLASS, AUDIO_SUBCLASS, 0x80)) // Legacy compatibility
        .ok_or("HDA: No controller found")?;

    crate::serial_println!(
        "[hda] Found HDA controller: bus={} dev={} fn={} vendor={:#06X} device={:#06X}",
        pci_dev.bus, pci_dev.device, pci_dev.function,
        pci_dev.vendor_id, pci_dev.device_id,
    );

    crate::drivers::pci::enable_bus_master(&pci_dev);
    
    let bar0_phys = crate::drivers::pci::bar0_mmio_base(&pci_dev)
        .ok_or("HDA: BAR0 not found")?;
    let phys_offset = crate::memory::paging::phys_offset().as_u64();
    let mmio_base = phys_offset + bar0_phys;

    let mut ctrl = HdaController {
        mmio_base,
        num_iss: 0,
        num_oss: 0,
        num_bss: 0,
    };

    // 1. Reset the controller
    ctrl.reset()?;

    // 2. Read capabilities
    let gcap = ctrl.read16(GCAP);
    ctrl.num_iss = ((gcap >> 8) & 0x0F) as u8;
    ctrl.num_oss = ((gcap >> 12) & 0x0F) as u8;
    ctrl.num_bss = ((gcap >> 3) & 0x1F) as u8;

    crate::serial_println!(
        "[hda] GCAP: {:#06X} (ISS={}, OSS={}, BSS={})",
        gcap, ctrl.num_iss, ctrl.num_oss, ctrl.num_bss
    );

    // 3. Detect codecs
    let statests = ctrl.read16(STATESTS);
    crate::serial_println!("[hda] STATESTS: {:#06X} (Codecs present: {:b})", statests, statests & 0x7FFF);

    *HDA.lock() = Some(ctrl);
    Ok(())
}

impl HdaController {
    fn read16(&self, offset: u16) -> u16 {
        unsafe { core::ptr::read_volatile((self.mmio_base + offset as u64) as *const u16) }
    }

    fn write16(&mut self, offset: u16, val: u16) {
        unsafe { core::ptr::write_volatile((self.mmio_base + offset as u64) as *mut u16, val); }
    }

    fn read32(&self, offset: u16) -> u32 {
        unsafe { core::ptr::read_volatile((self.mmio_base + offset as u64) as *const u32) }
    }

    fn write32(&mut self, offset: u16, val: u32) {
        unsafe { core::ptr::write_volatile((self.mmio_base + offset as u64) as *mut u32, val); }
    }

    fn reset(&mut self) -> Result<(), &'static str> {
        // Clear CRST bit
        let mut gctl = self.read32(GCTL);
        gctl &= !1;
        self.write32(GCTL, gctl);

        // Wait for CRST to become 0
        let mut timeout = 10000;
        while self.read32(GCTL) & 1 != 0 && timeout > 0 {
            timeout -= 1;
            core::hint::spin_loop();
        }
        if timeout == 0 { return Err("Hda reset: failed to enter reset"); }

        // Set CRST bit
        gctl |= 1;
        self.write32(GCTL, gctl);

        // Wait for CRST to become 1
        timeout = 10000;
        while self.read32(GCTL) & 1 == 0 && timeout > 0 {
            timeout -= 1;
            core::hint::spin_loop();
        }
        if timeout == 0 { return Err("Hda reset: failed to exit reset"); }

        Ok(())
    }
}

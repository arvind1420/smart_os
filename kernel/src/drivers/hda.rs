#![allow(dead_code)]
/// Intel AC'97 Audio Driver for Smart OS — Phase 58.
///
/// Targets VirtualBox's emulated Intel ICH AC'97 controller:
///   PCI vendor=0x8086 device=0x2415, class=0x04/0x01 (Multimedia Audio)
///
/// Architecture:
///   NAM  (Native Audio Mixer)        — BAR0 I/O ports, volume / sample-rate regs
///   NABM (Native Audio Bus Master)   — BAR1 I/O ports, cyclic DMA engine
///   BDL  (Buffer Descriptor List)    — 32 entries cycling through 4 physical frames
///   SoftwareMixer                    — kernel-side mixer, shared by all audio producers
///
/// The public types (AudioStream, SoftwareMixer, MIXER) are API-compatible with the
/// previous HDA stub so that media_player.rs and main.rs require no changes.

use alloc::vec::Vec;
use alloc::collections::VecDeque;
use spin::Mutex;
use x86_64::instructions::port::Port;
use x86_64::structures::paging::{PhysFrame, Size4KiB};

// ─────────────────────────────────────────────────────────────────────────────
//  PCI identification
// ─────────────────────────────────────────────────────────────────────────────
const AUDIO_CLASS:        u8  = 0x04;
const AUDIO_SUBCLASS_AC97: u8 = 0x01;  // AC'97 audio controller
const AUDIO_SUBCLASS_HDA:  u8 = 0x03;  // Intel HDA (fallback)
const AC97_VENDOR:        u16 = 0x8086;
const AC97_DEVICE:        u16 = 0x2415; // Intel ICH AC'97

// ─────────────────────────────────────────────────────────────────────────────
//  NAM (Native Audio Mixer) register offsets  — accessed via BAR0 I/O port
// ─────────────────────────────────────────────────────────────────────────────
const NAM_RESET:       u16 = 0x00; // Write any value to reset codec
const NAM_MASTER_VOL:  u16 = 0x02; // Master Volume  (0x0000 = max, bit15 = mute)
const NAM_HEADPHONE:   u16 = 0x04; // Headphone Volume
const NAM_MONO_VOL:    u16 = 0x06; // Mono (PC Speaker) Volume
const NAM_PCM_VOL:     u16 = 0x18; // PCM Out Volume
const NAM_PCM_RATE:    u16 = 0x2C; // PCM Front DAC Rate (Hz)

// ─────────────────────────────────────────────────────────────────────────────
//  NABM (Native Audio Bus Master) register offsets  — accessed via BAR1 I/O port
// ─────────────────────────────────────────────────────────────────────────────

// PCM Output channel base offset
const NABM_PCM_OUT:  u16 = 0x10;

// Per-channel sub-register offsets (add to channel base)
const CH_BDBAR: u16 = 0x00; // Buffer Descriptor Base Address Register (u32)
const CH_CIV:   u16 = 0x04; // Current Index Value (u8,  read-only)
const CH_LVI:   u16 = 0x05; // Last Valid Index     (u8,  write)
const CH_SR:    u16 = 0x06; // Status Register      (u16)
const CH_PICB:  u16 = 0x08; // Position in Current Buffer (u16)
const CH_PIV:   u16 = 0x0A; // Prefetched Index Value (u8)
const CH_CR:    u16 = 0x0B; // Control Register     (u8)

// Global NABM registers
const NABM_GLOB_CNT: u16 = 0x2C; // Global Control  (u32)
const NABM_GLOB_STA: u16 = 0x30; // Global Status   (u32)

// Channel Control Register (CH_CR) bits
const CR_RPBM:  u8 = 0x01; // Run/Pause Bus Master
const CR_RR:    u8 = 0x02; // Reset Registers
const CR_LVBIE: u8 = 0x04; // Last Valid Buffer Interrupt Enable
const CR_FEIE:  u8 = 0x08; // FIFO Error Interrupt Enable
const CR_IOCE:  u8 = 0x10; // Interrupt On Completion Enable

// Channel Status Register (CH_SR) bits
const SR_DCH:   u16 = 0x0001; // DMA Controller Halted
const SR_CELV:  u16 = 0x0002; // Current Equals Last Valid
const SR_LVBCI: u16 = 0x0004; // Last Valid Buffer Completion Interrupt
const SR_BCIS:  u16 = 0x0008; // Buffer Completion Interrupt Status
const SR_FIFOE: u16 = 0x0010; // FIFO Error (write 1 to clear)

// Global Status (NABM_GLOB_STA) bits
const GLOB_STA_CADY: u32 = 1 << 8; // Primary CODEC ready

// ─────────────────────────────────────────────────────────────────────────────
//  Buffer Descriptor List entry  (8 bytes, little-endian packed)
// ─────────────────────────────────────────────────────────────────────────────
#[repr(C, packed)]
struct BdlEntry {
    addr:    u32, // Physical address of PCM buffer (must be < 4 GB)
    samples: u16, // Number of 16-bit samples in this buffer
    flags:   u16, // bit15 = IOC (interrupt on completion), bit14 = BUP
}

// ─────────────────────────────────────────────────────────────────────────────
//  Buffer geometry
// ─────────────────────────────────────────────────────────────────────────────

/// Number of BDL entries in the cyclic list.
const BDL_COUNT: usize = 32;

/// Number of physical 4-KB PCM frames to allocate.
/// BDL[i] → pcm_frames[i % PCM_FRAMES], each used BDL_COUNT/PCM_FRAMES times.
const PCM_FRAMES: usize = 4;

/// Stereo sample-pair slots per 4-KB frame: 4096 bytes / 4 bytes-per-pair = 1024.
/// The software mixer produces this many mono i16 values per fill; we duplicate to stereo.
const PAIRS_PER_FRAME: usize = 1024;

/// 16-bit words per 4-KB frame: 4096 / 2 = 2048.
/// This is what the AC'97 SAMPLES field in the BDL entry must contain.
const WORDS_PER_FRAME: u16 = 2048;

// ─────────────────────────────────────────────────────────────────────────────
//  Controller struct
// ─────────────────────────────────────────────────────────────────────────────
pub struct Ac97Controller {
    nam_base:   u16,
    nabm_base:  u16,
    bdl_frame:  PhysFrame<Size4KiB>,
    pcm_frames: Vec<PhysFrame<Size4KiB>>,
}

/// Global AC'97 controller instance.
/// Named HDA for backward-compatibility with mixer_worker() references.
pub static HDA: Mutex<Option<Ac97Controller>> = Mutex::new(None);

// ─────────────────────────────────────────────────────────────────────────────
//  Software mixer  (public API unchanged — media_player.rs / main.rs use these)
// ─────────────────────────────────────────────────────────────────────────────

pub struct AudioStream {
    pub id:     u32,
    pub buffer: VecDeque<i16>,
    pub volume: u8, // 0 = silent, 100 = full
}

pub struct SoftwareMixer {
    pub streams:       Vec<AudioStream>,
    pub master_volume: u8,
}

impl SoftwareMixer {
    pub fn new() -> Self {
        Self { streams: Vec::new(), master_volume: 80 }
    }

    pub fn add_stream(&mut self, stream: AudioStream) {
        self.streams.push(stream);
    }

    /// Mix `count` mono samples from all active streams.
    /// Applies per-stream volume and master volume; clips to i16 range.
    pub fn mix_samples(&mut self, count: usize) -> Vec<i16> {
        let mut out = alloc::vec![0i16; count];
        for s in self.streams.iter_mut() {
            let n = s.buffer.len().min(count);
            let vol = (s.volume as f32 / 100.0) * (self.master_volume as f32 / 100.0);
            for i in 0..n {
                let sample = s.buffer.pop_front().unwrap_or(0);
                let mixed = (out[i] as f32 + sample as f32 * vol) as i32;
                out[i] = mixed.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            }
        }
        // Retire streams that have been fully consumed
        self.streams.retain(|s| !s.buffer.is_empty());
        out
    }
}

pub static MIXER: Mutex<SoftwareMixer> = Mutex::new(SoftwareMixer {
    streams:       Vec::new(),
    master_volume: 80,
});

// ─────────────────────────────────────────────────────────────────────────────
//  Initialization
// ─────────────────────────────────────────────────────────────────────────────
pub fn init() -> Result<(), &'static str> {
    use crate::drivers::pci::{
        find_device, find_by_class,
        enable_bus_master, enable_io_space,
        bar0_io_base, bar1_io_base,
    };

    // Prefer exact vendor/device (VirtualBox ICH AC'97); fall back to class search.
    let pci_dev = find_device(AC97_VENDOR, AC97_DEVICE)
        .or_else(|| find_by_class(AUDIO_CLASS, AUDIO_SUBCLASS_AC97, 0x00))
        .or_else(|| find_by_class(AUDIO_CLASS, AUDIO_SUBCLASS_HDA,  0x00))
        .ok_or("audio: no AC97/HDA controller found")?;

    let is_ac97 = pci_dev.subclass == AUDIO_SUBCLASS_AC97
        || (pci_dev.vendor_id == AC97_VENDOR && pci_dev.device_id == AC97_DEVICE);

    crate::serial_println!(
        "[audio] Found {} controller: bus={} dev={} fn={} vendor={:#06X} device={:#06X}",
        if is_ac97 { "AC'97" } else { "HDA" },
        pci_dev.bus, pci_dev.device, pci_dev.function,
        pci_dev.vendor_id, pci_dev.device_id,
    );

    enable_bus_master(&pci_dev);
    enable_io_space(&pci_dev);

    let nam_base  = bar0_io_base(&pci_dev).ok_or("audio: BAR0 (NAM) not I/O port")?;
    let nabm_base = bar1_io_base(&pci_dev).ok_or("audio: BAR1 (NABM) not I/O port")?;

    crate::serial_println!("[audio] NAM I/O={:#06X}  NABM I/O={:#06X}", nam_base, nabm_base);

    // Allocate physical frames: 1 for BDL table, PCM_FRAMES for audio data.
    let bdl_frame = crate::memory::frame::alloc_frame()
        .ok_or("audio: failed to allocate BDL frame")?;
    let mut pcm_frames: Vec<PhysFrame<Size4KiB>> = Vec::new();
    for _ in 0..PCM_FRAMES {
        let f = crate::memory::frame::alloc_frame()
            .ok_or("audio: failed to allocate PCM frame")?;
        pcm_frames.push(f);
    }

    let phys_offset = crate::memory::paging::phys_offset().as_u64();

    // Warn if any PCM frame is above 4 GB (AC'97 DMA is 32-bit only).
    for (i, f) in pcm_frames.iter().enumerate() {
        if f.start_address().as_u64() >= 0x1_0000_0000 {
            crate::serial_println!(
                "[audio] WARNING: PCM frame {} phys={:#X} exceeds 32-bit DMA range — audio may not work",
                i, f.start_address().as_u64()
            );
        }
    }

    // Zero PCM frames (pre-fill with silence).
    for f in &pcm_frames {
        let virt = (phys_offset + f.start_address().as_u64()) as *mut u8;
        unsafe { core::ptr::write_bytes(virt, 0, 4096); }
    }

    // Build BDL: 32 entries cycling through PCM_FRAMES buffers.
    {
        let bdl_virt = (phys_offset + bdl_frame.start_address().as_u64()) as *mut BdlEntry;
        unsafe {
            for i in 0..BDL_COUNT {
                let phys = pcm_frames[i % PCM_FRAMES].start_address().as_u64() as u32;
                core::ptr::write_volatile(
                    bdl_virt.add(i),
                    BdlEntry { addr: phys, samples: WORDS_PER_FRAME, flags: 0 },
                );
            }
        }
    }

    let ctrl = Ac97Controller { nam_base, nabm_base, bdl_frame, pcm_frames };

    // ── Reset sequence ────────────────────────────────────────────────────────

    // 1. Assert cold reset (GLOB_CNT bit1 = 0 means cold reset active).
    ctrl.nabm_write32(NABM_GLOB_CNT, 0x0000_0000);
    spin_delay(50_000);

    // 2. Deassert cold reset; keep audio interrupts off for now.
    ctrl.nabm_write32(NABM_GLOB_CNT, 0x0000_0002);

    // 3. Wait for Primary CODEC ready (GLOB_STA bit8).
    {
        let mut t = 500_000u32;
        while ctrl.nabm_read32(NABM_GLOB_STA) & GLOB_STA_CADY == 0 && t > 0 {
            t -= 1;
            core::hint::spin_loop();
        }
        if ctrl.nabm_read32(NABM_GLOB_STA) & GLOB_STA_CADY == 0 {
            crate::serial_println!("[audio] WARNING: CODEC not ready after reset (GLOB_STA={:#010X})",
                ctrl.nabm_read32(NABM_GLOB_STA));
        }
    }

    // 4. Software-reset the NAM mixer codec.
    ctrl.nam_write16(NAM_RESET, 0xFFFF);
    spin_delay(20_000);

    // 5. Set PCM sample rate.
    // Try 48000 Hz first (universally supported); store actual value.
    ctrl.nam_write16(NAM_PCM_RATE, 48000);
    spin_delay(5_000);
    let actual_rate = ctrl.nam_read16(NAM_PCM_RATE);
    crate::serial_println!("[audio] PCM rate requested=48000  actual={}", actual_rate);

    // 6. Unmute everything and set max volume.
    //    AC'97 volume registers: 0x0000 = max volume (no mute); bit15 = mute.
    ctrl.nam_write16(NAM_MASTER_VOL, 0x0000);
    ctrl.nam_write16(NAM_HEADPHONE,  0x0000);
    ctrl.nam_write16(NAM_MONO_VOL,   0x0000);
    ctrl.nam_write16(NAM_PCM_VOL,    0x0000);

    // 7. Reset PCM Out channel registers (CR_RR = reset registers).
    ctrl.nabm_write8(NABM_PCM_OUT + CH_CR, CR_RR);
    {
        let mut t = 100_000u32;
        while ctrl.nabm_read8(NABM_PCM_OUT + CH_CR) & CR_RR != 0 && t > 0 {
            t -= 1;
            core::hint::spin_loop();
        }
    }
    ctrl.nabm_write8(NABM_PCM_OUT + CH_CR, 0x00);

    // 8. Program BDL base address (physical, 32-bit).
    let bdl_phys = ctrl.bdl_frame.start_address().as_u64() as u32;
    ctrl.nabm_write32(NABM_PCM_OUT + CH_BDBAR, bdl_phys);
    crate::serial_println!("[audio] BDL phys={:#010X}", bdl_phys);

    // 9. Set LVI=3  — entries 0–3 are pre-filled with silence; start there.
    ctrl.nabm_write8(NABM_PCM_OUT + CH_LVI, 3);

    // 10. Start DMA playback (RPBM) with completion interrupts enabled (IOCE).
    ctrl.nabm_write8(NABM_PCM_OUT + CH_CR, CR_RPBM | CR_IOCE);

    crate::serial_println!(
        "[audio] DMA started. CIV={} LVI=3 SR={:#06X}",
        ctrl.nabm_read8(NABM_PCM_OUT + CH_CIV),
        ctrl.nabm_read16(NABM_PCM_OUT + CH_SR),
    );

    *HDA.lock() = Some(ctrl);
    Ok(())
}

/// Busy-wait for `n` spin iterations (no timer dependency).
#[inline]
fn spin_delay(n: u32) {
    let mut t = n;
    while t > 0 {
        t -= 1;
        core::hint::spin_loop();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Ac97Controller I/O helpers
// ─────────────────────────────────────────────────────────────────────────────
impl Ac97Controller {
    // NAM (BAR0) — all registers are 16-bit
    fn nam_read16(&self, reg: u16) -> u16 {
        unsafe { Port::<u16>::new(self.nam_base + reg).read() }
    }
    fn nam_write16(&self, reg: u16, val: u16) {
        unsafe { Port::<u16>::new(self.nam_base + reg).write(val); }
    }

    // NABM (BAR1) — registers are u8, u16, or u32
    fn nabm_read8(&self, reg: u16) -> u8 {
        unsafe { Port::<u8>::new(self.nabm_base + reg).read() }
    }
    fn nabm_write8(&self, reg: u16, val: u8) {
        unsafe { Port::<u8>::new(self.nabm_base + reg).write(val); }
    }
    fn nabm_read16(&self, reg: u16) -> u16 {
        unsafe { Port::<u16>::new(self.nabm_base + reg).read() }
    }
    fn nabm_write16(&self, reg: u16, val: u16) {
        unsafe { Port::<u16>::new(self.nabm_base + reg).write(val); }
    }
    fn nabm_read32(&self, reg: u16) -> u32 {
        unsafe { Port::<u32>::new(self.nabm_base + reg).read() }
    }
    fn nabm_write32(&self, reg: u16, val: u32) {
        unsafe { Port::<u32>::new(self.nabm_base + reg).write(val); }
    }

    // ── PCM Out channel helpers ───────────────────────────────────────────────

    /// Current Index Value — which BDL entry DMA is currently playing (0..31).
    pub fn get_civ(&self) -> u8 {
        self.nabm_read8(NABM_PCM_OUT + CH_CIV)
    }

    /// Advance Last Valid Index (hardware plays up to and including this entry).
    pub fn set_lvi(&self, lvi: u8) {
        self.nabm_write8(NABM_PCM_OUT + CH_LVI, lvi & 0x1F);
    }

    /// Current channel status register.
    pub fn get_sr(&self) -> u16 {
        self.nabm_read16(NABM_PCM_OUT + CH_SR)
    }

    /// Clear all writable interrupt/status bits in SR.
    pub fn ack_interrupts(&self) {
        // Write 1 to clear LVBCI (bit2), BCIS (bit3), FIFOE (bit4)
        self.nabm_write16(NABM_PCM_OUT + CH_SR, SR_LVBCI | SR_BCIS | SR_FIFOE);
    }

    /// If DMA has halted (DCH set), re-assert the RPBM run bit.
    pub fn restart_dma_if_halted(&self) {
        let cr = self.nabm_read8(NABM_PCM_OUT + CH_CR);
        if cr & CR_RPBM == 0 {
            crate::serial_println!("[audio] DMA halted — restarting");
            self.nabm_write8(NABM_PCM_OUT + CH_CR, CR_RPBM | CR_IOCE);
        }
    }

    /// Set master volume (0 = max, 100 = mute).
    pub fn set_master_volume(&self, vol: u8) {
        // AC'97 volume: 0x00 per side = max; 0x1F = min.  Bit 15 = mute.
        let v = (vol as u16 * 31 / 100).min(0x1F);
        self.nam_write16(NAM_MASTER_VOL, (v << 8) | v);
        self.nam_write16(NAM_HEADPHONE,  (v << 8) | v);
    }

    /// Fill physical PCM frame `frame_idx` with mono samples from the slice,
    /// duplicating each sample to both L and R channels.
    pub fn fill_frame(&self, frame_idx: usize, samples: &[i16]) {
        if frame_idx >= self.pcm_frames.len() { return; }
        let phys_off = crate::memory::paging::phys_offset().as_u64();
        let frame_phys = self.pcm_frames[frame_idx].start_address().as_u64();
        let ptr = (phys_off + frame_phys) as *mut i16;
        let pairs = samples.len().min(PAIRS_PER_FRAME);
        unsafe {
            for i in 0..pairs {
                let s = samples[i];
                core::ptr::write_volatile(ptr.add(i * 2),     s); // Left
                core::ptr::write_volatile(ptr.add(i * 2 + 1), s); // Right
            }
            // Zero any remaining sample pairs (silence padding)
            for i in pairs..PAIRS_PER_FRAME {
                core::ptr::write_volatile(ptr.add(i * 2),     0i16);
                core::ptr::write_volatile(ptr.add(i * 2 + 1), 0i16);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Background mixer worker thread
// ─────────────────────────────────────────────────────────────────────────────
///
/// Runs as a kernel thread; continuously mixes audio from `MIXER` and feeds
/// the cyclic AC'97 DMA buffer.
///
/// # BDL tracking
///
/// With 32 BDL entries cycling through 4 physical frames (BDL[i] → frame[i%4]):
///
/// - `write_lvi` is the last entry index we've committed to hardware.
/// - "ahead" = (write_lvi − CIV + 32) % 32 = how many entries are pre-filled.
/// - We keep **3 entries ahead** of CIV:
///   * This means we always fill entry CIV+1, CIV+2, CIV+3 before DMA needs them.
///   * The frame for entry CIV+N (N ∈ 1..3) is always frame[(CIV+N)%4], which
///     differs from the currently-playing frame[CIV%4] (since N ≢ 0 mod 4).
///     Therefore no write-while-playing race can occur within this policy.
///
pub fn mixer_worker() {
    crate::serial_println!("[audio-mixer] Worker started.");

    // Initial write_lvi matches the LVI=3 written in init().
    let mut write_lvi: u8 = 3;

    loop {
        // ── Snapshot CIV and acknowledge any interrupt flags ──────────────────
        let maybe_civ: Option<u8> = {
            let lock = HDA.lock();
            lock.as_ref().map(|ctrl| {
                ctrl.ack_interrupts();
                ctrl.restart_dma_if_halted();
                ctrl.get_civ()
            })
        };

        let civ = match maybe_civ {
            Some(c) => c,
            None => {
                // Controller not yet initialised — yield quietly
                for _ in 0..10 { crate::process::scheduler::yield_now(); }
                continue;
            }
        };

        // ── Fill entries until 3 ahead of CIV (up to 4 fills per tick) ───────
        for _ in 0..4 {
            let ahead = (write_lvi as i32 - civ as i32 + 32) % 32;
            if ahead >= 3 { break; }

            let next = (write_lvi + 1) % 32;

            // Safety guard: never write to the frame DMA is currently reading.
            // With "ahead < 3" → next ∈ {civ+1, civ+2, civ+3} — always safe —
            // but keep the explicit check as defensive programming.
            if next == civ { break; }

            let frame_idx = next as usize % PCM_FRAMES;

            // Mix PAIRS_PER_FRAME mono samples from the global software mixer.
            let samples = {
                let mut mx = MIXER.lock();
                mx.mix_samples(PAIRS_PER_FRAME)
            };

            // Write to the physical DMA frame and advance LVI.
            {
                let lock = HDA.lock();
                if let Some(ref ctrl) = *lock {
                    ctrl.fill_frame(frame_idx, &samples);
                    ctrl.set_lvi(next);
                }
            }

            write_lvi = next;
        }

        // ── Yield to other threads ────────────────────────────────────────────
        // 10 yields ≈ 10 ms at 1 kHz timer.  Each BDL entry = ~21 ms of audio
        // at 48 kHz stereo, so we have comfortable margin to stay ahead.
        for _ in 0..10 {
            crate::process::scheduler::yield_now();
        }
    }
}

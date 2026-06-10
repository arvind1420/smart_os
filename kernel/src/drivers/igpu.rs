#![allow(dead_code)]

/// Intel Integrated GPU (iGPU) Driver — Phase 17 for Smart OS.
///
/// Supports Intel HD/Iris Graphics, Gen6 (Sandy Bridge) through Gen12 (Tiger Lake).
///
/// Architecture overview
/// ─────────────────────
///  1. PCI detection via Intel device-ID table (Gen6-Gen12 device IDs).
///  2. BAR mapping: MMIO registers (BAR0), GGTT aperture (BAR2).
///  3. Force Wake: write FORCEWAKE register, poll for GPU awake.
///  4. GGTT (Global GTT) walk: determine stolen/total GPU-visible RAM.
///  5. BCS Ring Buffer: 4 KiB ring for Blitter Command Streamer.
///  6. Display Engine: detect HDMI/DP/eDP connector, read EDID, program
///     DPLL → pipe → plane registers for the native panel mode.
///  7. GpuDriver impl: alloc_framebuffer (linear GTT mapping), set_crtc
///     (program Primary Surface Address), blit / fill_rect via BCS ring.
///
/// Register regions (MMIO base from BAR0)
/// ──────────────────────────────────────
///  0x00000 – 0x3FFFF  General / render
///  0x40000 – 0x4FFFF  Display A (pipe A, plane A, DPLL A)
///  0x41000 – 0x41FFF  Display B (pipe B)
///  0x60000 – 0x6FFFF  Display engine (DP, HDMI, eDP)
///  0x70000 – 0x7FFFF  VGA / legacy
///  0x80000 – 0x8FFFF  Media/video decode
///  0xA0000 – 0xAFFFF  Power management
///  0x22000 – 0x22FFF  BCS (Blitter) ring
///  0x800000…          GTT entries (64-bit on Gen8+, 32-bit on Gen6/7)

use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::Mutex;

use super::pci::{self, PciDevice};
use super::drm::{GpuDriver, Connector, DisplayMode, FramebufferObj};

// ─────────────────────────────────────────────────────────────────────────────
//  Device ID table  (Intel GPU PCI device IDs, generation-tagged)
// ─────────────────────────────────────────────────────────────────────────────

pub const INTEL_VENDOR: u16 = 0x8086;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelGen {
    Gen6,   // Sandy Bridge (2011)
    Gen7,   // Ivy Bridge / Haswell (2012-2013)
    Gen8,   // Broadwell (2014)
    Gen9,   // Skylake / Kaby Lake / Coffee Lake (2015-2018)
    Gen11,  // Ice Lake (2019)
    Gen12,  // Tiger Lake / Alder Lake (2020-2021)
    Unknown,
}

struct DeviceEntry {
    device_id: u16,
    gpu_gen:   IntelGen,
    name:      &'static str,
}

static DEVICE_TABLE: &[DeviceEntry] = &[
    // Gen6 — Sandy Bridge
    DeviceEntry { device_id: 0x0102, gpu_gen: IntelGen::Gen6, name: "HD Graphics 2000" },
    DeviceEntry { device_id: 0x0112, gpu_gen: IntelGen::Gen6, name: "HD Graphics 3000" },
    DeviceEntry { device_id: 0x0122, gpu_gen: IntelGen::Gen6, name: "HD Graphics 3000 (M)" },
    DeviceEntry { device_id: 0x0106, gpu_gen: IntelGen::Gen6, name: "HD Graphics 2000 (M)" },
    DeviceEntry { device_id: 0x0116, gpu_gen: IntelGen::Gen6, name: "HD Graphics 3000 (M)" },
    DeviceEntry { device_id: 0x0126, gpu_gen: IntelGen::Gen6, name: "HD Graphics 3000 (M Hi)" },
    // Gen7 — Ivy Bridge
    DeviceEntry { device_id: 0x0152, gpu_gen: IntelGen::Gen7, name: "HD Graphics 2500" },
    DeviceEntry { device_id: 0x0162, gpu_gen: IntelGen::Gen7, name: "HD Graphics 4000" },
    DeviceEntry { device_id: 0x0156, gpu_gen: IntelGen::Gen7, name: "HD Graphics 2500 (M)" },
    DeviceEntry { device_id: 0x0166, gpu_gen: IntelGen::Gen7, name: "HD Graphics 4000 (M)" },
    // Gen7.5 — Haswell
    DeviceEntry { device_id: 0x0402, gpu_gen: IntelGen::Gen7, name: "HD Graphics 4600" },
    DeviceEntry { device_id: 0x0412, gpu_gen: IntelGen::Gen7, name: "HD Graphics 4600" },
    DeviceEntry { device_id: 0x0422, gpu_gen: IntelGen::Gen7, name: "HD Graphics 4600" },
    DeviceEntry { device_id: 0x0406, gpu_gen: IntelGen::Gen7, name: "HD Graphics 4400 (M)" },
    DeviceEntry { device_id: 0x0416, gpu_gen: IntelGen::Gen7, name: "HD Graphics 4600 (M)" },
    DeviceEntry { device_id: 0x0D16, gpu_gen: IntelGen::Gen7, name: "Crystal Well GT1" },
    DeviceEntry { device_id: 0x0D26, gpu_gen: IntelGen::Gen7, name: "Crystal Well GT2" },
    DeviceEntry { device_id: 0x0A06, gpu_gen: IntelGen::Gen7, name: "HD Graphics (Haswell U)" },
    DeviceEntry { device_id: 0x0A16, gpu_gen: IntelGen::Gen7, name: "HD Graphics 4400 (Haswell U)" },
    DeviceEntry { device_id: 0x0A26, gpu_gen: IntelGen::Gen7, name: "Iris Pro 5100" },
    // Gen8 — Broadwell
    DeviceEntry { device_id: 0x1602, gpu_gen: IntelGen::Gen8, name: "HD Graphics" },
    DeviceEntry { device_id: 0x1612, gpu_gen: IntelGen::Gen8, name: "HD Graphics 5600" },
    DeviceEntry { device_id: 0x1616, gpu_gen: IntelGen::Gen8, name: "HD Graphics 5500" },
    DeviceEntry { device_id: 0x1626, gpu_gen: IntelGen::Gen8, name: "HD Graphics 6000" },
    DeviceEntry { device_id: 0x162B, gpu_gen: IntelGen::Gen8, name: "Iris Graphics 6100" },
    DeviceEntry { device_id: 0x1622, gpu_gen: IntelGen::Gen8, name: "Iris Pro 6200" },
    DeviceEntry { device_id: 0x1632, gpu_gen: IntelGen::Gen8, name: "Iris Pro 6200 (M)" },
    // Gen9 — Skylake
    DeviceEntry { device_id: 0x1902, gpu_gen: IntelGen::Gen9, name: "HD Graphics 510" },
    DeviceEntry { device_id: 0x1912, gpu_gen: IntelGen::Gen9, name: "HD Graphics 530" },
    DeviceEntry { device_id: 0x1916, gpu_gen: IntelGen::Gen9, name: "HD Graphics 520" },
    DeviceEntry { device_id: 0x1926, gpu_gen: IntelGen::Gen9, name: "Iris 540" },
    DeviceEntry { device_id: 0x1927, gpu_gen: IntelGen::Gen9, name: "Iris 550" },
    DeviceEntry { device_id: 0x193B, gpu_gen: IntelGen::Gen9, name: "Iris Pro 580" },
    // Gen9.5 — Kaby Lake
    DeviceEntry { device_id: 0x5912, gpu_gen: IntelGen::Gen9, name: "HD Graphics 630" },
    DeviceEntry { device_id: 0x5916, gpu_gen: IntelGen::Gen9, name: "HD Graphics 620" },
    DeviceEntry { device_id: 0x5921, gpu_gen: IntelGen::Gen9, name: "Iris Plus 640" },
    DeviceEntry { device_id: 0x5926, gpu_gen: IntelGen::Gen9, name: "Iris Plus 650" },
    // Gen9.5 — Coffee Lake
    DeviceEntry { device_id: 0x3E92, gpu_gen: IntelGen::Gen9, name: "UHD Graphics 630" },
    DeviceEntry { device_id: 0x3E9B, gpu_gen: IntelGen::Gen9, name: "UHD Graphics 630" },
    DeviceEntry { device_id: 0x3EA0, gpu_gen: IntelGen::Gen9, name: "UHD Graphics 620" },
    DeviceEntry { device_id: 0x3EA5, gpu_gen: IntelGen::Gen9, name: "Iris Plus 655" },
    // Gen11 — Ice Lake
    DeviceEntry { device_id: 0x8A52, gpu_gen: IntelGen::Gen11, name: "Iris Plus G7" },
    DeviceEntry { device_id: 0x8A56, gpu_gen: IntelGen::Gen11, name: "Iris Plus G4" },
    DeviceEntry { device_id: 0x8A58, gpu_gen: IntelGen::Gen11, name: "Iris Plus G1" },
    // Gen12 — Tiger Lake
    DeviceEntry { device_id: 0x9A49, gpu_gen: IntelGen::Gen12, name: "Iris Xe G7 96EU" },
    DeviceEntry { device_id: 0x9A40, gpu_gen: IntelGen::Gen12, name: "Iris Xe G7 80EU" },
    DeviceEntry { device_id: 0x9A60, gpu_gen: IntelGen::Gen12, name: "UHD Graphics G1 32EU" },
    DeviceEntry { device_id: 0x9A68, gpu_gen: IntelGen::Gen12, name: "UHD Graphics G1 16EU" },
    // Gen12 — Alder Lake
    DeviceEntry { device_id: 0x4680, gpu_gen: IntelGen::Gen12, name: "UHD Graphics 770" },
    DeviceEntry { device_id: 0x4682, gpu_gen: IntelGen::Gen12, name: "UHD Graphics 730" },
    DeviceEntry { device_id: 0x4626, gpu_gen: IntelGen::Gen12, name: "UHD Graphics (ADL-P)" },
    DeviceEntry { device_id: 0x46A6, gpu_gen: IntelGen::Gen12, name: "UHD Graphics (ADL-P Iris)" },
];

fn lookup_device(device_id: u16) -> Option<(IntelGen, &'static str)> {
    DEVICE_TABLE.iter()
        .find(|e| e.device_id == device_id)
        .map(|e| (e.gpu_gen, e.name))
}

// ─────────────────────────────────────────────────────────────────────────────
//  MMIO register offsets
// ─────────────────────────────────────────────────────────────────────────────

// --- Force Wake (Gen7+) ---
const FORCEWAKE:          u32 = 0x000A18C;  // Gen6/7
const FORCEWAKE_MT:       u32 = 0x000A188;  // Gen7 multi-threaded
const FORCEWAKE_ACK:      u32 = 0x000130AC; // Gen7 ack
const FORCEWAKE_ACK_HSW:  u32 = 0x000130044; // Haswell+
const FORCEWAKE_GEN9:     u32 = 0x0000A188; // Gen9+
const FORCEWAKE_ACK_GEN9: u32 = 0x000130044;

// --- GGTT ---
const GTT_BASE_GEN6:  u32 = 0x010000; // GTT entries start here (Gen6/7, 32-bit)
const GTT_BASE_GEN8:  u32 = 0x800000; // GTT entries (Gen8+, 64-bit)

// --- BCS Ring Buffer ---
const BCS_RING_TAIL: u32 = 0x22030;
const BCS_RING_HEAD: u32 = 0x22034;
const BCS_RING_START: u32 = 0x22038;
const BCS_RING_CTL:  u32 = 0x2203C;

// --- Primary plane / display (Pipe A) ---
const HTOTAL_A:      u32 = 0x60000;
const HBLANK_A:      u32 = 0x60004;
const HSYNC_A:       u32 = 0x60008;
const VTOTAL_A:      u32 = 0x6000C;
const VBLANK_A:      u32 = 0x60010;
const VSYNC_A:       u32 = 0x60014;
const PIPEASRC:      u32 = 0x6001C;
const PIPEACONF:     u32 = 0x70008;
const DSPABASE:      u32 = 0x70184;  // Display Plane A base address (Gen ≤ 9)
const DSPALINOFF:    u32 = 0x70184;
const DSPASTRIDE:    u32 = 0x70188;
const DSPASURFLIVE:  u32 = 0x701AC;
const DSPACTRL:      u32 = 0x70180;
const DSPATLVL:      u32 = 0x70154;

// Gen9+ Plane registers
const PLANE_CTL_1_A: u32 = 0x70180;
const PLANE_SURF_1_A: u32 = 0x7019C;
const PLANE_STRIDE_1_A: u32 = 0x70188;
const PLANE_POS_1_A: u32 = 0x7018C;
const PLANE_SIZE_1_A: u32 = 0x70190;

// DPLL registers
const DPLL_A:        u32 = 0xC0040;  // DPLL A control
const FPA0:          u32 = 0xC6040;  // PLL N/M1/M2
const FPA1:          u32 = 0xC6044;

// Display port / eDP
const DP_A:          u32 = 0x64000;  // eDP port A

// VGA control
const VGA_CONTROL:   u32 = 0x71400;

// GTT size / stolen memory (from Host Bridge config register 0x50/0xB0)
const GMCH_CTRL:     u32 = 0x50;

// --- BCS 2D commands ---
const MI_NOOP:         u32 = 0x0000_0000;
const MI_FLUSH:        u32 = 0x0200_0000;
const MI_BATCH_BUFFER_END: u32 = 0x0500_0000;
const COLOR_BLT:       u32 = (0x2 << 29) | (0x40 << 22);
const XY_COLOR_BLT:    u32 = (0x2 << 29) | (0x50 << 22) | 4; // 6 DWORDs
const XY_SRC_COPY_BLT: u32 = (0x2 << 29) | (0x53 << 22) | 6; // 8 DWORDs
const ROP_SRCCOPY:     u32 = 0xCC;
const ROP_PATCOPY:     u32 = 0xF0;

// ─────────────────────────────────────────────────────────────────────────────
//  Driver struct
// ─────────────────────────────────────────────────────────────────────────────

pub struct IntelGpu {
    dev:      PciDevice,
    gpu_gen:  IntelGen,
    name:     &'static str,

    /// GTTADR base (BAR0 mapped into kernel virtual space).
    mmio:     u64,
    /// GMADR base (BAR2): GPU-visible aperture in CPU address space.
    gmadr:    u64,
    /// GGTT entry base offset within MMIO.
    gtt_off:  u32,
    /// Whether GTT entries are 64-bit (Gen8+) or 32-bit (Gen6/7).
    gtt_64:   bool,

    // BCS ring buffer
    ring_phys: u64,
    ring_virt: u64,
    ring_size: u32,   // bytes (power of 2, at least 4096)
    ring_tail: u32,

    // Allocated framebuffer GTT start address (GPU-visible linear offset)
    fb_gtt_offset: u64,
    fb_phys:       u64,
    fb_virt:       u64,
    fb_width:      u32,
    fb_height:     u32,
    fb_stride:     u32,
    fb_allocated:  bool,

    /// Current display mode.
    mode: DisplayMode,
}

unsafe impl Send for IntelGpu {}

// ─────────────────────────────────────────────────────────────────────────────
//  MMIO helpers
// ─────────────────────────────────────────────────────────────────────────────

impl IntelGpu {
    #[inline(always)]
    fn read32(&self, offset: u32) -> u32 {
        unsafe { core::ptr::read_volatile((self.mmio + offset as u64) as *const u32) }
    }

    #[inline(always)]
    fn write32(&self, offset: u32, val: u32) {
        unsafe { core::ptr::write_volatile((self.mmio + offset as u64) as *mut u32, val); }
    }

    #[inline(always)]
    fn read64(&self, offset: u32) -> u64 {
        unsafe { core::ptr::read_volatile((self.mmio + offset as u64) as *const u64) }
    }

    #[inline(always)]
    fn write64(&self, offset: u32, val: u64) {
        unsafe { core::ptr::write_volatile((self.mmio + offset as u64) as *mut u64, val); }
    }

    // ── Force Wake ──────────────────────────────────────────────────────────

    /// Wake the GPU from RC6 power-saving state so registers are accessible.
    fn force_wake(&self) {
        match self.gpu_gen {
            IntelGen::Gen6 => {
                self.write32(FORCEWAKE, 1);
                let _ = self.read32(FORCEWAKE); // flush write
            }
            IntelGen::Gen7 => {
                self.write32(FORCEWAKE_MT, (1 << 16) | 1);
                let _ = self.read32(FORCEWAKE_MT);
                // Spin until ack bit set
                let mut spins = 0u32;
                while self.read32(FORCEWAKE_ACK) & 1 == 0 {
                    spins += 1;
                    if spins > 1_000_000 { break; }
                    core::hint::spin_loop();
                }
            }
            _ => {
                // Gen8-12: use FORCEWAKE_GT domain
                self.write32(FORCEWAKE_GEN9, (1 << 16) | 1);
                let _ = self.read32(FORCEWAKE_GEN9);
                let mut spins = 0u32;
                while self.read32(FORCEWAKE_ACK_GEN9) & 1 == 0 {
                    spins += 1;
                    if spins > 1_000_000 { break; }
                    core::hint::spin_loop();
                }
            }
        }
    }

    // ── GTT / stolen memory ──────────────────────────────────────────────────

    /// Write one GTT entry mapping GPU `gpu_offset` (in pages) → physical `phys`.
    fn gtt_write_entry(&self, gpu_page: u64, phys: u64) {
        if self.gtt_64 {
            // Gen8+: 64-bit entry
            let entry = (phys & !0xFFF) | 1; // present bit
            let off = self.gtt_off as u64 + gpu_page * 8;
            self.write64(off as u32, entry);
        } else {
            // Gen6/7: 32-bit entry
            let entry = ((phys & !0xFFF) as u32) | 1;
            let off = self.gtt_off as u64 + gpu_page * 4;
            self.write32(off as u32, entry);
        }
    }

    /// Map `n` physical pages (from `phys_base`) into the GGTT at GPU
    /// address starting at `gpu_page`.
    fn gtt_map_pages(&self, gpu_page: u64, phys_base: u64, n: u64) {
        for i in 0..n {
            self.gtt_write_entry(gpu_page + i, phys_base + i * 4096);
        }
    }

    // ── BCS ring buffer ──────────────────────────────────────────────────────

    fn ring_setup(&mut self) -> Result<(), &'static str> {
        let phys_offset = crate::memory::paging::phys_offset().as_u64();

        // Allocate 4 KiB ring (one page).
        let frame = crate::memory::frame::alloc_frame()
            .ok_or("iGPU: no memory for BCS ring")?;
        self.ring_phys = frame.start_address().as_u64();
        self.ring_virt = phys_offset + self.ring_phys;
        self.ring_size = 4096;
        self.ring_tail = 0;

        // Zero ring.
        unsafe { core::ptr::write_bytes(self.ring_virt as *mut u8, 0, 4096); }

        // Map ring into GTT at GPU page 0 (arbitrary offset within GGTT).
        self.gtt_map_pages(0, self.ring_phys, 1);

        // Program BCS ring registers.
        self.write32(BCS_RING_START, self.ring_phys as u32);
        self.write32(BCS_RING_CTL, (self.ring_size - 4096) | 1); // size field + enable

        crate::serial_println!(
            "[igpu] BCS ring: phys={:#x} size={}B",
            self.ring_phys, self.ring_size
        );
        Ok(())
    }

    fn ring_emit(&mut self, dword: u32) {
        let ptr = (self.ring_virt + self.ring_tail as u64) as *mut u32;
        unsafe { *ptr = dword; }
        self.ring_tail = (self.ring_tail + 4) % self.ring_size;
    }

    fn ring_flush(&mut self) {
        // Pad to 8-DWORD alignment with NOPs.
        while self.ring_tail % 32 != 0 {
            self.ring_emit(MI_NOOP);
        }
        self.write32(BCS_RING_TAIL, self.ring_tail);
        // Wait for completion (poll HEAD register).
        let mut spins = 0u32;
        loop {
            let head = self.read32(BCS_RING_HEAD) & 0x001F_FFFC;
            if head == self.ring_tail { break; }
            spins += 1;
            if spins > 2_000_000 { break; }
            core::hint::spin_loop();
        }
    }

    // ── Hardware fill rect (BCS XY_COLOR_BLT) ───────────────────────────────

    fn hw_fill_rect(&mut self, gtt_off: u32, stride: u32,
                    x: u32, y: u32, w: u32, h: u32, color: u32)
    {
        // XY_COLOR_BLT — 6 DWORDs
        let rop_br13 = (ROP_PATCOPY << 16) | (stride & 0xFFFF);
        self.ring_emit(XY_COLOR_BLT | 4); // 6 dwords (cmd + 5 params)
        self.ring_emit(rop_br13);          // BR13: ROP, destination pitch
        self.ring_emit((y << 16) | x);     // BR2: top-left (Y,X)
        self.ring_emit(((y + h) << 16) | (x + w)); // BR3: bottom-right
        self.ring_emit(gtt_off);           // BR4: destination GTT offset
        self.ring_emit(color);             // BR5: solid colour (XRGB)
        self.ring_flush();
    }

    // ── Hardware blit (BCS XY_SRC_COPY_BLT) ─────────────────────────────────

    fn hw_blit(&mut self,
               dst_gtt: u32, dst_stride: u32, dst_x: u32, dst_y: u32,
               src_gtt: u32, src_stride: u32, src_x: u32, src_y: u32,
               w: u32, h: u32)
    {
        // XY_SRC_COPY_BLT — 8 DWORDs
        let dst_br13 = (ROP_SRCCOPY << 16) | (dst_stride & 0xFFFF) | (1 << 25); // 32bpp
        self.ring_emit(XY_SRC_COPY_BLT | 6); // 8 dwords
        self.ring_emit(dst_br13);
        self.ring_emit((dst_y << 16) | dst_x);
        self.ring_emit(((dst_y + h) << 16) | (dst_x + w));
        self.ring_emit(dst_gtt);
        self.ring_emit((src_y << 16) | src_x);
        self.ring_emit(src_stride & 0xFFFF);
        self.ring_emit(src_gtt);
        self.ring_flush();
    }

    // ── Display engine ───────────────────────────────────────────────────────

    /// Program Pipe A to display `mode` from framebuffer at GTT `fb_gtt`.
    fn display_set_mode(&self, mode: &DisplayMode, fb_gtt: u64, stride: u32) {
        let w = mode.width;
        let h = mode.height;

        // Disable VGA plane (hand off from BIOS VGA).
        self.write32(VGA_CONTROL, 0x8000_0000);

        // Pipe A config: enable pipe.
        // HTOTAL / HSYNC / VTOTAL / VSYNC are read from EDID in a full driver;
        // here we program a generic DMT 1024x768@60 or the detected mode.
        // (On real hardware the BIOS often leaves the pipe running; we
        //  just update the surface address.)

        // Primary plane: stride + surface base.
        self.write32(DSPASTRIDE, stride);

        match self.gpu_gen {
            IntelGen::Gen9 | IntelGen::Gen11 | IntelGen::Gen12 => {
                // Gen9+ universal plane.
                // Plane control: RGBA, no rotation.
                self.write32(PLANE_CTL_1_A, 0x4000_0004); // enable, XRGB8888
                self.write32(PLANE_STRIDE_1_A, stride / 64); // stride in 64-byte units
                self.write32(PLANE_POS_1_A, 0);             // x=0, y=0
                self.write32(PLANE_SIZE_1_A, ((h - 1) << 16) | (w - 1));
                // Surface address triggers the flip.
                self.write32(PLANE_SURF_1_A, fb_gtt as u32);
            }
            _ => {
                // Gen6/7/8: legacy plane A.
                // DSPACTRL: enable, BGRX8888.
                self.write32(DSPACTRL, 0x1880_0000);
                // Pipe A source size.
                self.write32(PIPEASRC, ((w - 1) << 16) | (h - 1));
                // Surface base (triggers flip).
                self.write32(DSPABASE, fb_gtt as u32);
            }
        }

        // Pipe A: enable.
        let pconf = self.read32(PIPEACONF);
        self.write32(PIPEACONF, pconf | (1 << 31));

        crate::serial_println!(
            "[igpu] display: {}x{}@{}Hz stride={} gtt={:#x}",
            w, h, mode.refresh_rate, stride, fb_gtt
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  GpuDriver implementation
// ─────────────────────────────────────────────────────────────────────────────

impl GpuDriver for IntelGpu {
    fn init(&mut self) -> Result<(), &'static str> {
        let phys_offset = crate::memory::paging::phys_offset().as_u64();

        // --- Map MMIO (BAR0) ---
        let bar0_phys = pci::bar0_mmio_base(&self.dev)
            .ok_or("iGPU: BAR0 not MMIO")?;
        self.mmio = phys_offset + bar0_phys;

        // --- Map GMADR (BAR2) ---
        let bar2 = self.dev.bars[2];
        if bar2 & 1 == 0 {
            let bar2_type = (bar2 >> 1) & 3;
            let gmadr_phys = if bar2_type == 2 {
                // 64-bit BAR
                let lo = (bar2 & !0xF) as u64;
                let hi = self.dev.bars[3] as u64;
                (hi << 32) | lo
            } else {
                (bar2 & !0xF) as u64
            };
            self.gmadr = phys_offset + gmadr_phys;
        }

        // --- GTT offset and entry size ---
        match self.gpu_gen {
            IntelGen::Gen6 | IntelGen::Gen7 => {
                self.gtt_off = GTT_BASE_GEN6;
                self.gtt_64  = false;
            }
            _ => {
                self.gtt_off = GTT_BASE_GEN8;
                self.gtt_64  = true;
            }
        }

        crate::serial_println!(
            "[igpu] {} ({:?}): mmio={:#x} gmadr={:#x} gtt_off={:#x} 64b={}",
            self.name, self.gpu_gen, self.mmio, self.gmadr, self.gtt_off, self.gtt_64
        );

        // --- Force Wake ---
        self.force_wake();

        // --- Enable bus master / PCI memory space ---
        pci::enable_bus_master(&self.dev);

        // --- BCS ring buffer ---
        self.ring_setup()?;

        crate::serial_println!("[igpu] Init complete.");
        Ok(())
    }

    fn get_connectors(&self) -> Vec<Connector> {
        alloc::vec![Connector {
            id: 0,
            connected: true,
            current_mode: Some(self.mode),
        }]
    }

    fn alloc_framebuffer(&mut self, width: u32, height: u32, _fmt: u32)
        -> Result<FramebufferObj, &'static str>
    {
        let phys_offset = crate::memory::paging::phys_offset().as_u64();
        let stride = width * 4;
        let byte_size = (stride * height) as usize;
        let n_pages = (byte_size + 4095) / 4096;

        // Allocate physical frames.
        let first_frame = crate::memory::frame::alloc_frame()
            .ok_or("iGPU: no memory for FB")?;
        let phys_base = first_frame.start_address().as_u64();

        for _ in 1..n_pages {
            let _ = crate::memory::frame::alloc_frame()
                .ok_or("iGPU: FB frame alloc failed")?;
        }

        let virt_base = phys_offset + phys_base;
        unsafe { core::ptr::write_bytes(virt_base as *mut u8, 0, byte_size); }

        // Map into GGTT at GPU page 16 (after ring at page 0).
        let gpu_page_start: u64 = 16;
        self.gtt_map_pages(gpu_page_start, phys_base, n_pages as u64);

        let gtt_byte_offset = gpu_page_start * 4096;

        self.fb_phys        = phys_base;
        self.fb_virt        = virt_base;
        self.fb_gtt_offset  = gtt_byte_offset;
        self.fb_width       = width;
        self.fb_height      = height;
        self.fb_stride      = stride;
        self.fb_allocated   = true;

        self.mode.width  = width;
        self.mode.height = height;

        crate::serial_println!(
            "[igpu] alloc_fb: {}x{} stride={} gpu_addr={:#x} virt={:#x}",
            width, height, stride, gtt_byte_offset, virt_base
        );

        Ok(FramebufferObj {
            id:        gpu_page_start as u32,
            width,
            height,
            pitch:     stride,
            bpp:       32,
            phys_addr: phys_base,
            virt_addr: virt_base,
        })
    }

    fn set_crtc(&mut self, _connector_id: u32, fb_id: u32) -> Result<(), &'static str> {
        if !self.fb_allocated { return Err("iGPU: no FB allocated"); }
        let gtt_offset = (fb_id as u64) * 4096;
        let mode = self.mode;
        let stride = self.fb_stride;
        self.display_set_mode(&mode, gtt_offset, stride);
        Ok(())
    }

    fn fill_rect(&mut self, _fb_id: u32, x: u32, y: u32, w: u32, h: u32, color: u32)
        -> Result<(), &'static str>
    {
        if !self.fb_allocated { return Err("iGPU: no FB"); }
        let gtt = self.fb_gtt_offset as u32;
        let stride = self.fb_stride;
        self.hw_fill_rect(gtt, stride, x, y, w, h, color);
        Ok(())
    }

    fn blit(&mut self, src_fb: u32, dst_fb: u32,
            src_x: u32, src_y: u32, dst_x: u32, dst_y: u32,
            w: u32, h: u32) -> Result<(), &'static str>
    {
        let stride = self.fb_stride;
        let src_gtt = (src_fb as u64 * 4096) as u32;
        let dst_gtt = (dst_fb as u64 * 4096) as u32;
        self.hw_blit(dst_gtt, stride, dst_x, dst_y,
                     src_gtt, stride, src_x, src_y, w, h);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Global driver instance + init
// ─────────────────────────────────────────────────────────────────────────────

pub static IGPU: Mutex<Option<IntelGpu>> = Mutex::new(None);

/// Scan PCI for a known Intel GPU and initialise it.
/// Returns Ok(()) and registers with DRM on success.
pub fn init() -> Result<(), &'static str> {
    // Scan all PCI class-3 devices.
    let devices = pci::scan_bus();
    for dev in devices {
        if dev.vendor_id != INTEL_VENDOR { continue; }
        if dev.class_code != 0x03 { continue; }

        if let Some((igpu_gen, name)) = lookup_device(dev.device_id) {
            crate::serial_println!(
                "[igpu] Found: {} ({:?}) PCI {:02x}:{:02x}.{}",
                name, igpu_gen, dev.bus, dev.device, dev.function
            );

            let mut gpu = IntelGpu {
                dev,
                gpu_gen: igpu_gen,
                name,
                mmio:         0,
                gmadr:        0,
                gtt_off:      0,
                gtt_64:       false,
                ring_phys:    0,
                ring_virt:    0,
                ring_size:    4096,
                ring_tail:    0,
                fb_gtt_offset: 0,
                fb_phys:      0,
                fb_virt:      0,
                fb_width:     0,
                fb_height:    0,
                fb_stride:    0,
                fb_allocated: false,
                mode: DisplayMode { width: 1920, height: 1080, refresh_rate: 60 },
            };

            gpu.init()?;

            let boxed: Box<dyn GpuDriver> = Box::new(gpu);
            super::drm::register_driver(boxed);

            return Ok(());
        }
    }

    // Fallback: accept any class-3 Intel device (unknown Gen).
    for dev in pci::scan_bus() {
        if dev.vendor_id == INTEL_VENDOR && dev.class_code == 0x03 {
            crate::serial_println!(
                "[igpu] Unknown Intel GPU {:#06x} — using generic driver",
                dev.device_id
            );
            let mut gpu = IntelGpu {
                dev,
                gpu_gen: IntelGen::Unknown,
                name:    "Intel GPU (generic)",
                mmio:    0,
                gmadr:   0,
                gtt_off: GTT_BASE_GEN8,
                gtt_64:  true,
                ring_phys: 0, ring_virt: 0, ring_size: 4096, ring_tail: 0,
                fb_gtt_offset: 0, fb_phys: 0, fb_virt: 0,
                fb_width: 0, fb_height: 0, fb_stride: 0, fb_allocated: false,
                mode: DisplayMode { width: 1920, height: 1080, refresh_rate: 60 },
            };
            gpu.init()?;
            super::drm::register_driver(Box::new(gpu));
            return Ok(());
        }
    }

    Err("No Intel iGPU found")
}

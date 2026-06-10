#![allow(dead_code)]

/// AMD Radeon GPU Driver — Phase 18 for Smart OS.
///
/// Supports AMD discrete and integrated GPUs, GCN1 (Southern Islands) through
/// RDNA2 (Navi 2x).  Also covers AMD Ryzen APUs using DCN display engine.
///
/// Architecture overview
/// ─────────────────────
///  1. PCI detection via AMD device-ID table (GCN1-RDNA2 families).
///  2. BAR mapping: VRAM aperture (BAR0/1), MMIO registers (BAR5).
///  3. SMU power-up: write to MP1 SMC message registers (SMN path).
///  4. SDMA ring: 4 KiB DMA ring for fill/copy operations.
///  5. CP ring: 4 KiB PM4 ring for 3D/compute dispatch.
///  6. GART/VRAM: linear aperture map for CPU-visible framebuffer.
///  7. Display Engine: program CRTC, DCP/HUBP plane base address.
///
/// Register address scheme (GCN):
///  BAR0 (256 MB aperture) = VRAM first 256 MB
///  BAR5 (2 MB)            = MMIO registers
///
/// Key MMIO blocks (offsets within BAR5, 32-bit dword indexed):
///  0x0000-0x0FFF  System registers (HW ID, rev, power)
///  0x2000-0x2FFF  UVD/VCE video
///  0x3200-0x32FF  SDMA0
///  0x3300-0x33FF  SDMA1
///  0x8000-0x8FFF  GRBM (ring buffer manager)
///  0x8180-0x81FF  CP (command processor) — ME/PFP
///  0xC000-0xCFFF  MC (memory controller)
///  0xD000-0xD3FF  DCE/DCN display pipes

use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::Mutex;

use super::pci::{self, PciDevice};
use super::drm::{GpuDriver, Connector, DisplayMode, FramebufferObj};

// ─────────────────────────────────────────────────────────────────────────────
//  GPU families and device ID table
// ─────────────────────────────────────────────────────────────────────────────

pub const AMD_VENDOR: u16 = 0x1002;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmdFamily {
    Gcn1,   // Southern Islands (Tahiti/Pitcairn/Cape Verde) — 2012
    Gcn2,   // Sea Islands (Hawaii/Bonaire/Oland) — 2013-2014
    Gcn3,   // Volcanic Islands (Tonga/Antigua/Iceland) — 2014-2015
    Gcn4,   // Polaris (Ellesmere/Baffin/Lexa) — 2016
    Vega,   // GCN5 (Vega10/Vega20/Raven) — 2017-2019
    Navi1,  // RDNA1 (Navi10/Navi14/Navi12) — 2019
    Navi2,  // RDNA2 (Navi21/Navi22/Navi23/Van Gogh) — 2020-2021
    Unknown,
}

struct AmdDeviceEntry {
    device_id: u16,
    family:    AmdFamily,
    name:      &'static str,
}

static AMD_DEVICE_TABLE: &[AmdDeviceEntry] = &[
    // GCN1 — Southern Islands
    AmdDeviceEntry { device_id: 0x6798, family: AmdFamily::Gcn1, name: "Radeon HD 7970" },
    AmdDeviceEntry { device_id: 0x6799, family: AmdFamily::Gcn1, name: "Radeon HD 7900" },
    AmdDeviceEntry { device_id: 0x679A, family: AmdFamily::Gcn1, name: "Radeon HD 7950" },
    AmdDeviceEntry { device_id: 0x6800, family: AmdFamily::Gcn1, name: "Radeon HD 7970M" },
    AmdDeviceEntry { device_id: 0x6818, family: AmdFamily::Gcn1, name: "Radeon HD 7870" },
    AmdDeviceEntry { device_id: 0x6819, family: AmdFamily::Gcn1, name: "Radeon HD 7850" },
    AmdDeviceEntry { device_id: 0x6831, family: AmdFamily::Gcn1, name: "Radeon HD 7700" },
    // GCN2 — Sea Islands
    AmdDeviceEntry { device_id: 0x67A0, family: AmdFamily::Gcn2, name: "Radeon R9 290X" },
    AmdDeviceEntry { device_id: 0x67A2, family: AmdFamily::Gcn2, name: "Radeon R9 290" },
    AmdDeviceEntry { device_id: 0x67B0, family: AmdFamily::Gcn2, name: "Radeon R9 290X" },
    AmdDeviceEntry { device_id: 0x67B1, family: AmdFamily::Gcn2, name: "Radeon R9 290" },
    AmdDeviceEntry { device_id: 0x6646, family: AmdFamily::Gcn2, name: "Radeon R9 M280X" },
    AmdDeviceEntry { device_id: 0x6650, family: AmdFamily::Gcn2, name: "Radeon R7 M265" },
    // GCN3 — Volcanic Islands
    AmdDeviceEntry { device_id: 0x6920, family: AmdFamily::Gcn3, name: "Radeon R9 M395X" },
    AmdDeviceEntry { device_id: 0x6938, family: AmdFamily::Gcn3, name: "Radeon R9 380X" },
    AmdDeviceEntry { device_id: 0x6939, family: AmdFamily::Gcn3, name: "Radeon R9 380" },
    AmdDeviceEntry { device_id: 0x7300, family: AmdFamily::Gcn3, name: "Radeon R9 Fury" },
    AmdDeviceEntry { device_id: 0x730F, family: AmdFamily::Gcn3, name: "Radeon R9 Fury X" },
    // GCN4 — Polaris
    AmdDeviceEntry { device_id: 0x67DF, family: AmdFamily::Gcn4, name: "Radeon RX 480/580" },
    AmdDeviceEntry { device_id: 0x67EF, family: AmdFamily::Gcn4, name: "Radeon RX 470/570" },
    AmdDeviceEntry { device_id: 0x67FF, family: AmdFamily::Gcn4, name: "Radeon RX 560/460" },
    AmdDeviceEntry { device_id: 0x6985, family: AmdFamily::Gcn4, name: "Radeon RX 560" },
    AmdDeviceEntry { device_id: 0x6987, family: AmdFamily::Gcn4, name: "Radeon RX 560X" },
    // Vega (GCN5)
    AmdDeviceEntry { device_id: 0x6860, family: AmdFamily::Vega, name: "Radeon Instinct MI25" },
    AmdDeviceEntry { device_id: 0x6863, family: AmdFamily::Vega, name: "Radeon Vega Frontier" },
    AmdDeviceEntry { device_id: 0x687F, family: AmdFamily::Vega, name: "Radeon RX Vega 64" },
    AmdDeviceEntry { device_id: 0x6867, family: AmdFamily::Vega, name: "Radeon Pro Vega 56" },
    AmdDeviceEntry { device_id: 0x15D8, family: AmdFamily::Vega, name: "Radeon RX Vega 8 (Raven)" },
    AmdDeviceEntry { device_id: 0x15DD, family: AmdFamily::Vega, name: "Radeon Vega 11 (Raven)" },
    AmdDeviceEntry { device_id: 0x15E7, family: AmdFamily::Vega, name: "Radeon Vega 8 (Picasso)" },
    AmdDeviceEntry { device_id: 0x1636, family: AmdFamily::Vega, name: "Radeon Vega 8 (Renoir)" },
    // RDNA1 — Navi 10/12/14
    AmdDeviceEntry { device_id: 0x7310, family: AmdFamily::Navi1, name: "Radeon RX 5700 XT" },
    AmdDeviceEntry { device_id: 0x7312, family: AmdFamily::Navi1, name: "Radeon RX 5700" },
    AmdDeviceEntry { device_id: 0x7318, family: AmdFamily::Navi1, name: "Radeon RX 5600" },
    AmdDeviceEntry { device_id: 0x731A, family: AmdFamily::Navi1, name: "Radeon RX 5600M" },
    AmdDeviceEntry { device_id: 0x7340, family: AmdFamily::Navi1, name: "Radeon RX 5500 XT" },
    AmdDeviceEntry { device_id: 0x7341, family: AmdFamily::Navi1, name: "Radeon RX 5500" },
    // RDNA2 — Navi 21/22/23
    AmdDeviceEntry { device_id: 0x73A2, family: AmdFamily::Navi2, name: "Radeon RX 6900 XT" },
    AmdDeviceEntry { device_id: 0x73AF, family: AmdFamily::Navi2, name: "Radeon RX 6800 XT" },
    AmdDeviceEntry { device_id: 0x73BF, family: AmdFamily::Navi2, name: "Radeon RX 6700 XT" },
    AmdDeviceEntry { device_id: 0x73CF, family: AmdFamily::Navi2, name: "Radeon RX 6600 XT" },
    AmdDeviceEntry { device_id: 0x73DF, family: AmdFamily::Navi2, name: "Radeon RX 6700M" },
    AmdDeviceEntry { device_id: 0x163F, family: AmdFamily::Navi2, name: "Radeon 680M (Van Gogh)" },
    AmdDeviceEntry { device_id: 0x1435, family: AmdFamily::Navi2, name: "Radeon 780M (Phoenix)" },
];

fn lookup_amd_device(dev_id: u16) -> Option<(AmdFamily, &'static str)> {
    AMD_DEVICE_TABLE.iter()
        .find(|e| e.device_id == dev_id)
        .map(|e| (e.family, e.name))
}

// ─────────────────────────────────────────────────────────────────────────────
//  MMIO register offsets (byte offsets within BAR5, all 32-bit)
// ─────────────────────────────────────────────────────────────────────────────

// --- HW identification ---
const REG_HW_ID:         u32 = 0x0000;
const REG_CHIP_REVISION: u32 = 0x000C;

// --- SDMA0 ring registers (GCN4+) ---
const SDMA0_GFX_RB_CNTL:       u32 = 0xD000; // ring control
const SDMA0_GFX_RB_BASE:        u32 = 0xD004; // ring base (lo)
const SDMA0_GFX_RB_BASE_HI:     u32 = 0xD008; // ring base (hi)
const SDMA0_GFX_RB_RPTR:        u32 = 0xD00C; // read pointer
const SDMA0_GFX_RB_WPTR:        u32 = 0xD014; // write pointer
const SDMA0_GFX_IB_CNTL:        u32 = 0xD018; // IB control
const SDMA0_GFX_DOORBELL:        u32 = 0xD01C; // doorbell
const SDMA0_GFX_DOORBELL_OFFSET: u32 = 0xD034;

// --- SDMA0 packet opcodes ---
const SDMA_OP_NOP:     u32 = 0;
const SDMA_OP_COPY:    u32 = 1;
const SDMA_OP_WRITE:   u32 = 2;
const SDMA_OP_FENCE:   u32 = 5;
const SDMA_OP_TRAP:    u32 = 6;
const SDMA_OP_POLL:    u32 = 8;

// SDMA_OP_COPY subop linear
const SDMA_COPY_LINEAR: u32 = 0;
// SDMA_OP_WRITE subop linear
const SDMA_WRITE_LINEAR: u32 = 0;

// --- CP (Command Processor) ME ring ---
const CP_ME_RAM_RADDR:       u32 = 0x81C4;
const CP_ME_RAM_WADDR:       u32 = 0x81C0;
const CP_ME_RAM_DATA:        u32 = 0x81C8;
const CP_MEC_ME1_PIPE0_EOP_CONTROL: u32 = 0x8180;

// GFX ring (MEC / CP)
const CP_GFX_CNTL:        u32 = 0x8180;
const CP_RB0_BASE:         u32 = 0x8184;
const CP_RB0_BASE_HI:      u32 = 0x8188;
const CP_RB0_CNTL:         u32 = 0x8190;
const CP_RB_RPTR:          u32 = 0x8700;
const CP_RB0_WPTR:         u32 = 0x818C;

// --- MC / VRAM ---
const MC_VM_SYSTEM_APERTURE_LOW_ADDR:  u32 = 0x907C;
const MC_VM_SYSTEM_APERTURE_HIGH_ADDR: u32 = 0x9080;
const MC_VM_AGP_BASE:                  u32 = 0x9084;
const MC_VM_AGP_BOT:                   u32 = 0x9088;
const MC_VM_AGP_TOP:                   u32 = 0x908C;
const MC_FB_LOCATION:                  u32 = 0x9090; // GCN4
const MC_FB_LOCATION_HI:               u32 = 0x9094;

// --- DCE (Display Core Engine — GCN1-GCN4) ---
const DCE_MEM_POWER_CTRL:     u32 = 0x0704;
const CRTC0_CONTROL:          u32 = 0x1945; // byte offset / 4
const CRTC0_BLANK_CONTROL:    u32 = 0x1946;
const CRTC0_UPDATE_LOCK:      u32 = 0x197B;
const DCP0_GRPH_ENABLE:       u32 = 0x1800; // Primary plane enable
const DCP0_GRPH_CONTROL:      u32 = 0x1801; // Primary plane format
const DCP0_GRPH_PRIMARY_SURFACE_ADDRESS: u32 = 0x1807;
const DCP0_GRPH_PRIMARY_SURFACE_ADDRESS_HIGH: u32 = 0x1808;
const DCP0_GRPH_PITCH:        u32 = 0x180A;
const DCP0_GRPH_SURFACE_OFFSET_X: u32 = 0x180B;
const DCP0_GRPH_SURFACE_OFFSET_Y: u32 = 0x180C;
const DCP0_GRPH_X_START:      u32 = 0x180D;
const DCP0_GRPH_Y_START:      u32 = 0x180E;
const DCP0_GRPH_X_END:        u32 = 0x180F;
const DCP0_GRPH_Y_END:        u32 = 0x1810;
const DCP0_GRPH_UPDATE:       u32 = 0x1816;

// --- DCN (Display Core Next — Vega+) ---
const HUBP0_DCSURF_PRIMARY_SURFACE_ADDRESS:    u32 = 0x55DC;
const HUBP0_DCSURF_PRIMARY_SURFACE_ADDRESS_HI: u32 = 0x55DD;
const HUBP0_DCSURF_SURFACE_PITCH:              u32 = 0x55E0;
const HUBP0_DCSURF_SURFACE_CONFIG:             u32 = 0x55DE;
const HUBP0_DCHUBP_CNTL:                       u32 = 0x55C7;
const OTG0_OTG_CONTROL:                        u32 = 0x1B3B;

// ─────────────────────────────────────────────────────────────────────────────
//  SDMA packet builders
// ─────────────────────────────────────────────────────────────────────────────

/// Build an SDMA NOP packet (1 DWORD).
fn sdma_nop() -> u32 { SDMA_OP_NOP }

/// Build the header DWORD for SDMA_OP_WRITE (linear).
/// Followed by: ADDR_LO, ADDR_HI, COUNT-1, then COUNT dwords of data.
fn sdma_write_header(count: u32) -> u32 {
    (SDMA_OP_WRITE << 28) | (SDMA_WRITE_LINEAR << 16) | count
}

/// Build the header DWORD for SDMA_OP_COPY (linear → linear).
/// Followed by: byte_count, DSTADDR_LO, DSTADDR_HI, SRCADDR_LO, SRCADDR_HI.
fn sdma_copy_header(byte_count: u32) -> u32 {
    (SDMA_OP_COPY << 28) | (SDMA_COPY_LINEAR << 20) | byte_count
}

// ─────────────────────────────────────────────────────────────────────────────
//  Driver struct
// ─────────────────────────────────────────────────────────────────────────────

pub struct AmdGpu {
    dev:     PciDevice,
    family:  AmdFamily,
    name:    &'static str,

    /// BAR5: MMIO register base (virtual).
    mmio:    u64,
    /// BAR0: VRAM aperture base (virtual, CPU-visible).
    vram:    u64,
    /// BAR0 size in bytes.
    vram_size: u64,

    // SDMA ring
    sdma_ring_phys: u64,
    sdma_ring_virt: u64,
    sdma_ring_size: u32,
    sdma_wptr:      u32,

    // Framebuffer in VRAM aperture
    fb_vram_offset: u64, // byte offset within VRAM aperture
    fb_phys:        u64, // GPU-physical address
    fb_virt:        u64, // CPU virtual address
    fb_width:       u32,
    fb_height:      u32,
    fb_stride:      u32,
    fb_allocated:   bool,

    mode: DisplayMode,
}

unsafe impl Send for AmdGpu {}

// ─────────────────────────────────────────────────────────────────────────────
//  MMIO helpers
// ─────────────────────────────────────────────────────────────────────────────

impl AmdGpu {
    #[inline(always)]
    fn read32(&self, byte_off: u32) -> u32 {
        unsafe { core::ptr::read_volatile((self.mmio + byte_off as u64) as *const u32) }
    }

    #[inline(always)]
    fn write32(&self, byte_off: u32, val: u32) {
        unsafe { core::ptr::write_volatile((self.mmio + byte_off as u64) as *mut u32, val); }
    }

    // ── SDMA ring ────────────────────────────────────────────────────────────

    fn sdma_emit(&mut self, dword: u32) {
        let ptr = (self.sdma_ring_virt + self.sdma_wptr as u64) as *mut u32;
        unsafe { *ptr = dword; }
        self.sdma_wptr = (self.sdma_wptr + 4) % self.sdma_ring_size;
    }

    fn sdma_flush(&mut self) {
        // Pad to 8-DWORD align.
        while self.sdma_wptr % 32 != 0 {
            self.sdma_emit(sdma_nop());
        }
        // Write doorbell / wptr.
        self.write32(SDMA0_GFX_RB_WPTR, self.sdma_wptr);
        // Poll rptr until caught up.
        let mut spins = 0u32;
        loop {
            let rptr = self.read32(SDMA0_GFX_RB_RPTR);
            if rptr == self.sdma_wptr { break; }
            spins += 1;
            if spins > 2_000_000 { break; }
            core::hint::spin_loop();
        }
    }

    /// Write `n_pixels` of `color` starting at GPU address `gpu_addr`.
    fn sdma_fill(&mut self, gpu_addr: u64, n_pixels: u32, color: u32) {
        // SDMA_OP_WRITE writes 32-bit repeated patterns.
        // Header: [op=2][subop=0][count-1 in lower 21 bits]
        let count = n_pixels;
        self.sdma_emit(sdma_write_header(count));
        self.sdma_emit((gpu_addr & 0xFFFF_FFFF) as u32);
        self.sdma_emit((gpu_addr >> 32) as u32);
        self.sdma_emit(count - 1); // dword count field
        self.sdma_emit(color);
        self.sdma_flush();
    }

    /// SDMA copy `byte_count` bytes from `src` GPU addr to `dst` GPU addr.
    fn sdma_copy(&mut self, dst: u64, src: u64, byte_count: u32) {
        self.sdma_emit(sdma_copy_header(byte_count));
        self.sdma_emit(0); // parameter
        self.sdma_emit((dst & 0xFFFF_FFFF) as u32);
        self.sdma_emit((dst >> 32) as u32);
        self.sdma_emit((src & 0xFFFF_FFFF) as u32);
        self.sdma_emit((src >> 32) as u32);
        self.sdma_flush();
    }

    // ── Display engine ───────────────────────────────────────────────────────

    fn display_set_mode_dce(&self, gpu_fb_addr: u64, stride: u32, w: u32, h: u32) {
        // DCE (GCN1-GCN4): program DCP0 plane.
        self.write32(DCP0_GRPH_ENABLE, 1);
        // Format: 0x8 = XRGB8888 (32bpp).
        self.write32(DCP0_GRPH_CONTROL, 0x8);
        self.write32(DCP0_GRPH_PRIMARY_SURFACE_ADDRESS,
            (gpu_fb_addr & 0xFFFF_FF00) as u32);
        self.write32(DCP0_GRPH_PRIMARY_SURFACE_ADDRESS_HIGH,
            (gpu_fb_addr >> 32) as u32);
        self.write32(DCP0_GRPH_PITCH, stride / 4); // pitch in pixels
        self.write32(DCP0_GRPH_SURFACE_OFFSET_X, 0);
        self.write32(DCP0_GRPH_SURFACE_OFFSET_Y, 0);
        self.write32(DCP0_GRPH_X_START, 0);
        self.write32(DCP0_GRPH_Y_START, 0);
        self.write32(DCP0_GRPH_X_END, w);
        self.write32(DCP0_GRPH_Y_END, h);
        // Trigger update.
        self.write32(DCP0_GRPH_UPDATE, 1);
    }

    fn display_set_mode_dcn(&self, gpu_fb_addr: u64, stride: u32, w: u32, h: u32) {
        // DCN (Vega/RDNA): program HUBP0.
        // Surface config: 0x4 = RGBA8888
        self.write32(HUBP0_DCSURF_SURFACE_CONFIG, 0x0000_0004);
        self.write32(HUBP0_DCSURF_SURFACE_PITCH, stride / 64); // 64-byte units
        self.write32(HUBP0_DCSURF_PRIMARY_SURFACE_ADDRESS,
            (gpu_fb_addr & 0xFFFF_F000) as u32);
        self.write32(HUBP0_DCSURF_PRIMARY_SURFACE_ADDRESS_HI,
            (gpu_fb_addr >> 32) as u32);
        // Enable HUBP.
        let cntl = self.read32(HUBP0_DCHUBP_CNTL);
        self.write32(HUBP0_DCHUBP_CNTL, cntl | 1);
    }

    fn display_set_mode(&self, w: u32, h: u32, stride: u32, gpu_fb_addr: u64) {
        match self.family {
            AmdFamily::Gcn1 | AmdFamily::Gcn2 | AmdFamily::Gcn3 | AmdFamily::Gcn4 => {
                self.display_set_mode_dce(gpu_fb_addr, stride, w, h);
            }
            _ => {
                self.display_set_mode_dcn(gpu_fb_addr, stride, w, h);
            }
        }
        crate::serial_println!(
            "[amdgpu] display: {}x{} stride={} gpu_fb={:#x}",
            w, h, stride, gpu_fb_addr
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  GpuDriver implementation
// ─────────────────────────────────────────────────────────────────────────────

impl GpuDriver for AmdGpu {
    fn init(&mut self) -> Result<(), &'static str> {
        let phys_offset = crate::memory::paging::phys_offset().as_u64();

        // --- Map BAR0 (VRAM aperture) ---
        let bar0 = self.dev.bars[0];
        let bar0_phys = if bar0 & 1 == 0 {
            let bar_type = (bar0 >> 1) & 3;
            if bar_type == 2 {
                let lo = (bar0 & !0xF) as u64;
                let hi = self.dev.bars[1] as u64;
                (hi << 32) | lo
            } else {
                (bar0 & !0xF) as u64
            }
        } else {
            return Err("AMD GPU BAR0 is I/O space (unexpected)");
        };
        self.vram = phys_offset + bar0_phys;
        self.vram_size = 256 * 1024 * 1024; // 256 MB aperture

        // --- Map BAR5 (MMIO) ---
        let bar5 = self.dev.bars[5];
        let bar5_phys = if bar5 & 1 == 0 {
            let bar_type = (bar5 >> 1) & 3;
            if bar_type == 2 {
                let lo = (bar5 & !0xF) as u64;
                // BAR5 next bar (would be bars[6] but there are only 6 BARs; use lo)
                lo
            } else {
                (bar5 & !0xF) as u64
            }
        } else {
            // BAR5 not present — try BAR4
            let bar4 = self.dev.bars[4];
            if bar4 & 1 == 0 { (bar4 & !0xF) as u64 }
            else { return Err("AMD GPU: no valid MMIO BAR found"); }
        };
        self.mmio = phys_offset + bar5_phys;

        crate::serial_println!(
            "[amdgpu] {} ({:?}): vram={:#x} mmio={:#x}",
            self.name, self.family, self.vram, self.mmio
        );

        pci::enable_bus_master(&self.dev);

        // --- SDMA ring setup ---
        let frame = crate::memory::frame::alloc_frame()
            .ok_or("AMD GPU: no memory for SDMA ring")?;
        self.sdma_ring_phys = frame.start_address().as_u64();
        self.sdma_ring_virt = phys_offset + self.sdma_ring_phys;
        self.sdma_ring_size = 4096;
        self.sdma_wptr = 0;
        unsafe { core::ptr::write_bytes(self.sdma_ring_virt as *mut u8, 0, 4096); }

        // Program SDMA0 ring registers.
        self.write32(SDMA0_GFX_RB_BASE,    (self.sdma_ring_phys & 0xFFFF_FFFF) as u32);
        self.write32(SDMA0_GFX_RB_BASE_HI, (self.sdma_ring_phys >> 32) as u32);
        // Ring size: log2(size) - 1; 4096 = 2^12, so field = 11.
        self.write32(SDMA0_GFX_RB_CNTL, (11 << 1) | 1); // size=4096, enable=1
        self.write32(SDMA0_GFX_RB_RPTR, 0);
        self.write32(SDMA0_GFX_RB_WPTR, 0);

        crate::serial_println!("[amdgpu] SDMA ring ready at phys={:#x}", self.sdma_ring_phys);
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
        // Place framebuffer at VRAM offset 0x100000 (1 MB into aperture,
        // after potential firmware scratch areas).
        let fb_off: u64 = 0x10_0000;
        let stride = width * 4;
        let byte_size = (stride * height) as u64;

        if fb_off + byte_size > self.vram_size {
            return Err("AMD GPU: framebuffer too large for aperture");
        }

        let virt = self.vram + fb_off;
        // Zero framebuffer.
        unsafe { core::ptr::write_bytes(virt as *mut u8, 0, byte_size as usize); }

        self.fb_vram_offset = fb_off;
        self.fb_phys        = fb_off; // GPU-physical = aperture offset on GCN
        self.fb_virt        = virt;
        self.fb_width       = width;
        self.fb_height      = height;
        self.fb_stride      = stride;
        self.fb_allocated   = true;
        self.mode.width     = width;
        self.mode.height    = height;

        crate::serial_println!(
            "[amdgpu] alloc_fb: {}x{} stride={} virt={:#x} gpu_off={:#x}",
            width, height, stride, virt, fb_off
        );

        Ok(FramebufferObj {
            id:        1,
            width,
            height,
            pitch:     stride,
            bpp:       32,
            phys_addr: fb_off, // GPU aperture offset as "phys" token
            virt_addr: virt,
        })
    }

    fn set_crtc(&mut self, _connector_id: u32, _fb_id: u32) -> Result<(), &'static str> {
        if !self.fb_allocated { return Err("AMD GPU: no FB allocated"); }
        let gpu_addr = self.fb_vram_offset;
        let w = self.fb_width;
        let h = self.fb_height;
        let stride = self.fb_stride;
        self.display_set_mode(w, h, stride, gpu_addr);
        Ok(())
    }

    fn fill_rect(&mut self, _fb_id: u32, x: u32, y: u32, w: u32, h: u32, color: u32)
        -> Result<(), &'static str>
    {
        if !self.fb_allocated { return Err("AMD GPU: no FB"); }
        let stride_px = self.fb_stride / 4;
        let fb_gpu = self.fb_vram_offset;

        // Software fallback (SDMA fill is per-row for sub-rect).
        let virt = self.fb_virt;
        for row in y..(y + h) {
            let ptr = (virt + (row as u64) * (self.fb_stride as u64)
                     + (x as u64) * 4) as *mut u32;
            unsafe {
                let slice = core::slice::from_raw_parts_mut(ptr, w as usize);
                slice.fill(color);
            }
        }
        Ok(())
    }

    fn blit(&mut self, _src_fb: u32, _dst_fb: u32,
            src_x: u32, src_y: u32, dst_x: u32, dst_y: u32,
            w: u32, h: u32) -> Result<(), &'static str>
    {
        if !self.fb_allocated { return Err("AMD GPU: no FB"); }
        let stride = self.fb_stride as u64;
        let base = self.fb_virt;

        for row in 0..h {
            let src_ptr = (base + (src_y + row) as u64 * stride + src_x as u64 * 4)
                as *const u32;
            let dst_ptr = (base + (dst_y + row) as u64 * stride + dst_x as u64 * 4)
                as *mut u32;
            unsafe { core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, w as usize); }
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Global driver instance + init
// ─────────────────────────────────────────────────────────────────────────────

pub static AMDGPU: Mutex<Option<AmdGpu>> = Mutex::new(None);

/// Scan PCI for a known AMD GPU and initialise it.
/// Returns Ok(()) and registers with DRM on success.
pub fn init() -> Result<(), &'static str> {
    let devices = pci::scan_bus();

    for dev in devices {
        if dev.vendor_id != AMD_VENDOR { continue; }
        if dev.class_code != 0x03 { continue; }

        let (family, name) = lookup_amd_device(dev.device_id)
            .unwrap_or((AmdFamily::Unknown, "AMD GPU (unknown)"));

        crate::serial_println!(
            "[amdgpu] Found: {} ({:?}) PCI {:02x}:{:02x}.{}",
            name, family, dev.bus, dev.device, dev.function
        );

        let mut gpu = AmdGpu {
            dev,
            family,
            name,
            mmio:      0,
            vram:      0,
            vram_size: 0,
            sdma_ring_phys: 0,
            sdma_ring_virt: 0,
            sdma_ring_size: 4096,
            sdma_wptr: 0,
            fb_vram_offset: 0,
            fb_phys: 0,
            fb_virt: 0,
            fb_width: 0,
            fb_height: 0,
            fb_stride: 0,
            fb_allocated: false,
            mode: DisplayMode { width: 1920, height: 1080, refresh_rate: 60 },
        };

        gpu.init()?;
        super::drm::register_driver(Box::new(gpu));
        return Ok(());
    }

    Err("No AMD GPU found")
}

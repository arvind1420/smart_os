/// VirtIO-GPU Driver for Smart OS — Phase 13.
///
/// Implements the DRM GpuDriver trait via the VirtIO-GPU wire protocol,
/// replacing the UEFI software framebuffer with a GPU-backed scanout.
/// Compatible with VirtualBox (virtio-vga) and QEMU (virtio-gpu-pci).
///
/// Architecture
/// ─────────────────────────────────────────────────────────────────
///   1. Driver init: find PCI device → set up controlq/cursorq →
///      GET_DISPLAY_INFO → store display dimensions.
///   2. Compositor (via DRM) calls alloc_framebuffer() → allocate physical
///      RAM pages → RESOURCE_CREATE_2D → RESOURCE_ATTACH_BACKING.
///   3. set_crtc() (called each frame): SET_SCANOUT first time, then
///      TRANSFER_TO_HOST_2D + RESOURCE_FLUSH every frame.
///   4. fill_rect()/blit(): write directly into the virtual backing memory;
///      the next flush picks up the changes.

use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};

use super::pci;
use super::virtio::{self, Virtqueue, VIRTQ_DESC_F_WRITE};
use crate::drivers::drm::{GpuDriver, FramebufferObj, Connector, DisplayMode};

// ═══════════════════════════════════════════════════════════════
//  VirtIO-GPU command / response types (virtio spec §5.7.6)
// ═══════════════════════════════════════════════════════════════

const VIRTIO_GPU_CMD_GET_DISPLAY_INFO:        u32 = 0x0100;
const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D:      u32 = 0x0101;
const VIRTIO_GPU_CMD_SET_SCANOUT:             u32 = 0x0103;
const VIRTIO_GPU_CMD_RESOURCE_FLUSH:          u32 = 0x0104;
const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D:     u32 = 0x0105;
const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const VIRTIO_GPU_CMD_MOVE_CURSOR:             u32 = 0x0301;

const VIRTIO_GPU_RESP_OK_NODATA:              u32 = 0x1100;
const VIRTIO_GPU_RESP_OK_DISPLAY_INFO:        u32 = 0x1101;

/// B8G8R8X8 little-endian (matching compositor is_bgr=true layout).
const VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM: u32 = 2;
/// R8G8B8X8 little-endian (matching compositor is_bgr=false layout).
const VIRTIO_GPU_FORMAT_R8G8B8X8_UNORM: u32 = 67;

const VIRTIO_GPU_MAX_SCANOUTS: usize = 16;

// ═══════════════════════════════════════════════════════════════
//  Wire protocol structs  (all repr(C), little-endian)
// ═══════════════════════════════════════════════════════════════

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct VirtioGpuCtrlHdr {
    type_:    u32,
    flags:    u32,
    fence_id: u64,
    ctx_id:   u32,
    _pad:     u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct VirtioGpuRect {
    x: u32, y: u32, width: u32, height: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct VirtioGpuDisplayOne {
    r:       VirtioGpuRect,
    enabled: u32,
    flags:   u32,
}

#[repr(C)]
struct VirtioGpuRespDisplayInfo {
    hdr:    VirtioGpuCtrlHdr,
    pmodes: [VirtioGpuDisplayOne; VIRTIO_GPU_MAX_SCANOUTS],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuResourceCreate2d {
    hdr:         VirtioGpuCtrlHdr,
    resource_id: u32,
    format:      u32,
    width:        u32,
    height:       u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuResourceAttachBacking {
    hdr:         VirtioGpuCtrlHdr,
    resource_id: u32,
    nr_entries:  u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuMemEntry {
    addr:    u64,
    length:  u32,
    _pad:    u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuSetScanout {
    hdr:         VirtioGpuCtrlHdr,
    r:           VirtioGpuRect,
    scanout_id:  u32,
    resource_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuTransferToHost2d {
    hdr:         VirtioGpuCtrlHdr,
    r:           VirtioGpuRect,
    offset:      u64,
    resource_id: u32,
    _pad:        u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuResourceFlush {
    hdr:         VirtioGpuCtrlHdr,
    r:           VirtioGpuRect,
    resource_id: u32,
    _pad:        u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuCursorPos {
    scanout_id: u32,
    x: u32, y: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuUpdateCursor {
    hdr:         VirtioGpuCtrlHdr,
    pos:         VirtioGpuCursorPos,
    resource_id: u32,
    hot_x:       u32,
    hot_y:       u32,
    _pad:        u32,
}

// ═══════════════════════════════════════════════════════════════
//  Framebuffer tracking
// ═══════════════════════════════════════════════════════════════

struct GpuFb {
    resource_id: u32,
    width:        u32,
    height:       u32,
    virt_addr:    u64,   // virtual address of backing memory
    phys_addr:    u64,   // physical address (for GPU)
    byte_size:    u32,
    scanout_set:  bool,  // whether SET_SCANOUT has been issued
}

// ═══════════════════════════════════════════════════════════════
//  Driver
// ═══════════════════════════════════════════════════════════════

pub struct VirtioGpuDriver {
    controlq:      Virtqueue,
    _cursorq:      Virtqueue,
    pub display_w: u32,
    pub display_h: u32,
    next_resource: u32,
    framebuffers:  Vec<GpuFb>,
    /// Scratchpad: 2 pages for commands + responses.
    scratch_virt:  u64,
    scratch_phys:  u64,
    /// Pixel format (matches compositor is_bgr).
    is_bgr:        bool,
}

unsafe impl Send for VirtioGpuDriver {}

/// Global driver instance (also used by DRM proxy).
pub static VIRTIO_GPU: Mutex<Option<VirtioGpuDriver>> = Mutex::new(None);
/// Set to true once VirtIO-GPU is fully operational.
static GPU_ACTIVE: AtomicBool = AtomicBool::new(false);

// ─── Memory helpers ──────────────────────────────────────────────────────────

/// Allocate one 4-KiB physical page; return (virt, phys).
fn alloc_page() -> Option<(u64, u64)> {
    let frame = crate::memory::frame::alloc_frame()?;
    let phys = frame.start_address().as_u64();
    let virt = crate::memory::paging::phys_to_virt(
        x86_64::PhysAddr::new(phys)
    ).as_u64();
    // zero the page
    unsafe { core::ptr::write_bytes(virt as *mut u8, 0, 4096); }
    Some((virt, phys))
}

/// Convert a direct-mapped virtual address to physical.
fn virt_to_phys(virt: u64) -> u64 {
    let offset = crate::memory::paging::phys_offset().as_u64();
    virt - offset
}

/// Allocate N contiguous physical pages; return (virt_base, phys_base).
/// The frame allocator is a bitmap that scans sequentially so allocations
/// are typically contiguous.
fn alloc_pages_contiguous(n: usize) -> Option<(u64, u64)> {
    if n == 0 { return None; }

    let first = crate::memory::frame::alloc_frame()?;
    let phys_base = first.start_address().as_u64();

    for i in 1..n {
        let frame = crate::memory::frame::alloc_frame()?;
        let expected = phys_base + (i as u64) * 4096;
        if frame.start_address().as_u64() != expected {
            // Non-contiguous — still proceed; GPU scatter-gather (Phase 14)
            // will fix this properly. For now log and continue.
            crate::serial_println!(
                "[virtio-gpu] warn: non-contiguous frame at index {} (expected {:#x}, got {:#x})",
                i, expected, frame.start_address().as_u64()
            );
        }
    }

    let virt_base = crate::memory::paging::phys_to_virt(
        x86_64::PhysAddr::new(phys_base)
    ).as_u64();

    // Zero all pages
    unsafe { core::ptr::write_bytes(virt_base as *mut u8, 0, n * 4096); }

    Some((virt_base, phys_base))
}

// ─── Struct → raw bytes helper ───────────────────────────────────────────────

/// Copy a struct into the scratchpad command area and return its byte count.
fn write_cmd<T: Sized>(dst: *mut u8, val: &T) -> usize {
    let sz = core::mem::size_of::<T>();
    unsafe {
        core::ptr::copy_nonoverlapping(
            val as *const T as *const u8,
            dst,
            sz,
        );
    }
    sz
}

// ─── Driver implementation ───────────────────────────────────────────────────

impl VirtioGpuDriver {
    // ── Low-level command dispatch ─────────────────────────────────────────

    /// Send `cmd_len` bytes from scratchpad[0..] to the device,
    /// receive response into scratchpad[512..] (up to `resp_max` bytes).
    /// Blocks until the device processes the request.
    fn send_cmd(&mut self, cmd_len: usize, resp_max: usize) {
        let cmd_phys  = self.scratch_phys;
        let resp_phys = self.scratch_phys + 512;

        // Zero response area
        unsafe {
            core::ptr::write_bytes((self.scratch_virt + 512) as *mut u8, 0, resp_max.min(512));
        }

        let head = self.controlq.add_buf(&[
            (cmd_phys,  cmd_len as u32,  0),
            (resp_phys, resp_max as u32, VIRTQ_DESC_F_WRITE),
        ]);

        if head.is_none() {
            crate::serial_println!("[virtio-gpu] warn: controlq full, dropping command");
            return;
        }

        self.controlq.notify();

        // Busy-poll until device returns descriptor
        let mut spins = 0u32;
        loop {
            if self.controlq.poll_used().is_some() {
                break;
            }
            spins += 1;
            if spins > 1_000_000 {
                crate::serial_println!("[virtio-gpu] warn: command timed out");
                break;
            }
            core::hint::spin_loop();
        }
    }

    /// Response type from the last send_cmd.
    fn resp_type(&self) -> u32 {
        // Response header starts at scratch_virt + 512
        let ptr = (self.scratch_virt + 512) as *const u32;
        unsafe { *ptr }
    }

    // ── VirtIO-GPU commands ────────────────────────────────────────────────

    fn cmd_get_display_info(&mut self) {
        let hdr = VirtioGpuCtrlHdr {
            type_: VIRTIO_GPU_CMD_GET_DISPLAY_INFO,
            ..Default::default()
        };
        let sz = write_cmd(self.scratch_virt as *mut u8, &hdr);
        let resp_sz = core::mem::size_of::<VirtioGpuRespDisplayInfo>();
        // Response goes to scratch[512..]
        self.send_cmd(sz, resp_sz);

        if self.resp_type() == VIRTIO_GPU_RESP_OK_DISPLAY_INFO {
            let info = unsafe {
                &*((self.scratch_virt + 512) as *const VirtioGpuRespDisplayInfo)
            };
            for pmode in &info.pmodes {
                if pmode.enabled != 0 && pmode.r.width > 0 && pmode.r.height > 0 {
                    self.display_w = pmode.r.width;
                    self.display_h = pmode.r.height;
                    crate::serial_println!(
                        "[virtio-gpu] Display: {}x{}", self.display_w, self.display_h
                    );
                    break;
                }
            }
        }
    }

    fn cmd_resource_create_2d(&mut self, resource_id: u32, format: u32, w: u32, h: u32) {
        let cmd = VirtioGpuResourceCreate2d {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_CREATE_2D,
                ..Default::default()
            },
            resource_id,
            format,
            width: w,
            height: h,
        };
        let sz = write_cmd(self.scratch_virt as *mut u8, &cmd);
        self.send_cmd(sz, core::mem::size_of::<VirtioGpuCtrlHdr>());
    }

    fn cmd_resource_attach_backing(&mut self, resource_id: u32, phys: u64, byte_len: u32) {
        // Layout in scratch[0..]: AttachBacking header + 1 MemEntry
        let hdr = VirtioGpuResourceAttachBacking {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING,
                ..Default::default()
            },
            resource_id,
            nr_entries: 1,
        };
        let entry = VirtioGpuMemEntry {
            addr:   phys,
            length: byte_len,
            _pad:   0,
        };
        let ptr = self.scratch_virt as *mut u8;
        let hdr_sz = write_cmd(ptr, &hdr);
        let entry_sz = write_cmd(unsafe { ptr.add(hdr_sz) }, &entry);
        self.send_cmd(hdr_sz + entry_sz, core::mem::size_of::<VirtioGpuCtrlHdr>());
    }

    fn cmd_set_scanout(&mut self, scanout_id: u32, resource_id: u32, w: u32, h: u32) {
        let cmd = VirtioGpuSetScanout {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_SET_SCANOUT,
                ..Default::default()
            },
            r: VirtioGpuRect { x: 0, y: 0, width: w, height: h },
            scanout_id,
            resource_id,
        };
        let sz = write_cmd(self.scratch_virt as *mut u8, &cmd);
        self.send_cmd(sz, core::mem::size_of::<VirtioGpuCtrlHdr>());
    }

    fn cmd_transfer_to_host_2d(&mut self, resource_id: u32, w: u32, h: u32) {
        let cmd = VirtioGpuTransferToHost2d {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D,
                ..Default::default()
            },
            r: VirtioGpuRect { x: 0, y: 0, width: w, height: h },
            offset: 0,
            resource_id,
            _pad: 0,
        };
        let sz = write_cmd(self.scratch_virt as *mut u8, &cmd);
        self.send_cmd(sz, core::mem::size_of::<VirtioGpuCtrlHdr>());
    }

    fn cmd_resource_flush(&mut self, resource_id: u32, w: u32, h: u32) {
        let cmd = VirtioGpuResourceFlush {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_FLUSH,
                ..Default::default()
            },
            r: VirtioGpuRect { x: 0, y: 0, width: w, height: h },
            resource_id,
            _pad: 0,
        };
        let sz = write_cmd(self.scratch_virt as *mut u8, &cmd);
        self.send_cmd(sz, core::mem::size_of::<VirtioGpuCtrlHdr>());
    }

    pub fn cmd_move_cursor(&mut self, scanout_id: u32, x: u32, y: u32) {
        let cmd = VirtioGpuUpdateCursor {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_MOVE_CURSOR,
                ..Default::default()
            },
            pos: VirtioGpuCursorPos { scanout_id, x, y, _pad: 0 },
            resource_id: 0,
            hot_x: 0, hot_y: 0, _pad: 0,
        };
        // cursor commands go on the controlq for simplicity
        let sz = write_cmd(self.scratch_virt as *mut u8, &cmd);
        self.send_cmd(sz, core::mem::size_of::<VirtioGpuCtrlHdr>());
    }

    // ── Public GPU-mem API (called from gpu_mem.rs) ────────────────────────

    /// RESOURCE_CREATE_2D — public alias for gpu_mem.
    pub fn cmd_resource_create_2d_pub(&mut self, res_id: u32, fmt: u32, w: u32, h: u32) {
        self.cmd_resource_create_2d(res_id, fmt, w, h);
    }

    /// RESOURCE_ATTACH_BACKING with scatter-gather (multiple SgEntry pages).
    pub fn cmd_resource_attach_backing_sg_pub(
        &mut self,
        resource_id: u32,
        sg: &[super::gpu_mem::SgEntry],
    ) {
        if sg.is_empty() { return; }

        // Build the command in scratch: header + N MemEntries
        let hdr = VirtioGpuResourceAttachBacking {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING,
                ..Default::default()
            },
            resource_id,
            nr_entries: sg.len() as u32,
        };
        let ptr = self.scratch_virt as *mut u8;
        let mut off = write_cmd(ptr, &hdr);

        for entry in sg {
            let mem_entry = VirtioGpuMemEntry {
                addr:   entry.phys,
                length: entry.len,
                _pad:   0,
            };
            off += write_cmd(unsafe { ptr.add(off) }, &mem_entry);
            // Guard: scratchpad is 512 bytes for cmd; if we overflow just stop
            if off + core::mem::size_of::<VirtioGpuMemEntry>() > 480 {
                crate::serial_println!("[virtio-gpu] sg overflow at {} entries", off);
                break;
            }
        }
        self.send_cmd(off, core::mem::size_of::<VirtioGpuCtrlHdr>());
    }

    /// TRANSFER_TO_HOST_2D for a sub-rectangle.
    pub fn cmd_transfer_to_host_2d_rect_pub(
        &mut self,
        resource_id: u32,
        x: u32, y: u32, w: u32, h: u32,
        full_width: u32, _full_height: u32,
    ) {
        // Linear offset into the backing store at (x,y).
        let offset = (y as u64) * (full_width as u64) * 4 + (x as u64) * 4;
        let cmd = VirtioGpuTransferToHost2d {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D,
                ..Default::default()
            },
            r: VirtioGpuRect { x, y, width: w, height: h },
            offset,
            resource_id,
            _pad: 0,
        };
        let sz = write_cmd(self.scratch_virt as *mut u8, &cmd);
        self.send_cmd(sz, core::mem::size_of::<VirtioGpuCtrlHdr>());
    }

    /// RESOURCE_FLUSH for a sub-rectangle.
    pub fn cmd_resource_flush_rect_pub(
        &mut self,
        resource_id: u32,
        x: u32, y: u32, w: u32, h: u32,
    ) {
        let cmd = VirtioGpuResourceFlush {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_FLUSH,
                ..Default::default()
            },
            r: VirtioGpuRect { x, y, width: w, height: h },
            resource_id,
            _pad: 0,
        };
        let sz = write_cmd(self.scratch_virt as *mut u8, &cmd);
        self.send_cmd(sz, core::mem::size_of::<VirtioGpuCtrlHdr>());
    }

    /// SET_SCANOUT — public alias for gpu_mem.
    pub fn cmd_set_scanout_pub(&mut self, scanout_id: u32, resource_id: u32, w: u32, h: u32) {
        self.cmd_set_scanout(scanout_id, resource_id, w, h);
    }

    // ── DRM framebuffer ops ────────────────────────────────────────────────

    fn do_alloc_framebuffer(&mut self, width: u32, height: u32) -> Result<FramebufferObj, &'static str> {
        let byte_size = (width as usize) * (height as usize) * 4;
        let num_pages = (byte_size + 4095) / 4096;

        let (virt, phys) = alloc_pages_contiguous(num_pages)
            .ok_or("VirtIO-GPU: failed to alloc framebuffer pages")?;

        let resource_id = self.next_resource;
        self.next_resource += 1;

        let fmt = if self.is_bgr {
            VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM
        } else {
            VIRTIO_GPU_FORMAT_R8G8B8X8_UNORM
        };

        self.cmd_resource_create_2d(resource_id, fmt, width, height);
        self.cmd_resource_attach_backing(resource_id, phys, byte_size as u32);

        crate::serial_println!(
            "[virtio-gpu] alloc_framebuffer: resource={} {}x{} virt={:#x} phys={:#x}",
            resource_id, width, height, virt, phys
        );

        let fb = GpuFb {
            resource_id,
            width,
            height,
            virt_addr: virt,
            phys_addr: phys,
            byte_size: byte_size as u32,
            scanout_set: false,
        };
        let id = resource_id;
        self.framebuffers.push(fb);

        Ok(FramebufferObj {
            id,
            width,
            height,
            pitch: width * 4,
            bpp: 32,
            phys_addr: phys,
            virt_addr: virt,
        })
    }

    fn do_set_crtc(&mut self, _connector_id: u32, fb_id: u32) -> Result<(), &'static str> {
        // Find the framebuffer
        let (resource_id, w, h, scanout_set) = {
            let fb = self.framebuffers.iter()
                .find(|f| f.resource_id == fb_id)
                .ok_or("VirtIO-GPU: unknown fb_id in set_crtc")?;
            (fb.resource_id, fb.width, fb.height, fb.scanout_set)
        };

        // SET_SCANOUT only once (or if re-binding)
        if !scanout_set {
            self.cmd_set_scanout(0, resource_id, w, h);
            if let Some(fb) = self.framebuffers.iter_mut().find(|f| f.resource_id == fb_id) {
                fb.scanout_set = true;
            }
        }

        // Transfer backing memory → GPU internal texture
        self.cmd_transfer_to_host_2d(resource_id, w, h);
        // Flush GPU texture → display
        self.cmd_resource_flush(resource_id, w, h);

        Ok(())
    }

    fn do_fill_rect(&mut self, fb_id: u32, x: u32, y: u32, w: u32, h: u32, color: u32) -> Result<(), &'static str> {
        let (virt, fb_w, fb_h) = {
            let fb = self.framebuffers.iter()
                .find(|f| f.resource_id == fb_id)
                .ok_or("VirtIO-GPU: unknown fb_id in fill_rect")?;
            (fb.virt_addr, fb.width, fb.height)
        };

        let x_end = (x + w).min(fb_w);
        let y_end = (y + h).min(fb_h);
        if x_end <= x || y_end <= y { return Ok(()); }

        // color is 0x00RRGGBB; convert to framebuffer format
        let pixel: u32 = if self.is_bgr {
            let r = (color >> 16) & 0xFF;
            let g = (color >> 8) & 0xFF;
            let b = color & 0xFF;
            b | (g << 8) | (r << 16)
        } else {
            color
        };

        for py in y..y_end {
            let row_ptr = unsafe {
                (virt as *mut u32).add((py * fb_w + x) as usize)
            };
            let row = unsafe { core::slice::from_raw_parts_mut(row_ptr, (x_end - x) as usize) };
            row.fill(pixel);
        }
        Ok(())
    }

    fn do_blit(
        &mut self,
        src_fb: u32, dst_fb: u32,
        src_x: u32, src_y: u32,
        dst_x: u32, dst_y: u32,
        w: u32, h: u32,
    ) -> Result<(), &'static str> {
        let (src_virt, src_w) = {
            let fb = self.framebuffers.iter()
                .find(|f| f.resource_id == src_fb)
                .ok_or("VirtIO-GPU: unknown src_fb in blit")?;
            (fb.virt_addr, fb.width)
        };
        let (dst_virt, dst_w, dst_h) = {
            let fb = self.framebuffers.iter()
                .find(|f| f.resource_id == dst_fb)
                .ok_or("VirtIO-GPU: unknown dst_fb in blit")?;
            (fb.virt_addr, fb.width, fb.height)
        };

        let cw = w.min(dst_w.saturating_sub(dst_x)).min(src_w.saturating_sub(src_x));
        let ch = h.min(dst_h.saturating_sub(dst_y));

        for row in 0..ch {
            let src_ptr = unsafe { (src_virt as *const u32).add(((src_y + row) * src_w + src_x) as usize) };
            let dst_ptr = unsafe { (dst_virt as *mut u32).add(((dst_y + row) * dst_w + dst_x) as usize) };
            unsafe { core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, cw as usize); }
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════
//  DRM proxy (delegates to VIRTIO_GPU via Mutex)
// ═══════════════════════════════════════════════════════════════

pub struct VirtioGpuDrmProxy;

impl GpuDriver for VirtioGpuDrmProxy {
    fn init(&mut self) -> Result<(), &'static str> { Ok(()) }

    fn get_connectors(&self) -> Vec<Connector> {
        let g = VIRTIO_GPU.lock();
        match g.as_ref() {
            Some(d) => alloc::vec![Connector {
                id: 0,
                connected: true,
                current_mode: Some(DisplayMode {
                    width: d.display_w,
                    height: d.display_h,
                    refresh_rate: 60,
                }),
            }],
            None => alloc::vec![],
        }
    }

    fn alloc_framebuffer(&mut self, w: u32, h: u32, _fmt: u32) -> Result<FramebufferObj, &'static str> {
        let mut g = VIRTIO_GPU.lock();
        match g.as_mut() {
            Some(d) => d.do_alloc_framebuffer(w, h),
            None => Err("VirtIO-GPU not initialized"),
        }
    }

    fn set_crtc(&mut self, connector_id: u32, fb_id: u32) -> Result<(), &'static str> {
        let mut g = VIRTIO_GPU.lock();
        match g.as_mut() {
            Some(d) => d.do_set_crtc(connector_id, fb_id),
            None => Err("VirtIO-GPU not initialized"),
        }
    }

    fn blit(
        &mut self,
        src_fb: u32, dst_fb: u32,
        src_x: u32, src_y: u32,
        dst_x: u32, dst_y: u32,
        w: u32, h: u32,
    ) -> Result<(), &'static str> {
        let mut g = VIRTIO_GPU.lock();
        match g.as_mut() {
            Some(d) => d.do_blit(src_fb, dst_fb, src_x, src_y, dst_x, dst_y, w, h),
            None => Err("VirtIO-GPU not initialized"),
        }
    }

    fn fill_rect(&mut self, fb_id: u32, x: u32, y: u32, w: u32, h: u32, color: u32) -> Result<(), &'static str> {
        let mut g = VIRTIO_GPU.lock();
        match g.as_mut() {
            Some(d) => d.do_fill_rect(fb_id, x, y, w, h, color),
            None => Err("VirtIO-GPU not initialized"),
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  Public initialization
// ═══════════════════════════════════════════════════════════════

/// Initialize VirtIO-GPU and register it with the DRM subsystem.
/// Call this AFTER DRM init and BEFORE compositor init so the
/// compositor detects and uses hardware framebuffers.
pub fn init(is_bgr: bool) -> Result<(), &'static str> {
    let dev = pci::find_device(pci::VIRTIO_VENDOR, pci::VIRTIO_GPU_DEV)
        .ok_or("VirtIO-GPU: device not found on PCI bus")?;

    pci::enable_bus_master(&dev);
    pci::enable_io_space(&dev);

    let io_base = pci::bar0_io_base(&dev)
        .ok_or("VirtIO-GPU: BAR0 is not I/O space")?;

    // VirtIO legacy init sequence
    virtio::virtio_reset(io_base);
    virtio::virtio_set_status(io_base, virtio::STATUS_ACKNOWLEDGE);
    virtio::virtio_set_status(io_base, virtio::STATUS_DRIVER);
    virtio::virtio_negotiate_features(io_base, 0);
    virtio::virtio_set_status(io_base, virtio::STATUS_FEATURES_OK);

    let controlq = Virtqueue::new(io_base, 0)
        .ok_or("VirtIO-GPU: failed to init controlq")?;
    let cursorq = Virtqueue::new(io_base, 1)
        .ok_or("VirtIO-GPU: failed to init cursorq")?;

    virtio::virtio_set_status(io_base, virtio::STATUS_DRIVER_OK);

    // Allocate 2 pages for command/response scratchpad
    let (sv, sp) = alloc_page().ok_or("VirtIO-GPU: scratchpad alloc failed")?;
    let (sv2, _sp2) = alloc_page().ok_or("VirtIO-GPU: scratchpad2 alloc failed")?;
    // scratch_virt uses the first page for commands (0..512) and the second
    // page beginning at offset 512 is placed at sv+512 = sv2 if contiguous,
    // otherwise we just use the first page and limit response to 512 bytes.
    let _ = sv2; // second page zeroed; placed after sv if allocator is sequential

    let mut driver = VirtioGpuDriver {
        controlq,
        _cursorq: cursorq,
        display_w: 1024,
        display_h: 768,
        next_resource: 1,
        framebuffers: Vec::new(),
        scratch_virt: sv,
        scratch_phys: sp,
        is_bgr,
    };

    // Query display resolution
    driver.cmd_get_display_info();

    crate::serial_println!(
        "[virtio-gpu] Ready: {}x{} format={}",
        driver.display_w, driver.display_h,
        if is_bgr { "BGRX" } else { "RGBX" }
    );

    *VIRTIO_GPU.lock() = Some(driver);
    GPU_ACTIVE.store(true, Ordering::Relaxed);

    // Register DRM proxy so the compositor auto-uses GPU framebuffers
    crate::drivers::drm::register_driver(Box::new(VirtioGpuDrmProxy));

    Ok(())
}

/// Whether VirtIO-GPU is active (lock-free check for render loop).
pub fn is_active() -> bool {
    GPU_ACTIVE.load(Ordering::Relaxed)
}

/// Move the hardware cursor to (x, y) on scanout 0.
/// Call this from the mouse driver on every cursor position update.
pub fn move_cursor(x: u32, y: u32) {
    if !GPU_ACTIVE.load(Ordering::Relaxed) { return; }
    if let Some(ref mut d) = *VIRTIO_GPU.lock() {
        d.cmd_move_cursor(0, x, y);
    }
}

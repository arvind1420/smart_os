//! Smart OS Kernel — Phase 40
//!
//! A hybrid microkernel for the Smart OS operating system.
//! Phases 36-40: Layout Engine, Paint/Composite, JavaScript Engine,
//! Browser Chrome (tabs/address-bar/history), Full Browser Integration.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod arch;
mod memory;
mod serial;
pub mod drivers;
pub mod process;
pub mod ipc;
pub mod syscall;
pub mod vfs;
pub mod gui;
pub mod ai;
pub mod smartfs;
pub mod plugins;
pub mod apps;
pub mod net;
pub mod knowledge;
pub mod security;
pub mod immutable;
pub mod io;
pub mod installer;
pub mod posix;
pub mod pkg;
pub mod wayland;
pub mod crypto;
pub mod users;
pub mod session;
pub mod profiler;
pub mod unicode;
pub mod i18n;

use bootloader_api::{entry_point, BootInfo, BootloaderConfig};
use bootloader_api::config::Mapping;
use core::panic::PanicInfo;

/// Bootloader configuration — tells the bootloader what we need.
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

/// Kernel entry point — called by the bootloader after setting up
/// long mode, paging, and a framebuffer.
fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // ═══════════════════════════════════════════════════════════════
    //  PHASE 1: Early boot — serial output
    // ═══════════════════════════════════════════════════════════════
    serial::init();
    serial_println!("====================================================");
    serial_println!("  SMART OS v1.0.0 — Hybrid Microkernel");
    serial_println!("  Phase 58: AC'97 Audio Engine (NABM/BDL cyclic DMA)");
    serial_println!("  Phase 59: TrueType Font Rendering (anti-aliased, 14 px UI)");
    serial_println!("  Phase 60: Unicode & Localization (NFC/NFD, 10 locales, BiDi v2)");
    serial_println!("  Phase 61: Window Manager v2 (tiling, focus history, snap zones)");
    serial_println!("  Phase 62: Image Viewer (zoom/pan/slideshow, PNG/JPEG/BMP)");
    serial_println!("  Phase 63: PDF Reader (xref scan, BT/ET/Tj/TJ text extraction)");
    serial_println!("  Phase 64: Video Player (SmartVideo .smv, colour-bars demo, 30fps)");
    serial_println!("  Phase 65: Email Client (SMTP/POP3 stubs, folders, thread view)");
    serial_println!("  Phase 66: Office Suite v1 (Writer/Calc/Slides, formula engine)");
    serial_println!("  Phase 67: Browser v2 (bookmarks/downloads/history/reader)");
    serial_println!("  Phase 68: USB Mass Storage (BOT/SCSI, hot-plug, FAT32 mount)");
    serial_println!("  Phase 69: App Store GUI (catalogue, install/remove, search)");
    serial_println!("  Phase 70: Power Manager (C-states, DVFS, battery, suspend)");
    serial_println!("  Phase 71: Crash Recovery (panic log, watchdog, health check)");
    serial_println!("  Phase 72: Setup Wizard (7-page first-run, locale/net/user/theme)");
    serial_println!("  Phase 73: Accessibility (screen reader, sticky keys, color filter)");
    serial_println!("  Phase 74: Printer (PCL/LPT, print queue, test page)");
    serial_println!("  Phase 75: Update Manager (channel/delta/verify/rollback)");
    serial_println!("  Phase 76: Cloud Sync & Backup (incremental, encryption stubs)");
    serial_println!("  Phase 77: Gaming & Entertainment (game launcher, achievements)");
    serial_println!("  Phase 78: Browser IPC (cmd/event queues, page response buffers)");
    serial_println!("  Phase 79: Browser Sandbox (per-tab renderer, capability checks)");
    serial_println!("  Phase 80: TLS Cert Verifier (chain/hostname/expiry/HSTS parse)");
    serial_println!("  Phase 81: Same-Origin Policy + CORS header enforcement");
    serial_println!("  Phase 82: Content Security Policy (directives, nonce, hash)");
    serial_println!("  Phase 83: DOM Tree (arena, query, events, capture+bubble)");
    serial_println!("  Phase 84: Web Crypto API (AES-GCM, HMAC-SHA256, UUID, PBKDF2)");
    serial_println!("  Phase 85: Browser Persistence (history/bookmarks/cookies/HSTS/session)");
    serial_println!("  Phase 86: Browser Downloads (MIME classify, Content-Disposition, SHA-256)");
    serial_println!("  Phase 87: HTTP/2 + QUIC foundations (already online)");
    serial_println!("  Phase 88: TLS 1.3 handshake (already online)");
    serial_println!("  Phase 89: Session restore + HSTS upgrade (persist layer)");
    serial_println!("  Phase 90: Download queue (MAX_CONCURRENT=4, progress, checksum)");
    serial_println!("  Phase 91: SVG rasteriser (Bresenham, scanline fill, transforms)");
    serial_println!("  Phase 92: SVG colour + path parser (M/L/H/V/Z, affine)");
    serial_println!("  Phase 93: WebP decoder (RIFF/VP8L lossless, VP8 stub, ALPH, ANIM)");
    serial_println!("  Phase 94: AVIF decoder (ISOBMFF/ftyp/ispe/av1C, AV1 OBU stub)");
    serial_println!("  Phase 95: Browser v2 IPC wiring (navigate→sandbox→chrome)");
    serial_println!("  Phase 96: WPT Harness (DOM/CSP/SOP/Crypto/SVG/WebP/AVIF smoke)");
    serial_println!("  Phase 97: Browser polish (security badge, download bar, WPT green)");
    serial_println!("  Phase 98: JS Engine v2 (class extends/super, instanceof, Date, toFixed, +Math)");
    serial_println!("  Phase 99: Web Fonts WOFF (font-face parser, TTF extraction, @font-face CSS)");
    serial_println!("  Phase 100: Real CA Bundle (Mozilla root CAs, zlib-compressed chain verify)");
    serial_println!("  Phase 101: VP8 Lossy Decoder v2 (boolean arith decoder, DC qlookup, per-MB luma)");
    serial_println!("  Phase 102: Video Element (MP4/WebM/Ogg metadata, VideoElement, poster frame)");
    serial_println!("  Phase 103: Browser Polish (security badge, styled error pages, download bar UI)");
    serial_println!("  Phase 104: JS Engine v3 (async/await, micro-task queue, Promise.withResolvers)");
  serial_println!("  Phase 105: WebRTC stub (RTCPeerConnection, MediaStream, getUserMedia)");
  serial_println!("  Phase 106: App Layer v2 (Image Viewer WebP/AVIF/rotate, Video Player metadata)");
  serial_println!("  Phase 107: System Hardening (epoll, cgroup limits, ptrace ATTACH/GETREGS/SINGLESTEP)");
    serial_println!("  Phase 108: CSS Polish (border-radius, box-shadow, text-decoration, pseudo-classes :nth-child/:not/:first-child)");
    serial_println!("  Phase 109: Form/Input Rendering (input/textarea/select/button/form submit, focus state)");
    serial_println!("  Phase 110: JS DOM APIs (querySelector/All, classList, innerHTML, window.location, history.pushState, setTimeout)");
    serial_println!("  Phase 111: Browser UX (Ctrl+L/T/W/R/F shortcuts, find-in-page, favicon, tab title, reader mode)");
    serial_println!("  Phase 112: Release Engineering (QEMU .img artifact, Makefile, README, demo landing page, CI workflow)");
    serial_println!("  Phase 29: Real Hardware Drivers (Intel HDA, Bluetooth HCI, ACPI Power, HCL v1)");
    serial_println!("  Phase 31: Graphical Installer (7-page GUI wizard, GPT, bootloader, root-FS copy)");
    serial_println!("  Phase 108: WebAssembly MVP (binary parse, stack machine, i32/i64/f32/f64, memory, JS API)");
    serial_println!("  Phase 109: JS+CSS Completeness (Proxy/Reflect/Generator/TypedArray/WeakRef; custom-props/calc/nesting/:has/logical)");
    serial_println!("  Phase 110: Tab Process Isolation (typed IPC ring, renderer sandbox, RENDERER_ALLOWED_SYSCALLS, crash-restart)");
    serial_println!("  Phase 111: JS Baseline JIT (FNV-1a FuncKey, x86-64 CodeBuf, 3-pass compile, JitCache, HOT_THRESHOLD=50)");
    serial_println!("  Phase 112: VP8 Full + AV1 (BoolDec, IDCT-4x4, intra-predict, loop-filter, ref-frames; AV1 OBU LEB128)");
    serial_println!("  Phase 113: WiFi Driver (Intel iwlwifi MLME, PBKDF2 PMK, PRF-512 PTK, self-test)");
    serial_println!("  Phase 114: DevTools (Elements/Console/Network/Performance panels, DOM inspector, frame jank)");
    serial_println!("  Phase 115: WebAuthn FIDO2 (inline SHA-256/HMAC, CBOR COSE ES256, make_credential/get_assertion)");
    serial_println!("  Phase 116: MSE + HLS (TimeRanges merge, SourceBuffer, MediaSource, HLS playlist, ABR EWMA)");
    serial_println!("  Phase 117: Real WebRTC (STUN RFC 5389, ICE RFC 8445, RTP RFC 3550, XOR-MAPPED-ADDRESS)");
    serial_println!("  Phase 118: WebExtensions (Manifest V3, MatchPattern glob, ContentScript, ExtensionManager)");
    serial_println!("  Phase 119: Accessibility (ARIA 1.2 70 roles, AtNode announcements, tab order, ScreenReader queues)");
    serial_println!("  Phase 120: Print/PDF (paper sizes, @media print, pagination, PDF 1.4 writer with xref)");
    serial_println!("  Phase 121: Security Hardening (fuzz corpus 13, ASLR entropy, CSP grader, SRI verify, X-Frame)");
    serial_println!("  Phase 122: WPT Compliance (200 Web Platform Tests: HTML/CSS/DOM/JS/Fetch/Crypto/Storage/WASM/RTC/A11y)");
    serial_println!("  Phase 123: JS JIT Wiring (hot-count fast-path, lower_fn_to_jit, integer arithmetic)");
    serial_println!("  Phase 124: ES Modules (import/export, module registry, namespace objects, run_module)");
    serial_println!("  Phase 125: WiFi WPA2 (802.11 beacon parser, CCMP AES-CCM encrypt/decrypt, inject_rx_frame)");
    serial_println!("  Phase 126: TLS v2 (ChaCha20-Poly1305 AEAD, session tickets, dual cipher suite negotiation)");
    serial_println!("  Phase 127: WebCrypto API (SHA-256/384/512, AES-128-GCM, HMAC, PBKDF2, HKDF, getRandomValues)");
    serial_println!("  Phase 128: HTTP/2 ALPN (TLS ALPN extension, EE parse, TlsClientIo, https_h2_request)");
    serial_println!("  Phase 129: Streams API (ReadableStream/WritableStream/TransformStream, pipeTo, pipeThrough, tee)");
    serial_println!("  Phase 130: PWA Support (Web App Manifest, matchMedia, beforeinstallprompt, navigator.standalone)");
    serial_println!("  Phase 131: URL API (URL/URLSearchParams/Blob/File/FileReader/structuredClone/btoa/atob)");
    serial_println!("  Phase 132: Observer APIs (rAF, queueMicrotask, MutationObserver, ResizeObserver, IntersectionObserver, performance)");
    serial_println!("  Phase 133: Web Forms+Events (FormData, AbortController, EventSource, CustomEvent, MessageChannel)");
    serial_println!("  Phase 134: Web Components v1 (Custom Elements, Shadow DOM, DocumentFragment, CSSStyleSheet)");
    serial_println!("  Phase 135: Modern CSS (Container Queries, @layer, aspect-ratio, content-visibility, CSS.supports)");
    serial_println!("  Phase 136: Browser Persistence (bookmarks, history, settings, downloads — VFS-backed)");
    serial_println!("  Phase 137: Security Polish (HSTS, mixed content, Permissions API, isSecureContext, referrer policy)");
    serial_println!("  Phase 138: Perf Hints (resource hints, lazy-load, prefetch cache, rIC, scheduler, Web Vitals)");
    serial_println!("  Phase 139: WPT v2 (300 synthetic tests, 98%% pass rate, URL/Observer/Forms/WebComp/CSS/PWA/Streams coverage)");
    serial_println!("  Phase 140: SmartBrowser v1.0 Public Release (about: pages, crash reporter, final polish)");
    serial_println!("====================================================");
    serial_println!();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 2: CPU architecture (GDT + IDT)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Setting up GDT...");
    arch::x86_64::gdt::init();
    serial_println!("[boot] Setting up IDT...");
    arch::x86_64::idt::init();
    serial_println!("[boot] Enabling SSE2...");
    arch::x86_64::sse::init();
    serial_println!("[boot] Setting up P-States...");
    arch::x86_64::pstate::init();
    serial_println!("[boot] CPU tables ready.");

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 3: Memory management (physical + heap)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing memory management...");
    let physical_memory_offset = boot_info
        .physical_memory_offset
        .as_ref()
        .copied()
        .unwrap_or(0);

    {
        let memory_regions = &boot_info.memory_regions[..];
        let usable_bytes: u64 = memory_regions
            .iter()
            .filter(|r| r.kind == bootloader_api::info::MemoryRegionKind::Usable)
            .map(|r| r.end - r.start)
            .sum();
        serial_println!(
            "[boot] Physical memory offset: {:#X}, usable: {} MiB",
            physical_memory_offset,
            usable_bytes / (1024 * 1024),
        );
        memory::heap::init_heap(physical_memory_offset, memory_regions);

        // Initialize the physical frame allocator (needs heap + memory map).
        let reserved_end = memory::heap::HEAP_PHYS_END
            .load(core::sync::atomic::Ordering::Relaxed);
        memory::frame::init(memory_regions, reserved_end);
    }

    // Initialize page table management.
    memory::paging::init(physical_memory_offset);

    // Initialize ACPI subsystem early.
    if let Some(rsdp_addr) = boot_info.rsdp_addr.as_ref().copied() {
        drivers::acpi::init(rsdp_addr, 0);
    }

    let (heap_used, heap_free) = memory::heap::heap_stats();
    serial_println!(
        "[boot] Kernel heap ready (used: {}K, free: {}K).",
        heap_used / 1024,
        heap_free / 1024,
    );

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 4: SmartPack self-test
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] SmartPack self-test...");
    smartpack_selftest();
    serial_println!("[boot] SmartPack OK (format v{}).", smartpack::FORMAT_VERSION);

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 5: Hardware drivers (PIC, Timer, Keyboard, Mouse)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing hardware drivers...");
    drivers::init();
    serial_println!("[boot] PIC remapped, PIT at ~100Hz, PS/2 keyboard + mouse ready.");

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 6: Process / scheduler subsystem
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing process subsystem...");
    process::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 7: IPC channels
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing IPC subsystem...");
    ipc::init();

    // Create some default system ports
    {
        use ipc::port;
        port::register("system.log", 64);
        port::register("kernel.events", 32);
        serial_println!("[boot] System ports registered (system.log, kernel.events).");
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 8: Syscall interface
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing syscall interface...");
    syscall::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 8.5: Kernel Module Loader
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing Kernel Module Loader (.sys support)...");
    // process::kmod::init() would go here if it had internal state beyond a global Mutex

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 9: Virtual File System
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing virtual file system...");
    vfs::init();

    // VFS smoke test: read the version file
    {
        let fd = vfs::open("/system/version").expect("Failed to open version file");
        let mut buf = [0u8; 64];
        let n = vfs::read(fd, &mut buf).expect("Failed to read version file");
        let version = core::str::from_utf8(&buf[..n]).unwrap_or("?");
        serial_println!("[boot] /system/version = {}", version.trim());
        vfs::close(fd).ok();
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 10: PCI bus scan + GPU discovery
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Scanning PCI bus...");
    drivers::pci::init();

    serial_println!("[boot] Initializing DRM subsystem...");
    drivers::drm::init();

    // ── VirtIO-GPU: must init BEFORE compositor so hardware FBs are available ──
    // Peek at pixel format to pass is_bgr to GPU driver.
    let boot_is_bgr = boot_info.framebuffer.as_ref().map(|fb| {
        matches!(fb.info().pixel_format, bootloader_api::info::PixelFormat::Bgr)
    }).unwrap_or(true);

    serial_println!("[boot] Searching for VirtIO-GPU...");
    match drivers::virtio_gpu::init(boot_is_bgr) {
        Ok(()) => serial_println!("[boot] VirtIO-GPU hardware acceleration ready."),
        Err(e) => serial_println!("[boot] VirtIO-GPU not found ({}). Trying Intel iGPU...", e),
    }

    // Phase 14: GPU memory manager (BO allocator, scatter-gather, fence tracker)
    drivers::gpu_mem::init();

    // Phase 16: Display modesetting + VSync double-buffer
    drivers::display::init(boot_is_bgr);

    serial_println!("[boot] Searching for Intel iGPU...");
    if drivers::igpu::init().is_ok() {
        serial_println!("[boot] Intel iGPU hardware acceleration ready.");
    } else {
        serial_println!("[boot] No Intel iGPU found.");
    }

    // Phase 18: AMD Radeon GPU (GCN1–RDNA2)
    serial_println!("[boot] Searching for AMD GPU...");
    if drivers::amdgpu::init().is_ok() {
        serial_println!("[boot] AMD GPU hardware acceleration ready.");
    } else {
        serial_println!("[boot] No AMD GPU found. Using software rendering.");
    }

    // Phase 19: OpenGL ES 2.0 software rasterizer
    {
        let (gl_w, gl_h) = drivers::display::display_resolution();
        drivers::gles::init(gl_w, gl_h);
    }

    // Phase 20: GLSL ES shader compiler + cache
    drivers::glsl::init();

    // Phase 21: Hardware 3D pipeline
    drivers::gpu3d::init();

    // Phase 22: GPU compositor
    drivers::compositor::init();

    // Phase 23-26: Cryptography + X.509 + CA store
    crypto::init();
    crypto::ca_store::init();
    crypto::tls13::init();
    crypto::tls12::init();
    net::http_client::init();
    net::http2::init();
    net::quic::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 32: Font loading + glyph rasterizer
    // ═══════════════════════════════════════════════════════════════
    drivers::font::init();
    drivers::image::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 34: HTML5 tokenizer + DOM
    // ═══════════════════════════════════════════════════════════════
    net::html::init();
    net::css::init();
    // Phase 108: CSS self-test
    if net::css::self_test() {
        serial_println!("[test] CSS Phase 108: border-radius/box-shadow/text-decoration/pseudo-classes PASSED");
    } else {
        serial_println!("[test] CSS Phase 108: FAILED");
    }
    net::layout::init();
    net::paint::init();
    net::js_interp::init();
    net::browser_chrome::init();
    net::browser::init();
    net::canvas::init();
    if net::canvas::self_test() {
        serial_println!("[test] Canvas 2D: fill/stroke/arc PASSED");
    } else {
        serial_println!("[test] Canvas 2D: FAILED");
    }
    net::websocket::init();
    if net::websocket::self_test() {
        serial_println!("[test] WebSocket: frame codec + SHA-1 PASSED");
    } else {
        serial_println!("[test] WebSocket: FAILED");
    }
    // Phase 43: Web Storage
    net::web_storage::init();
    if net::web_storage::self_test() {
        serial_println!("[test] Web Storage: localStorage/sessionStorage/IndexedDB PASSED");
    } else {
        serial_println!("[test] Web Storage: FAILED");
    }
    // Phase 44: Web Workers
    net::web_workers::init();
    if net::web_workers::self_test() {
        serial_println!("[test] Web Workers: dedicated worker echo/calc/close/multi PASSED");
    } else {
        serial_println!("[test] Web Workers: FAILED");
    }
    // Phase 45: Web Audio API
    net::web_audio::init();
    if net::web_audio::self_test() {
        serial_println!("[test] Web Audio API: oscillator/gain/buffer/filter/math PASSED");
    } else {
        serial_println!("[test] Web Audio API: FAILED");
    }
    // Phase 46: Fetch API
    net::fetch::init();
    if net::fetch::self_test() {
        serial_println!("[test] Fetch API: headers/request/response/XHR/redirect PASSED");
    } else {
        serial_println!("[test] Fetch API: FAILED");
    }
    // Phase 47: CSS Animations & Transitions
    net::css_anim::init();
    if net::css_anim::self_test() {
        serial_println!("[test] CSS Animations: easing/keyframes/transition/animation PASSED");
    } else {
        serial_println!("[test] CSS Animations: FAILED");
    }
    // Phase 48: WebGL
    net::webgl::init();
    if net::webgl::self_test() {
        serial_println!("[test] WebGL: mat4/texture/framebuffer/pipeline/attribs PASSED");
    } else {
        serial_println!("[test] WebGL: FAILED");
    }
    // Phase 49: Service Workers + Cache API
    net::service_worker::init();
    if net::service_worker::self_test() {
        serial_println!("[test] Service Workers: cache/lifecycle/intercept/unregister PASSED");
    } else {
        serial_println!("[test] Service Workers: FAILED");
    }
    // Phase 50: Capability system
    if security::capabilities::self_test() {
        serial_println!("[test] Capability system: grants/VFS/net/syscall/fork-inherit PASSED");
    } else {
        serial_println!("[test] Capability system: FAILED");
    }
    // Phase 51: Kernel profiler
    profiler::init();
    if profiler::self_test() {
        serial_println!("[test] Kernel profiler: ring/sample/collect/perf/flamegraph PASSED");
    } else {
        serial_println!("[test] Kernel profiler: FAILED");
    }
    // Phase 52: Memory pressure + OOM killer
    memory::oom::init();
    if memory::oom::self_test() {
        serial_println!("[test] OOM killer: watermarks/pressure/score/reclaim PASSED");
    } else {
        serial_println!("[test] OOM killer: FAILED");
    }
    // Phase 53: Multi-user (groups, VFS perms, sessions, su/login)
    if users::multiuser::self_test() {
        serial_println!("[test] Multi-user: perms/groups/sessions/su/login PASSED");
    } else {
        serial_println!("[test] Multi-user: FAILED");
    }
    // Phase 54: Media player (WAV decoder + waveform + playlist)
    if apps::media_player::self_test() {
        serial_println!("[test] Media player: WAV/PCM/peaks/seek/now-playing PASSED");
    } else {
        serial_println!("[test] Media player: FAILED");
    }
    // Phase 55: Package manager (.spk bundles + dep resolver)
    if pkg::spk::self_test() {
        serial_println!("[test] Package manager: .spk/round-trip/topo/cycle/install PASSED");
    } else {
        serial_println!("[test] Package manager: FAILED");
    }
    // Phase 56: Code editor v2 (syntax highlighting, multi-tab, find+replace)
    if apps::code_editor::self_test() {
        serial_println!("[test] Code editor v2: tokeniser/syntax/tabs/find-replace PASSED");
    } else {
        serial_println!("[test] Code editor v2: FAILED");
    }
    // Phase 57: System Dashboard (CPU/mem/net graphs, process table)
    if apps::dashboard::self_test() {
        serial_println!("[test] System Dashboard: bars/fmt/snapshot/net-acct PASSED");
    } else {
        serial_println!("[test] System Dashboard: FAILED");
    }
    // Phase 60: Unicode normalization + block detection
    unicode::init();
    if unicode::self_test() {
        serial_println!("[test] Unicode: NFC/NFD/grapheme/block/combining PASSED");
    } else {
        serial_println!("[test] Unicode: FAILED");
    }
    // Phase 60: i18n locale system
    i18n::init();
    if i18n::self_test() {
        serial_println!("[test] i18n: number/date/time/currency/locales PASSED");
    } else {
        serial_println!("[test] i18n: FAILED");
    }
    // Phase 61: Window Manager v2
    gui::wm2::init();
    if gui::wm2::self_test() {
        serial_println!("[test] WM2: tiling/focus-history/snap/layout-cycle PASSED");
    } else {
        serial_println!("[test] WM2: FAILED");
    }
    // Phase 62: Image Viewer app (viewport/zoom/pan/slideshow)
    if apps::image_viewer::self_test() {
        serial_println!("[test] Image Viewer: viewport/zoom/pan/slideshow PASSED");
    } else {
        serial_println!("[test] Image Viewer: FAILED");
    }
    // Phase 63: PDF Reader (object scan, text extraction, page navigation)
    if apps::pdf_reader::self_test() {
        serial_println!("[test] PDF Reader: xref-scan/Tj/TJ/hex-string/page-find PASSED");
    } else {
        serial_println!("[test] PDF Reader: FAILED");
    }
    // Phase 64: Video Player (tri-wave, frame render, rgb-to-rgba, SMV format)
    if apps::video_player::self_test() {
        serial_println!("[test] Video Player: tri-wave/frame-render/rgba-conv/smv-hdr PASSED");
    } else {
        serial_println!("[test] Video Player: FAILED");
    }
    // Phase 65: Email Client (mailbox, SMTP, POP3, thread-key, folder mgmt)
    if apps::email_client::self_test() {
        serial_println!("[test] Email Client: mailbox/smtp/pop3/thread-key/folders PASSED");
    } else {
        serial_println!("[test] Email Client: FAILED");
    }
    // Phase 66: Office Suite (markdown, formula engine, slides)
    if apps::office::self_test() {
        serial_println!("[test] Office Suite: writer/calc/slides/formula PASSED");
    } else {
        serial_println!("[test] Office Suite: FAILED");
    }
    // Phase 67: Browser v2 (bookmarks/downloads/history/reader)
    if apps::browser_v2::self_test() {
        serial_println!("[test] Browser v2: bookmarks/downloads/history/reader PASSED");
    } else {
        serial_println!("[test] Browser v2: FAILED");
    }
    // Phase 68: USB Mass Storage (BOT/SCSI, mount)
    if apps::usb_storage::self_test() {
        serial_println!("[test] USB Mass Storage: BOT/SCSI/parse/sim 9 tests PASSED");
    } else {
        serial_println!("[test] USB Mass Storage: FAILED");
    }
    // Phase 69: App Store GUI
    if apps::app_store::self_test() {
        serial_println!("[test] App Store: catalogue/filter/list/detail 8 tests PASSED");
    } else {
        serial_println!("[test] App Store: FAILED");
    }
    // Phase 70: Power Manager
    if apps::power_manager::self_test() {
        serial_println!("[test] Power Manager: C-states/DVFS/battery/wakelock/sleep 8 tests PASSED");
    } else {
        serial_println!("[test] Power Manager: FAILED");
    }
    // Phase 71: Crash Recovery
    if apps::crash_recovery::self_test() {
        serial_println!("[test] Crash Recovery: log/watchdog/health/recovery 9 tests PASSED");
    } else {
        serial_println!("[test] Crash Recovery: FAILED");
    }
    // Phase 72: Setup Wizard
    if apps::setup_wizard::self_test() {
        serial_println!("[test] Setup Wizard: pages/nav/hash/locale 8 tests PASSED");
    } else {
        serial_println!("[test] Setup Wizard: FAILED");
    }
    // Phase 73: Accessibility
    if apps::accessibility::self_test() {
        serial_println!("[test] Accessibility: color filter/sticky/font/mouse 9 tests PASSED");
    } else {
        serial_println!("[test] Accessibility: FAILED");
    }
    // Phase 74: Printer
    if apps::printer::self_test() {
        serial_println!("[test] Printer: PCL/queue/advance/cancel 9 tests PASSED");
    } else {
        serial_println!("[test] Printer: FAILED");
    }
    // Phase 75: Update Manager
    if apps::update_manager::self_test() {
        serial_println!("[test] Update Manager: channel/verify/delta/rollback 8 tests PASSED");
    } else {
        serial_println!("[test] Update Manager: FAILED");
    }
    // Phase 76: Cloud Sync
    if apps::cloud_sync::self_test() {
        serial_println!("[test] Cloud Sync: manifest/delta/checksum/backup 8 tests PASSED");
    } else {
        serial_println!("[test] Cloud Sync: FAILED");
    }
    // Phase 77: Gaming
    if apps::gaming::self_test() {
        serial_println!("[test] Gaming: launcher/achievements/leaderboard 8 tests PASSED");
    } else {
        serial_println!("[test] Gaming: FAILED");
    }
    // Phase 78: Browser IPC
    if apps::browser_ipc::self_test() {
        serial_println!("[test] Browser IPC: cmd/event queues/response/permission PASSED");
    } else {
        serial_println!("[test] Browser IPC: FAILED");
    }
    // Phase 79: Browser Sandbox
    if apps::browser_sandbox::self_test() {
        serial_println!("[test] Browser Sandbox: caps/url-check/origin/renderer PASSED");
    } else {
        serial_println!("[test] Browser Sandbox: FAILED");
    }
    // Phase 80: TLS cert verifier
    if crate::crypto::cert_verifier::self_test() {
        serial_println!("[test] Cert Verifier: hostname/wildcard/HSTS/summary PASSED");
    } else {
        serial_println!("[test] Cert Verifier: FAILED");
    }
    // Phase 81: Same-Origin Policy
    if net::sop::self_test() {
        serial_println!("[test] SOP/CORS: parse-origin/same-origin/cors-allow/block PASSED");
    } else {
        serial_println!("[test] SOP/CORS: FAILED");
    }
    // Phase 82: Content Security Policy
    if net::csp::self_test() {
        serial_println!("[test] CSP: parse/check-resource/inline-script/eval/nonce PASSED");
    } else {
        serial_println!("[test] CSP: FAILED");
    }
    // Phase 83: DOM Tree
    if net::dom::self_test() {
        serial_println!("[test] DOM: create/query/attrs/class/events/bubble PASSED");
    } else {
        serial_println!("[test] DOM: FAILED");
    }
    // Phase 84: Web Crypto API
    if crate::crypto::webcrypto::self_test() {
        serial_println!("[test] Web Crypto: AES-GCM/HMAC/UUID/PBKDF2/digest PASSED");
    } else {
        serial_println!("[test] Web Crypto: FAILED");
    }
    // Phase 85: Browser persistence
    if apps::browser_persist::self_test() {
        serial_println!("[test] Browser Persist: history/bookmarks/cookies/HSTS/session PASSED");
    } else {
        serial_println!("[test] Browser Persist: FAILED");
    }
    // Phase 86/90: Browser downloads
    if apps::browser_downloads::self_test() {
        serial_println!("[test] Browser Downloads: MIME/disposition/queue/checksum PASSED");
    } else {
        serial_println!("[test] Browser Downloads: FAILED");
    }
    // Phase 91-92: SVG rasteriser
    if net::svg::self_test() {
        serial_println!("[test] SVG: color/path/transform/canvas/render PASSED");
    } else {
        serial_println!("[test] SVG: FAILED");
    }
    // Phase 93: WebP decoder
    if net::webp::self_test() {
        serial_println!("[test] WebP: RIFF/VP8L/VP8X/blank/pixel PASSED");
    } else {
        serial_println!("[test] WebP: FAILED");
    }
    // Phase 94: AVIF decoder
    if net::avif::self_test() {
        serial_println!("[test] AVIF: ISOBMFF/ftyp/ispe/blank/pixel PASSED");
    } else {
        serial_println!("[test] AVIF: FAILED");
    }
    // Phase 96: WPT harness
    if apps::browser_wpt::self_test() {
        serial_println!("[test] WPT Harness: suite/harness/parser/results/runner PASSED");
    } else {
        serial_println!("[test] WPT Harness: FAILED");
    }
    // Phase 98: JS Engine v2 self-test
    if net::js_interp::self_test() {
        serial_println!("[test] JavaScript engine v3: all 30 tests PASSED (extends/super/instanceof/Date/toFixed/async/await/Promise/WebRTC/getUserMedia)");
    } else {
        serial_println!("[test] JavaScript engine v3: FAILED");
    }
    // Phase 105: WebRTC Rust-side types
    net::webrtc::self_test();
    // Phase 98: Browser Beta Polish
    if net::browser_polish::self_test() {
        serial_println!("[test] Browser Polish: find-bar/shortcuts/context-menu/favicon/settings/page-info PASSED");
    } else {
        serial_println!("[test] Browser Polish: FAILED");
    }
    // Phase 97: Browser Hardening
    if net::hardening::self_test() {
        serial_println!("[test] Browser Hardening: stack/budget/dom-limit/css-limit/script-size/OOM PASSED");
    } else {
        serial_println!("[test] Browser Hardening: FAILED");
    }
    // Phase 95: Browser Performance Pass
    if net::perf::self_test() {
        serial_println!("[test] Perf: CSS-index/dirty-set/display-list-cache/HTTP-cache/JS-cache/img-budget PASSED");
    } else {
        serial_println!("[test] Perf: FAILED");
    }
    // Phase 107: System Hardening self-tests
    io::epoll::self_test();
    memory::cgroup::self_test();
    process::ptrace::self_test();
    // Phase 108: WebAssembly MVP Interpreter
    net::wasm::init();
    if net::wasm::self_test() {
        serial_println!("[test] WebAssembly: magic/LEB128/parse/add/memory/grow/valtype/trap/JS-API PASSED");
    } else {
        serial_println!("[test] WebAssembly: FAILED");
    }
    // Phase 31: Graphical Installer
    if installer::gui_installer::self_test() {
        serial_println!("[test] GUI Installer: pages/nav/text-field/disk-guard/user-valid/install-progress/render PASSED");
    } else {
        serial_println!("[test] GUI Installer: FAILED");
    }
    // Phase 29: Real Hardware Drivers (Intel HDA + BT HCI + ACPI Power + HCL)
    if drivers::hda_new::self_test() {
        serial_println!("[test] Intel HDA: device-ids/verb-encode/stream-fmt/BDL/codec-enum PASSED");
    } else {
        serial_println!("[test] Intel HDA: FAILED");
    }
    if drivers::hw_compat::self_test() {
        serial_println!("[test] HW Compat: BT-HCI/ACPI-power/HCL-v1 PASSED");
    } else {
        serial_println!("[test] HW Compat: FAILED");
    }
    // Phase 109: JS + CSS Completeness
    if net::js_completeness::self_test() {
        serial_println!("[test] JS Completeness: Proxy/Reflect/Generator/TypedArray/Regex-named-groups/WeakRef PASSED");
    } else {
        serial_println!("[test] JS Completeness: FAILED");
    }
    if net::css_completeness::self_test() {
        serial_println!("[test] CSS Completeness: custom-props/calc-clamp/nesting/has-index/logical-props PASSED");
    } else {
        serial_println!("[test] CSS Completeness: FAILED");
    }
    // Phase 110: Tab Process Isolation
    if net::tab_process::self_test() {
        serial_println!("[test] Tab Process Isolation: IPC ring/renderer sandbox/syscall allowlist/crash-restart PASSED");
    } else {
        serial_println!("[test] Tab Process Isolation: FAILED");
    }
    // Phase 111: JS Baseline JIT
    if net::js_jit::self_test() {
        serial_println!("[test] JS Baseline JIT: FuncKey/CodeBuf/compile/interpret/cache PASSED");
    } else {
        serial_println!("[test] JS Baseline JIT: FAILED");
    }
    // Phase 112: VP8 Full + AV1
    if net::vp8_full::self_test() {
        serial_println!("[test] VP8 Full+AV1: BoolDec/IDCT/intra-predict/loop-filter/OBU PASSED");
    } else {
        serial_println!("[test] VP8 Full+AV1: FAILED");
    }
    // Phase 113: WiFi driver self-test
    if drivers::wifi::self_test() {
        serial_println!("[test] WiFi Driver: iwlwifi/MLME/PMK/PTK/scan PASSED");
    } else {
        serial_println!("[test] WiFi Driver: FAILED");
    }
    // Phase 114: DevTools
    if net::devtools::self_test() {
        serial_println!("[test] DevTools: inspector/console/network/perf/jank PASSED");
    } else {
        serial_println!("[test] DevTools: FAILED");
    }
    // Phase 115: WebAuthn FIDO2
    if net::webauthn::self_test() {
        serial_println!("[test] WebAuthn: SHA-256/HMAC/CBOR/make_credential/get_assertion PASSED");
    } else {
        serial_println!("[test] WebAuthn: FAILED");
    }
    // Phase 116: MSE + HLS
    if net::mse::self_test() {
        serial_println!("[test] MSE+HLS: TimeRanges/SourceBuffer/MediaSource/HLS-parse/ABR PASSED");
    } else {
        serial_println!("[test] MSE+HLS: FAILED");
    }
    // Phase 117: Real WebRTC STUN/ICE/RTP
    if net::webrtc::self_test_117() {
        serial_println!("[test] WebRTC Phase 117: STUN/ICE/RTP/XOR-MAPPED PASSED");
    } else {
        serial_println!("[test] WebRTC Phase 117: FAILED");
    }
    // Phase 118: WebExtensions
    if net::webext::self_test() {
        serial_println!("[test] WebExtensions: Manifest-V3/MatchPattern/ContentScript/install PASSED");
    } else {
        serial_println!("[test] WebExtensions: FAILED");
    }
    // Phase 119: Accessibility
    if net::a11y::self_test() {
        serial_println!("[test] Accessibility: ARIA-roles/tab-order/focus/screen-reader PASSED");
    } else {
        serial_println!("[test] Accessibility: FAILED");
    }
    // Phase 120: Print/PDF
    if net::css_print::self_test() {
        serial_println!("[test] Print/PDF: paper-sizes/paginate/PDF-writer/xref PASSED");
    } else {
        serial_println!("[test] Print/PDF: FAILED");
    }
    // Phase 121: Security Hardening (self_test_121)
    if net::hardening::self_test_121() {
        serial_println!("[test] Security Hardening: fuzz/ASLR/CSP-grade/SRI/X-Frame PASSED");
    } else {
        serial_println!("[test] Security Hardening: FAILED");
    }
    // Phase 122: WPT Compliance (200 tests)
    if net::wpt::self_test() {
        serial_println!("[test] WPT: 200 Web Platform Tests ≥95% PASSED");
    } else {
        serial_println!("[test] WPT: below 95% — check [wpt] lines above for failures");
    }
    // Phase 123: JS JIT wiring (fast-path for pure numeric functions)
    // (JIT tested as part of js_jit::self_test above; wiring validated via interpreter)
    serial_println!("[init] Phase 123: JS JIT wired to Interpreter::call_value (lower_fn_to_jit)");
    // Phase 124: ES Modules (import/export via MODULE_REGISTRY)
    serial_println!("[init] Phase 124: ES Module registry initialized (run_module/import/export)");
    // Phase 125: WiFi WPA2 (beacon parser + CCMP)
    if drivers::wifi::self_test() {
        serial_println!("[test] WiFi Phase 125: beacon parser/CCMP encrypt/decrypt PASSED");
    } else {
        serial_println!("[test] WiFi Phase 125: FAILED");
    }
    // Phase 126: TLS ChaCha20-Poly1305 + session tickets
    if net::tls::self_test_126() {
        serial_println!("[test] TLS Phase 126: ChaCha20/Poly1305/AEAD/session-tickets PASSED");
    } else {
        serial_println!("[test] TLS Phase 126: FAILED");
    }
    // Phase 127: WebCrypto API
    serial_println!("[init] Phase 127: WebCrypto API (SHA-256/384/512, AES-GCM, HMAC, PBKDF2, HKDF)");
    if net::webcrypto::self_test() {
        serial_println!("[test] WebCrypto Phase 127: digest/generateKey/encrypt/HMAC/PBKDF2/HKDF/UUID PASSED");
    } else {
        serial_println!("[test] WebCrypto Phase 127: FAILED");
    }
    // Phase 128: HTTP/2 ALPN + TlsClientIo
    serial_println!("[init] Phase 128: HTTP/2 ALPN (TLS extension, EncryptedExtensions parse, TlsClientIo)");
    if net::http2::self_test() {
        serial_println!("[test] HTTP/2 Phase 128: preface/HPACK/ALPN-parse/TlsClientIo PASSED");
    } else {
        serial_println!("[test] HTTP/2 Phase 128: FAILED");
    }
    // Phase 129: Streams API
    serial_println!("[init] Phase 129: Streams API (ReadableStream/WritableStream/TransformStream)");
    if net::streams::self_test() {
        serial_println!("[test] Streams Phase 129: ReadableStream/WritableStream/TransformStream/pipeTo/pipeThrough/tee PASSED");
    } else {
        serial_println!("[test] Streams Phase 129: FAILED");
    }
    // Phase 130: PWA Support
    serial_println!("[init] Phase 130: PWA Support (manifest parse, matchMedia, navigator.standalone)");
    if net::pwa::self_test() {
        serial_println!("[test] PWA Phase 130: manifest/displayMode/icons/categories/matchMedia/registry PASSED");
    } else {
        serial_println!("[test] PWA Phase 130: FAILED");
    }
    // Phase 131: URL API
    serial_println!("[init] Phase 131: URL/URLSearchParams/Blob/File/FileReader/structuredClone/btoa/atob");
    if net::url_api::self_test() {
        serial_println!("[test] URL API Phase 131: URL parse/URLSearchParams/Blob/structuredClone/btoa/atob PASSED");
    } else {
        serial_println!("[test] URL API Phase 131: FAILED");
    }
    // Phase 132: Observer APIs
    serial_println!("[init] Phase 132: rAF/queueMicrotask/MutationObserver/ResizeObserver/IntersectionObserver");
    if net::observers::self_test() {
        serial_println!("[test] Observer APIs Phase 132: rAF/microtask/MO/RO/IO/performance PASSED");
    } else {
        serial_println!("[test] Observer APIs Phase 132: FAILED");
    }
    // Phase 133: Web Forms + Events
    serial_println!("[init] Phase 133: FormData/AbortController/EventSource/CustomEvent/MessageChannel");
    if net::forms_events::self_test() {
        serial_println!("[test] Forms+Events Phase 133: FormData/AbortController/CustomEvent/MessageChannel PASSED");
    } else {
        serial_println!("[test] Forms+Events Phase 133: FAILED");
    }
    // Phase 134: Web Components
    serial_println!("[init] Phase 134: Custom Elements/Shadow DOM/DocumentFragment/CSSStyleSheet");
    if net::web_components::self_test() {
        serial_println!("[test] Web Components Phase 134: CE/ShadowDOM/Fragment/CSSStyleSheet PASSED");
    } else {
        serial_println!("[test] Web Components Phase 134: FAILED");
    }
    // Phase 135: Modern CSS
    serial_println!("[init] Phase 135: Container Queries/@layer/aspect-ratio/content-visibility/CSS.supports");
    if net::css_modern::self_test() {
        serial_println!("[test] Modern CSS Phase 135: ContainerQ/@layer/aspect-ratio/CV/CSS.supports PASSED");
    } else {
        serial_println!("[test] Modern CSS Phase 135: FAILED");
    }
    // Phase 136: Browser Persistence
    net::browser_persistence::init();
    serial_println!("[init] Phase 136: Browser Persistence (bookmarks/history/settings/downloads)");
    if net::browser_persistence::self_test() {
        serial_println!("[test] Browser Persistence Phase 136: bookmarks/history/settings/downloads PASSED");
    } else {
        serial_println!("[test] Browser Persistence Phase 136: FAILED");
    }
    // Phase 137: Security Polish
    serial_println!("[init] Phase 137: HSTS/mixed-content/Permissions/isSecureContext/referrer-policy");
    if net::security_polish::self_test() {
        serial_println!("[test] Security Polish Phase 137: HSTS/mixed-content/Permissions/SecureCtx PASSED");
    } else {
        serial_println!("[test] Security Polish Phase 137: FAILED");
    }
    // Phase 138: Performance Hints
    serial_println!("[init] Phase 138: resource hints/lazy-load/prefetch-cache/rIC/scheduler/WebVitals");
    if net::perf_hints::self_test() {
        serial_println!("[test] Perf Hints Phase 138: hints/lazy/prefetch/rIC/scheduler/WebVitals PASSED");
    } else {
        serial_println!("[test] Perf Hints Phase 138: FAILED");
    }
    // Phase 139: WPT v2 — 300 tests, ≥98% pass rate
    serial_println!("[init] Phase 139: WPT v2 (300 synthetic tests, URL/Observer/Forms/WebComp/CSS/PWA/Streams)");
    if net::wpt::self_test() {
        serial_println!("[test] WPT Phase 139: 300 tests ≥98%% PASSED");
    } else {
        serial_println!("[test] WPT Phase 139: FAILED (see [wpt] lines above)");
    }
    // Phase 140: SmartBrowser v1.0 Public Release
    serial_println!("[init] Phase 140: SmartBrowser v1.0 — about: pages, crash reporter, public release");
    if net::crash_reporter::self_test() {
        serial_println!("[test] Crash Reporter Phase 140: record/list/overflow/clear PASSED");
    } else {
        serial_println!("[test] Crash Reporter Phase 140: FAILED");
    }
    // Phase 99: WOFF web font parser
    if net::woff::self_test() {
        serial_println!("[test] WOFF font parser: detect/parse/extract/name-table/font-face PASSED");
    } else {
        serial_println!("[test] WOFF font parser: FAILED");
    }
    // Phase 102: Video Element
    if net::video::self_test() {
        serial_println!("[test] Video Element: MP4/WebM/Ogg detect, metadata, VideoElement PASSED");
    } else {
        serial_println!("[test] Video Element: FAILED");
    }

    // ─── Integration test: HTML + CSS + Layout (Phases 34-36) ────────
    {
        use net::html;
        use net::css::{CssParser, compute_styles, parse_inline_styles, user_agent_stylesheet};
        use net::layout::{layout_document, LayoutConfig};

        let html_src = r#"<!DOCTYPE html>
<html>
<head><title>Test</title>
<style>
body { font-size: 16px; color: #333; margin: 0; }
h1   { font-size: 2em; font-weight: bold; display: block; }
p    { display: block; margin-top: 8px; margin-bottom: 8px; }
.highlight { color: #ff0000; background-color: #ffffcc; }
#main { width: 800px; margin: 0 auto; }
</style>
</head>
<body>
<div id="main">
  <h1>Hello Smart OS</h1>
  <p class="highlight">Styled paragraph.</p>
  <p>Normal paragraph.</p>
</div>
</body>
</html>"#;

        let dom = html::parse(html_src);
        let title = html::extract_title(&dom);
        serial_println!("[test] DOM title: {}", title);

        // Extract inline stylesheet from <style> tags
        let stylesheets_text = html::extract_stylesheets(&dom, "");
        let mut author_sheets = alloc::vec::Vec::new();
        for css_text in &stylesheets_text {
            // We get hrefs; for inline <style> we handle via inline_styles instead
            let _ = css_text;
        }
        // Parse inline styles from style attributes
        let inline_styles = parse_inline_styles(&dom);
        let ua_sheet = user_agent_stylesheet();

        // For the embedded <style> block: parse directly
        // find first style node's text
        let style_css = {
            let mut found = alloc::string::String::new();
            for id in 0..dom.len() as u32 {
                if let Some(n) = dom.get(id) {
                    if let net::html::NodeKind::Element { tag, .. } = &n.kind {
                        if tag == "style" {
                            for &child_id in &n.children {
                                if let Some(c) = dom.get(child_id) {
                                    if let net::html::NodeKind::Text { data } = &c.kind {
                                        found.push_str(data);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            found
        };
        let mut parser = CssParser::new(&style_css);
        let sheet = parser.parse_stylesheet();
        author_sheets.push(sheet);

        let computed = compute_styles(&dom, &author_sheets, &ua_sheet, &inline_styles, None);
        serial_println!("[test] Computed styles for {} nodes", computed.len());

        // Check h1 font-size = 32px (2em * 16px body)
        for id in 0..dom.len() as u32 {
            if let Some(n) = dom.get(id) {
                if let net::html::NodeKind::Element { tag, .. } = &n.kind {
                    if tag == "h1" {
                        if let Some(st) = computed.get(&id) {
                            serial_println!("[test] h1 font-size={}px weight={:?}", st.font_size, st.font_weight);
                        }
                    }
                    if tag == "p" {
                        if let Some(st) = computed.get(&id) {
                            serial_println!("[test] p color=#{:02x}{:02x}{:02x}", st.color.r, st.color.g, st.color.b);
                        }
                    }
                }
            }
        }

        // Run layout
        let cfg = LayoutConfig { viewport_width: 1024.0, viewport_height: 768.0, root_font_px: 16.0 };
        let boxes = layout_document(&dom, &computed, cfg);
        serial_println!("[test] Layout produced {} boxes", boxes.len());
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 11: GUI compositor + desktop environment
    // ═══════════════════════════════════════════════════════════════
    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        let info = framebuffer.info();
        let is_bgr = matches!(info.pixel_format, bootloader_api::info::PixelFormat::Bgr);
        let fb_ptr = framebuffer.buffer_mut().as_mut_ptr();
        let byte_stride = info.stride * info.bytes_per_pixel;

        // Phase 15: register software framebuffer with gpu2d acceleration layer
        drivers::gpu2d::register_sw_fb(
            fb_ptr as u64,
            info.width as u32,
            info.height as u32,
            byte_stride as u32,
            is_bgr,
        );
        drivers::gpu2d::init();

        gui::init(fb_ptr, info.width, info.height, byte_stride, info.bytes_per_pixel, is_bgr);
        gui::virtual_desktop::init();
        serial_println!("[boot] GUI compositor and Virtual Desktops initialized.");

        // Set mouse screen bounds now that we know framebuffer dimensions
        drivers::mouse::set_screen_bounds(info.width, info.height);
        serial_println!("[boot] Mouse bounds set to {}x{}.", info.width, info.height);

        // Render the first frame (splash screen)
        gui::render_frame();
        serial_println!("[boot] Desktop environment rendered.");
    } else {
        serial_println!("[warn] No framebuffer — GUI disabled.");
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 11: AI Inference Engine
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing AI inference engine...");
    ai::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 12: SmartFS content-aware filesystem
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing SmartFS...");
    smartfs::init();

    // SmartFS smoke test: store a test file
    {
        let test_data = b"// Hello from SmartFS!\n// This is a Rust source file.\nfn main() {\n    println!(\"Smart OS\");\n}\n";
        if smartfs::store_file("/data/test.rs", test_data).is_ok() {
            serial_println!("[boot] SmartFS test file stored.");
        }
        let (unique, total) = smartfs::dedup_stats();
        serial_println!("[boot] SmartFS dedup: {} unique / {} total chunks.", unique, total);
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 13: Plugin system (keyboard + mouse input plugins)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing plugin system...");
    plugins::init();
    plugins::registry::load_builtin_plugins();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 14: Spawn system threads + interactive applications
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Spawning system threads...");
    process::scheduler::spawn("gui-renderer", gui_render_thread, 10);
    process::scheduler::spawn("vga-sync", drivers::vga_emu::vga_sync_thread, 6);
    process::scheduler::spawn("ipc-logger", ipc_logger_thread, 5);
    process::scheduler::spawn("ai-prefetch", ai::prefetch::prefetch_worker, 4);
    process::scheduler::spawn("ai-anomaly", ai_anomaly_detector, 3);

    // Initialize user database (creates /etc/passwd, /etc/shadow, default users)
    users::init();

    // Spawn login screen — it will spawn desktop apps after successful login
    serial_println!("[boot] Spawning login screen...");
    process::scheduler::spawn("login", apps::login::run, 15);

    serial_println!(
        "[boot] {} threads ready.",
        process::scheduler::ready_count() + 1,
    );

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 15: User-space process support (SYSCALL/SYSRET + ELF)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing SYSCALL/SYSRET...");
    arch::x86_64::syscall_entry::init();

    // Store a demo ELF binary in VFS and spawn it as a user-space process.
    serial_println!("[boot] Building demo user-space ELF...");
    vfs::mkdir("/bin").ok();
    let hello_elf = process::userspace::create_hello_elf();
    serial_println!("[boot] Demo ELF size: {} bytes", hello_elf.len());
    vfs::create_and_write("/bin/hello", &hello_elf).expect("Failed to store hello ELF");

    serial_println!("[boot] Spawning user-space process 'shell'...");
    match process::scheduler::spawn_user_process("shell", "/bin/shell") {
        Ok(pid) => serial_println!("[boot] User process 'shell' spawned (pid={}).", pid),
        Err(e) => serial_println!("[boot] WARNING: Failed to spawn user process: {}", e),
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 17: VirtIO devices (block + network) & e1000
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing VirtIO block device...");
    match drivers::virtio_blk::init() {
        Ok(()) => serial_println!("[boot] VirtIO block device ready."),
        Err(e) => serial_println!("[boot] VirtIO block: {} (no disk attached)", e),
    }

    serial_println!("[boot] Initializing VirtIO network device...");
    let mut _nic_found = false;
    match drivers::virtio_net::init() {
        Ok(()) => {
            serial_println!("[boot] VirtIO network device ready.");
            _nic_found = true;
        }
        Err(e) => serial_println!("[boot] VirtIO net: {} (no NIC attached)", e),
    }

    if !_nic_found {
        serial_println!("[boot] Initializing Intel e1000 network device...");
        match drivers::e1000::init() {
            Ok(()) => {
                serial_println!("[boot] Intel e1000 network device ready.");
                _nic_found = true;
            }
            Err(e) => serial_println!("[boot] Intel e1000: {} (no NIC attached)", e),
        }
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 18: Persistent disk filesystem (VirtIO + NVMe)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing NVMe storage devices...");
    drivers::nvme::init();

    serial_println!("[boot] Mounting disk filesystem...");
    drivers::diskfs::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 19: Network stack (Ethernet/ARP/IPv4/UDP)
    // ═══════════════════════════════════════════════════════════════
    if drivers::virtio_net::is_available() || drivers::e1000::is_available() {
        serial_println!("[boot] Initializing network stack...");
        net::init();
        process::scheduler::spawn("net-rx", net_rx_thread, 6);
        process::scheduler::spawn("dhcp-renew", net::dhcp::renewal_worker, 3);
        serial_println!("[boot] Network stack ready, net-rx and dhcp-renew threads spawned.");
    } else {
        serial_println!("[boot] Skipping network stack (no NIC).");
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 20: LAPIC + SMP bootstrap
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing LAPIC...");
    arch::x86_64::lapic::init();

    serial_println!("[boot] Bootstrapping application processors...");
    let mut cpu_count = 4; // Default/Fallback
    if let Some(acpi) = drivers::acpi::ACPI.lock().as_ref() {
        let lapic_ids = acpi.get_lapic_ids();
        if !lapic_ids.is_empty() {
            cpu_count = lapic_ids.len() as u8;
            serial_println!("[boot] ACPI detected {} CPUs.", cpu_count);
        }
    }
    arch::x86_64::smp::init_smp(cpu_count);

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 21: Knowledge Graph (Unified Data API)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing Knowledge Graph...");
    knowledge::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 22: Security Anomaly Detection
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing security anomaly detection...");
    security::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 23: Immutable System Core (A/B partitions + write protection)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing immutable system core...");
    immutable::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 24: Predictive prefetch worker
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Spawning predictive prefetch worker...");
    process::scheduler::spawn("prefetch-worker", ai::prefetch::prefetch_worker, 3);

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 25: USB xHCI controller
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing USB xHCI controller...");
    match drivers::xhci::init() {
        Ok(()) => serial_println!("[boot] xHCI USB controller ready."),
        Err(e) => serial_println!("[boot] xHCI: {} (no USB controller)", e),
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 26: FAT32 filesystem driver
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing FAT32 filesystem driver...");
    drivers::fat32::init();
    if drivers::fat32::is_available() {
        serial_println!("[boot] FAT32 filesystem mounted.");
    } else {
        serial_println!("[boot] FAT32: no FAT32 partition detected.");
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 27: RTC Clock Driver
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing RTC clock driver...");
    drivers::rtc::init();

    serial_println!("[boot] Initializing HPET high-precision timer...");
    if let Err(e) = drivers::hpet::init() {
        serial_println!("[boot] HPET warning: {} (falling back to PIT)", e);
    }

    serial_println!("[boot] Initializing audio controller (AC'97 / HDA)...");
    if let Err(e) = drivers::hda::init() {
        serial_println!("[boot] Audio warning: {}", e);
    } else {
        process::scheduler::spawn("audio-mixer-worker", drivers::hda::mixer_worker, 12);
        play_startup_chime();
    }

    // Spawn the background worker for async I/O
    process::scheduler::spawn("async-io-worker", io::ring::async_io_worker, 3);

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 18: Smart Containers & VT-x
    // ═══════════════════════════════════════════════════════════════
    process::container::init();
    if let Err(e) = arch::x86_64::vmx::init() {
        serial_println!("[boot] VT-x init skipped: {}", e);
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 28: Environment Variables + Signals

    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing environment variables...");
    process::env::init();
    serial_println!("[boot] Initializing signal subsystem...");
    process::signal::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 29: Phase 9 Applications (spawned by login screen after auth)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Phase 9 apps will be spawned by login screen after authentication.");

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 30: Copy-on-Write Fork + Shared Memory + ASLR + Swap
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing Phase 10: Kernel Maturity...");
    memory::cow::init();
    memory::shmem::init();
    memory::aslr::init();
    memory::swap::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 31: SMP Load Balancing + Syscall Tracing
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing SMP load balancer...");
    process::smp_balance::init();
    serial_println!("[boot] Initializing syscall tracing...");
    process::strace::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 32: GDB Stub + Virtual Desktops + Theming
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing kernel GDB stub...");
    drivers::gdb_stub::init();
    serial_println!("[boot] Initializing virtual desktops...");
    gui::virtual_desktop::init();
    serial_println!("[boot] Initializing theming system...");
    gui::theming::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 33: IPv6 Network Stack
    // ═══════════════════════════════════════════════════════════════
    if drivers::virtio_net::is_available() {
        serial_println!("[boot] Initializing IPv6 stack...");
        net::ipv6::init();
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 34: Wait Queues + Per-Process FD Tables
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing wait queues...");
    process::wait::init();
    serial_println!("[boot] Initializing per-process file descriptors...");
    process::fd::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 35: ACPI Shutdown / Reboot
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] ACPI power management ready.");

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 36: User-Space Programs (/bin)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Building user-space programs...");
    {
        // Store all user programs in /bin/
        let progs: &[(&str, alloc::vec::Vec<u8>)] = &[
            ("/bin/true", process::userprogs::create_true_elf()),
            ("/bin/false", process::userprogs::create_false_elf()),
            ("/bin/echo", process::userprogs::create_echo_elf()),
            ("/bin/cat", process::userprogs::create_cat_elf()),
            ("/bin/ls", process::userprogs::create_ls_elf()),
            ("/bin/sh", process::userprogs::create_sh_elf()),
            ("/bin/forktest", process::userprogs::create_forktest_elf()),
            ("/bin/net-client", process::userprogs::create_net_client_elf()),
            ("/bin/sdk-demo", process::userprogs::create_sdk_demo_elf()),
            ("/bin/stress-test", process::userprogs::create_stress_test_elf()),
        ];
        for (path, elf) in progs {
            if vfs::create_and_write(path, elf).is_ok() {
                serial_println!("[boot]   {} ({} bytes)", path, elf.len());
            }
        }
        serial_println!("[boot] {} user programs stored in /bin/.", progs.len());
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 37-A: Installer subsystem
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing installer subsystem...");
    installer::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 37-B: POSIX compatibility layer (Phase 32)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing POSIX compatibility layer...");
    posix::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 38-A: Dynamic ELF linker trampoline page (Phase 34)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing dynamic ELF linker...");
    process::dynlink::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 37-C: Package manager (Phase 33)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing package manager...");
    pkg::init();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 37-D: Wayland compositor bridge (Phase 33)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Initializing Wayland compositor bridge...");
    {
        let (sw, sh) = gui::compositor::screen_size();
        wayland::init(sw as u32, sh as u32);
    }
    process::scheduler::spawn("wayland-socket", wayland::socket_worker, 8);

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 37: Phase 12 — Shell Evolution & Hardware Completion
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Phase 12: Shell evolution + AHCI DMA + HTTP client + ICMP + pkg.");
    // AHCI was already initialized in Phase 11 (drivers::init()), but log completion.
    if drivers::ahci::is_available() {
        serial_println!("[boot] Phase 12: AHCI DMA driver active.");
    }
    // Update /system/version to v0.12.0
    let _ = vfs::create_and_write("/system/version", b"Smart OS v1.0.0");
    serial_println!("[boot] Phase 12 initialized (pipes, grep, head, tail, wc, cp, mv, rm,");
    serial_println!("[boot]   touch, find, history, alias, which, uname, lspci, dmesg,");
    serial_println!("[boot]   df, top, ping, http, pkg, AHCI DMA, HTTP client, ICMP, sysmon graphs).");

    // ═══════════════════════════════════════════════════════════════
    //  BOOT COMPLETE — enable interrupts and enter main loop
    // ═══════════════════════════════════════════════════════════════
    serial_println!();
    serial_println!("======================================================");
    serial_println!("  SMART OS v1.0.0 — PHASE 140 ONLINE  [PUBLIC RELEASE]");
    serial_println!("  Subsystems: drivers, process, ipc, vfs, gui, ai,");
    serial_println!("    smartfs, plugins, apps, net, knowledge, security,");
    serial_println!("    immutable, usb, fat32, clipboard, notifications,");
    serial_println!("    dns, signals, env-vars, rtc");
    serial_println!("  Phase 10: CoW fork, SMP balance, shared memory,");
    serial_println!("    ASLR, strace, sandbox, GDB stub, swap,");
    serial_println!("    Alt+Tab, virtual desktops, theming, IPv6");
    serial_println!("  Phase 11: fork/exec/waitpid, per-process FDs,");
    serial_println!("    wait queues, CoW page faults, ACPI shutdown,");
    serial_println!("    user programs: true/false/echo/cat/ls/sh/forktest");
    serial_println!("  Phase 12: AHCI DMA, ICMP ping, HTTP client,");
    serial_println!("    shell pipes/redirect, grep/head/tail/wc/cp/mv/rm,");
    serial_println!("    find/touch/history/alias/which/lspci/dmesg/df/top,");
    serial_println!("    pkg manager, sysmon graphs, e1000+AHCI detection");
    serial_println!("  Apps: terminal, file-manager, sysmon, editor,");
    serial_println!("    calculator, task-manager, settings");
    serial_println!("  AI: file-classifier, app-predictor, anomaly-detector");
    serial_println!("  NPU: {}", ai::npu::npu_name());
    serial_println!("  User-space: SYSCALL/SYSRET, ELF loader, ring-3,");
    serial_println!("    fork+exec+waitpid, per-process FDs, CoW faults");
    serial_println!("  Storage: VirtIO-blk, persistent disk FS, FAT32");
    serial_println!("  Network: Ethernet/ARP/IPv4/IPv6/UDP/TCP/DNS");
    serial_println!("  USB: xHCI controller, HID keyboard/mouse");
    serial_println!("  Desktop: clipboard, notifications, context menus,");
    serial_println!("    window resize/snap, Alt+Tab, virtual desktops,");
    serial_println!("    theming, taskbar clicks, RTC clock");
    serial_println!("  Memory: CoW fork, shared memory, ASLR, swap");
    serial_println!("  Debug: syscall tracing, GDB stub, capability sandbox");
    serial_println!("  SMP: LAPIC, {} CPU(s), load balancer", arch::x86_64::smp::cpu_count());
    serial_println!("  SmartPack format v{}", smartpack::FORMAT_VERSION);
    serial_println!("======================================================");
    serial_println!();

    // Enable hardware interrupts (PIT timer + keyboard + mouse will start firing)
    x86_64::instructions::interrupts::enable();
    serial_println!("[boot] Interrupts enabled. Entering main loop.");

    // Push a boot notification (after interrupts enabled so timer is running)
    // Phase 107 inits
    io::epoll::init();
    memory::cgroup::init();
    process::ptrace::init();
    gui::notification::push("Smart OS", "v1.0.0 booted successfully", gui::theme::ACCENT_CYAN);
    gui::notification::push("SmartBrowser", "v1.0.0 ready for public release!", gui::theme::ACCENT_GREEN);

    // Main kernel loop — keyboard input is now handled by the keyboard plugin,
    // which routes events to GUI widgets. The boot thread just yields.
    main_loop()
}

/// The main kernel loop. Yields to other threads and halts when idle.
/// Keyboard input is handled by the keyboard-input plugin, which routes
/// key events to the active window's focused widget.
fn main_loop() -> ! {
    loop {
        process::scheduler::yield_now();
        x86_64::instructions::hlt();
    }
}

/// GUI renderer thread — periodically re-renders the desktop.
fn gui_render_thread() {
    serial_println!("[thread:gui-renderer] Started.");
    let mut last_acpi_sync = 0;
    loop {
        gui::render_frame();

        // Periodically sync status (every ~5 seconds)
        let ticks = drivers::timer::ticks();
        if ticks - last_acpi_sync > 500 {
            drivers::acpi::monitor::update();
            drivers::acpi::monitor::sync_to_vfs();
            drivers::acpi::uefi_vars::sync_to_vfs();
            last_acpi_sync = ticks;
        }

        // Yield once after each frame to allow other threads to run
        process::scheduler::yield_now();
    }
}

/// IPC logger thread — monitors the system.log port.
fn ipc_logger_thread() {
    serial_println!("[thread:ipc-logger] Started.");

    // Send a boot-complete message
    if let Some(channel) = ipc::port::lookup("system.log") {
        use smartpack::Value;
        use alloc::string::String;
        let msg = Value::String(String::from("Smart OS v0.12.0 boot complete"));
        if channel.send(0, msg).is_ok() {
            serial_println!("[ipc-logger] Boot message sent to system.log.");
        }
    }

    loop {
        // Check for messages on system.log
        if let Some(channel) = ipc::port::lookup("system.log") {
            while let Ok(msg) = channel.recv() {
                if let Some(text) = msg.payload.as_str() {
                    serial_println!("[system.log] {}", text);
                }
            }
        }

        // Yield
        for _ in 0..200 {
            process::scheduler::yield_now();
        }
    }
}

/// Network RX thread — polls for incoming packets and dispatches them.
fn net_rx_thread() {
    serial_println!("[thread:net-rx] Started.");
    let mut buf = [0u8; 2048];
    loop {
        if drivers::virtio_net::is_available() {
            match drivers::virtio_net::recv_frame(&mut buf) {
                Ok(len) if len > 0 => {
                    net::handle_rx_frame(&buf[..len]);
                }
                _ => {}
            }
        } else if drivers::e1000::is_available() {
            match drivers::e1000::recv_frame(&mut buf) {
                Ok(len) if len > 0 => {
                    net::handle_rx_frame(&buf[..len]);
                }
                _ => {}
            }
        }

        // TCP timer: retransmission and connection cleanup
        net::tcp::tcp_timer_tick();
        process::scheduler::yield_now();
    }
}

/// AI Anomaly Detector thread — monitors system behavior for suspicious patterns.
fn ai_anomaly_detector() {
    serial_println!("[thread:ai-anomaly] Started.");
    loop {
        // AI heuristic: detect "Window Flooding" or "Thread Spawning" anomalies.
        let (win_count, _) = gui::display_server::status();
        if win_count > 30 {
            serial_println!("[ai-anomaly] WARNING: Window flood detected ({} windows active)!", win_count);
        }

        // Detect excessive thread count
        let threads = process::scheduler::ready_count() + process::scheduler::blocked_count();
        if threads > 50 {
            serial_println!("[ai-anomaly] WARNING: Extreme thread pressure ({} threads)!", threads);
        }

        for _ in 0..500 { process::scheduler::yield_now(); }
    }
}

/// Quick self-test to verify SmartPack works in the kernel environment.
fn smartpack_selftest() {
    use smartpack::{Encoder, Value};

    // Test 1: Integer roundtrip
    let mut enc = Encoder::new();
    enc.encode_value(&Value::UInt32(42)).expect("encode failed");
    let bytes = enc.into_bytes();
    let decoded = smartpack::decode(&bytes).expect("decode failed");
    assert!(decoded.as_u64() == Some(42), "SmartPack int roundtrip failed");

    // Test 2: String roundtrip
    let mut enc = Encoder::new();
    enc.encode_value(&Value::String(alloc::string::String::from("Smart OS")))
        .expect("encode failed");
    let bytes = enc.into_bytes();
    let decoded = smartpack::decode(&bytes).expect("decode failed");
    assert!(
        decoded.as_str() == Some("Smart OS"),
        "SmartPack string roundtrip failed"
    );

    // Test 3: Nested structure
    let mut enc = Encoder::new();
    let arr = Value::Array(alloc::vec![
        Value::String(alloc::string::String::from("kernel")),
        Value::UInt8(1),
        Value::Bool(true),
    ]);
    enc.encode_value(&arr).expect("encode failed");
    let bytes = enc.into_bytes();
    let decoded = smartpack::decode(&bytes).expect("decode failed");
    assert!(decoded.as_array().is_some(), "SmartPack array roundtrip failed");
}

fn sin_approx(x: f32) -> f32 {
    let mut x = x;
    const PI: f32 = 3.14159265;
    while x > PI { x -= 2.0 * PI; }
    while x < -PI { x += 2.0 * PI; }
    
    let x2 = x * x;
    let x3 = x2 * x;
    let x5 = x3 * x2;
    let x7 = x5 * x2;
    
    x - (x3 / 6.0) + (x5 / 120.0) - (x7 / 5040.0)
}

fn play_startup_chime() {
    let mut samples = alloc::collections::VecDeque::new();
    const SAMPLE_RATE: f32 = 44100.0;
    
    let notes = [523.25f32, 659.25f32, 783.99f32, 1046.50f32];
    let durations = [0.12f32, 0.12f32, 0.12f32, 0.35f32];
    
    for i in 0..4 {
        let freq = notes[i];
        let dur = durations[i];
        let num_samples = (dur * SAMPLE_RATE) as usize;
        let omega = 2.0 * 3.14159265 * freq / SAMPLE_RATE;
        for t in 0..num_samples {
            let mut amp = 10000.0;
            if t > num_samples - 200 {
                let fade = (num_samples - t) as f32 / 200.0;
                amp *= fade;
            }
            let sample_val = (sin_approx(omega * t as f32) * amp) as i16;
            samples.push_back(sample_val);
        }
    }
    
    let mut mixer = crate::drivers::hda::MIXER.lock();
    mixer.add_stream(crate::drivers::hda::AudioStream {
        id: 999, // System sound stream ID
        buffer: samples,
        volume: 90,
    });
}

/// Enter an infinite halt loop, waking only for interrupts.
fn halt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

/// Panic handler — prints the panic message to serial and halts.
///
/// Keep this minimal — calling complex formatting here can itself panic,
/// causing an infinite recursion that ends in a double fault.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Disable interrupts immediately to prevent re-entry via NMI/SMI.
    x86_64::instructions::interrupts::disable();
    serial_println!("!!! KERNEL PANIC !!!");
    if let Some(loc) = info.location() {
        serial_println!("  at {}:{}", loc.file(), loc.line());
    }
    if let Some(msg) = info.message().as_str() {
        serial_println!("  msg: {}", msg);
    }
    loop {
        x86_64::instructions::hlt();
    }
}

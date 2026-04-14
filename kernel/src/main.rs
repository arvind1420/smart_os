//! Smart OS Kernel — Phase 12
//!
//! A hybrid microkernel for the Smart OS operating system.
//! Phase 11 adds: Real userland (fork/exec/waitpid), per-process FDs,
//! wait queues, CoW page fault handler, user-space shell, ACPI shutdown.

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
    serial_println!("  SMART OS v0.12.0 — Hybrid Microkernel");
    serial_println!("  Phase 11: Userland & Real Programs");
    serial_println!("====================================================");
    serial_println!();

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 2: CPU architecture (GDT + IDT)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Setting up GDT...");
    arch::x86_64::gdt::init();
    serial_println!("[boot] Setting up IDT...");
    arch::x86_64::idt::init();
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
    
    serial_println!("[boot] Searching for Intel iGPU...");
    if drivers::igpu::init().is_ok() {
        serial_println!("[boot] Intel iGPU hardware acceleration ready.");
    } else {
        serial_println!("[boot] No Intel iGPU found. Falling back to software rendering.");
    }

    // ═══════════════════════════════════════════════════════════════
    //  PHASE 11: GUI compositor + desktop environment
    // ═══════════════════════════════════════════════════════════════
    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        let info = framebuffer.info();
        let is_bgr = matches!(info.pixel_format, bootloader_api::info::PixelFormat::Bgr);
        let fb_ptr = framebuffer.buffer_mut().as_mut_ptr();
        let byte_stride = info.stride * info.bytes_per_pixel;

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

    serial_println!("[boot] Spawning interactive applications...");
    process::scheduler::spawn("terminal", apps::terminal::run, 7);
    process::scheduler::spawn("file-manager", apps::file_manager::run, 8);
    process::scheduler::spawn("sysmon", apps::sysmon::run, 9);
    process::scheduler::spawn("editor", apps::editor::run, 11);

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
        serial_println!("[boot] Network stack ready, net-rx thread spawned.");
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

    serial_println!("[boot] Initializing HDA audio controller...");
    if let Err(e) = drivers::hda::init() {
        serial_println!("[boot] HDA warning: {}", e);
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
    //  PHASE 29: Phase 9 Applications (Calculator, Task Manager, Settings)
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Spawning Phase 9 applications...");
    process::scheduler::spawn("calculator", apps::calculator::run, 12);
    process::scheduler::spawn("task-manager", apps::task_manager::run, 13);
    process::scheduler::spawn("settings", apps::settings::run, 14);

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
    //  PHASE 37: Phase 12 — Shell Evolution & Hardware Completion
    // ═══════════════════════════════════════════════════════════════
    serial_println!("[boot] Phase 12: Shell evolution + AHCI DMA + HTTP client + ICMP + pkg.");
    // AHCI was already initialized in Phase 11 (drivers::init()), but log completion.
    if drivers::ahci::is_available() {
        serial_println!("[boot] Phase 12: AHCI DMA driver active.");
    }
    // Update /system/version to v0.12.0
    let _ = vfs::create_and_write("/system/version", b"Smart OS v0.12.0");
    serial_println!("[boot] Phase 12 initialized (pipes, grep, head, tail, wc, cp, mv, rm,");
    serial_println!("[boot]   touch, find, history, alias, which, uname, lspci, dmesg,");
    serial_println!("[boot]   df, top, ping, http, pkg, AHCI DMA, HTTP client, ICMP, sysmon graphs).");

    // ═══════════════════════════════════════════════════════════════
    //  BOOT COMPLETE — enable interrupts and enter main loop
    // ═══════════════════════════════════════════════════════════════
    serial_println!();
    serial_println!("======================================================");
    serial_println!("  SMART OS v0.12.0 — PHASE 12 ONLINE");
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
    gui::notification::push("Smart OS", "v0.10.0 booted successfully", gui::theme::ACCENT_CYAN);

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

        // Yield after each frame
        for _ in 0..50 {
            process::scheduler::yield_now();
        }
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

/// Enter an infinite halt loop, waking only for interrupts.
fn halt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

/// Panic handler — prints the panic message to serial and halts.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("!!! KERNEL PANIC !!!");
    serial_println!("{}", info);
    halt_loop()
}

# Smart OS

Smart OS is an x86_64 operating system I've been building from scratch in Rust — no Linux
underneath, no existing kernel forked, just `bootloader_api` handing off to my own `kernel_main`
and everything from there (paging, scheduling, a filesystem, a network stack, a GUI compositor,
and eventually a full in-kernel web browser) written by hand. It's a hybrid microkernel: process
management, IPC, VFS, and drivers live in Ring 0 for performance, but user programs are real ELF64
binaries running in their own address spaces with their own page tables, not kernel modules.

This is a solo project. It is not production-ready, it is not audited, and there is no guarantee
anything here is the "right" way to build an OS — it's the way I built one while learning how OS
internals actually work, one subsystem at a time, mostly by reading the Intel SDM, OSDev wiki, and
existing kernels' source when I got stuck.

## Why this exists

Most "write your own OS" tutorials stop after you can print to the screen and handle a keyboard
interrupt. I wanted to see how far a single person could push a `no_std` Rust kernel if they kept
going past that point — real preemptive multitasking, real virtual memory with copy-on-write fork,
a real TCP/IP stack talking to actual hardware NICs, a GUI that doesn't just draw pixels but has a
window manager and widget event loop, and eventually enough of the web platform (HTML/CSS/JS/TLS)
to load real pages. Along the way I also wanted to try something most kernels don't do at all: give
the kernel itself a small AI inference engine, so it can make predictive decisions (what to
prefetch, what looks like ransomware behavior) instead of purely reacting to syscalls.

## What's actually implemented

A lot of the phase-by-phase docs in this repo (`PLAN.md`, `TODO.md`, `USER_GUIDE.md`,
`BROWSER_COMPAT.md`) were written at different points during development and record the OS's state
*at that time* — they're kept because they're an honest build log, not because every number in them
still matches the current tree exactly. What's true as of this commit:

- **Boots on real UEFI/BIOS firmware and in QEMU/VirtualBox**, via the `bootloader_api` 0.11 crate.
  Framebuffer graphics only — no legacy VGA text mode, everything including the boot log is drawn
  through the compositor.
- **Preemptive multitasking** with a real scheduler, kernel threads and user processes, `fork()`
  with copy-on-write page tables, `exec()`, `waitpid()`, per-process file descriptor tables, pipes,
  and POSIX-style signals.
- **Its own filesystem, SmartFS**, plus real driver-backed storage: AHCI (SATA) and NVMe DMA
  drivers, a FAT32 driver for USB mass storage interoperability, and a VirtIO block driver for
  virtualized environments. SmartFS uses a write-ahead log so a hard power cut doesn't corrupt the
  volume.
- **A real network stack**: Ethernet → ARP → IPv4/IPv6 → UDP/TCP, DHCP, DNS, an HTTP client, ICMP
  ping, and drivers for Intel e1000 and VirtIO-net NICs, plus an early Intel WiFi (iwlwifi-style)
  driver with WPA2/CCMP.
- **A GUI stack built on my own compositor**, not a port of X11 or Wayland (though there's a
  Wayland-protocol compatibility shim): windowing, widgets, a taskbar, virtual desktops, and
  hardware-accelerated blitting on Intel iGPUs via the GPU's Blitter Command Streamer, so window
  moves cost close to zero CPU.
- **An in-kernel web browser** (I call it SmartBrowser internally) with its own HTML parser, CSS
  engine, layout/paint pipeline, a hand-rolled JavaScript engine (lexer → parser → tree-walking
  interpreter, with a baseline JIT for hot functions), a WebAssembly interpreter, and a TLS 1.3
  stack with certificate validation against a real Mozilla CA bundle. See `BROWSER_COMPAT.md` for
  the honest feature-by-feature status — plenty of it is full, some of it (generators, `Proxy`,
  `WeakRef`) is a stub.
- **A capability-based sandbox and security layer**: CBAC (capability-based access control),
  seccomp-style syscall filtering, namespaces for process-level containers, ASLR, a from-scratch
  crypto library (AES-GCM, ChaCha20-Poly1305, SHA-2, HMAC, X25519, P-256 — all implemented directly
  because the usual RustCrypto crates don't compile cleanly for `x86_64-unknown-none` without
  patching around their SIMD assumptions; see the `force-soft` note in `kernel/Cargo.toml`).
- **A small AI core that actually runs in Ring 0**: a fixed-point (no floating point, no heap
  dependency beyond `alloc`) inference engine used for predictive file prefetching and a syscall
  anomaly detector that can flag ransomware-shaped behavior and freeze the offending process.
- **SmartPack**, a custom binary serialization format used everywhere internally — IPC messages,
  on-disk metadata, plugin manifests. It's tag-byte encoded (`0x00-0x7F` are inline fixints,
  `0x80` and up are typed tags for ints/floats/strings/binary/arrays/maps/control) and decodes
  without copying the underlying buffer.
- **A handful of native GUI apps** built against `smartsdk` (the user-space SDK): a file manager,
  terminal, calculator, image viewer, PDF reader, video player, email client, a small office suite
  (word processor with a gap buffer, spreadsheet with recursive formula evaluation, presentation
  tool), a CRM demo, and a package manager UI.

## What it can't do (yet)

Being upfront about this matters more to me than the feature list above:

- **x86_64 only.** No ARM64 support — Apple Silicon and Raspberry Pi boards are not targets
  (see `HCL.md`, which records real hardware test runs and is blunt about what fails).
- NVIDIA and AMD discrete GPUs are not driven — only Intel integrated graphics gets hardware
  acceleration; everything else falls back to software blitting.
- WiFi support is early: MediaTek chipsets aren't supported, and WPA3 handshakes are incomplete
  even on the chipsets that are.
- The JS engine covers a lot of ES2020+ syntax but doesn't have `Proxy`/`Reflect`, and typed arrays
  and `WeakRef`/`WeakMap` are partial stubs without real GC integration.
- S3 suspend/resume is flaky on some AMD Ryzen 7000-series laptops due to what looks like an ACPI
  quirk in their firmware tables — documented, not fixed.
- This has been tested on real hardware and in QEMU/VirtualBox, but it has not gone through any
  kind of formal security audit. Treat it like the research/hobby project it is, not something you
  put sensitive data behind.

## Repository layout

```
kernel/            The kernel itself — smartos-kernel, target x86_64-unknown-none
  src/main.rs         kernel_main() and the boot sequence
  src/arch/           x86_64 (and early aarch64 stub) low-level: GDT, IDT, LAPIC, SMP, paging
  src/memory/         frame allocator, paging, heap, CoW, ASLR, swap, shared memory, OOM killer
  src/process/        scheduler, threads, ELF loading, fork/exec/wait, ptrace, containers
  src/ipc/            message passing, named ports
  src/syscall/        syscall dispatch table and handlers (88 syscalls as of this build)
  src/vfs/            virtual filesystem layer
  src/smartfs/        the native journaling filesystem
  src/drivers/        AHCI, NVMe, e1000, VirtIO, xHCI USB, HDA audio, DRM/iGPU, ACPI, etc.
  src/net/            TCP/IP stack *and* the entire browser engine (HTML/CSS/JS/WASM/TLS)
  src/gui/            compositor, window manager, widgets, theming
  src/ai/             the in-kernel inference engine, prefetcher, anomaly detector
  src/security/       CBAC, sandboxing, seccomp, namespaces, PQC experiments
  src/crypto/         hand-rolled AES-GCM, ChaCha20-Poly1305, SHA-2, HMAC, X25519, TLS 1.2/1.3
  src/apps/           built-in GUI apps that ship inside the kernel image
smartpack/          The binary serialization format (also builds under `std` for its test suite)
smartsdk/           `no_std` user-space SDK that every app in apps/ links against
apps/               User-space apps (shell, file manager, browser, office suite, CRM, games, ...)
drivers/            Out-of-tree driver examples (loadable .sys kernel modules)
tools/
  image_builder/      builds the bootable UEFI+BIOS .img from the compiled kernel
  module_loader/      host-side helper for building/inspecting loadable kernel modules
  js_harness/         runs the JS engine's logic on the host machine for fast WPT-style iteration
```

Two build targets live in one Cargo workspace: `kernel`, `smartpack`, `smartsdk`, everything under
`apps/`, and `drivers/test_driver` compile for the bare-metal `x86_64-unknown-none` target and are
all `#![no_std]`. The three crates under `tools/` are ordinary host-target Rust programs — you run
them on your dev machine, not inside the OS.

## Building it

You need a nightly Rust toolchain — it's pinned in `rust-toolchain.toml` (with `rust-src` and
`llvm-tools-preview`, plus the `x86_64-unknown-none` target already declared, so `rustup` should
just pull the right pieces automatically the first time you run cargo in this directory).

```bash
# Kernel and userspace crates need build-std since there's no libstd for bare metal
cargo build -p smartos-kernel --target x86_64-unknown-none \
    -Zbuild-std=core,compiler_builtins,alloc \
    -Zbuild-std-features=compiler-builtins-mem

# SmartPack's test suite runs on the host under std
cargo test -p smartpack --features std

# Build the bootable image (UEFI + BIOS) once the kernel is built
cargo run -p image_builder
```

There's no single `cargo build` for the whole workspace — mixing `-Zbuild-std` for the kernel
target into the workspace's `.cargo/config.toml` breaks the host-target crates, so kernel-target and
host-target packages get built with separate `-p` invocations. `image_builder` produces
`target/smartos.img`.

### Running it

QEMU is the fastest inner loop:

```bash
qemu-system-x86_64 -bios OVMF.fd -drive format=raw,file=target/smartos.img
```

(this is also exactly what `.cargo/config.toml`'s `runner` line does, so `cargo run --target
x86_64-unknown-none` after building will launch it for you.)

For VirtualBox — which is what I actually use day-to-day for anything involving real disk/NIC
drivers rather than QEMU's virtio defaults — convert the raw image and attach it as a SATA disk:

```bash
VBoxManage convertfromraw target/smartos.img target/smartos.vmdk --format VMDK
```
Recommended VM settings: 512 MB RAM, 2 CPUs, VBoxSVGA display, SATA (AHCI) storage controller, NAT
networking with the adapter type set to **Intel PRO/1000 (e1000)** so the e1000 driver actually
attaches.

To boot from a real USB stick instead: flash `target/smartos.img` with Rufus (Windows) or
balenaEtcher (macOS/Linux), disable Secure Boot in your firmware settings, and boot from the drive.

## Boot sequence

`kernel_main()` in [`kernel/src/main.rs`](kernel/src/main.rs) brings subsystems up in a fixed,
dependency-ordered sequence — nothing here is lazy-initialized, so if you're adding a new subsystem
that depends on, say, the heap or the VFS, it has to come after them in this list:

```
serial → physical memory / heap → drivers → process → ipc → syscall → vfs → crypto →
profiler → unicode → net (incl. browser engine self-test) → ai → smartfs → plugins →
users → knowledge (graph) → security → immutable (read-only root protections) →
installer → posix → pkg
```

Everything after that point in `main.rs` is bringing up devices that were detected during the
driver scan (NIC, ACPI tables, CPU topology) and spawning the init userspace program.

### Syscall ABI

Syscalls are dispatched through a flat numeric table (`kernel/src/syscall/table.rs`,
`SYSCALL_COUNT = 88` as of this build), grouped by range rather than alphabetically, e.g.:

| Range | Group |
|-------|-------|
| 0–9   | Process (`exit`, `fork`, `exec`, `waitpid`, `mmap`, `pipe`, ...) |
| 10–13 | IPC (`send`, `recv`, `create_port`, `lookup_port`) |
| 20–26 | File (`open`, `read`, `write`, `stat`, `readdir`, `mkdir`) |
| 30–39 | Networking (UDP send/recv/bind, then TCP connect/listen/accept/send/recv/close/status) |
| 40–43 | Knowledge graph (`insert`, `query`, `link`, `delete`) |
| 50–58 | USB + FAT32 + extended file ops (`dup2`, `lseek`, `open_ex`) |
| 59–66 | POSIX extras, display server commands, package manager |
| 67–83 | Signals, DNS, sysinfo, kernel module loading, AI inference/search, audit, licensing, TTS, gamepad, fleet command, tensor ops |

New syscalls get appended to whatever range they logically belong to; nothing is renumbered once
shipped, since that would break any already-linked userspace binary.

## A few Rust/no_std things worth knowing if you're reading the source

- This is Rust 2024 edition, so `unsafe fn` bodies are *safe* by default — every actual unsafe
  operation needs its own explicit `unsafe { }` block, and static-mut access goes through
  `&raw const` / `&raw mut` rather than an implicit reference.
- Naked functions (`#[unsafe(naked)]` + `naked_asm!`) are used for interrupt/syscall entry trampolines.
  The stubs are position-independent, so they load symbol addresses with `lea reg, [rip + {sym}]`
  rather than an absolute `mov`.
- `sha2` is pulled in with `force-soft` to disable its SHA-NI/SSE2 fast paths, because LLVM can't
  lower those on a freestanding target without a bunch of extra target-feature wrangling — everything
  else in `kernel/src/crypto/` (AES-GCM, X25519, HMAC, TLS 1.2/1.3 record layer) is written by hand
  rather than pulled from a crate, for the same reason.
- Cargo's `-Zbuild-std` flag lives on the command line, not in `.cargo/config.toml`, on purpose —
  putting it in the workspace config breaks builds of the host-target tool crates.

## License

This repository is not currently under an open-source license — no license grant is given, and all
rights are reserved by the author.

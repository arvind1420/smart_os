# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Smart OS is a real, bootable x86_64 operating system written in Rust, using a hybrid microkernel
architecture. It boots via UEFI/BIOS (through `bootloader_api` v0.11), and includes a from-scratch
AI inference engine, a custom filesystem (SmartFS), a plugin system, POSIX-ish user-space, a
compositor/GUI stack, and a full in-kernel web browser (SmartBrowser) with its own JS engine, CSS
engine, layout/paint pipeline, TLS stack, and WASM runtime — all `no_std`.

Cargo workspace members fall into two build targets:
- **`x86_64-unknown-none`** (bare metal, no OS): `kernel` (`smartos-kernel`), `smartpack`,
  `smartsdk`, apps under `apps/*`, and `drivers/test_driver`. These are `#![no_std]` crates linked
  either into the kernel or run as freestanding user-space ELF binaries loaded by the kernel.
- **Host target** (normal Rust, runs on the dev machine): `tools/image_builder`,
  `tools/module_loader`, `tools/js_harness`.

## Build Commands

Always add cargo to PATH first when using bash: `export PATH="$HOME/.cargo/bin:$PATH"`.

```bash
# Run SmartPack (serialization format) unit tests — the only crate with a `std` test target
cargo test -p smartpack --features std

# Build the kernel for bare metal (requires nightly + build-std, see rust-toolchain.toml)
cargo build -p smartos-kernel --target x86_64-unknown-none -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem

# Build a release kernel the same way, adding --release

# Build the bootable disk image (UEFI + BIOS .img) after the kernel is built
cargo run -p image_builder

# Run the host-side JS engine test harness (WPT-style JS tests, runs on the dev machine, not in-kernel)
cargo run -p js_harness
```

There is no top-level `cargo test`/`cargo build` for the whole workspace — the kernel and apps
target `x86_64-unknown-none` and cannot build on the host target, and mixing `-Zbuild-std` into the
workspace-level `.cargo/config.toml` breaks host-target builds (tools, smartpack `std` tests). Build
kernel-target and host-target packages with separate `-p` invocations as shown above.

To test in QEMU: build the kernel, run `image_builder` to produce `target/smartos.img`, then boot it
with `qemu-system-x86_64 -bios OVMF.fd -drive format=raw,file=target/smartos.img` (this is also what
`.cargo/config.toml`'s `runner` does for `cargo run --target x86_64-unknown-none`).

Toolchain is pinned via `rust-toolchain.toml`: nightly with `rust-src`, `llvm-tools-preview`, and the
`x86_64-unknown-none` target already declared — no manual `rustup target add` needed.

## Architecture

### Kernel module layout (`kernel/src/`)

`kernel_main` in [main.rs](kernel/src/main.rs) runs a strict, linear boot sequence — later
subsystems assume earlier ones are already initialized. Key modules and their init order:

`serial` → memory/heap → `drivers` → `process` → `ipc` → `syscall` → `vfs` → `crypto` →
`profiler` → `unicode` → `ai` → `smartfs` → `plugins` → `users` → `knowledge` → `security` →
`immutable` → `installer` → `posix` → `pkg` → (net/GUI/session brought up inline nearby).

Other top-level modules: `arch` (CPU/boot arch glue), `io`, `wayland` (compositor protocol),
`gui` (widgets, desktop, window manager), `net` (drivers, TCP/UDP/IPv6/WiFi/TLS, and the full
browser network/DOM/JS/CSS/WASM stack), `apps` (in-kernel built-in apps), `i18n`.

Boot phases are numbered sequentially in code comments and `serial_println!` boot banners (see
`kernel/src/main.rs`) — treat the phase number in a comment/log line as a rough changelog pointer,
not a module name. Very high-level phase groupings (see project memory for full detail): 1–15 core
kernel primitives, 16–29 drivers/net/SMP/security, 30–40 CoW/ASLR/GDB/layout+paint+JS foundations,
41–57 Web APIs + OS hardening + apps, 58–107 desktop apps + full browser (TLS/DOM/CSP/codecs),
108–130 JS async/WASM/JIT/WiFi-WPA2/TLS-v2/WebCrypto/HTTP2/PWA, up to 140 (SmartBrowser public
release, current `main.rs` doc comment says "Phase 40" but the boot banner list runs through 140 —
the module doc comment lags behind; trust the banner list and git history over the header comment).

### SmartPack (`smartpack/`)

The universal binary serialization format used everywhere in the OS: kernel↔user IPC, on-disk
metadata, plugin manifests, network payloads. `no_std` by default (`alloc` feature on), with a
`std` feature only for its test suite. Tag-byte space is load-bearing and must not be changed
carelessly — see project memory for the exact tag ranges (0x00–0x7F fixints, 0x80+ tagged types
split into ints/floats/strings/binary/arrays/maps/special, with zero overlap required).

### SmartSDK (`smartsdk/`)

`#![no_std]` user-space development kit. Every app under `apps/*` depends on it instead of talking
to the kernel directly — syscalls, widgets, and app scaffolding are wrapped here. New user-space
apps should follow the existing `apps/*` package shape: a `[[bin]]` target named after the app,
depending only on `smartsdk` (path dependency).

### Tools (`tools/`, host target)

- `image_builder`: assembles the bootable UEFI+BIOS `.img` from the compiled kernel (GPT/MBR via
  `gpt`/`mbrman`, FAT via `fatfs`, boot files via the `bootloader` crate).
- `module_loader`: host-side helper for building/inspecting dynamically-loadable kernel modules
  (`.sys` files, Ring-0 ELF relocation).
- `js_harness`: host-side test runner for the in-kernel JS engine (Tier 1 WPT-style JS iteration) —
  lets the JS engine's logic be exercised on the dev machine instead of only inside QEMU.

### Rust nightly / no_std gotchas specific to this repo

- Rust 2024 edition: unsafe fn bodies are safe by default — explicit `unsafe { }` blocks are
  required around actual unsafe operations, and static mut access needs `&raw const`/`&raw mut`.
- `#[unsafe(no_mangle)]` and `#[unsafe(naked)]` + `naked_asm!` are the required forms (naked
  functions have been stable since 1.88).
- PIE-compatible naked asm must use `lea reg, [rip + {sym}]`, never `mov reg, [{sym}]`.
- Function-pointer-to-integer casts must go through `fn as *const () as u64`, not `fn as u64`.
- `InterruptDescriptorTable` is indexed by `u8`, not `usize`; `Cr2::read()` returns a `Result` that
  must be unwrapped/expected.
- RustCrypto crates don't work as-is on `x86_64-unknown-none`: `sha2` needs `force-soft` (disables
  SHA-NI/SSE2 so LLVM can lower it without SIMD target features); AES-128-GCM and X25519 are
  hand-implemented inline rather than pulling in `aes-gcm`/`x25519-dalek` (see
  `memory/feedback_crypto_baremetal.md` for the full constraint list before touching crypto code).

## Git / Commit Conventions

- User is sole author/copyright holder of this repository (unlicensed/proprietary, not MIT).
- Do not add `Co-Authored-By` lines beyond what the system attribution reminder specifies.

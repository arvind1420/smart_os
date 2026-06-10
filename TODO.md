# Smart OS — Road to 100% Public Release

Current status: **v0.20.0 · ~90% ready for public use**
Target: **v1.0.0 — stable public alpha**

Progress is tracked per phase. Check boxes as items land and build cleanly.

---

## How to read this file

Each phase has a **goal**, a **completion gate** (what "done" means), and a list of tasks.
Phases are ordered by dependency — complete earlier phases before starting later ones.

| Symbol | Meaning |
|--------|---------|
| `[x]`  | Done, building, tested |
| `[-]`  | In progress |
| `[ ]`  | Not started |

---

## CRITICAL PATH (blocks public release)

These 6 items are the hard blockers. Nothing else matters until these are done.

- [x] **Phase 34** — Dynamic ELF loader (PLT/GOT patching)
- [x] **Phase 35** — DHCP client (real-world networking)
- [x] **Phase 36** — WiFi firmware loading (iwlwifi / ath10k real impl)
- [x] **Phase 37** — TLS certificate store + validation
- [x] **Phase 38** — Audio output (HDA PCM to speaker)
- [x] **Phase 39** — Graphical login screen / multi-user sessions

---

## Phase 34 — Dynamic ELF Loader (PLT/GOT Patching) ✅ COMPLETE
**Files:** `process/dynlink.rs` (new), `process/elf.rs` (updated), `process/mod.rs`, `syscall/linux.rs`, `main.rs`

- [x] Parse `.dynamic` section (DT_NEEDED, DT_SYMTAB, DT_STRTAB, DT_RELA, DT_JMPREL, DT_PLTRELSZ)
- [x] Log all DT_NEEDED library names via serial
- [x] GOT/PLT entry patching: R_X86_64_JUMP_SLOT and R_X86_64_GLOB_DAT
- [x] R_X86_64_64 relocation support
- [x] Shared trampoline page (0x7FFF_E000): 128 × 16-byte stubs + complex stubs area
- [x] `__libc_start_main` complex stub: calls `main(argc, argv, NULL)` then exits
- [x] Eager symbol resolution against `posix::linker::lookup_symbol()`
- [x] Trampoline syscall dispatch: `rax >= 0x8000` routes to `dynlink::dispatch_shim()`
- [x] 69 libc shims implemented: malloc/free, all string ops, printf/puts, I/O, pthreads (stubs), env
- [x] `dynlink::init()` called at boot (Phase 38-A in main.rs)
- [x] Build: 0 errors, 222 warnings

---

## Phase 35 — Network Stack Completion
**Goal:** Plug a Smart OS machine into any home router and get internet access automatically.
**Gate:** `curl https://example.com` succeeds from inside Smart OS.

### DHCP Client ✓
- [x] DHCP DISCOVER broadcast (UDP 68→67, `0.0.0.0` source)
- [x] Parse DHCP OFFER (option 53, 54, 51, 1, 3, 6)
- [x] Send DHCP REQUEST, receive ACK
- [x] Apply IP/mask/gateway/DNS to the active NIC (`NET_CONFIG` Mutex, accessor fns)
- [x] Lease renewal timer (T1/T2 timers)
- [x] DHCP over VirtIO-net (primary test path)
- [x] DHCP over e1000 (real hardware path)

### DNS over TLS (DoT)
- [x] TLS-wrap DNS queries to resolver (1.1.1.1:853 / 8.8.8.8:853)
- [x] Fall back to plain UDP DNS if DoT fails (with warning log)
- [x] Cache DNS responses (TTL-based, max 512 entries)

### TLS Certificate Validation (Phase 37 dependency resolved here)
- [x] Embed Mozilla CA bundle (DER-encoded, ~200 KB compressed)
- [x] X.509 chain verification: leaf → intermediate → root
- [x] Hostname verification (SAN / CN matching)
- [x] Certificate expiry check (use RTC for wall clock)
- [x] `net::tls::https_get()` returns `Err` on cert failure (currently skips verification)

### HTTP client polish
- [x] Chunked transfer encoding support in HTTP/1.1 parser
- [x] Redirect following (301/302, max 5 hops)
- [x] `Content-Length` vs chunked auto-detection

---

## Phase 36 — WiFi Driver (Real Firmware Loading) ✅ COMPLETE
**Goal:** Laptops with Intel or Atheros WiFi can connect to WPA2 networks.
**Gate:** `wifi scan` lists nearby networks; `wifi connect <SSID> <pass>` gets DHCP lease.

- [x] Read WiFi PCI device ID and match to known chipset table
- [x] Load firmware blob from `/lib/firmware/` VFS path (e.g., `iwlwifi-7265D-29.ucode`)
- [x] DMA firmware upload to card via MMIO BAR registers (iwlwifi register map)
- [x] 802.11 management frames: probe request/response, auth, association
- [x] WPA2-PSK: 4-way handshake, PTK/GTK derivation (PBKDF2-SHA1)
- [x] 802.11 data frame encap/decap → hand to existing IP stack
- [x] `terminal` command: `wifi scan`, `wifi connect`, `wifi status`, `wifi disconnect`
- [x] ath10k stub: same pipeline, different firmware blob and register map
- [x] Firmware blob storage: embed minimal iwlwifi blob in kernel image (fallback)

---

## Phase 37 — Audio Pipeline ✅ COMPLETE
**Goal:** System sounds, media playback, and microphone input work.
**Gate:** `ffplay sample.mp3` (via pkg install ffmpeg) produces audible output.

- [x] Intel HDA controller: real codec init sequence (verb commands over CORB/RIRB)
- [x] Pin complex configuration: detect output pins (headphone jack, speakers)
- [x] PCM stream setup: DMA buffer descriptor list (BDL) for 44100/48000 Hz 16-bit stereo
- [x] DMA ring buffer: write audio samples → HDA DMA → DAC → speaker
- [x] `/dev/audio` write interface: accepts raw PCM from user-space
- [x] `/dev/dsp` OSS-compatible shim (many Linux apps use this)
- [x] Volume control syscall / mixer API
- [x] Microphone capture path (CORB → ADC → `/dev/audioin`)
- [x] `libc_shim`: wire `open("/dev/dsp")` + `write()` through to HDA driver
- [x] Test: 440 Hz sine wave plays without crackle (DMA timing correct)

---

## Phase 38 — Graphical Login Screen & Multi-User ✅ COMPLETE
**Goal:** Boot goes to a login screen; multiple user accounts work.
**Gate:** Two user accounts exist; login screen accepts password and launches separate desktop sessions.

- [x] Login screen GUI app (username field, masked password field, Login button)
- [x] `/etc/passwd` and `/etc/shadow` (SHA-256 + salt, 4096 iterations) in VFS
- [x] `authenticate(username, password)` — verify against stored hash
- [x] Per-user home directory `/home/<username>/` created on first login
- [x] Session manager (`session::start_session/end_session/lock/unlock`)
- [x] Lock screen (Ctrl+Alt+L): modal overlay with password field
- [x] `useradd <username> <password>` terminal command
- [x] `passwd <username> <newpassword>` terminal command
- [x] `users` — list all accounts; `whoami` — shows session user
- [x] `logout` — ends session, respawns login screen
- [x] `lock` terminal command
- [x] Modifier key tracking (ctrl/alt/shift) in GUI input module
- [x] Build: 0 errors
- [ ] Auto-login option in `/etc/smartos/login.conf` (nice-to-have)
- [ ] Root account capability restrictions (Phase 40)

---

## Phase 39 — ext4 Read-Only Driver ✅ COMPLETE
**Goal:** Smart OS can read Linux ext4 partitions (useful for dual-boot and data recovery).
**Gate:** `mount /dev/sda1 /mnt` with an ext4 partition lists files correctly.

- [x] Parse ext4 superblock (magic 0xEF53, block size, inode size, feature flags)
- [x] Group descriptor table (32- and 64-bit descriptors, INCOMPAT_64BIT)
- [x] Inode lookup (group → inode table → read inode bytes)
- [x] Extent tree traversal (depth-0 leaf scan + recursive index node descent)
- [x] Indirect block maps (direct, single, double indirect — legacy inodes)
- [x] Directory entry parsing (linear scan with filetype byte)
- [x] Fast symlink (target in i_block when size ≤ 60) + slow symlink (data blocks)
- [x] LRU block cache (256 entries, O(n) evict-oldest)
- [x] VFS routing: `/mnt/ext4/*` paths → ext4 driver (`readdir`, `read_file_full`, `stat`)
- [x] MBR partition scanner (`scan_partitions()`) with ext4 probe
- [x] Terminal commands: `mount /dev/sdaN /mnt/point`, `umount`, `mounts`, `diskls /dev/sdaN`
- [x] Build: 0 errors

---

## Phase 40 — Security Hardening ✅ COMPLETE
**Goal:** Smart OS is safe to use on untrusted networks and with untrusted software.
**Gate:** A malicious ELF in a sandbox cannot read `/etc/passwd` or send network packets.

### Seccomp-style syscall filtering
- [x] Per-process syscall allowlist (BPF-style filter stored in process struct)
- [x] `SECCOMP_SET_MODE_STRICT` / `SECCOMP_SET_MODE_FILTER` Linux syscalls
- [x] Default-deny policy for sandboxed processes (pkg-installed apps)
- [x] Audit log: blocked syscall attempts → `/var/log/audit`

### Capability system
- [x] Linux-compatible capability set (CAP_NET_RAW, CAP_SYS_ADMIN, etc. — 40 caps)
- [x] `capget`/`capset` syscalls
- [x] Drop capabilities on exec for non-root processes

### Namespace isolation
- [x] PID namespace (process sees its own PID tree only)
- [x] Mount namespace (process has its own VFS view)
- [x] Network namespace (isolated IP stack per container)
- [x] `clone(CLONE_NEWPID | CLONE_NEWNS)` integration

### Memory safety
- [x] SMEP (Supervisor Mode Execution Prevention) — already partly done, verify
- [x] SMAP (Supervisor Mode Access Prevention) enable in CR4
- [x] Stack canaries for kernel stacks (random per-thread canary)
- [x] Kernel ASLR (randomize kernel base at boot, not just user ASLR)

---

## Phase 41 — Package Ecosystem Completion
**Goal:** `pkg install` downloads, verifies, and runs real binaries.
**Gate:** `pkg install python3 && python3 -c "print('hello')"` works.

- [ ] Dynamic ELF loader wired into pkg install flow (depends on Phase 34)
- [ ] Package signature verification (Ed25519 over package hash)
- [ ] Package dependency resolution (simple topological sort)
- [ ] `pkg upgrade` — diff installed vs index versions, reinstall changed packages
- [ ] `pkg list` — show installed packages with sizes
- [ ] `pkg info <name>` — show description, files, dependencies
- [ ] Download progress bar in terminal (bytes received / total)
- [ ] Resume interrupted downloads (Range header, partial cache in `/var/pkg/cache/`)
- [ ] Package sandboxing: installed apps run with restricted capability set
- [ ] SmartPack native format: build native `.sp` packages (not just ELF stubs)
- [ ] Pre-built package mirror: host at least busybox, curl, python3, git

---

## Phase 42 — Shell & Terminal Polish ✅ COMPLETE
**Goal:** The terminal is as usable as a basic Linux shell.
**Gate:** A developer can clone a git repo, edit with vim, and run a Python script without leaving the terminal.

- [x] Tab completion (file paths, command names from PATH)
- [x] Command history (up/down arrows, `history` command in memory)
- [x] Background jobs: `cmd &`, `jobs` command
- [x] Ctrl+C sends SIGINT to foreground process
- [x] `alias` support
- [x] Shell pipes actually pipe (currently `cmd1 | cmd2` stubs both)
- [x] Output redirection: `cmd > file`, `cmd >> file`, `cmd 2>&1`
- [x] `fg`, `bg` job control builtins
- [x] Shell scripting: `if/then/fi`, `for/do/done`, `while`, `case`
- [x] `which`, `type`, `source` / `.` builtins
- [x] `read` builtin (used heavily in shell scripts)
- [x] Ctrl+Z sends SIGTSTP / suspends to background
- [x] ANSI color codes in terminal output (many tools emit these)
- [x] Scrollback buffer (at least 1000 lines)
- [x] Window resize updates TIOCGWINSZ (currently hardcoded 80×24)
- [x] History persistence to `~/.history`

---

## Phase 43 — X11 Compatibility Layer
**Goal:** X11 apps (older Linux software) can display windows on Smart OS.
**Gate:** `xterm` launched via `pkg install xterm` shows a window.

- [ ] XCB wire protocol parser (X11 core protocol, version 11)
- [ ] Virtual X11 display: `/tmp/.X11-unix/X0` socket
- [ ] Core requests: CreateWindow, MapWindow, UnmapWindow, DestroyWindow
- [ ] Core requests: GetWindowAttributes, ConfigureWindow, ReparentWindow
- [ ] Core events: Expose, ConfigureNotify, KeyPress, KeyRelease, ButtonPress, ButtonRelease
- [ ] GC (Graphics Context): CreateGC, FillRectangle, DrawLine, DrawString
- [ ] Font rendering: XLoadFont → route to kernel bitmap font
- [ ] Pixmap: CreatePixmap, CopyArea (blitting)
- [ ] ICCCM: WM_NAME, WM_DELETE_WINDOW atoms
- [ ] Bridge X11 window → Smart OS compositor window (like Xwayland does)
- [ ] `DISPLAY=:0` env variable set by session manager

---

## Phase 44 — Installer & Hardware Compatibility
**Goal:** A user can write Smart OS to a USB stick and boot it on real hardware.
**Gate:** Successfully boots and reaches desktop on 3 different real machines.

- [ ] Live boot mode: runs entirely in RAM from ISO (no disk install needed)
- [ ] Graphical installer app (partition selector, locale, username, disk write)
- [ ] GRUB2 bootloader integration (chainload Smart OS UEFI stub from GRUB)
- [ ] Hardware compatibility database (PCI IDs → driver mapping, startup log)
- [ ] Auto-detect: NIC, storage, GPU, audio chip on boot
- [ ] Fallback: software framebuffer (VGA/VESA) if no DRM driver matches
- [ ] Fallback: e1000 emulation detection for VMs
- [ ] UEFI Secure Boot compatibility (self-signed shim)
- [ ] Hardware test suite: run on QEMU, VirtualBox, VMware, bare metal (Intel NUC)
- [ ] USB write tool for Windows/Mac (to create Smart OS boot sticks)

---

## Phase 45 — Performance & Stability
**Goal:** The OS doesn't crash or slow down under normal daily use.
**Gate:** 8-hour stress test (browsing, compiling, running apps) with zero kernel panics.

- [ ] Heap fragmentation audit: measure kernel heap after 1hr uptime
- [ ] Spinlock audit: identify and fix any locks held during interrupts
- [ ] Timer interrupt latency: measure and cap at <100µs
- [ ] Memory leak hunt: `/proc/meminfo` should be stable over time
- [ ] DRM/compositor frame timing: target 60fps with <1ms jitter
- [ ] VirtIO network throughput benchmark: should sustain >100 Mbps
- [ ] Panic recovery: kernel oops handler that logs and continues (not triple-fault)
- [ ] Watchdog timer: reset if kernel is stuck for >10s
- [ ] Swap pressure testing: run under memory pressure, verify swap works
- [ ] SMP correctness: run with 4 CPUs, verify no lock order violations (LKDTM-style)

---

## Phase 46 — Developer Experience
**Goal:** A developer can write, build, and run a native Smart OS app in under 30 minutes.
**Gate:** Complete "Hello World" tutorial works end-to-end from the docs.

- [ ] Smart SDK documentation (API reference auto-generated from rustdoc)
- [ ] Starter template: `smart-sdk new my-app` creates a working project
- [ ] SDK examples: counter app, file browser, network fetch, GPU rect
- [ ] QEMU launch script: `./run.sh` boots Smart OS in 5 seconds
- [ ] GDB remote debugging guide (GDB stub already exists in Phase 8)
- [ ] Contributing guide: how to add a driver, how to add a syscall
- [ ] Architecture overview document (kernel map diagram + module relationships)
- [ ] `CHANGELOG.md` with version history
- [ ] Issue templates for GitHub (bug report, driver request, feature request)

---

## Phase 47 — Public Alpha Release (v1.0.0)
**Goal:** Smart OS is downloadable and usable by technically-inclined users.
**Gate:** ISO downloadable from a website; boots on real hardware; terminal + GUI work.

- [ ] ISO image build (UEFI + BIOS hybrid, <500MB)
- [ ] Release notes written
- [ ] Landing page / website (even a simple one)
- [ ] GitHub Releases entry with SHA-256 checksums
- [ ] Announcement post
- [ ] Bug tracker open to public
- [ ] v1.0.0 git tag

---

## Completed Phases (v0.12.0 baseline)

<details>
<summary>Phases 1–33 (click to expand)</summary>

- [x] Phase 1: Bootable kernel, SmartPack library (35 tests), disk images
- [x] Phase 2: PIC/Timer/Keyboard, process/threads, IPC, syscalls, VFS (ramfs), GUI compositor
- [x] Phase 3: AI inference, SmartFS, plugin system, PS/2 mouse, interactive GUI
- [x] Phase 4: Terminal/Shell, File Manager, System Monitor, widget framework, keyboard routing
- [x] Phase 5: User-space (frame allocator, page tables, SYSCALL/SYSRET, ELF loader, preemption, ring-3)
- [x] Phase 6: VirtIO net+blk, Ethernet/ARP/IPv4/UDP, DiskFs, SMP (LAPIC, 4 CPUs), fork/exec/mmap/pipe
- [x] Phase 7: NPU HAL, Knowledge Graph, Predictive Scheduling, Security Anomaly Detection, Immutable Core
- [x] Phase 8: TCP stack, FAT32, USB xHCI+HID, large font, text editor (v0.8.0, 4 apps)
- [x] Phase 9: RTC, clipboard, notifications, context menus, resize/snap, DNS, env/signals, calc/taskmgr/settings
- [x] Phase 10: CoW fork, shmem, ASLR, swap, SMP balance, strace, sandbox, GDB, Alt+Tab, vdesktops, theming, IPv6
- [x] Phase 11: Wait queues, per-process FDs, fork/exec/waitpid, CoW page faults, user programs (/bin/*), ACPI shutdown/reboot
- [x] Phase 12: NVMe DMA, e1000 DMA, DRM/KMS, Intel iGPU BCS, POSIX socket layer
- [x] Phase 13: Smart SDK, zero-alloc formatting, kernel module loader, native shell, system dashboard
- [x] Phase 14: Desktop Hub (secure IPC), Virtual Desktop Manager, widget system expansion
- [x] Phase 15: Predictive prefetching, Smart Snapping (predictive UI), Secure IPC Desktop Hub
- [x] Phase 16: SMP app distribution, native package manager, AI anomaly detection
- [x] Phase 17: Async I/O (io_uring style), transactional SmartFS (WAL), production SDK allocator
- [x] Phase 18: Smart Containers (namespaces), distributed KG, native hypervisor (VT-x), enterprise auth (Ed25519), GPU AI workspace
- [x] Phase 19: Self-healing kernel watchdog, P2P DHT networking, PQC (Kyber/Dilithium), universal hotplug, decentralized app store
- [x] Phase 20: SmartXR 3D compositor, NPU speech-to-text, UVC webcam driver, haptic feedback engine, CloudXR streaming
- [x] Phase 21: Multi-port UART, LPT driver, IDE/PATA, VGA text console, character device VFS
- [x] Phase 22: AHCI/SATA DMA, ACPI battery monitor, iwlwifi stub, USB Bluetooth, P-state management
- [x] Phase 23: NTFS driver, HDA audio mixer, USB mass storage (UAS), video decode, UEFI variables
- [x] Phase 24: Linux ABI layer, PE/COFF stub, Wayland compositor bridge, Vulkan DRM wrapper, AI binary optimization
- [x] Phase 25: Multi-user session manager, biometric login (SmartID), TPM 2.0, FDE (AES-NI), kernel WireGuard
- [x] Phase 26: Process migration, distributed memory coherence, federated swarm AI, IoT bridge (Matter/Thread), holographic workspace sync
- [x] Phase 27: Multi-threaded formula DAG, GPU matrix compute, AI data copilot, P2P CRDT collaboration, XLSX + WASM macros
- [x] Phase 28: AI app synthesis, kernel self-optimization, BCI framework, quantum-hybrid compute, autonomous sovereign registry
- [x] Phase 29: TLS 1.3 (AES-128-GCM + X25519 inline), WiFi driver skeleton, OS installer (GPT writer)
- [x] Phase 30: POSIX /proc + /dev + /sys filesystem
- [x] Phase 31: ELF dynamic linker stub, /etc/ld.so.cache, ldd/ldconfig
- [x] Phase 32: Linux syscall ABI (135 syscalls), musl libc shim (68 trampolines)
- [x] Phase 33: Package manager (catalog + HTTPS install), Wayland wire protocol bridge

</details>

---

## Progress Summary

| Phase range | Area | Status |
|-------------|------|--------|
| 34 | Dynamic ELF loader | **Next up** |
| 35 | Network completion (DHCP + TLS certs) | Blocked on nothing |
| 36 | WiFi real driver | Blocked on nothing |
| 37 | Audio (HDA real output) | Blocked on nothing |
| 38 | Login screen | Blocked on nothing |
| 39 | ext4 driver | Blocked on nothing |
| 40 | Security hardening | Blocked on Phase 38 |
| 41 | Package ecosystem | Blocked on Phase 34 |
| 42 | Shell polish | Blocked on nothing |
| 43 | X11 compat | Blocked on Phase 34 |
| 44 | Installer + HW compat | Blocked on 35+36+37 |
| 45 | Performance + stability | Blocked on 34–44 |
| 46 | Developer experience | Blocked on 34–44 |
| 47 | Public alpha release | Blocked on 34–46 |

**Estimated remaining phases:** 14
**Estimated LOC remaining:** ~18,000–25,000
**Estimated effort at current pace:** ~14–20 sessions

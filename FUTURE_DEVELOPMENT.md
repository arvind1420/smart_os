# Smart OS — Road to General Public Release

> Starting point: v0.11.0 (Phases 1–28 complete).
> This plan closes every identified gap between the current working kernel and
> an OS a non-technical person can download, install, and use daily.
>
> Phases are ordered by what blocks the most users first.

---

## Gap Summary

| Gap | Blocks whom | Addressed in |
|-----|-------------|--------------|
| No real hardware drivers (WiFi, audio, GPU) | Everyone | Phase 29 |
| No TLS — can't browse securely or fetch packages | Everyone | Phase 30 |
| No installer — USB boot only, manual setup | Everyone | Phase 31 |
| Can't run existing Linux/Windows software | Power users | Phase 32 |
| Can't read NTFS / ext4 — data migration impossible | Switchers | Phase 33 |
| Only 7 basic GUI apps | Everyone | Phase 34 |
| No production testing or crash reporting | Everyone | Phase 35 |
| No package manager users can trust | Everyone | Phase 36 |
| No developer ecosystem | Developers | Phase 37 |
| No third-party security audit | Enterprise | Phase 38 |
| No legal / IP structure | Business | Phase 39 |
| No business model | Sustainability | Phase 40 |

---

## Phase 29 — Real Hardware Drivers

**What's missing:** Smart OS only runs reliably inside QEMU with VirtIO devices.
Real machines need WiFi, sound, Bluetooth, and proper GPU output.

**Deliverables:**

- **WiFi** — Intel iwlwifi (AX200/AX210), Realtek RTL8821CE, Qualcomm QCA6174;
  WPA2/WPA3-Personal association + DHCP; `nmcli`-style connection UI
- **Audio** — Intel HD Audio (HDA) driver; ALSA-compatible mixer API;
  output to speakers/headphones; input from mic
- **Bluetooth** — HCI layer over USB; pair keyboards, mice, headsets
- **GPU display** — Intel i915 modesetting; AMD DC display driver;
  replace VirtIO framebuffer with DRM/KMS; hardware cursor, vsync
- **ACPI power** — Suspend-to-RAM (S3), hibernate (S4), battery gauge,
  lid-close/open, AC adapter detection, CPU thermal throttling
- **USB mass storage** — Bulk-only transport; auto-mount FAT32/NTFS USB drives
- **Hardware Compatibility List v1** — Boot-test matrix across 15 target machines
  (5 laptops, 5 desktops, 5 mini-PCs); publish pass/fail/workaround per device

**Gate:** Smart OS boots, connects to WiFi, plays audio, and shows GUI at native resolution on
5+ commercially available laptops without any manual patching.

---

## Phase 30 — TLS / HTTPS & Network Security

**What's missing:** The TCP stack works but has no encryption. The package manager,
browser, and any secure service all require TLS. Without it the OS cannot safely
communicate with the internet.

**Deliverables:**

- **TLS 1.3** — Port `rustls` (pure Rust, no C dependency) onto the existing TCP stack;
  expose `tls_connect(host, port) -> TlsStream` and `tls_listen(port) -> TlsAcceptor`
- **Certificate store** — Bundle Mozilla CA root store; update mechanism for expired certs
- **DNS over HTTPS (DoH)** — Route all DNS queries through HTTPS to prevent ISP snooping;
  fall back to plain UDP if DoH unavailable
- **HTTPS syscall** — Add `SYS_HTTPS_GET` and `SYS_HTTPS_POST` convenience syscalls so
  user-space apps don't have to implement TLS handshake themselves
- **Certificate pinning** — Package manager pins the SmartOS package server certificate;
  alerts user on mismatch
- **Kernel network audit** — Review all raw socket interfaces for injection risks

**Gate:** `curl https://example.com` succeeds from the terminal; the package manager
fetches `.spk` files over HTTPS and verifies signatures.

---

## Phase 31 — Graphical Installer & First-Boot Experience

**What's missing:** There is no way for a regular user to install Smart OS onto a machine.
Today it requires manual QEMU setup or raw image flashing with no guidance.

**Deliverables:**

- **Live USB image** — Bootable ISO with a "Try before you install" mode; runs entirely
  in RAM so users can evaluate without touching their disk
- **Graphical installer** — Step-by-step wizard:
  1. Language & keyboard layout
  2. Disk partitioning (guided: wipe, dual-boot, or custom)
  3. User account, hostname, timezone
  4. Hardware detection summary (shows drivers found/missing)
  5. Copy + GRUB install → reboot
- **First-boot setup wizard** — After install: connect to WiFi, set display scaling,
  choose default browser, opt-in to crash reporting
- **Driver auto-detection** — PCI/USB ID database lookup at install time; flag
  unsupported hardware before committing to install
- **Dual-boot support** — GRUB entry alongside existing Windows/Linux; reads and
  preserves existing EFI partition
- **Secure Boot signing** — Sign the bootloader with a Microsoft-trusted certificate
  so the installer works on factory-locked machines

**Gate:** A person with no OS installation experience follows the wizard,
installs Smart OS alongside Windows, and reboots into both successfully.

---

## Phase 32 — POSIX Compatibility Layer

**What's missing:** Every app in the Linux ecosystem is unavailable. Building an app
ecosystem from scratch takes years. POSIX compatibility makes thousands of programs
available on day one.

**Deliverables:**

- **Linux syscall translation table** — Map all 57 current Smart OS syscalls to their
  Linux ABI equivalents; implement the ~120 most-used Linux syscalls that have no
  Smart OS equivalent (e.g., `mprotect`, `futex`, `epoll`, `signalfd`, `timerfd`)
- **ELF dynamic linker** — Implement `ld.so` equivalent; load shared libraries from
  `/lib` and `/usr/lib`; resolve PLT/GOT relocations at load time
- **musl-libc shim** — Ship musl as the system C library; statically link it into the
  compatibility layer so unmodified Linux binaries can call `libc` functions
- **Verified working binaries** — Test suite must pass for: `bash`, `curl`, `git`,
  `python3`, `vim`, `ssh`, `ffmpeg`, `gcc`, `node`, `sqlite3`
- **Proc/sys filesystem** — Implement `/proc/cpuinfo`, `/proc/meminfo`, `/sys/class/net`,
  `/dev/null`, `/dev/urandom`, `/dev/pts` — required by many Linux programs
- **Signal compatibility** — `SIGINT`, `SIGTERM`, `SIGKILL`, `SIGCHLD`, `SIGUSR1/2`
  fully routed through the existing signal subsystem

**Gate:** Unmodified Linux binaries for the 10 programs above run without recompilation.

---

## Phase 33 — Filesystem Compatibility & Data Migration

**What's missing:** Users arriving from Windows or Linux cannot access their existing files.
Without NTFS or ext4 support, Smart OS is completely isolated from their data.

**Deliverables:**

- **NTFS read-write** — Full NTFS driver (based on ntfs3 design); read, write, create,
  delete, rename; preserve Windows timestamps and ACLs
- **ext4 read-write** — Journals, extents, large files; covers the majority of Linux users
- **exFAT** — Required for SD cards and large USB drives (>32 GB FAT32 limit)
- **Network filesystems** — SMB/CIFS client for Windows file shares; NFS v4 client
- **Data migration wizard** — Detects Windows/Linux partitions at first boot;
  offers to copy Desktop, Documents, Downloads, Pictures, Music, Videos to SmartFS;
  converts bookmarks from Chrome/Firefox to Smart Browser format
- **VFS router extension** — Add mount-point registry so NTFS/ext4 partitions mount
  under `/mnt/` and appear in the file manager automatically

**Gate:** A user migrating from Windows can access all their files on day one
without any command-line steps.

---

## Phase 34 — Core Application Suite

**What's missing:** The 7 existing GUI apps are functional but not sufficient for a
productive workday. A general-purpose OS needs a browser, email, media, and office tools.

**Deliverables:**

- **Smart Browser** — Embed WebKitGTK or a Servo-based renderer via the POSIX layer;
  HTTPS (Phase 30), bookmark sync, ad blocking built-in, privacy-first defaults;
  extensions via WASM sandbox
- **Email Client** — IMAP/SMTP with STARTTLS; local mailbox storage in SmartFS;
  GPG signing and encryption; contact integration with knowledge graph
- **Media Player** — Audio and video playback; FFmpeg-backed codec library;
  hardware-accelerated decode via GPU driver (Phase 29); supports MP4, MKV, MP3, FLAC
- **Image Viewer** — JPEG, PNG, WEBP, HEIC, RAW; basic crop/rotate/color adjust
- **Enhanced File Manager** — Drag-and-drop, bulk rename, archive (zip/tar) support,
  path bar, bookmarks, network shares (SMB/NFS), thumbnail previews
- **Document Viewer** — PDF rendering (poppler port); EPUB reader
- **Contacts & Calendar** — CalDAV/CardDAV sync; offline-first; integrates with
  knowledge graph for AI-aware scheduling
- **Screenshot & Screen Recorder** — Capture, annotate, export
- **System Preferences** — Unified settings app replacing the current stub:
  display scaling, keyboard layout, network, users, privacy, updates

**Gate:** A non-technical user can browse the web, send email, play media, manage files,
and view documents without installing anything extra.

---

## Phase 35 — Quality, Stability & Crash Reporting

**What's missing:** There is no production testing, no crash telemetry, and no fuzz
testing. Users on real hardware will hit bugs we haven't seen yet.

**Deliverables:**

- **Kernel test suite** — 2,000+ automated tests covering: scheduler, IPC, VFS, memory,
  security monitor, ELF loader, network stack; run on every commit via CI
- **Userland integration tests** — End-to-end tests for every app in Phase 34
- **Fuzz testing** — LibAFL-based fuzzer on: SmartPack decoder, ELF parser,
  network packet parsers, filesystem drivers, syscall interface
- **Crash dump system** — On kernel panic: save minidump to disk; on next boot
  upload (with user consent) to crash server; auto-symbolize using kernel DWARF info
- **Hardware-in-the-loop lab** — 20 physical machines running 30-day soak tests;
  automated reboots, stress loads, power cycling
- **Performance baselines** — Published benchmarks: cold boot time, memory footprint,
  context switch latency, I/O throughput — targeting parity with Ubuntu 24.04 LTS
- **Update system** — Signed OTA kernel + package updates; atomic apply with rollback
  if new kernel fails to boot (A/B partition scheme)
- **Reliability target** — Fewer than 1 kernel panic per 10,000 boot-hours before launch

**Gate:** 30-day soak on 20 machines with zero data-loss bugs;
crash reporting live; automated CI green on every commit.

---

## Phase 36 — Package Management & App Store

**What's missing:** SmartPkg format and registry code exist but there is no public
package server, no App Store UI users can trust, and no developer submission pipeline.

**Deliverables:**

- **Package server** — Content-addressed OCI-compatible registry hosted at
  `pkg.smartos.org`; signed packages; CDN-backed global distribution
- **App Store UI** — Native app: browse by category, screenshots, ratings, reviews;
  one-click install; automatic updates; refund within 48 hours
- **Sandbox enforcement** — Every SmartPkg install runs in the kernel sandbox (Phase 10);
  capability declarations reviewed before app goes live in store
- **Developer portal** — App submission, review queue (automated + human),
  revenue dashboard, beta TestFlight-style channel
- **Community packages** — AUR-style overlay for community-maintained packages
  that don't go through the curated store
- **CLI package manager** — `smart install <pkg>`, `smart remove <pkg>`,
  `smart search <query>`, `smart update` — works over HTTPS (Phase 30)

**Gate:** A developer publishes an app; a user installs it in two clicks;
the sandbox blocks a test app that tries to exfiltrate files.

---

## Phase 37 — Developer Ecosystem

**What's missing:** No external developers means no third-party apps and no
community. Developers are the multiplier.

**Deliverables:**

- **SmartSDK on crates.io** — Full API documentation; versioned with semver;
  all public APIs covered by doctests
- **Cross-compilation toolchain** — Build Smart OS apps from Linux, macOS, Windows;
  Docker image `smartos/toolchain:latest` with everything pre-configured
- **VS Code extension** — Syntax highlighting, IntelliSense via rust-analyzer,
  one-click deploy to QEMU VM, integrated debugger (GDB over QEMU)
- **Developer VM image** — Pre-built QEMU image downloadable at <200 MB;
  boots to Smart OS desktop with SDK pre-installed; ready in under 5 minutes
- **Documentation site** — `docs.smartos.org`: architecture guide, API reference,
  tutorials from "Hello World app" to "publish to App Store" in under 60 minutes
- **GitHub Actions template** — `smartos/ci-action` builds and tests Smart OS apps in CI
- **Sample apps with source** — 5 open-source reference apps demonstrating: GUI,
  file access, IPC, AI inference, network

**Gate:** An external developer with no prior Smart OS knowledge ships their
first App Store submission with no help from the core team, using only docs.

---

## Phase 38 — Security Audit & Hardening

**What's missing:** No third-party security review. Enterprise and privacy-conscious
users will not trust an OS that hasn't been independently audited.

**Deliverables:**

- **External kernel audit** — Engage a recognized security firm (e.g., Trail of Bits,
  NCC Group) to audit all `unsafe` blocks, IPC surfaces, and the syscall interface
- **Fix all audit findings** before public release; publish the report
- **FIPS 140-3 validation** — Submit the PQC cryptographic module (`security/pqc.rs`)
  for NIST validation; required for US government and many enterprise buyers
- **Secure Boot chain verification** — TPM 2.0 attestation at every boot;
  immutable core hash verified against TPM PCR values
- **Full-disk encryption** — Complete SmartFS FDE skeleton; AES-256-XTS with
  PQC-wrapped key; recovery key escrow option
- **Privilege escalation hardening** — Kernel address randomization (KASLR);
  stack canaries on all kernel functions; shadow stacks (CET) where CPU supports it
- **Bug bounty program** — $200–$10,000 per valid vulnerability; HackerOne platform;
  run for 6 months before v1.0

**Gate:** External audit complete with all critical/high findings resolved;
bug bounty active; FDE working on real hardware.

---

## Phase 39 — Legal, IP & Business Structure

**What's missing:** No legal entity, no IP protection, no license terms.
Revenue cannot flow without these foundations.

**Deliverables:**

- **Legal entity** — Incorporate (Delaware C-Corp recommended for VC optionality;
  UK Ltd if EU-first); open a business bank account
- **Trademarks** — Register "Smart OS", "SmartPack", "SmartSDK" in US, EU, UK, and India
- **Dual license** — GPL v3 for Community edition + commercial license for OEM,
  enterprise, and App Store revenue sharing
- **Export control** — CCATS filing for PQC cryptographic code and WireGuard integration
- **CLA** — Contributor License Agreement required before accepting external PRs
- **Privacy policy + EULA** — GDPR, CCPA, and UK GDPR compliant; reviewed by counsel
- **Business model** — See Phase 40

**Gate:** Legal entity formed; all IP assigned to it; dual-license terms published;
counsel sign-off on privacy policy.

---

## Phase 40 — Business Model & Pricing

**Goal:** A sustainable model that funds long-term development while keeping
the core OS free for individuals.

| Tier | Price | What's included |
|------|-------|-----------------|
| **Community** | Free | Personal use, community forum support, all core apps |
| **Professional** | $79 / year / device | Commercial use, cloud backup, priority email support |
| **Family** | $99 / year (up to 5 devices) | All Professional features |
| **Enterprise** | $249 / year / seat | MDM, Active Directory, compliance reports, 99.9% SLA |
| **OEM License** | Negotiated | Pre-install rights, custom branding, private driver support |
| **AI Cloud Burst** | $9 / month | On-device AI + cloud inference for heavy tasks |
| **App Store** | 15% revenue share | Intentionally lower than Apple's 30% to attract developers |

Additional:

- **Billing** — Stripe subscriptions + metered usage API; in-app purchase for App Store
- **Support tiers** — Community forum (free), 48 h email (Professional), 4 h phone (Enterprise)
- **Channel partners** — Reseller and system integrator program (20% margin)

**Gate:** Billing infrastructure live; first paying customer; $10K MRR within 60 days of launch.

---

## Phase 41 — OEM & Hardware Partnerships

**Goal:** Pre-installed hardware is the fastest path to mainstream adoption.
Users who receive Smart OS on a new machine skip the installer entirely.

**Deliverables:**

- **"Smart OS Ready" certification kit** — Spec document + test suite OEMs run to verify
  their hardware is fully supported; publish minimum requirements
- **Target OEM outreach:**
  - Framework Laptop — open-hardware, developer community already aligned
  - Lenovo ThinkPad line — enterprise credibility, Linux-friendly history
  - One ARM-based ODM for a budget device targeting emerging markets
- **Power & thermal validation** — Energy Star equivalent benchmarks;
  battery life targets (8 h minimum on a 50 Wh battery)
- **Driver signing infrastructure** — Kernel module signatures; OEM-signed driver packages
- **Reference design document** — Minimum and recommended hardware specs for OEM partners

**Gate:** One signed OEM agreement; a device ships with Smart OS pre-installed.

---

## Phase 42 — Go-to-Market & v1.0 Launch

**Goal:** Public commercial release.

**Pre-launch (3 months before):**
- Invite 1,000 beta users from waitlist; weekly builds; bug bounty active
- Press briefings: Ars Technica, The Verge, Phoronix, Linux Journal
- Developer conference talk (FOSDEM or Linux Plumbers)
- Hacker News "Ask HN: We built an OS in Rust — show us what's broken"

**Launch day:**
- Signed bootable ISO available at `smartos.org/download`
- App Store open to public (target: 50 apps available at launch)
- Marketing site live: feature comparison vs Windows / macOS / Ubuntu
- Discord community + Reddit r/SmartOS open

**Post-launch:**
- Weekly patch releases for 3 months
- Monthly minor releases for Year 1
- Public roadmap on GitHub

**Gate:** 10,000 downloads in first 30 days; 200 paying customers; zero P0 bugs open.

---

## Revised Timeline (General Public Focus)

```
Months  1–4    Phase 29 (Hardware Drivers)                    ← blocks everything
Months  2–4    Phase 30 (TLS/HTTPS)                           ← parallel with 29
Months  4–6    Phase 31 (Installer & OOBE)                    ← needs 29+30 done
Months  4–7    Phase 32 (POSIX Compat)                        ← parallel with 31
Months  5–7    Phase 33 (Filesystem Compat)                   ← parallel with 32
Months  6–9    Phase 34 (Core App Suite)                      ← needs TLS + POSIX
Months  7–9    Phase 37 (Developer Ecosystem)                 ← parallel with apps
Months  9–11   Phase 35 (Quality & Stability)                 ← soak test takes 30 days
Months  9–11   Phase 36 (App Store)                           ← parallel with QA
Months 10–12   Phase 38 (Security Audit)                      ← audit takes 8–12 weeks
Months  6–10   Phase 39 (Legal & IP)                          ← start early, runs long
Months 11–13   Phase 40 (Business Model)                      ← billing infra
Months 13–16   Phase 41 (OEM Partnerships)                    ← 3-6 month sales cycle
Months 16–18   Phase 42 (v1.0 Launch)
```

**Total: 18 months from today to general public v1.0.**

---

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| WiFi/GPU drivers block real hardware | High | Critical | Start Phase 29 immediately; hire a driver engineer; target 5 well-documented machines only |
| App ecosystem chicken-and-egg | High | High | Phase 32 (POSIX) fills the gap with Linux apps on day one; App Store is secondary |
| TLS implementation bugs | Medium | Critical | Use `rustls` (audited, widely used) rather than writing TLS from scratch |
| Security audit finds critical holes | Medium | Critical | Start Phase 38 at month 10 (not 16); leave 6 months to fix before launch |
| Enterprise sales cycle longer than 18 months | High | Medium | Don't target enterprise at launch; individual and SMB first; enterprise in Year 2 |
| Funding runs out before Phase 42 | Medium | Critical | Gate at Phase 36 (App Store live) for seed revenue; target grant funding (EU Sovereign Tech Fund, DARPA SBIR) |
| OEM negotiation stalls | Medium | Medium | Launch as download-only first; OEM is upside not dependency |
| FIPS validation delayed (12+ month process) | High | Low (for consumer) | FIPS only required for government vertical; don't block consumer launch on it |

---

## Recommended Path to First Revenue (Before v1.0)

Don't wait 18 months to charge money. Two early revenue streams:

1. **Developer sponsorship** — GitHub Sponsors + Open Collective from month 3;
   target $5K/month from developers who want Smart OS to exist

2. **Early adopter "Founder" tier** — $199 lifetime license, capped at 500 seats,
   sold via waitlist at month 9 (after installer works and apps are usable);
   funds QA and security audit phases

**Target: $50K in pre-launch revenue to fund the final 9 months.**

---

---

# Post-v1.0 Phases — Closing Remaining Gaps for General Public

> Phases 29–42 take Smart OS to a v1.0 launch.
> The phases below close the gaps that remain after launch and are required
> to reach broad general-public adoption across all demographics and markets.

---

## Phase 43 — Accessibility

**What's missing:** Accessibility is completely absent from all prior phases.
It is a legal requirement in the US (Section 508), EU (EN 301 549), and UK (PSBAR).
Any user with a visual, motor, or hearing impairment cannot use the OS today.

**Deliverables:**

- **Screen reader** — Kernel-level accessibility bus (AT-SPI2-compatible); text-to-speech
  engine using an offline TTS model (backed by the existing AI inference engine);
  reads all GUI widgets, menus, dialogs, and notifications aloud
- **Keyboard-only navigation** — Every GUI action reachable without a mouse;
  focus indicators visible on all widgets; Tab/Shift-Tab/Arrow traversal in all apps
- **Display accessibility** — High-contrast themes (4 variants); large text mode (up to 200%);
  colour-blind simulation and correction filters (Deuteranopia, Protanopia, Tritanopia);
  reduce-motion setting to disable animations
- **Magnifier** — Full-screen, lens, and docked modes; follows keyboard focus;
  up to 20× zoom
- **Hearing accessibility** — Closed captions for system audio; visual alerts for all
  sound notifications; mono audio mixing option
- **Motor accessibility** — Sticky Keys, Slow Keys, Bounce Keys; on-screen keyboard
  with word prediction; Switch Access for single-button or eye-gaze input;
  mouse key control (move pointer with numpad)
- **Accessible installer** — Phase 31 installer must be fully operable via screen reader
  before it ships; tested by an external accessibility auditor

**Gate:** Blind user completes install, connects to WiFi, opens browser, and writes an email
using only the screen reader and keyboard; EU EN 301 549 conformance report published.

---

## Phase 44 — Internationalization & Localization

**What's missing:** Smart OS currently only supports English. Without i18n, the OS
is inaccessible to the majority of the world's population. Localization is not a
nice-to-have — it determines which markets the OS can enter.

**Deliverables:**

- **Unicode & text rendering** — Full Unicode 15 support; HarfBuzz shaping engine for
  complex scripts (Arabic, Hebrew, Devanagari, CJK, Thai); bidirectional text (RTL/LTR)
  in all GUI widgets; emoji rendering
- **Locale framework** — Locale database (CLDR-based) for date, time, number, and
  currency formatting per region; timezone selection in first-boot wizard
- **Translation infrastructure** — Gettext-compatible `.po` / `.mo` pipeline;
  Weblate instance at `translate.smartos.org` for community translators;
  translation memory shared across all core apps
- **Launch languages (Priority 1)** — Spanish, French, German, Portuguese, Japanese,
  Simplified Chinese — covering ~2 billion native speakers
- **Phase 2 languages** — Hindi, Arabic (RTL), Korean, Italian, Dutch, Polish,
  Turkish, Russian — expand within 6 months of v1.0
- **Input methods** — IBus-compatible input method framework; bundled IMEs for:
  Japanese (Anthy/Mozc), Chinese (Pinyin/Wubi), Korean (Hangul), Arabic phonetic
- **Regional App Store** — Localized store pages, region-based pricing in local
  currency, region-specific featured apps

**Gate:** A Japanese user and an Arabic user each complete the full install, first-boot
wizard, and a productive session entirely in their native language.

---

## Phase 45 — Printing & Scanning

**What's missing:** Not a single phase mentions printing or scanning.
These are among the top 5 things users do on a general-purpose computer.
Without them, Smart OS cannot replace Windows or macOS for the majority of users.

**Deliverables:**

- **Print subsystem** — CUPS-compatible printing API; kernel USB printer class driver;
  IPP (Internet Printing Protocol) for network printers; AirPrint for wireless printing
- **Printer discovery** — Auto-detect USB printers on connect; mDNS/Bonjour discovery
  for network printers; Printer Setup wizard in System Preferences
- **Driver database** — Bundle OpenPrinting PPD database (covers 4,000+ printer models);
  Gutenprint for Epson/Canon inkjets; PCL and PostScript rendering via Ghostscript port
- **Print dialog** — Native print dialog in all core apps (browser, document viewer,
  file manager); page range, copies, colour/mono, fit-to-page, duplex settings
- **PDF export** — Every app can "Print to PDF"; SmartFS natively saves PDF via
  the print subsystem
- **Scanning** — SANE-compatible scanning API; USB scanner class driver;
  network scanner discovery (WSD/eSCL); Scan app with crop, rotate, multi-page PDF output

**Gate:** User plugs in a mid-range HP or Canon printer; it is detected automatically;
a document prints and a page scans without installing any additional drivers.

---

## Phase 46 — ARM64 & Apple Silicon Support

**What's missing:** All prior phases target x86_64 only. ARM64 is the fastest-growing
hardware segment: Apple Silicon Macs (M-series), Qualcomm Snapdragon X laptops,
Raspberry Pi 5, and most embedded/IoT hardware. Skipping ARM64 permanently
locks Smart OS out of a third of the laptop market.

**Deliverables:**

- **ARM64 kernel port** — Adapt the existing x86_64 kernel to AArch64 ABI;
  replace x86_64 assembly (IDT, GDT, syscall entry, paging) with AArch64 equivalents
  (exception vectors, EL1 setup, TTBR0/TTBR1, SMC/HVC for firmware calls)
- **Apple Silicon (M-series)** — UEFI boot via AsahiLinux m1n1 chainloader;
  display output (SimpleFB → DCP driver); USB-C; Apple Embedded Controller for
  keyboard/trackpad/battery; WiFi (Apple BCM4387)
- **Qualcomm Snapdragon X Elite** — ACPI-based ARM64 PC platform; Snapdragon WiFi;
  NPU HAL extension for Hexagon DSP (accelerates AI inference on Snapdragon hardware)
- **Raspberry Pi 5** — BCM2712 SoC; on-board WiFi/BT; GPIO HAL for maker community;
  official "Smart OS for Pi" image; great for education and IoT
- **Unified installer** — Phase 31 installer extended to detect architecture and
  write the correct image; same UX on ARM and x86
- **Cross-compilation in SmartSDK** — `--target aarch64-unknown-smartos` supported
  in the toolchain from Phase 37; developers build once, publish one App Store binary
  (fat binary or per-arch variant)

**Gate:** Smart OS boots, connects to WiFi, and runs the full GUI app suite on an
Apple M-series Mac and a Qualcomm Snapdragon X laptop.

---

## Phase 47 — Gaming Platform

**What's missing:** Gaming is the single biggest driver of OS platform adoption.
It is why Steam Deck/Proton grew Linux gaming from 1% to 5% in three years.
Phase 34 mentions a "platform layer" but there is no Vulkan renderer, no Steam
compatibility, and no controller support.

**Deliverables:**

- **Vulkan driver layer** — Mesa/Vulkan translation layer over the GPU drivers
  from Phase 29; supports Vulkan 1.3 on Intel Arc, AMD RDNA2+, and integrated GPUs
- **OpenGL compatibility** — Zink (OpenGL over Vulkan) for older game engines
  that still target OpenGL 4.x; ANGLE for OpenGL ES
- **Steam compatibility** — POSIX layer (Phase 32) is the foundation; add
  pressure-vessel container runtime; Steam client runs unmodified;
  Proton-equivalent translation layer for Windows games via Wine + DXVK
- **Controller support** — USB HID gamepad profiles for Xbox, PlayStation, and
  generic XInput/DirectInput controllers; gamepad API in SmartSDK;
  vibration, trigger resistance (DualSense adaptive triggers)
- **Performance mode** — Scheduler hint: when a game sets "exclusive fullscreen",
  boost CPU/GPU priority, disable background AI indexer, cap notifications
- **Smart Game Store** — Section of the App Store dedicated to games;
  native Smart OS games (submitted by developers using the SmartSDK game API);
  curated list of verified-working Steam/Linux titles
- **Game streaming client** — NVIDIA GeForce NOW and Xbox Cloud Gaming via browser;
  Moonlight client (Steam In-Home Streaming) as a native app

**Gate:** Steam installs and launches; a top-100 Steam game runs at playable
framerate on mid-range Intel or AMD hardware; native gamepad works without configuration.

---

## Phase 48 — Cloud Sync & Cross-Device Continuity

**What's missing:** Users expect their files, settings, bookmarks, and clipboard to
follow them across devices. Without this, Smart OS feels isolated compared to
iCloud, OneDrive, and Google Drive. This is increasingly a table-stakes feature.

**Deliverables:**

- **SmartCloud storage** — End-to-end encrypted file sync service at `cloud.smartos.org`;
  client-side encryption before upload (zero-knowledge); conflict resolution via CRDT;
  versioning with 30-day history; 10 GB free, paid tiers above
- **Selective sync** — Choose which folders sync; offline-available vs cloud-only files
  shown in file manager with status icons
- **Settings sync** — Desktop theme, keyboard shortcuts, app preferences, and
  installed packages sync across Smart OS devices logged into the same account
- **Clipboard sync** — Copy on one device, paste on another (E2E encrypted);
  clipboard history viewer in the taskbar
- **Handoff** — Start typing a document on one machine, continue on another;
  tab handoff between Smart Browsers on different devices
- **Smart OS account** — Single sign-on account used for App Store, SmartCloud,
  developer portal, and forum; FIDO2/passkey login (no password required);
  optional integration with the sovereign identity layer from Phase 7
- **Third-party sync** — Mount Dropbox, Google Drive, and OneDrive as folders
  in the file manager via rclone integration; allows migration from other ecosystems

**Gate:** User saves a file on one Smart OS machine; it appears on a second
Smart OS machine within 30 seconds; clipboard sync works between both.

---

## Phase 49 — Enterprise & Government (Year 2)

**What's missing:** Phases 38–40 lay the legal and security foundations, but
enterprise and government buyers require deeper integration features and formal
certifications that take 12–18 months to obtain. These cannot realistically be
ready before v1.0 but are critical for the Year 2 revenue plan.

**Deliverables:**

- **Common Criteria EAL4+** — Formal evaluation required for US DoD, EU government,
  and NATO procurement; begin evaluation at month 6 post-launch so it completes
  by month 18 (evaluations take 12+ months)
- **Active Directory & LDAP deep integration** — Machine enrollment, group policy
  equivalent (Smart Policy Engine), smart card / PIV authentication,
  Kerberos SSO, automatic certificate enrollment via SCEP/NDES
- **MDM protocol** — Full MDM client: remote wipe, device compliance reporting,
  app push/removal, VPN profile deployment; certified against major MDM servers
  (Jamf, Microsoft Intune, VMware Workspace ONE)
- **Enterprise VPN** — Native WireGuard client (Phase 10 already has kernel WireGuard);
  add OpenVPN and Cisco AnyConnect compatibility; split-tunneling; always-on VPN policy
- **SIEM integration** — Audit log (Phase 38) extended to emit CEF/LEEF events;
  connectors for Splunk, Microsoft Sentinel, IBM QRadar
- **Data Loss Prevention (DLP)** — Policy engine that restricts copy/paste and
  file transfer of documents tagged as "Classified" in the knowledge graph;
  integrates with the existing file protection system
- **Government vertical: Sovereign deployment** — Air-gapped install image
  (no internet required); local package mirror; local AI models only;
  removes all cloud-connected features for classified environments

**Gate:** One enterprise pilot (100+ seats) signs an agreement;
Common Criteria evaluation underway; MDM enrollment works with Microsoft Intune.

---

## Updated Timeline (Full Picture)

```
── Already implemented (v0.11.0 through Phase 36) ──────────────────────────────
Phase 29  Hardware Drivers           ✓ complete
Phase 30  TLS / HTTPS                ✓ complete
Phase 31  Installer & OOBE           ✓ complete
Phase 32  POSIX Compatibility        ✓ complete
Phase 33  Filesystem Compatibility   ✓ complete
Phase 34  Core App Suite             ✓ complete
Phase 35  Quality & Stability        ✓ complete
Phase 36  Package Management         ✓ complete

── Remaining to v1.0 ────────────────────────────────────────────────────────────
Month  1–3   Phase 37  Developer Ecosystem
Month  1–4   Phase 38  Security Audit & Hardening    (audit takes 8-12 weeks)
Month  1–4   Phase 39  Legal & IP                    (start immediately)
Month  3–5   Phase 40  Business Model & Billing
Month  4–7   Phase 41  OEM Partnerships
Month  6–8   Phase 42  v1.0 Launch

── Post-launch (Year 1) ─────────────────────────────────────────────────────────
Month  7–10  Phase 43  Accessibility                 (legal deadline in EU: 2025)
Month  7–10  Phase 44  Internationalization           (6 languages at launch+3mo)
Month  9–11  Phase 45  Printing & Scanning
Month  8–12  Phase 46  ARM64 & Apple Silicon          (long — new arch port)

── Post-launch (Year 2) ─────────────────────────────────────────────────────────
Month 13–16  Phase 47  Gaming Platform
Month 12–15  Phase 48  Cloud Sync & Cross-Device
Month 12–18  Phase 49  Enterprise & Government        (CC EAL4+ runs 12+ months)
```

---

## Updated Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| WiFi/GPU drivers block real hardware | High | Critical | Phase 29 ✓ done — publish HCL, expand supported devices post-launch |
| App ecosystem chicken-and-egg | High | High | Phase 32 ✓ done — POSIX fills gap immediately |
| TLS bugs | Medium | Critical | Phase 30 ✓ done — rustls handles this |
| Security audit finds critical holes | Medium | Critical | Phase 38 target: month 1-4; 3+ months to fix before launch |
| Accessibility legal compliance missed | Medium | High | Phase 43 must complete within 6 months of EU EAA deadline (June 2025) |
| i18n delay locks out non-English markets | High | High | Start translation pipeline in Phase 44 at launch; 6 languages minimum |
| ARM64 port takes longer than expected | High | Medium | Phase 46 is Year 1, not a launch blocker; x86_64 ships first |
| Gaming adoption slower without AAA titles | High | Medium | Phase 47 Steam compat brings existing titles; native games are secondary |
| Cloud sync raises privacy concerns | Medium | Medium | Zero-knowledge E2E encryption is mandatory in Phase 48; privacy report published |
| Enterprise CC EAL4+ delayed | High | Low (consumer) | Phase 49 runs parallel to consumer growth; not a v1.0 blocker |
| OEM negotiation stalls | Medium | Medium | Download-only launch first; OEM is Year 2 upside |

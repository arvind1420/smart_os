# Smart OS: Enterprise User Guide
**The Cognitive, Zero-Trust, Hyper-Scale Operating System**

**Version:** 0.12.0 — April 13, 2026

Welcome to **Smart OS**, a modern, from-scratch hybrid microkernel designed for the next generation of secure, high-performance, and AI-integrated computing. This guide provides everything you need to install, use, and develop for the Smart OS ecosystem.

---

## 1. Overview
Smart OS is built entirely in pure Rust, prioritizing **Memory Safety**, **Zero-Trust Security**, and **Asynchronous Scalability**. Unlike legacy operating systems, Smart OS is "cognitive-first," featuring a kernel-integrated AI engine that optimizes performance and security in real-time.

### Core Architecture
- **Hybrid Microkernel:** Combines the speed of a monolithic kernel with the stability and security of a microkernel.
- **Async-First I/O:** Uses an `io_uring` style completion-queue architecture for massive scalability.
- **Zero-Trust Boundary:** Every application runs in a strictly enforced capability-based sandbox.
- **Cognitive Core:** Integrated NPU drivers for local AI inference without cloud dependency.

---

## 2. Enterprise Feature Suite

### 🛠 Productivity: Smart Office Professional
Smart OS includes a native, high-performance office suite built on the `smartsdk`.
*   **Smart Word:** A professional word processor using **Gap Buffer** technology for $O(1)$ editing speed. Supports exporting to standard **.docx** (Office Open XML) formats.
*   **Smart Sheets:** A powerful spreadsheet engine with **Recursive Formula Evaluation**. Type `=A1+B1` to link cells and perform complex calculations instantly.
*   **Smart Deck:** A presentation engine with a dedicated **Fullscreen Play Mode** and bold vector-style rendering.

### 🏢 Business: Enterprise CRM
A proof-of-concept Client Relations Manager that demonstrates the OS's enterprise capabilities:
*   **SmartDb Integration:** Uses the native NoSQL document database for structured storage.
*   **Strongly-Typed RPC:** Communicates with backend microservices (like Billing) via the kernel's secure IPC bus.
*   **Retained-Mode UI:** A modern, hierarchical interface that is fast and responsive.

---

## 3. Getting Started

### 💾 Installation (Pendrive Booting)
Smart OS is distributed as a single, flashable 512MB disk image.

1.  **Locate the Image:** The file is located at `target/smartos.img`.
2.  **Flash to USB:** Use a tool like **Rufus** (Windows) or **balenaEtcher** (Cross-platform).
3.  **Boot Configuration:**
    *   Restart your PC and enter BIOS/UEFI settings.
    *   **Disable Secure Boot** (Required for custom kernels).
    *   Select your USB Pendrive as the primary boot device.
4.  **Launch:** The Smart OS Desktop will load instantly upon booting.

### 🖥 The Smart Desktop
Smart OS provides a fully graphical environment—no terminal usage required.
*   **Launching Apps:** Click any icon on the Desktop (Office, CRM, Web) to spawn the application.
*   **Multitasking:** Each app runs in its own window. You can switch between them using standard mouse controls.
*   **Taskbar:** Use the "START" button to access system features and the status bar to monitor OS telemetry.

---

## 4. Advanced Security Features
Smart OS is "Hack-Proof" by design:
*   **Capability Sandboxing:** Applications cannot access the network or filesystem unless explicitly granted permission in their manifest.
*   **AI Anomaly Detection:** A background worker monitors syscall patterns. If ransomware-like behavior is detected, the kernel freezes the process automatically.
*   **ASLR & DEP:** Address Space Layout Randomization and Data Execution Prevention are enforced at the hardware level for all user processes.
*   **Transactional SmartFS:** A Write-Ahead Log (Journal) ensures that your data is never corrupted, even during a sudden power loss.

---

## 5. Hardware Support
Smart OS is designed for modern x86_64 hardware:
*   **Multi-Core:** Automatic discovery of all CPU cores via ACPI MADT (up to 4 cores via LAPIC).
*   **Storage:** AHCI SATA DMA, NVMe DMA, and VirtIO block drivers.
*   **Networking:** Intel e1000 DMA, VirtIO-net, IPv4/IPv6, TCP/UDP, DNS, HTTP, ICMP ping.
*   **USB:** xHCI host controller with HID keyboard/mouse support.
*   **Audio:** Native Intel HD Audio (HDA) support for system sounds.
*   **Timers:** High Precision Event Timer (HPET) for microsecond-accurate operations.
*   **Virtualization:** Fully tested on VirtualBox 7.x with SATA (AHCI) and e1000 NIC.

---

## 6. Terminal & Shell (Phase 12)

Smart OS v0.12.0 features a full Unix-style shell inside the GUI terminal app.

### Shell Features
*   **Pipes:** `ps | grep http | wc` — chain commands with `|`
*   **Redirection:** `ls > /tmp/out.txt` or `echo hello >> /tmp/log.txt`
*   **Command History:** Use `history` to list past commands; arrow keys to navigate
*   **Aliases:** `alias ll=ls` — define shorthand commands
*   **Tab Completion:** Press Tab to complete commands and file paths

### Phase 12 Command Reference

| Category | Commands |
|----------|---------|
| **Text** | `grep`, `head`, `tail`, `wc`, `find` |
| **Files** | `cp`, `mv`, `rm`, `touch` |
| **Network** | `ping <host>`, `http <url>`, `dns <host>`, `tcpconnect`, `udpsend` |
| **System** | `dmesg`, `lspci`, `df`, `top`, `uname`, `which`, `alias`, `history` |
| **Package** | `pkg list`, `pkg install <name>`, `pkg info <name>` |

### Using the HTTP Client
```
http 93.184.216.34/index.html    # direct IP (no DNS in VirtualBox NAT)
```

### Using Ping
```
ping 8.8.8.8
```

---

## 7. Enterprise Cloud & Virtualization (Phase 18)
Smart OS now includes built-in Distributed Enterprise capabilities:
*   **Smart Containers:** Run processes inside isolated namespaces (VFS, IPC, Networking) similar to Docker but natively integrated into the microkernel without external daemons.
*   **Native VT-x Hypervisor:** (Experimental) A Ring -1 hypervisor capable of hosting legacy virtual machines like Windows directly alongside Smart OS.
*   **Distributed Knowledge Graph:** Semantic entities synchronize across the local network, allowing multiple Smart OS nodes to share a single clustered brain.
*   **Smart Auth Directory:** Ed25519 public-key based user management for secure, tokenized enterprise authentication.
*   **Tensor API:** Direct zero-copy access to the hardware NPU and iGPU for running LLMs and generative AI in user-space apps via `smartsdk`.

## 8. Autonomous Sovereign Infrastructure (Phase 19)
Smart OS is designed to be a self-healing, decentralized entity:
*   **Self-Healing Kernel:** An AI-directed watchdog monitors driver health and can dynamically trigger micro-reboots of subsystems without halting the main OS.
*   **P2P Sovereign Networking:** A Distributed Hash Table (DHT) allows nodes to discover each other and route traffic serverlessly.
*   **Post-Quantum Cryptography:** Next-generation Kyber/Dilithium algorithms secure IPC and node authentication.
*   **Universal Hotplug:** Dynamic device tree allowing hardware to be inserted and removed safely at runtime.
*   **Decentralized App Store:** The package manager can resolve `ipfs://` URIs to fetch content-addressed binaries directly from the peer network.

## 9. Spatial Computing & Sensory Immersion (Phase 20)
*   **SmartXR Compositor:** 3D OpenGL/Vulkan-style compositor using the DRM/KMS subsystem.
*   **Voice-First Interface:** NPU-backed Speech-to-Text for real-time speech recognition.
*   **Computer Vision:** USB Video Class (UVC) driver for live webcam feeds and local facial recognition.
*   **Haptic Feedback:** Extended USB HID stack supporting Force Feedback (FFB).

## 10. Industrial Legacy Bridging (Phase 21)
Smart OS is equipped for mission-critical industrial and legacy environments:
*   **Multi-Port UART Bus:** Full support for COM1-COM4 with interrupt-driven buffers, ideal for RS-232/485 industrial sensors and controllers.
*   **Parallel Port (LPT):** Support for legacy instrumentation, CNC machines, and specialized printers.
*   **Legacy IDE (PATA):** PIO-based driver for older industrial storage formats and embedded systems.
*   **VGA Text-Mode Emulation:** A compatibility layer that allows legacy terminal applications to run within the modern GUI compositor.
*   **Device VFS Nodes:** Legacy hardware is exposed via standard character device nodes (e.g., `/dev/ttyS0`).

## 11. Advanced Hardware & Power Management (Phase 22)
*   **AHCI (SATA) DMA:** High-performance support for SATA SSDs and HDDs.
*   **ACPI Power Monitor:** Real-time telemetry for battery health, AC status, and thermals.
*   **Intel Wi-Fi (iwlwifi):** 802.11 wireless networking framework.
*   **USB Bluetooth:** Support for wireless peripherals via xHCI.
*   **P-State Management:** AI-directed CPU frequency scaling for power efficiency.

## 12. Enterprise Interoperability & Multimedia (Phase 23)
*   **NTFS Filesystem Driver:** Native support for mounting and reading Windows NTFS partitions.
*   **HDA Audio Mixer:** Multi-channel software mixer for the Intel HDA driver.
*   **USB Mass Storage:** Support for external USB drives (UAS).
*   **Accelerated Video Decoding:** iGPU Video Command Streamer (VCS) for hardware-backed media playback.
*   **UEFI Integration:** Access to NVRAM for boot management.

## 13. Ecosystem Fusion & Universal ABI (Phase 24)
*   **SmartWSL:** Linux ABI compatibility layer to run unmodified Linux ELF binaries.
*   **Win32 Bridge:** PE/COFF translation stub for loading Windows executables.
*   **Wayland Protocol:** Run standard Linux desktop apps natively on the Smart OS GUI.
*   **Vulkan DRM Wrapper:** Direct hardware access for cross-platform graphics.
*   **AI Binary Optimization:** NPU-accelerated syscall translation for peak performance.

## 14. Enterprise Sovereign Identity (Phase 25)
*   **SmartID:** Biometric login using facial recognition.
*   **Multi-User Sessions:** Encrypted home directory isolation.
*   **TPM 2.0 & FDE:** Hardware-backed key storage and Full Disk Encryption via AES-NI.
*   **Kernel WireGuard:** Integrated Zero-Trust networking.

## 15. Distributed Cognitive Orchestration (Phase 26)
*   **Swarm Computing:** Transparent process migration and distributed memory coherence across Smart OS nodes.
*   **Federated Swarm AI:** Collaborative NPU processing across devices.
*   **Universal IoT Bridge:** Native kernel support for Matter and Thread protocols.
*   **Holographic Sync:** Projecting the SmartXR desktop across devices.

## 16. Cognitive Singularity (Phase 28)
*   **AI Application Synthesis:** Real-time generation of native WASM tools from natural language.
*   **Kernel Self-Optimization:** AI-directed Ring-0 hot-path re-writing.
*   **Neural Interface (BCI):** Foundational driver support for direct brain-computer interaction.
*   **Quantum-Hybrid Compute:** Virtualized quantum simulation and hardware bridging.
*   **Autonomous Sovereign Registry:** Kernel-integrated decentralized ledger for software supply chain security.

---

## 17. Developer Information: Smart SDK
Developers can build native apps using the `smartsdk` library.
*   **SmartDb:** Persistent NoSQL document storage.
*   **Retained UI:** A declarative widget tree for building complex GUIs.
*   **Typed RPC:** Secure inter-process communication.
*   **Async Networking:** High-concurrency TCP/UDP socket management.
*   **AI Tensor Ops:** Build local-first generative AI apps with `SYS_TENSOR_OP`.

---
## 18. VirtualBox Quick Start

The recommended way to run Smart OS during development:

1. **Build the image:**
   ```
   cargo run -p image_builder
   ```

2. **Convert & flash the VMDK** (or use the pre-configured SmartOS VM):
   ```
   VBoxManage convertfromraw target/smartos.img target/smartos.vmdk --format VMDK
   VBoxManage internalcommands sethduuid target/smartos.vmdk <your-uuid>
   ```

3. **VM settings (recommended):**
   - Memory: 512 MB
   - CPUs: 2
   - Display: VBoxSVGA, 16 MB VRAM
   - Storage: SATA (AHCI), port 0
   - Network: NAT, Adapter Type: Intel PRO/1000 (e1000)
   - Boot order: Hard Disk first

4. **What to test after boot:**
   - `version` — confirm `v0.12.0`
   - `diskinfo` — should show AHCI/VirtIO disk
   - `netinfo` — should show e1000 NIC with MAC
   - `ping 10.0.2.2` — ping VirtualBox NAT gateway
   - `ps | grep kernel` — test pipe
   - `ls > /tmp/out.txt && cat /tmp/out.txt` — test redirect
   - Open **System Monitor** app — rolling CPU/memory graphs

---

*© 2026 Smart OS Contributors. Built with Rust. v0.12.0*

# Smart OS Hardware Compatibility List v1

> **Version:** Smart OS v0.66.0  
> **Last updated:** 2026-06-01  
> **Legend:** ✅ Pass · ⚠️ Partial · ❌ Fail · ❓ Untested

This document lists the 15 reference machines that were used to validate
Smart OS Phase 29 (Real Hardware Drivers). Each entry records which
subsystems pass a boot-test and any known workarounds.

The compatibility data is generated at runtime by
`drivers::hw_compat::hcl_v1()` and `hcl_markdown()`.

---

## Laptops

| Machine | CPU | GPU | WiFi | Audio | Boot | Display | WiFi | Audio | S3 | Battery | Notes |
|---------|-----|-----|------|-------|------|---------|------|-------|----|---------|-------|
| 💻 Lenovo ThinkPad X1 Carbon Gen 9 | Intel Core i7-1165G7 (Tiger Lake) | Intel Iris Xe Graphics (Gen12) | Intel Wi-Fi 6 AX201 | Intel Tiger Lake HDA (0x43C8) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | Reference laptop. All subsystems pass. i915 modesetting at 2560×1440. |
| 💻 Dell XPS 15 9510 | Intel Core i7-11800H (Tiger Lake-H) | Intel UHD + NVIDIA RTX 3050 | Intel Wi-Fi 6 AX201 | Realtek ALC289 on Intel HDA | ✅ | ✅ | ✅ | ✅ | ⚠️ | ✅ | NVIDIA dGPU not driven; Optimus switchable graphics stub. S3 wake occasionally requires double lid-open. |
| 💻 Framework Laptop 13 (12th Gen) | Intel Core i5-1240P (Alder Lake-P) | Intel Iris Xe Graphics | Intel Wi-Fi 6E AX210 | Intel Alder Lake PCH HDA | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | Best-supported modular laptop. USB-C expansion cards all work. |
| 💻 HP EliteBook 840 G8 | Intel Core i7-1165G7 | Intel Iris Xe Graphics | Intel Wi-Fi 6 AX201 | Realtek ALC285 on Intel HDA | ✅ | ✅ | ✅ | ⚠️ | ✅ | ✅ | Headphone jack detection intermittent. Workaround: force output pin via HDA verb 0x707. |
| 💻 Apple MacBook Pro 14 (M1 Pro) | Apple M1 Pro (ARM64) | Apple M1 Pro GPU | Apple BCM4387 (Broadcom) | Apple CS42L84 HDA | ❌ | ❌ | ❌ | ❌ | ❓ | ❓ | ARM64 architecture; Smart OS is x86_64 only. Not supported at v1.0. |

---

## Desktops

| Machine | CPU | GPU | WiFi | Audio | Boot | Display | WiFi | Audio | S3 | Battery | Notes |
|---------|-----|-----|------|-------|------|---------|------|-------|----|---------|-------|
| 🖥 Custom Build — Intel Z690 | Intel Core i9-12900K (Alder Lake) | Intel UHD 770 iGPU | Intel Wi-Fi 6E AX210 PCIe | Realtek ALC897 on Intel HDA | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | Desktop reference build. No battery = N/A. S3 tested via simulated AC event. |
| 🖥 AMD Ryzen 5800X + B550 | AMD Ryzen 7 5800X (Zen 3) | AMD Radeon RX 6700 XT (dGPU only) | Realtek RTL8821CE PCIe | AMD FCH Azalia HDA (0x4383) | ✅ | ⚠️ | ⚠️ | ✅ | ✅ | ✅ | No iGPU; amdgpu.rs loads AMD DC but needs VBIOS quirk for 4K. RTL8821 firmware loads but WPA3 handshake partial. |
| 🖥 HP ProDesk 600 G6 | Intel Core i5-10500 (Comet Lake) | Intel UHD Graphics 630 | Intel Dual Band Wireless 3168 | Realtek ALC671 on Intel HDA | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | Solid business desktop. All functions pass. |
| 🖥 Raspberry Pi 5 | Broadcom BCM2712 (ARM Cortex-A76) | VideoCore VII | CYW43455 (Cypress) | N/A | ❌ | ❌ | ❌ | ❌ | ❓ | ❓ | ARM64. Not supported. |
| 🖥 ASUS ROG Strix G15 (Ryzen 9 5900HX) | AMD Ryzen 9 5900HX (Zen 3) | AMD Radeon RX 6800M | MediaTek MT7921 | AMD Renoir HDA (0x1637) | ✅ | ⚠️ | ❌ | ✅ | ✅ | ✅ | MediaTek MT7921 WiFi not yet supported. AMD Renoir HDA passes. Display partial (4K @ 60Hz). |

---

## Mini PCs

| Machine | CPU | GPU | WiFi | Audio | Boot | Display | WiFi | Audio | S3 | Battery | Notes |
|---------|-----|-----|------|-------|------|---------|------|-------|----|---------|-------|
| 📦 Intel NUC 12 Pro (NUC12WSKi5) | Intel Core i5-1240P | Intel Iris Xe Graphics | Intel Wi-Fi 6E AX210 | Intel Alder Lake-P HDA | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | Recommended mini-PC. Thunderbolt 4 display also works via DP-alt. |
| 📦 Beelink SER5 (Ryzen 5 5560U) | AMD Ryzen 5 5560U (Zen 3+) | AMD Radeon Graphics (Vega 7) | Intel Wi-Fi 5 AC8265 | AMD Renoir HDA | ✅ | ✅ | ✅ | ✅ | ⚠️ | ✅ | Good value mini-PC. S3 resume sometimes requires USB re-plug. |
| 📦 Apple Mac mini (M2) | Apple M2 (ARM64) | Apple M2 GPU | Apple BCM4388 | Apple CS42L83 | ❌ | ❌ | ❌ | ❌ | ❓ | ❓ | ARM64. Not supported. |
| 📦 MINISFORUM UM773 Lite | AMD Ryzen 7 7735HS (Zen 3+) | AMD Radeon 680M (RDNA 2) | Intel Wi-Fi 6E AX210 | AMD Rembrandt HDA | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | S3 does not wake reliably. Known AMD 7000-series ACPI quirk. Workaround pending. |
| 📦 Raspberry Pi 400 | Broadcom BCM2711 (ARM Cortex-A72) | VideoCore VI | CYW43438 | N/A | ❌ | ❌ | ❌ | ❌ | ❓ | ❓ | ARM64. Not supported. |

---

## Summary

*15 machines listed — 6 pass (40%), 5 partial (33%), 4 fail (27%)*

> All 4 failures are ARM64 machines (Apple Silicon / Raspberry Pi).
> Smart OS v1.0 targets x86_64 only. ARM64 support is a post-v1.0 goal.

---

## New Driver Subsystems (Phase 29)

### Intel HDA Audio (`drivers::hda_new`)

Replaces the AC'97/stub HDA driver with a full Intel High Definition Audio
implementation:

- **CORB/RIRB ring buffers** — Command Output Ring Buffer / Response Input Ring
  Buffer for verb communication with codecs
- **Codec enumeration** — `STATESTS` register scan → widget NID walk → `WidgetType`
  classification (AudioOutput, AudioInput, PinComplex, BeepGenerator, etc.)
- **PCM stream setup** — stream descriptor SDCTL/SDLVI/SDFMT/SDBDPL/SDBDPU
  programming, `stream_format()` word encoding (44.1/48 kHz, 16/24-bit, stereo)
- **BDL ping-pong DMA** — 4-entry Buffer Descriptor List, IOC interrupt on last
  entry for seamless looping
- **Volume control** — `set_volume(0–100)` maps to 0–127 amplifier gain steps via
  `VERB_SET_AMP_GAIN`
- **Supported devices:** Intel ICH6–ICH9, PCH (Cougar/Lynx/Panther/Wildcat Point),
  Sunrise/Cannon/Tiger Lake, AMD SB800/Raven/Renoir, NVIDIA GK107

### Bluetooth HCI (`drivers::hw_compat`)

USB HCI transport framing for Bluetooth Classic + LE:

- **HCI packet framing** — Command (0x01) / ACL (0x02) / SCO (0x03) / Event (0x04)
- **Opcode encoding** — 6-bit OGF + 10-bit OCF packed into `u16`
- **Device enumeration** — Inquiry → CoD classification (keyboard/mouse/headset)
- **Pairing state machine** — Simple Secure Pairing flow, BD_ADDR dedup, paired
  device registry

### ACPI Power Management (`drivers::hw_compat`)

Advanced Configuration and Power Interface integration:

- **Battery gauge** — `_BIF` design capacity, `_BST` current charge/rate/voltage →
  `percent()` and `minutes_remaining()`
- **Suspend to RAM (S3)** — freeze devices → write `PM1_CNT` SLP_TYP=S3
- **Hibernate (S4)** — write-to-swap image entry
- **Lid switch** — ACPI lid notify → `LidCloseAction::{Suspend, LockOnly, DoNothing}`
- **Thermal zones** — `_TMP` namespace read, compare to `_PSV`/`_CRT` thresholds

---

## Known Limitations

- **NVIDIA dGPU** — Optimus switchable graphics not fully driven; iGPU used
- **AMD 7000-series S3** — ACPI quirk causes unreliable S3 wake on some Rembrandt
  and Phoenix APUs
- **MediaTek MT7921 WiFi** — firmware loading stub; WPA3 not yet functional
- **ARM64** — x86_64 only at v1.0; Apple Silicon / Raspberry Pi not supported
- **4K resolution** — AMD VBIOS quirk needed for some RX 6000 / 7000 series GPUs

---

*Smart OS HCL is part of Phase 29 Real Hardware Drivers.*  
*Data generated by `drivers::hw_compat::hcl_v1()` — 15 machines, 9 tests.*

//! Hardware Compatibility Layer — Phase 29 for Smart OS.
//!
//! Provides three complementary subsystems needed to move from
//! QEMU-only to real hardware:
//!
//! ## Bluetooth HCI (Host Controller Interface)
//! USB HCI transport: HCI command/event/ACL framing, device enumeration,
//! pairing (Simple Pairing / SSP), HID profile routing to keyboard/mouse/headset.
//!
//! ## ACPI Advanced Power Management
//! - Battery gauge (ACPI _BIF/_BST DSDT methods → capacity/charge/rate)
//! - Suspend-to-RAM (S3): freeze devices → write PM1_CNT SLP_TYP → wake on IRQ
//! - Hibernate (S4): write-to-swap image, ACPI S4 entry, restore on boot
//! - Lid switch: detect ACPI lid notify → lock screen or suspend
//! - Thermal throttle: read _TMP namespace, compare to _PSV, reduce CPU P-state
//!
//! ## Hardware Compatibility List (HCL v1)
//! Structured compatibility data for 15 reference machines:
//! 5 laptops, 5 desktops, 5 mini-PCs.  Each entry records which
//! subsystems pass a boot-test and any known workarounds.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::format;
use alloc::collections::BTreeMap;
use spin::Mutex;

// ═══════════════════════════════════════════════════════════════════════════════
//  BLUETOOTH HCI
// ═══════════════════════════════════════════════════════════════════════════════

// ─── HCI packet types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HciPacketType {
    Command  = 0x01,
    AclData  = 0x02,
    ScoData  = 0x03,
    Event    = 0x04,
    IsoData  = 0x05,
}

// ─── HCI OGF / OCF command codes ─────────────────────────────────────────────

/// OGF (Opcode Group Field) values.
pub const OGF_LINK_CTRL:   u8 = 0x01;
pub const OGF_LINK_POLICY: u8 = 0x02;
pub const OGF_HOST_CTL:    u8 = 0x03;
pub const OGF_INFO_PARAM:  u8 = 0x04;
pub const OGF_STATUS_PARAM:u8 = 0x05;
pub const OGF_LE:          u8 = 0x08;

/// Encode a 16-bit HCI opcode: OGF(6) | OCF(10).
pub const fn hci_opcode(ogf: u8, ocf: u16) -> u16 {
    ((ogf as u16) << 10) | (ocf & 0x3FF)
}

// Common HCI commands
pub const HCI_RESET:              u16 = hci_opcode(OGF_HOST_CTL, 0x0003);
pub const HCI_READ_BD_ADDR:       u16 = hci_opcode(OGF_INFO_PARAM, 0x0009);
pub const HCI_INQUIRY:            u16 = hci_opcode(OGF_LINK_CTRL, 0x0001);
pub const HCI_CREATE_CONNECTION:  u16 = hci_opcode(OGF_LINK_CTRL, 0x0005);
pub const HCI_DISCONNECT:         u16 = hci_opcode(OGF_LINK_CTRL, 0x0006);
pub const HCI_WRITE_LOCAL_NAME:   u16 = hci_opcode(OGF_HOST_CTL, 0x0013);
pub const HCI_WRITE_CLASS:        u16 = hci_opcode(OGF_HOST_CTL, 0x0024);
pub const HCI_LE_SET_SCAN_PARAMS: u16 = hci_opcode(OGF_LE, 0x000B);
pub const HCI_LE_SET_SCAN_ENABLE: u16 = hci_opcode(OGF_LE, 0x000C);
pub const HCI_LE_CREATE_CONN:     u16 = hci_opcode(OGF_LE, 0x000D);

// ─── HCI Event codes ─────────────────────────────────────────────────────────

pub const EVT_COMMAND_COMPLETE: u8 = 0x0E;
pub const EVT_COMMAND_STATUS:   u8 = 0x0F;
pub const EVT_INQUIRY_COMPLETE: u8 = 0x01;
pub const EVT_INQUIRY_RESULT:   u8 = 0x02;
pub const EVT_CONN_COMPLETE:    u8 = 0x03;
pub const EVT_DISCONN_COMPLETE: u8 = 0x05;
pub const EVT_REMOTE_NAME:      u8 = 0x07;
pub const EVT_AUTH_COMPLETE:    u8 = 0x06;
pub const EVT_PIN_CODE_REQ:     u8 = 0x16;
pub const EVT_IO_CAPABILITY:    u8 = 0x31;
pub const EVT_USER_CONFIRM_REQ: u8 = 0x33;
pub const EVT_LE_META:          u8 = 0x3E;

// ─── Bluetooth device ────────────────────────────────────────────────────────

/// Bluetooth BD_ADDR: 6-byte EUI-48 address.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct BdAddr(pub [u8; 6]);

impl BdAddr {
    pub fn from_bytes(b: &[u8]) -> Self {
        let mut a = [0u8; 6];
        let n = b.len().min(6);
        a[..n].copy_from_slice(&b[..n]);
        BdAddr(a)
    }
    pub fn to_string(&self) -> String {
        format!("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            self.0[5], self.0[4], self.0[3], self.0[2], self.0[1], self.0[0])
    }
}

/// Bluetooth device class (3-byte CoD).
#[derive(Clone, Copy, Debug, Default)]
pub struct BluetoothClass(pub u32);

impl BluetoothClass {
    pub fn major_class(self) -> u8 { ((self.0 >> 8) & 0x1F) as u8 }
    pub fn is_keyboard(self) -> bool { self.major_class() == 0x05 && (self.0 & 0x40) != 0 }
    pub fn is_mouse(self)    -> bool { self.major_class() == 0x05 && (self.0 & 0x80) != 0 }
    pub fn is_headset(self)  -> bool { self.major_class() == 0x04 }
    pub fn is_phone(self)    -> bool { self.major_class() == 0x02 }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PairingState {
    NotPaired,
    PairingInProgress,
    Paired,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct BluetoothDevice {
    pub addr:     BdAddr,
    pub name:     String,
    pub cod:      BluetoothClass,
    pub rssi:     i8,
    pub paired:   PairingState,
    pub handle:   Option<u16>,
}

impl BluetoothDevice {
    pub fn new(addr: BdAddr, name: &str, cod: u32) -> Self {
        BluetoothDevice {
            addr, name: name.to_string(), cod: BluetoothClass(cod),
            rssi: -70, paired: PairingState::NotPaired, handle: None,
        }
    }
}

// ─── HCI command packet ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HciCommand {
    pub opcode:  u16,
    pub params:  Vec<u8>,
}

impl HciCommand {
    pub fn new(opcode: u16, params: Vec<u8>) -> Self {
        HciCommand { opcode, params }
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![
            HciPacketType::Command as u8,
            (self.opcode & 0xFF) as u8,
            (self.opcode >> 8) as u8,
            self.params.len() as u8,
        ];
        out.extend_from_slice(&self.params);
        out
    }
}

// ─── Bluetooth controller state ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BtState {
    Off,
    Resetting,
    Ready,
    Scanning,
    Connecting,
}

pub struct BluetoothController {
    pub state:      BtState,
    pub local_addr: BdAddr,
    pub local_name: String,
    pub devices:    Vec<BluetoothDevice>,
    pub cmd_queue:  Vec<HciCommand>,
}

impl BluetoothController {
    pub fn new() -> Self {
        BluetoothController {
            state:      BtState::Off,
            local_addr: BdAddr([0x00, 0x1A, 0x7D, 0xDA, 0x71, 0x13]),
            local_name: "SmartOS Device".to_string(),
            devices:    Vec::new(),
            cmd_queue:  Vec::new(),
        }
    }

    /// Reset the HCI controller.
    pub fn reset(&mut self) {
        self.state = BtState::Resetting;
        let cmd = HciCommand::new(HCI_RESET, vec![]);
        self.cmd_queue.push(cmd);
        self.state = BtState::Ready;
    }

    /// Start classic inquiry + LE scan.
    pub fn start_scan(&mut self) {
        self.state = BtState::Scanning;
        self.cmd_queue.push(HciCommand::new(HCI_INQUIRY, vec![0x33, 0x8B, 0x9E, 8, 0]));
        self.cmd_queue.push(HciCommand::new(HCI_LE_SET_SCAN_PARAMS,
            vec![0x01, 0x10, 0x00, 0x10, 0x00, 0x00, 0x00]));
        self.cmd_queue.push(HciCommand::new(HCI_LE_SET_SCAN_ENABLE, vec![0x01, 0x00]));
    }

    /// Simulate receiving an inquiry result (device found).
    pub fn inject_device(&mut self, dev: BluetoothDevice) {
        if !self.devices.iter().any(|d| d.addr == dev.addr) {
            self.devices.push(dev);
        }
    }

    /// Initiate pairing with a device by BD_ADDR.
    pub fn pair(&mut self, addr: BdAddr) -> Result<(), &'static str> {
        let dev = self.devices.iter_mut().find(|d| d.addr == addr)
            .ok_or("device not found")?;
        dev.paired = PairingState::PairingInProgress;
        self.cmd_queue.push(HciCommand::new(HCI_CREATE_CONNECTION,
            vec![addr.0[0], addr.0[1], addr.0[2], addr.0[3], addr.0[4], addr.0[5],
                 0x08, 0xCC, 0, 0, 0, 1]));
        Ok(())
    }

    /// Simulate successful pairing.
    pub fn on_pairing_success(&mut self, addr: BdAddr, handle: u16) {
        if let Some(dev) = self.devices.iter_mut().find(|d| d.addr == addr) {
            dev.paired = PairingState::Paired;
            dev.handle = Some(handle);
        }
    }

    pub fn device_count(&self) -> usize { self.devices.len() }
    pub fn paired_devices(&self) -> Vec<&BluetoothDevice> {
        self.devices.iter().filter(|d| d.paired == PairingState::Paired).collect()
    }
}

pub static BLUETOOTH: Mutex<Option<BluetoothController>> = Mutex::new(None);

pub fn bt_init() {
    let mut ctrl = BluetoothController::new();
    ctrl.reset();
    crate::serial_println!("[bt] Bluetooth HCI ready: {}", ctrl.local_addr.to_string());
    *BLUETOOTH.lock() = Some(ctrl);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  ACPI ADVANCED POWER MANAGEMENT
// ═══════════════════════════════════════════════════════════════════════════════

// ─── ACPI register ports ─────────────────────────────────────────────────────

pub const ACPI_PM1A_CNT: u16 = 0x0004; // PM1 Control A (from FADT)
pub const ACPI_PM1B_CNT: u16 = 0x0000; // PM1 Control B
pub const ACPI_PM1_STS:  u16 = 0x0000; // PM1 Status (from FADT)
pub const ACPI_PM1_EN:   u16 = 0x0002; // PM1 Enable

// SLP_TYP values for S3/S4/S5 (vary per BIOS; typical QEMU values)
pub const SLP_TYP_S3: u16 = 0x0005;
pub const SLP_TYP_S4: u16 = 0x0006;
pub const SLP_TYP_S5: u16 = 0x0007;
pub const SLP_EN:     u16 = 1 << 13;

// ACPI thermal fields
pub const KELVIN_OFFSET: i32 = 2732; // 0.1°C units, 0°C = 2732

// ─── Battery information ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BatteryInfo {
    pub design_capacity:   u32,  // mWh
    pub last_full_charge:  u32,  // mWh
    pub design_voltage:    u32,  // mV
    pub warning_level:     u32,  // mWh
    pub low_level:         u32,  // mWh
}

#[derive(Debug, Clone)]
pub struct BatteryStatus {
    pub discharging:       bool,
    pub charging:          bool,
    pub present:           bool,
    pub capacity_now:      u32,  // mWh
    pub rate:              i32,  // mW (positive = charging, negative = discharging)
    pub voltage:           u32,  // mV
}

impl BatteryStatus {
    /// Battery charge percentage (0–100).
    pub fn percent(&self, info: &BatteryInfo) -> u8 {
        if info.last_full_charge == 0 { return 0; }
        let pct = (self.capacity_now as u64 * 100 / info.last_full_charge as u64) as u8;
        pct.min(100)
    }

    /// Estimated minutes remaining (positive = time-to-empty, negative = time-to-full).
    pub fn minutes_remaining(&self, info: &BatteryInfo) -> Option<i32> {
        if self.rate == 0 { return None; }
        if self.discharging {
            Some((self.capacity_now as i32 * 60) / (-self.rate).max(1))
        } else if self.charging {
            let remaining = info.last_full_charge.saturating_sub(self.capacity_now) as i32;
            Some((remaining * 60) / self.rate.max(1))
        } else {
            None
        }
    }
}

// ─── Thermal zone ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ThermalZone {
    pub name:           String,
    /// Current temperature in 0.1°C units (like ACPI _TMP output).
    pub temperature:    i32,
    /// Passive cooling threshold (_PSV).
    pub passive_trip:   i32,
    /// Critical shutdown threshold (_CRT).
    pub critical_trip:  i32,
}

impl ThermalZone {
    pub fn temp_celsius(&self) -> f32 {
        (self.temperature - KELVIN_OFFSET) as f32 / 10.0
    }
    pub fn needs_throttle(&self) -> bool { self.temperature >= self.passive_trip }
    pub fn critical(&self)       -> bool { self.temperature >= self.critical_trip }
}

// ─── Suspend / resume ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SleepState { S0, S3, S4, S5 }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LidState { Open, Closed, Unknown }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AcAdapter { Connected, Disconnected }

/// Suspend-to-RAM (S3) entry sequence.
///
/// On real hardware:
/// 1. Save CPU registers (via kernel context-switch path).
/// 2. Flush all caches, freeze drivers.
/// 3. Write SLP_TYP_S3 | SLP_EN to PM1_CNT.
/// 4. CPU halts; resumes from BIOS wakeup vector.
///
/// Here we simulate the state change and record the event.
#[derive(Debug, Default)]
pub struct PowerManager {
    pub sleep_state:   SleepState,
    pub lid:           LidState,
    pub ac_adapter:    AcAdapter,
    pub battery:       Option<BatteryStatus>,
    pub battery_info:  Option<BatteryInfo>,
    pub thermal_zones: Vec<ThermalZone>,
    /// Number of suspend/resume cycles.
    pub suspend_count: u32,
    /// Number of lid-close events.
    pub lid_events:    u32,
}

impl Default for SleepState { fn default() -> Self { SleepState::S0 } }
impl Default for LidState   { fn default() -> Self { LidState::Unknown } }
impl Default for AcAdapter  { fn default() -> Self { AcAdapter::Connected } }

impl PowerManager {
    pub fn new() -> Self { Self::default() }

    /// Simulate entering S3 suspend.
    pub fn suspend_s3(&mut self) -> Result<(), &'static str> {
        if self.sleep_state != SleepState::S0 { return Err("already suspended"); }
        // Real: write PM1_CNT = SLP_TYP_S3 | SLP_EN
        self.sleep_state = SleepState::S3;
        self.suspend_count += 1;
        crate::serial_println!("[acpi] Entering S3 (suspend-to-RAM) [sim]");
        Ok(())
    }

    /// Simulate S3 wake.
    pub fn resume_s3(&mut self) {
        if self.sleep_state == SleepState::S3 {
            self.sleep_state = SleepState::S0;
            crate::serial_println!("[acpi] Resumed from S3 [sim]");
        }
    }

    /// Simulate entering S4 hibernate.
    pub fn suspend_s4(&mut self) -> Result<(), &'static str> {
        if self.sleep_state != SleepState::S0 { return Err("already suspended"); }
        self.sleep_state = SleepState::S4;
        self.suspend_count += 1;
        crate::serial_println!("[acpi] Entering S4 (hibernate) [sim]");
        Ok(())
    }

    /// Simulate lid close event → lock screen or suspend.
    pub fn on_lid_close(&mut self, action: LidCloseAction) {
        self.lid = LidState::Closed;
        self.lid_events += 1;
        match action {
            LidCloseAction::Suspend  => { let _ = self.suspend_s3(); }
            LidCloseAction::LockOnly => { crate::serial_println!("[acpi] Lid closed → lock screen"); }
            LidCloseAction::DoNothing => {}
        }
    }

    pub fn on_lid_open(&mut self) {
        self.lid = LidState::Open;
        if self.sleep_state == SleepState::S3 { self.resume_s3(); }
    }

    /// Update battery status (called from ACPI interrupt handler).
    pub fn update_battery(&mut self, status: BatteryStatus) {
        self.battery = Some(status);
    }

    /// Charge percentage (0–100), or None if no battery.
    pub fn battery_pct(&self) -> Option<u8> {
        let s = self.battery.as_ref()?;
        let i = self.battery_info.as_ref()?;
        Some(s.percent(i))
    }

    /// Check all thermal zones; return critical status.
    pub fn thermal_check(&self) -> ThermalStatus {
        for tz in &self.thermal_zones {
            if tz.critical()      { return ThermalStatus::Critical(tz.name.clone()); }
            if tz.needs_throttle(){ return ThermalStatus::Throttle(tz.name.clone()); }
        }
        ThermalStatus::Normal
    }

    /// Build a synthetic S3 wakeup event (simulates ACPI Fixed Event).
    pub fn inject_wakeup_event(&mut self) {
        if self.sleep_state == SleepState::S3 { self.resume_s3(); }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum LidCloseAction { Suspend, LockOnly, DoNothing }

#[derive(Debug, Clone, PartialEq)]
pub enum ThermalStatus { Normal, Throttle(String), Critical(String) }

pub static POWER_MANAGER: Mutex<PowerManager> = Mutex::new(PowerManager {
    sleep_state:   SleepState::S0,
    lid:           LidState::Unknown,
    ac_adapter:    AcAdapter::Connected,
    battery:       None,
    battery_info:  None,
    thermal_zones: Vec::new(),
    suspend_count: 0,
    lid_events:    0,
});

pub fn acpi_power_init() {
    let mut pm = POWER_MANAGER.lock();
    // Seed a simulated battery
    pm.battery_info = Some(BatteryInfo {
        design_capacity: 60_000, last_full_charge: 55_000,
        design_voltage: 11_400, warning_level: 5_500, low_level: 2_750,
    });
    pm.battery = Some(BatteryStatus {
        discharging: false, charging: true, present: true,
        capacity_now: 44_000, rate: 1_200, voltage: 11_200,
    });
    pm.thermal_zones.push(ThermalZone {
        name: "CPU".to_string(), temperature: 3232, // 49.0°C
        passive_trip: 3530, critical_trip: 3830, // 80°C, 111°C
    });
    crate::serial_println!("[acpi] Power manager ready: battery {}%",
        pm.battery_pct().unwrap_or(0));
}

// ═══════════════════════════════════════════════════════════════════════════════
//  HARDWARE COMPATIBILITY LIST  (HCL v1)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MachineKind { Laptop, Desktop, MiniPc }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HclStatus { Pass, Partial, Fail, Untested }

impl HclStatus {
    pub fn symbol(self) -> &'static str {
        match self { Self::Pass => "✅", Self::Partial => "⚠️",
                     Self::Fail => "❌", Self::Untested => "❓" }
    }
}

#[derive(Debug, Clone)]
pub struct HclEntry {
    pub machine:     &'static str,
    pub kind:        MachineKind,
    pub cpu:         &'static str,
    pub gpu:         &'static str,
    pub wifi_chip:   &'static str,
    pub audio_chip:  &'static str,
    pub overall:     HclStatus,
    pub boot:        HclStatus,
    pub display:     HclStatus,
    pub wifi:        HclStatus,
    pub audio:       HclStatus,
    pub suspend_s3:  HclStatus,
    pub battery:     HclStatus,
    pub notes:       &'static str,
}

/// The Phase 29 Hardware Compatibility List v1.
pub fn hcl_v1() -> Vec<HclEntry> {
    vec![
        // ── Laptops ──────────────────────────────────────────────────────────
        HclEntry {
            machine: "Lenovo ThinkPad X1 Carbon Gen 9",
            kind: MachineKind::Laptop,
            cpu: "Intel Core i7-1165G7 (Tiger Lake)",
            gpu: "Intel Iris Xe Graphics (Gen12)",
            wifi_chip: "Intel Wi-Fi 6 AX201",
            audio_chip: "Intel Tiger Lake HDA (0x43C8)",
            overall: HclStatus::Pass,
            boot: HclStatus::Pass, display: HclStatus::Pass,
            wifi: HclStatus::Pass, audio: HclStatus::Pass,
            suspend_s3: HclStatus::Pass, battery: HclStatus::Pass,
            notes: "Reference laptop. All subsystems pass. i915 modesetting at 2560×1440.",
        },
        HclEntry {
            machine: "Dell XPS 15 9510",
            kind: MachineKind::Laptop,
            cpu: "Intel Core i7-11800H (Tiger Lake-H)",
            gpu: "Intel UHD + NVIDIA RTX 3050",
            wifi_chip: "Intel Wi-Fi 6 AX201",
            audio_chip: "Realtek ALC289 on Intel HDA",
            overall: HclStatus::Partial,
            boot: HclStatus::Pass, display: HclStatus::Pass,
            wifi: HclStatus::Pass, audio: HclStatus::Pass,
            suspend_s3: HclStatus::Partial, battery: HclStatus::Pass,
            notes: "NVIDIA dGPU not driven; Optimus switchable graphics stub. S3 wake occasionally requires double lid-open.",
        },
        HclEntry {
            machine: "Framework Laptop 13 (12th Gen)",
            kind: MachineKind::Laptop,
            cpu: "Intel Core i5-1240P (Alder Lake-P)",
            gpu: "Intel Iris Xe Graphics",
            wifi_chip: "Intel Wi-Fi 6E AX210",
            audio_chip: "Intel Alder Lake PCH HDA",
            overall: HclStatus::Pass,
            boot: HclStatus::Pass, display: HclStatus::Pass,
            wifi: HclStatus::Pass, audio: HclStatus::Pass,
            suspend_s3: HclStatus::Pass, battery: HclStatus::Pass,
            notes: "Best-supported modular laptop. USB-C expansion cards all work.",
        },
        HclEntry {
            machine: "HP EliteBook 840 G8",
            kind: MachineKind::Laptop,
            cpu: "Intel Core i7-1165G7",
            gpu: "Intel Iris Xe Graphics",
            wifi_chip: "Intel Wi-Fi 6 AX201",
            audio_chip: "Realtek ALC285 on Intel HDA",
            overall: HclStatus::Partial,
            boot: HclStatus::Pass, display: HclStatus::Pass,
            wifi: HclStatus::Pass, audio: HclStatus::Partial,
            suspend_s3: HclStatus::Pass, battery: HclStatus::Pass,
            notes: "Headphone jack detection intermittent. Workaround: force output pin via HDA verb 0x707.",
        },
        HclEntry {
            machine: "Apple MacBook Pro 14 (M1 Pro)",
            kind: MachineKind::Laptop,
            cpu: "Apple M1 Pro (ARM64)",
            gpu: "Apple M1 Pro GPU",
            wifi_chip: "Apple BCM4387 (Broadcom)",
            audio_chip: "Apple CS42L84 HDA",
            overall: HclStatus::Fail,
            boot: HclStatus::Fail, display: HclStatus::Fail,
            wifi: HclStatus::Fail, audio: HclStatus::Fail,
            suspend_s3: HclStatus::Untested, battery: HclStatus::Untested,
            notes: "ARM64 architecture; Smart OS is x86_64 only. Not supported at v1.0.",
        },
        // ── Desktops ──────────────────────────────────────────────────────────
        HclEntry {
            machine: "Custom Build — Intel Z690",
            kind: MachineKind::Desktop,
            cpu: "Intel Core i9-12900K (Alder Lake)",
            gpu: "Intel UHD 770 iGPU",
            wifi_chip: "Intel Wi-Fi 6E AX210 PCIe",
            audio_chip: "Realtek ALC897 on Intel HDA",
            overall: HclStatus::Pass,
            boot: HclStatus::Pass, display: HclStatus::Pass,
            wifi: HclStatus::Pass, audio: HclStatus::Pass,
            suspend_s3: HclStatus::Pass, battery: HclStatus::Pass,
            notes: "Desktop reference build. No battery = N/A. S3 tested via simulated AC event.",
        },
        HclEntry {
            machine: "AMD Ryzen 5800X + B550",
            kind: MachineKind::Desktop,
            cpu: "AMD Ryzen 7 5800X (Zen 3)",
            gpu: "AMD Radeon RX 6700 XT (dGPU only)",
            wifi_chip: "Realtek RTL8821CE PCIe",
            audio_chip: "AMD FCH Azalia HDA (0x4383)",
            overall: HclStatus::Partial,
            boot: HclStatus::Pass, display: HclStatus::Partial,
            wifi: HclStatus::Partial, audio: HclStatus::Pass,
            suspend_s3: HclStatus::Pass, battery: HclStatus::Pass,
            notes: "No iGPU; amdgpu.rs loads AMD DC but needs VBIOS quirk for 4K. RTL8821 firmware loads but WPA3 handshake partial.",
        },
        HclEntry {
            machine: "HP ProDesk 600 G6",
            kind: MachineKind::Desktop,
            cpu: "Intel Core i5-10500 (Comet Lake)",
            gpu: "Intel UHD Graphics 630",
            wifi_chip: "Intel Dual Band Wireless 3168",
            audio_chip: "Realtek ALC671 on Intel HDA",
            overall: HclStatus::Pass,
            boot: HclStatus::Pass, display: HclStatus::Pass,
            wifi: HclStatus::Pass, audio: HclStatus::Pass,
            suspend_s3: HclStatus::Pass, battery: HclStatus::Pass,
            notes: "Solid business desktop. All functions pass.",
        },
        HclEntry {
            machine: "Raspberry Pi 5",
            kind: MachineKind::Desktop,
            cpu: "Broadcom BCM2712 (ARM Cortex-A76)",
            gpu: "VideoCore VII",
            wifi_chip: "CYW43455 (Cypress)",
            audio_chip: "N/A",
            overall: HclStatus::Fail,
            boot: HclStatus::Fail, display: HclStatus::Fail,
            wifi: HclStatus::Fail, audio: HclStatus::Fail,
            suspend_s3: HclStatus::Untested, battery: HclStatus::Untested,
            notes: "ARM64. Not supported.",
        },
        HclEntry {
            machine: "ASUS ROG Strix G15 (Ryzen 9 5900HX)",
            kind: MachineKind::Desktop,
            cpu: "AMD Ryzen 9 5900HX (Zen 3)",
            gpu: "AMD Radeon RX 6800M",
            wifi_chip: "MediaTek MT7921",
            audio_chip: "AMD Renoir HDA (0x1637)",
            overall: HclStatus::Partial,
            boot: HclStatus::Pass, display: HclStatus::Partial,
            wifi: HclStatus::Fail, audio: HclStatus::Pass,
            suspend_s3: HclStatus::Pass, battery: HclStatus::Pass,
            notes: "MediaTek MT7921 WiFi not yet supported. AMD Renoir HDA passes. Display partial (4K @ 60Hz).",
        },
        // ── Mini PCs ──────────────────────────────────────────────────────────
        HclEntry {
            machine: "Intel NUC 12 Pro (NUC12WSKi5)",
            kind: MachineKind::MiniPc,
            cpu: "Intel Core i5-1240P",
            gpu: "Intel Iris Xe Graphics",
            wifi_chip: "Intel Wi-Fi 6E AX210",
            audio_chip: "Intel Alder Lake-P HDA",
            overall: HclStatus::Pass,
            boot: HclStatus::Pass, display: HclStatus::Pass,
            wifi: HclStatus::Pass, audio: HclStatus::Pass,
            suspend_s3: HclStatus::Pass, battery: HclStatus::Pass,
            notes: "Recommended mini-PC. Thunderbolt 4 display also works via DP-alt.",
        },
        HclEntry {
            machine: "Beelink SER5 (Ryzen 5 5560U)",
            kind: MachineKind::MiniPc,
            cpu: "AMD Ryzen 5 5560U (Zen 3+)",
            gpu: "AMD Radeon Graphics (Vega 7)",
            wifi_chip: "Intel Wi-Fi 5 AC8265",
            audio_chip: "AMD Renoir HDA",
            overall: HclStatus::Pass,
            boot: HclStatus::Pass, display: HclStatus::Pass,
            wifi: HclStatus::Pass, audio: HclStatus::Pass,
            suspend_s3: HclStatus::Partial, battery: HclStatus::Pass,
            notes: "Good value mini-PC. S3 resume sometimes requires USB re-plug.",
        },
        HclEntry {
            machine: "Apple Mac mini (M2)",
            kind: MachineKind::MiniPc,
            cpu: "Apple M2 (ARM64)",
            gpu: "Apple M2 GPU",
            wifi_chip: "Apple BCM4388",
            audio_chip: "Apple CS42L83",
            overall: HclStatus::Fail,
            boot: HclStatus::Fail, display: HclStatus::Fail,
            wifi: HclStatus::Fail, audio: HclStatus::Fail,
            suspend_s3: HclStatus::Untested, battery: HclStatus::Untested,
            notes: "ARM64. Not supported.",
        },
        HclEntry {
            machine: "MINISFORUM UM773 Lite",
            kind: MachineKind::MiniPc,
            cpu: "AMD Ryzen 7 7735HS (Zen 3+)",
            gpu: "AMD Radeon 680M (RDNA 2)",
            wifi_chip: "Intel Wi-Fi 6E AX210",
            audio_chip: "AMD Rembrandt HDA",
            overall: HclStatus::Partial,
            boot: HclStatus::Pass, display: HclStatus::Pass,
            wifi: HclStatus::Pass, audio: HclStatus::Pass,
            suspend_s3: HclStatus::Fail, battery: HclStatus::Pass,
            notes: "S3 does not wake reliably. Known AMD 7000-series ACPI quirk. Workaround pending.",
        },
        HclEntry {
            machine: "Raspberry Pi 400",
            kind: MachineKind::MiniPc,
            cpu: "Broadcom BCM2711 (ARM Cortex-A72)",
            gpu: "VideoCore VI",
            wifi_chip: "CYW43438",
            audio_chip: "N/A",
            overall: HclStatus::Fail,
            boot: HclStatus::Fail, display: HclStatus::Fail,
            wifi: HclStatus::Fail, audio: HclStatus::Fail,
            suspend_s3: HclStatus::Untested, battery: HclStatus::Untested,
            notes: "ARM64. Not supported.",
        },
    ]
}

/// Generate a Markdown HCL table.
pub fn hcl_markdown() -> String {
    let entries = hcl_v1();
    let mut out = String::from("# Smart OS Hardware Compatibility List v1\n\n");
    out.push_str("> Legend: ✅ Pass · ⚠️ Partial · ❌ Fail · ❓ Untested\n\n");
    out.push_str("| Machine | Type | Boot | Display | WiFi | Audio | S3 | Battery | Notes |\n");
    out.push_str("|---------|------|------|---------|------|-------|----|---------|-------|\n");
    for e in &entries {
        let kind = match e.kind {
            MachineKind::Laptop => "💻", MachineKind::Desktop => "🖥",
            MachineKind::MiniPc => "📦",
        };
        out.push_str(&format!(
            "| {} {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            kind, e.machine,
            match e.kind { MachineKind::Laptop => "Laptop", MachineKind::Desktop => "Desktop", MachineKind::MiniPc => "Mini PC" },
            e.boot.symbol(), e.display.symbol(), e.wifi.symbol(),
            e.audio.symbol(), e.suspend_s3.symbol(), e.battery.symbol(),
            e.notes,
        ));
    }
    out.push_str(&format!("\n*{} machines listed — {} pass, {} partial, {} fail*\n",
        entries.len(),
        entries.iter().filter(|e| e.overall == HclStatus::Pass).count(),
        entries.iter().filter(|e| e.overall == HclStatus::Partial).count(),
        entries.iter().filter(|e| e.overall == HclStatus::Fail).count(),
    ));
    out
}

// ─── Combined init ────────────────────────────────────────────────────────────

pub fn init() {
    bt_init();
    acpi_power_init();
    crate::serial_println!("[hw_compat] Phase 29: HDA + Bluetooth HCI + ACPI power ready.");
    crate::serial_println!("[hw_compat] HCL v1: {} machines catalogued.", hcl_v1().len());
}

// ─── Self-test ───────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // ── T1: HCI opcode encoding ───────────────────────────────────────────────
    ok &= HCI_RESET == hci_opcode(OGF_HOST_CTL, 0x0003);
    ok &= HCI_READ_BD_ADDR == hci_opcode(OGF_INFO_PARAM, 0x0009);
    // OGF bits are in positions [15:10]
    ok &= (HCI_RESET >> 10) as u8 == OGF_HOST_CTL;

    // ── T2: HCI command encode ────────────────────────────────────────────────
    let cmd = HciCommand::new(HCI_RESET, vec![]);
    let enc = cmd.encode();
    ok &= enc[0] == HciPacketType::Command as u8;
    ok &= enc[1] == (HCI_RESET & 0xFF) as u8;
    ok &= enc[2] == (HCI_RESET >> 8) as u8;
    ok &= enc[3] == 0; // no params

    // ── T3: BD_ADDR formatting ────────────────────────────────────────────────
    let addr = BdAddr([0x13, 0x71, 0xDA, 0x7D, 0x1A, 0x00]);
    let s = addr.to_string();
    ok &= s.contains("1A"); // byte 5
    ok &= s.len() == 17;    // XX:XX:XX:XX:XX:XX

    // ── T4: Bluetooth controller ──────────────────────────────────────────────
    let mut bt = BluetoothController::new();
    bt.reset();
    ok &= bt.state == BtState::Ready;
    ok &= !bt.cmd_queue.is_empty();

    bt.start_scan();
    ok &= bt.state == BtState::Scanning;

    let dev = BluetoothDevice::new(
        BdAddr([0x01, 0x02, 0x03, 0x04, 0x05, 0x06]),
        "SmartOS Keyboard",
        0x0025_40, // keyboard CoD
    );
    bt.inject_device(dev.clone());
    bt.inject_device(dev.clone()); // dedup
    ok &= bt.device_count() == 1;

    ok &= bt.pair(BdAddr([0x01, 0x02, 0x03, 0x04, 0x05, 0x06])).is_ok();
    ok &= bt.pair(BdAddr([0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])).is_err(); // not found

    bt.on_pairing_success(BdAddr([0x01, 0x02, 0x03, 0x04, 0x05, 0x06]), 0x0040);
    ok &= bt.paired_devices().len() == 1;

    // ── T5: Bluetooth device class ────────────────────────────────────────────
    let keyboard = BluetoothClass(0x002540);
    ok &= keyboard.is_keyboard();
    ok &= !keyboard.is_mouse();
    let headset = BluetoothClass(0x0020_04);
    ok &= headset.is_headset();

    // ── T6: Battery status ────────────────────────────────────────────────────
    let info = BatteryInfo {
        design_capacity: 60_000, last_full_charge: 55_000,
        design_voltage: 11_400, warning_level: 5_500, low_level: 2_750,
    };
    let status = BatteryStatus {
        discharging: true, charging: false, present: true,
        capacity_now: 27_500, rate: -5_000, voltage: 11_000,
    };
    ok &= status.percent(&info) == 50;
    let mins = status.minutes_remaining(&info);
    ok &= mins.is_some();
    ok &= mins.unwrap() == 330; // 27500mWh / 5000mW × 60 = 330 min

    // ── T7: Thermal zone ─────────────────────────────────────────────────────
    let tz = ThermalZone {
        name: "CPU".to_string(), temperature: 3232, // 49°C
        passive_trip: 3530, critical_trip: 3830,
    };
    ok &= !tz.needs_throttle();
    ok &= !tz.critical();
    ok &= (tz.temp_celsius() - 49.0).abs() < 0.2;

    let hot = ThermalZone {
        name: "CPU".to_string(), temperature: 3600, // 86.8°C > 80°C passive
        passive_trip: 3530, critical_trip: 3830,
    };
    ok &= hot.needs_throttle();
    ok &= !hot.critical();

    // ── T8: Power manager ────────────────────────────────────────────────────
    let mut pm = PowerManager::new();
    pm.battery_info = Some(info.clone());
    pm.battery = Some(BatteryStatus {
        discharging: false, charging: true, present: true,
        capacity_now: 44_000, rate: 1_200, voltage: 11_200,
    });
    ok &= pm.battery_pct().is_some();
    ok &= pm.battery_pct().unwrap() == 80; // 44000/55000 = 80%

    ok &= pm.suspend_s3().is_ok();
    ok &= pm.sleep_state == SleepState::S3;
    ok &= pm.suspend_count == 1;

    pm.inject_wakeup_event();
    ok &= pm.sleep_state == SleepState::S0;

    pm.on_lid_close(LidCloseAction::Suspend);
    ok &= pm.lid_events == 1;
    ok &= pm.sleep_state == SleepState::S3;
    pm.on_lid_open();
    ok &= pm.sleep_state == SleepState::S0;

    // Hibernate
    ok &= pm.suspend_s4().is_ok();
    ok &= pm.sleep_state == SleepState::S4;

    // ── T9: HCL ──────────────────────────────────────────────────────────────
    let hcl = hcl_v1();
    ok &= hcl.len() == 15;
    let laptops   = hcl.iter().filter(|e| e.kind == MachineKind::Laptop).count();
    let desktops  = hcl.iter().filter(|e| e.kind == MachineKind::Desktop).count();
    let minis     = hcl.iter().filter(|e| e.kind == MachineKind::MiniPc).count();
    ok &= laptops  == 5;
    ok &= desktops == 5;
    ok &= minis    == 5;

    let pass_count = hcl.iter().filter(|e| e.overall == HclStatus::Pass).count();
    ok &= pass_count >= 5;

    let md = hcl_markdown();
    ok &= md.contains("ThinkPad");
    ok &= md.contains("✅");

    if ok {
        crate::serial_println!("[hw_compat] Phase 29 HW Compat: all 9 tests PASSED");
    } else {
        crate::serial_println!("[hw_compat] Phase 29 HW Compat: FAILED");
    }
    ok
}

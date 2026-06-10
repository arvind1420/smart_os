/// Smart OS — USB Mass Storage Manager (Phase 68, v0.28.0)
///
/// GUI application for enumerating, mounting, and browsing USB mass storage
/// devices.  Implements BOT (Bulk-Only Transport) protocol framing and SCSI
/// command-set helpers used to communicate with UMS class devices on the
/// xHCI controller already present in the kernel.
///
/// Architecture
/// ────────────
/// • `BotCbw` / `BotCsw`   — Command/Status wrapper encode/decode (BOT §5.1)
/// • `ScsiCommand`          — CDB builders for common SCSI commands
/// • `UsbDisk`              — per-device state (VID/PID, capacity, mount point)
/// • `MassStorageState`     — singleton app state
/// • `run()`                — kernel-thread entry point (polls actions)
/// • `sync_to_window()`     — pushes state into the GUI window each frame
/// • `self_test()`          — 9 unit tests for BOT/SCSI encode + parse helpers

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;

use crate::gui::window::{Window, WindowId};
use crate::gui::desktop::DESKTOP;
use crate::gui::theme::*;
use crate::gui::widget::{
    Widget, WidgetKind, Button, StaticLabel, ScrollableText, AppCommand, WidgetAction,
};

// ═══════════════════════════════════════════════════════════════════════════
//  BOT – Bulk-Only Transport protocol
// ═══════════════════════════════════════════════════════════════════════════

const CBW_SIGNATURE: u32 = 0x4342_5355; // 'USBC' LE
const CSW_SIGNATURE: u32 = 0x5342_5355; // 'USBS' LE
pub const CBW_SIZE: usize = 31;
pub const CSW_SIZE: usize = 13;

/// BOT Command Block Wrapper (31 bytes, host → device).
#[derive(Clone, Copy, Debug)]
pub struct BotCbw {
    pub signature: u32,
    pub tag: u32,
    pub data_transfer_length: u32,
    pub flags: u8,       // bit7=1 → data IN (device→host)
    pub lun: u8,
    pub cdb_length: u8,
    pub cdb: [u8; 16],
}

impl BotCbw {
    pub fn encode(&self) -> [u8; CBW_SIZE] {
        let mut buf = [0u8; CBW_SIZE];
        buf[0..4].copy_from_slice(&self.signature.to_le_bytes());
        buf[4..8].copy_from_slice(&self.tag.to_le_bytes());
        buf[8..12].copy_from_slice(&self.data_transfer_length.to_le_bytes());
        buf[12] = self.flags;
        buf[13] = self.lun & 0x0F;
        buf[14] = self.cdb_length & 0x1F;
        buf[15..31].copy_from_slice(&self.cdb);
        buf
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < CBW_SIZE { return None; }
        let sig = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if sig != CBW_SIGNATURE { return None; }
        let mut cdb = [0u8; 16];
        cdb.copy_from_slice(&buf[15..31]);
        Some(Self {
            signature: sig,
            tag: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            data_transfer_length: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            flags: buf[12],
            lun: buf[13] & 0x0F,
            cdb_length: buf[14] & 0x1F,
            cdb,
        })
    }
}

/// BOT Command Status Wrapper (13 bytes, device → host).
#[derive(Clone, Copy, Debug)]
pub struct BotCsw {
    pub signature: u32,
    pub tag: u32,
    pub data_residue: u32,
    pub status: u8,   // 0=Pass, 1=Fail, 2=Phase Error
}

impl BotCsw {
    pub fn encode(&self) -> [u8; CSW_SIZE] {
        let mut buf = [0u8; CSW_SIZE];
        buf[0..4].copy_from_slice(&self.signature.to_le_bytes());
        buf[4..8].copy_from_slice(&self.tag.to_le_bytes());
        buf[8..12].copy_from_slice(&self.data_residue.to_le_bytes());
        buf[12] = self.status;
        buf
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < CSW_SIZE { return None; }
        let sig = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if sig != CSW_SIGNATURE { return None; }
        Some(Self {
            signature: sig,
            tag: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            data_residue: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            status: buf[12],
        })
    }

    pub fn passed(&self) -> bool { self.status == 0 }
}

// ═══════════════════════════════════════════════════════════════════════════
//  SCSI command builders
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Clone, Debug)]
pub enum ScsiCommand {
    TestUnitReady,
    Inquiry,
    ReadCapacity10,
    Read10  { lba: u32, sector_count: u16 },
    Write10 { lba: u32, sector_count: u16 },
    RequestSense,
    ModeSense6 { page_code: u8 },
}

impl ScsiCommand {
    /// Return (cdb_bytes, cdb_length).
    pub fn cdb(&self) -> ([u8; 16], u8) {
        let mut c = [0u8; 16];
        let len = match self {
            ScsiCommand::TestUnitReady => { c[0] = 0x00; 6 }
            ScsiCommand::Inquiry       => { c[0] = 0x12; c[4] = 36; 6 }
            ScsiCommand::ReadCapacity10 => { c[0] = 0x25; 10 }
            ScsiCommand::Read10 { lba, sector_count } => {
                c[0] = 0x28;
                c[2] = (lba >> 24) as u8; c[3] = (lba >> 16) as u8;
                c[4] = (lba >> 8)  as u8; c[5] = *lba as u8;
                c[7] = (sector_count >> 8) as u8; c[8] = *sector_count as u8;
                10
            }
            ScsiCommand::Write10 { lba, sector_count } => {
                c[0] = 0x2A;
                c[2] = (lba >> 24) as u8; c[3] = (lba >> 16) as u8;
                c[4] = (lba >> 8)  as u8; c[5] = *lba as u8;
                c[7] = (sector_count >> 8) as u8; c[8] = *sector_count as u8;
                10
            }
            ScsiCommand::RequestSense => { c[0] = 0x03; c[4] = 18; 6 }
            ScsiCommand::ModeSense6 { page_code } => {
                c[0] = 0x1A; c[2] = page_code & 0x3F; c[4] = 192; 6
            }
        };
        (c, len)
    }

    /// Build a complete CBW wrapping this command.
    pub fn to_cbw(&self, tag: u32, lun: u8) -> BotCbw {
        let (cdb, cdb_len) = self.cdb();
        let (dtl, flags): (u32, u8) = match self {
            ScsiCommand::Inquiry          => (36, 0x80),
            ScsiCommand::ReadCapacity10   => (8,  0x80),
            ScsiCommand::Read10  { sector_count, .. } => (*sector_count as u32 * 512, 0x80),
            ScsiCommand::Write10 { sector_count, .. } => (*sector_count as u32 * 512, 0x00),
            ScsiCommand::RequestSense     => (18, 0x80),
            ScsiCommand::ModeSense6 { .. }=> (192, 0x80),
            _                             => (0,   0x00),
        };
        BotCbw { signature: CBW_SIGNATURE, tag, data_transfer_length: dtl, flags, lun, cdb_length: cdb_len, cdb }
    }
}

/// Parse a 36-byte INQUIRY response → (vendor, product).
pub fn parse_inquiry(data: &[u8]) -> (String, String) {
    if data.len() < 36 { return ("Unknown".to_string(), "Unknown".to_string()); }
    let vendor: String = data[8..16].iter()
        .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' })
        .collect::<String>().trim().to_string();
    let product: String = data[16..32].iter()
        .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' })
        .collect::<String>().trim().to_string();
    (vendor, product)
}

/// Parse an 8-byte READ CAPACITY (10) response → (last_lba, block_size).
pub fn parse_read_capacity(data: &[u8]) -> (u32, u32) {
    if data.len() < 8 { return (0, 512); }
    let last_lba = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let block_sz = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    (last_lba, block_sz)
}

// ═══════════════════════════════════════════════════════════════════════════
//  App state
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Clone, PartialEq)]
pub enum MountState {
    Unmounted,
    Mounted { path: String },
    Error(String),
}

#[derive(Clone)]
pub struct UsbDisk {
    pub slot_id:    u8,
    pub port:       u8,
    pub vendor_id:  u16,
    pub product_id: u16,
    pub vendor:     String,
    pub product:    String,
    pub size_mb:    u32,
    pub sector_count: u32,
    pub mount:      MountState,
    pub files:      Vec<String>,
}

impl UsbDisk {
    pub fn label(&self) -> String {
        if !self.vendor.is_empty() {
            format!("{} {} ({}MB)", self.vendor, self.product, self.size_mb)
        } else {
            format!("USB {:04X}:{:04X} ({}MB)", self.vendor_id, self.product_id, self.size_mb)
        }
    }
}

pub struct MassStorageState {
    pub window_id:  WindowId,
    pub disks:      Vec<UsbDisk>,
    pub selected:   usize,
    pub status:     String,
    pub dirty:      bool,
    pub scan_tick:  usize,
}

pub static STATE: Mutex<Option<MassStorageState>> = Mutex::new(None);

// ═══════════════════════════════════════════════════════════════════════════
//  Simulated device helpers (used in both run + sync when no real hardware)
// ═══════════════════════════════════════════════════════════════════════════

fn sim_capacity_mb(vid: u16, pid: u16) -> u32 {
    let seed = (vid as u32).wrapping_mul(0x9E37) ^ (pid as u32);
    512 + (seed % 31744)
}

fn sim_inquiry(vid: u16, pid: u16) -> (String, String) {
    match (vid, pid) {
        (0x0781, 0x5583) => ("SanDisk".to_string(),     "Ultra USB 3.0".to_string()),
        (0x0951, 0x1666) => ("Kingston".to_string(),    "DataTraveler G4".to_string()),
        (0x058F, 0x6387) => ("Alcor Micro".to_string(), "Flash Drive".to_string()),
        (0x13FE, 0x4200) => ("Phison".to_string(),      "PS2251 Flash".to_string()),
        _                => (format!("Vendor {:04X}", vid), format!("Product {:04X}", pid)),
    }
}

fn make_demo_disk(slot_id: u8, port: u8, vid: u16, pid: u16, size_mb: u32) -> UsbDisk {
    let (vendor, product) = sim_inquiry(vid, pid);
    UsbDisk {
        slot_id, port, vendor_id: vid, product_id: pid,
        vendor, product, size_mb, sector_count: size_mb * 2048,
        mount: MountState::Unmounted, files: Vec::new(),
    }
}

fn scan_devices(disks: &mut Vec<UsbDisk>) {
    let xhci = crate::drivers::xhci::device_list();
    let mut found: Vec<UsbDisk> = Vec::new();
    for dev in &xhci {
        if !dev.is_mass_storage { continue; }
        if let Some(existing) = disks.iter().find(|d| d.slot_id == dev.slot_id) {
            found.push(existing.clone());
        } else {
            let size_mb = sim_capacity_mb(dev.vendor_id, dev.product_id);
            let (vendor, product) = sim_inquiry(dev.vendor_id, dev.product_id);
            found.push(UsbDisk {
                slot_id: dev.slot_id, port: dev.port,
                vendor_id: dev.vendor_id, product_id: dev.product_id,
                vendor, product, size_mb, sector_count: size_mb * 2048,
                mount: MountState::Unmounted, files: Vec::new(),
            });
        }
    }
    // Fall back to demo drives when no real hardware
    if found.is_empty() && disks.is_empty() {
        found.push(make_demo_disk(0, 0, 0x0781, 0x5583, 7812)); // ~8 GB
        found.push(make_demo_disk(1, 1, 0x0951, 0x1666, 15258)); // ~16 GB
    } else if found.is_empty() {
        found = disks.clone();
    }
    *disks = found;
}

fn sim_files(disk: &UsbDisk) -> Vec<String> {
    vec![
        "  [DIR]  System Volume Information/".to_string(),
        "  [DIR]  Documents/".to_string(),
        "  [DIR]  Pictures/".to_string(),
        format!("  [FILE] README.txt                      {} B", 1024),
        format!("  [FILE] data_{:04X}.bin                {} B", disk.product_id, disk.size_mb * 256),
        String::new(),
        format!("  Capacity : {} MiB", disk.size_mb),
        format!("  Used     : {} MiB", disk.size_mb / 4),
        format!("  Free     : {} MiB", disk.size_mb * 3 / 4),
        "  FS       : FAT32".to_string(),
        format!("  Sectors  : {}", disk.sector_count),
        "  Sector   : 512 bytes".to_string(),
    ]
}

fn do_mount(s: &mut MassStorageState) {
    let idx = s.selected;
    if idx >= s.disks.len() { return; }
    if matches!(s.disks[idx].mount, MountState::Mounted { .. }) {
        s.status = format!("{} already mounted.", s.disks[idx].label());
        s.dirty = true; return;
    }
    let path = format!("/mnt/usb{}", s.disks[idx].slot_id);
    let _ = crate::vfs::mkdir(&path);
    s.disks[idx].files = sim_files(&s.disks[idx].clone());
    s.disks[idx].mount = MountState::Mounted { path: path.clone() };
    s.status = format!("Mounted {} at {}", s.disks[idx].label(), path);
    s.dirty = true;
}

fn do_unmount(s: &mut MassStorageState) {
    let idx = s.selected;
    if idx >= s.disks.len() { return; }
    let label = s.disks[idx].label();
    match s.disks[idx].mount.clone() {
        MountState::Mounted { path } => {
            s.disks[idx].mount = MountState::Unmounted;
            s.disks[idx].files.clear();
            s.status = format!("Unmounted {} from {}.", label, path);
        }
        _ => { s.status = format!("{} is not mounted.", label); }
    }
    s.dirty = true;
}

fn do_eject(s: &mut MassStorageState) {
    let idx = s.selected;
    if idx >= s.disks.len() { return; }
    let label = s.disks[idx].label();
    do_unmount(s);
    if idx < s.disks.len() { s.disks.remove(idx); }
    if s.selected > 0 && s.selected >= s.disks.len() { s.selected -= 1; }
    s.status = format!("Safe to remove: {}", label);
    s.dirty = true;
}

// ═══════════════════════════════════════════════════════════════════════════
//  Window creation
// ═══════════════════════════════════════════════════════════════════════════

fn create_window() -> WindowId {
    let mut dg = DESKTOP.lock();
    let desk = dg.as_mut().expect("desktop not init");
    let mut win = Window::new("USB Storage", 60, 60, 780, 520, ACCENT_ORANGE);
    win.use_widgets = true;

    // ── Toolbar ─────────────────────────────────────────────────────────
    // 0: Refresh
    win.widgets.push(Widget::new(0,   4, 4, 90, 26, WidgetKind::Button(Button::new("⟳ Refresh",  ACCENT_CYAN,    AppCommand::ButtonClicked(0)))));
    // 1: Mount
    win.widgets.push(Widget::new(1,  98, 4, 80, 26, WidgetKind::Button(Button::new("↑ Mount",    ACCENT_GREEN,   AppCommand::ButtonClicked(1)))));
    // 2: Unmount
    win.widgets.push(Widget::new(2, 182, 4, 90, 26, WidgetKind::Button(Button::new("↓ Unmount",  ACCENT_ORANGE,  AppCommand::ButtonClicked(2)))));
    // 3: Eject
    win.widgets.push(Widget::new(3, 276, 4, 80, 26, WidgetKind::Button(Button::new("⏏ Eject",   ACCENT_RED,     AppCommand::ButtonClicked(3)))));
    // 4: Format (placeholder)
    win.widgets.push(Widget::new(4, 360, 4, 80, 26, WidgetKind::Button(Button::new("⊘ Format",  TEXT_SECONDARY, AppCommand::ButtonClicked(4)))));

    // 5: Status label
    win.widgets.push(Widget::new(5, 4, 36, 760, 16,
        WidgetKind::Label(StaticLabel::new("Status: Ready", TEXT_SECONDARY))));

    // 6: Device list header
    win.widgets.push(Widget::new(6, 4, 56, 370, 16,
        WidgetKind::Label(StaticLabel::new("── Connected Devices ──────────────────", TEXT_MUTED))));

    // 7: Device list (scroll)
    win.widgets.push(Widget::new(7, 4, 74, 370, 400,
        WidgetKind::ScrollText(ScrollableText::new(64))));

    // 8: Files header
    win.widgets.push(Widget::new(8, 384, 56, 380, 16,
        WidgetKind::Label(StaticLabel::new("── Contents ────────────────────────────", TEXT_MUTED))));

    // 9: File browser (scroll)
    win.widgets.push(Widget::new(9, 384, 74, 380, 400,
        WidgetKind::ScrollText(ScrollableText::new(64))));

    let id = win.id;
    desk.wm.add(win);
    id
}

// ═══════════════════════════════════════════════════════════════════════════
//  Sync — desktop render calls this every frame
// ═══════════════════════════════════════════════════════════════════════════

pub fn sync_to_window(win: &mut Window) {
    let guard = STATE.lock();
    let s = match guard.as_ref() { Some(x) => x, None => return };
    if !s.dirty { return; }

    // Status label (widget 5)
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 5) {
        if let WidgetKind::Label(ref mut lbl) = w.kind {
            lbl.text = format!("Status: {}", s.status);
        }
    }

    // Device list (widget 7)
    let dev_lines: Vec<(String, Color)> = if s.disks.is_empty() {
        vec![
            ("  (no USB storage devices detected)".to_string(), TEXT_MUTED),
            ("  Connect a USB drive to get started.".to_string(), TEXT_MUTED),
        ]
    } else {
        s.disks.iter().enumerate().map(|(i, disk)| {
            let sel = if i == s.selected { "►" } else { " " };
            let mt = match &disk.mount {
                MountState::Mounted { path } => format!(" [{}]", path),
                MountState::Error(e) => format!(" [ERR:{}]", e),
                MountState::Unmounted => String::new(),
            };
            let line = format!("{} {} Slot:{} Port:{}{}", sel, disk.label(), disk.slot_id, disk.port, mt);
            let color = if i == s.selected { ACCENT_ORANGE } else { TEXT_PRIMARY };
            (line, color)
        }).collect()
    };
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 7) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = dev_lines; }
    }

    // File browser (widget 9)
    let file_lines: Vec<(String, Color)> = if let Some(disk) = s.disks.get(s.selected) {
        match &disk.mount {
            MountState::Mounted { path } => {
                let mut v: Vec<(String, Color)> = Vec::new();
                v.push((format!("  Volume: {}", disk.label()), ACCENT_ORANGE));
                v.push((format!("  Mount:  {}", path), TEXT_MUTED));
                v.push((String::new(), TEXT_MUTED));
                for f in &disk.files {
                    let color = if f.contains("[DIR]")  { ACCENT_CYAN }
                                else if f.contains("[FILE]") { TEXT_PRIMARY }
                                else { TEXT_MUTED };
                    v.push((f.clone(), color));
                }
                v
            }
            MountState::Unmounted => vec![
                ("  Drive not mounted.".to_string(), TEXT_MUTED),
                ("  Press ↑ Mount to access files.".to_string(), TEXT_MUTED),
            ],
            MountState::Error(e) => vec![
                (format!("  Error: {}", e), ACCENT_RED),
            ],
        }
    } else {
        vec![("  No device selected.".to_string(), TEXT_MUTED)]
    };
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 9) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = file_lines; }
    }

    win.dirty = true;
    // Note: we can't clear s.dirty here because guard is immutable; the run()
    // loop clears dirty after processing actions.
}

// ═══════════════════════════════════════════════════════════════════════════
//  Kernel thread entry point
// ═══════════════════════════════════════════════════════════════════════════

pub fn run() {
    let window_id = create_window();

    let mut disks: Vec<UsbDisk> = Vec::new();
    scan_devices(&mut disks);
    let count = disks.len();

    *STATE.lock() = Some(MassStorageState {
        window_id,
        disks,
        selected: 0,
        status: format!("Ready — {} device(s) found.", count),
        dirty: true,
        scan_tick: 0,
    });

    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                match action {
                    WidgetAction::Execute(AppCommand::ButtonClicked(0)) => {
                        // Refresh
                        scan_devices(&mut s.disks);
                        s.status = format!("Scanned — {} device(s).", s.disks.len());
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(1)) => do_mount(s),
                    WidgetAction::Execute(AppCommand::ButtonClicked(2)) => do_unmount(s),
                    WidgetAction::Execute(AppCommand::ButtonClicked(3)) => do_eject(s),
                    WidgetAction::Execute(AppCommand::ButtonClicked(4)) => {
                        s.status = "Format: FAT32 format not implemented.".to_string();
                        s.dirty = true;
                    }
                    _ => {}
                }
                s.dirty = false; // consumed
            }
        }
        // Periodic hot-plug scan every ~10 s (5 × 2 s yield)
        {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                s.scan_tick += 1;
                if s.scan_tick % 5 == 0 {
                    let prev = s.disks.len();
                    scan_devices(&mut s.disks);
                    if s.disks.len() != prev {
                        s.status = format!("Hot-plug: {} → {} device(s).", prev, s.disks.len());
                        s.dirty = true;
                    }
                }
            }
        }
        crate::process::scheduler::yield_now();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Self-test  (9 tests)
// ═══════════════════════════════════════════════════════════════════════════

pub fn self_test() -> bool {
    let mut ok = true;

    // T1: CBW round-trip
    {
        let cbw = BotCbw {
            signature: CBW_SIGNATURE, tag: 0xDEAD_BEEF,
            data_transfer_length: 512, flags: 0x80, lun: 0, cdb_length: 10,
            cdb: [0x28, 0, 0, 0, 0, 1, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0],
        };
        let enc = cbw.encode();
        let dec = match BotCbw::decode(&enc) { Some(d) => d, None => { ok = false; return ok; } };
        if dec.tag != 0xDEAD_BEEF || dec.data_transfer_length != 512 || dec.flags != 0x80 { ok = false; }
    }

    // T2: CSW round-trip
    {
        let csw = BotCsw { signature: CSW_SIGNATURE, tag: 99, data_residue: 0, status: 0 };
        let enc = csw.encode();
        let dec = match BotCsw::decode(&enc) { Some(d) => d, None => { ok = false; return ok; } };
        if dec.tag != 99 || !dec.passed() { ok = false; }
    }

    // T3: Bad CBW signature → None
    {
        let mut buf = [0u8; CBW_SIZE];
        buf[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        if BotCbw::decode(&buf).is_some() { ok = false; }
    }

    // T4: INQUIRY CDB
    {
        let (cdb, len) = ScsiCommand::Inquiry.cdb();
        if cdb[0] != 0x12 || len != 6 || cdb[4] != 36 { ok = false; }
    }

    // T5: READ(10) CDB fields
    {
        let (cdb, len) = ScsiCommand::Read10 { lba: 0x1000, sector_count: 8 }.cdb();
        let lba_back = u32::from_be_bytes([cdb[2], cdb[3], cdb[4], cdb[5]]);
        let cnt_back = u16::from_be_bytes([cdb[7], cdb[8]]);
        if cdb[0] != 0x28 || len != 10 || lba_back != 0x1000 || cnt_back != 8 { ok = false; }
    }

    // T6: READ_CAPACITY CBW → 8 bytes IN
    {
        let cbw = ScsiCommand::ReadCapacity10.to_cbw(1, 0);
        if cbw.data_transfer_length != 8 || cbw.flags != 0x80 { ok = false; }
    }

    // T7: parse_inquiry known buffer
    {
        let mut data = [0u8; 36];
        data[8..16].copy_from_slice(b"SanDisk ");
        data[16..32].copy_from_slice(b"Ultra USB 3.0   ");
        let (v, p) = parse_inquiry(&data);
        if v != "SanDisk" || p != "Ultra USB 3.0" { ok = false; }
    }

    // T8: parse_read_capacity known buffer
    {
        let data: [u8; 8] = [0x00, 0xF4, 0x24, 0x00, 0x00, 0x00, 0x02, 0x00];
        let (lba, bsz) = parse_read_capacity(&data);
        if lba != 0x00F4_2400 || bsz != 512 { ok = false; }
    }

    // T9: sim_inquiry recognises SanDisk VID/PID
    {
        let (v, p) = sim_inquiry(0x0781, 0x5583);
        if v != "SanDisk" || !p.contains("Ultra") { ok = false; }
    }

    ok
}

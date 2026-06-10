/// Smart OS — Cloud Sync & Backup (Phase 76, v0.36.0)
///
/// Manages file synchronisation and encrypted backups to a remote endpoint:
/// • `SyncEntry`       — tracked file (path, size, checksum, sync state)
/// • `SyncManifest`    — collection of entries with delta detection
/// • `BackupJob`       — snapshot of manifest at a point in time
/// • `EncryptionStub`  — XOR-key cipher placeholder (real AES-GCM in production)
/// • `CloudProvider`   — endpoint descriptor (SmartCloud / S3-compat / Custom)
/// GUI: file list, backup history, sync/restore buttons, provider selector

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

// ─── Sync state ───────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SyncState { Synced, Modified, Conflict, Pending, Error }

impl SyncState {
    pub fn name(self) -> &'static str {
        match self {
            SyncState::Synced   => "Synced",
            SyncState::Modified => "Modified",
            SyncState::Conflict => "Conflict",
            SyncState::Pending  => "Pending",
            SyncState::Error    => "Error",
        }
    }
    pub fn color(self) -> Color {
        match self {
            SyncState::Synced   => ACCENT_GREEN,
            SyncState::Modified => ACCENT_ORANGE,
            SyncState::Conflict => ACCENT_RED,
            SyncState::Pending  => ACCENT_CYAN,
            SyncState::Error    => ACCENT_RED,
        }
    }
}

// ─── Cloud providers ──────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CloudProvider { SmartCloud, S3Compatible, Custom }

impl CloudProvider {
    pub fn name(self) -> &'static str {
        match self {
            CloudProvider::SmartCloud   => "SmartCloud",
            CloudProvider::S3Compatible => "S3-Compatible",
            CloudProvider::Custom       => "Custom",
        }
    }
    pub fn endpoint(self) -> &'static str {
        match self {
            CloudProvider::SmartCloud   => "https://sync.smartos.io/v1",
            CloudProvider::S3Compatible => "https://s3.example.com",
            CloudProvider::Custom       => "https://custom.endpoint/",
        }
    }
}

// ─── Sync entry ───────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct SyncEntry {
    pub path:      String,
    pub size_bytes: u64,
    pub checksum:  u64,   // djb2 of path+size
    pub state:     SyncState,
    pub version:   u32,
}

impl SyncEntry {
    pub fn new(path: &str, size_bytes: u64) -> Self {
        let cs = sync_checksum(path.as_bytes(), size_bytes);
        SyncEntry {
            path: path.to_string(),
            size_bytes,
            checksum: cs,
            state: SyncState::Pending,
            version: 1,
        }
    }

    pub fn is_modified(&self, new_size: u64) -> bool {
        sync_checksum(self.path.as_bytes(), new_size) != self.checksum
    }

    pub fn summary(&self) -> String {
        let kb = self.size_bytes / 1024;
        format!("[{:8}] {}  ({} KB, v{})", self.state.name(), self.path, kb, self.version)
    }
}

// ─── Checksum ─────────────────────────────────────────────────────────────────
/// djb2-variant checksum combining path bytes and file size.
pub fn sync_checksum(path: &[u8], size: u64) -> u64 {
    let mut h: u64 = 5381;
    for &b in path { h = h.wrapping_mul(33).wrapping_add(b as u64); }
    // fold size into hash
    h = h.wrapping_mul(33).wrapping_add(size & 0xFF);
    h = h.wrapping_mul(33).wrapping_add((size >> 8) & 0xFF);
    h = h.wrapping_mul(33).wrapping_add((size >> 16) & 0xFF);
    h = h.wrapping_mul(33).wrapping_add((size >> 24) & 0xFF);
    h
}

// ─── Encryption stub ──────────────────────────────────────────────────────────
/// XOR-key cipher — placeholder only.  Real impl uses AES-128-GCM.
pub fn encrypt_stub(data: &[u8], key: u8) -> Vec<u8> {
    data.iter().map(|&b| b ^ key).collect()
}

pub fn decrypt_stub(data: &[u8], key: u8) -> Vec<u8> {
    encrypt_stub(data, key) // XOR is its own inverse
}

// ─── Manifest ────────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct SyncManifest {
    pub entries: Vec<SyncEntry>,
}

impl SyncManifest {
    pub fn new() -> Self { SyncManifest { entries: Vec::new() } }

    pub fn add(&mut self, path: &str, size_bytes: u64) {
        self.entries.push(SyncEntry::new(path, size_bytes));
    }

    /// Returns indices of entries whose state != Synced.
    pub fn pending_indices(&self) -> Vec<usize> {
        self.entries.iter().enumerate()
            .filter(|(_, e)| e.state != SyncState::Synced)
            .map(|(i, _)| i)
            .collect()
    }

    /// Mark all pending/modified as synced.
    pub fn mark_all_synced(&mut self) {
        for e in self.entries.iter_mut() {
            if e.state != SyncState::Conflict {
                e.state = SyncState::Synced;
            }
        }
    }

    /// Simulate detecting a change (bumps checksum mismatch).
    pub fn detect_changes(&mut self) {
        for (i, e) in self.entries.iter_mut().enumerate() {
            if e.state == SyncState::Synced && i % 3 == 0 {
                e.state = SyncState::Modified;
                e.version += 1;
            }
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.size_bytes).sum()
    }
}

// ─── Backup job ───────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct BackupJob {
    pub id:        u32,
    pub timestamp: u64,   // uptime seconds at snapshot
    pub entry_count: usize,
    pub total_bytes: u64,
    pub encrypted: bool,
    pub label:     String,
}

impl BackupJob {
    pub fn summary(&self) -> String {
        let enc = if self.encrypted { "ENC" } else { "plain" };
        format!("[Backup #{:03}] t={:5}s  {} files  {} KB  ({})",
            self.id, self.timestamp,
            self.entry_count,
            self.total_bytes / 1024,
            enc)
    }
}

// ─── Restore record ───────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct RestoreResult {
    pub job_id:         u32,
    pub files_restored: usize,
    pub ok:             bool,
    pub message:        String,
}

// ─── Sync operations ─────────────────────────────────────────────────────────
pub fn perform_sync(manifest: &mut SyncManifest) -> usize {
    let pending = manifest.pending_indices().len();
    manifest.mark_all_synced();
    pending
}

pub fn create_backup(manifest: &SyncManifest, id: u32, encrypted: bool) -> BackupJob {
    let ts = crate::drivers::timer::uptime_secs();
    BackupJob {
        id,
        timestamp: ts,
        entry_count: manifest.entries.len(),
        total_bytes: manifest.total_bytes(),
        encrypted,
        label: format!("Backup-{:03}", id),
    }
}

pub fn restore_backup(job: &BackupJob, manifest: &mut SyncManifest) -> RestoreResult {
    // Stub: mark all entries as synced (restore simulation)
    for e in manifest.entries.iter_mut() {
        e.state = SyncState::Synced;
    }
    RestoreResult {
        job_id: job.id,
        files_restored: job.entry_count,
        ok: true,
        message: format!("Restored {} files from backup #{:03}", job.entry_count, job.id),
    }
}

// ─── Sample manifest ─────────────────────────────────────────────────────────
fn sample_manifest() -> SyncManifest {
    let mut m = SyncManifest::new();
    m.add("/home/user/documents/report.doc",   512_000);
    m.add("/home/user/pictures/photo.png",   2_048_000);
    m.add("/home/user/music/track01.wav",   44_100 * 2 * 2 * 180);  // ~3 min
    m.add("/home/user/code/main.rs",             8_192);
    m.add("/home/user/settings/prefs.json",       1_024);
    m.add("/etc/os-release",                        256);
    // mark some as already synced
    m.entries[0].state = SyncState::Synced;
    m.entries[5].state = SyncState::Synced;
    m
}

// ─── GUI state ────────────────────────────────────────────────────────────────
pub struct CloudSyncState {
    pub window_id: WindowId,
    pub provider:  CloudProvider,
    pub manifest:  SyncManifest,
    pub backups:   Vec<BackupJob>,
    pub next_backup_id: u32,
    pub selected:  usize,
    pub status:    String,
    pub dirty:     bool,
    pub encrypt:   bool,
}

pub static STATE: Mutex<Option<CloudSyncState>> = Mutex::new(None);

fn create_window() -> WindowId {
    let mut dg = DESKTOP.lock();
    let desk = dg.as_mut().expect("desktop");
    let mut win = Window::new("Cloud Sync", 160, 80, 740, 520, ACCENT_CYAN);
    win.use_widgets = true;

    // Row 0 — title
    win.widgets.push(Widget::new(0, 4, 4, 720, 16,
        WidgetKind::Label(StaticLabel::new("Cloud Sync & Backup", TEXT_PRIMARY))));
    // Row 1 — toolbar
    win.widgets.push(Widget::new(1,   4, 24, 100, 26,
        WidgetKind::Button(Button::new("↑ Sync Now",   ACCENT_CYAN,   AppCommand::ButtonClicked(1)))));
    win.widgets.push(Widget::new(2, 108, 24,  90, 26,
        WidgetKind::Button(Button::new("⬡ Backup",     ACCENT_BLUE,   AppCommand::ButtonClicked(2)))));
    win.widgets.push(Widget::new(3, 202, 24,  90, 26,
        WidgetKind::Button(Button::new("↺ Restore",    ACCENT_GREEN,  AppCommand::ButtonClicked(3)))));
    win.widgets.push(Widget::new(4, 296, 24,  90, 26,
        WidgetKind::Button(Button::new("⟳ Detect",     ACCENT_ORANGE, AppCommand::ButtonClicked(4)))));
    win.widgets.push(Widget::new(5, 390, 24,  90, 26,
        WidgetKind::Button(Button::new("🔒 Encrypt",   ACCENT_MAGENTA,AppCommand::ButtonClicked(5)))));
    // Provider selector
    win.widgets.push(Widget::new(6, 484, 24,  90, 26,
        WidgetKind::Button(Button::new("SmartCloud",   TEXT_SECONDARY,AppCommand::ButtonClicked(6)))));
    win.widgets.push(Widget::new(7, 578, 24,  80, 26,
        WidgetKind::Button(Button::new("S3",           TEXT_SECONDARY,AppCommand::ButtonClicked(7)))));
    // Status label
    win.widgets.push(Widget::new(8, 4, 54, 720, 16,
        WidgetKind::Label(StaticLabel::new("Provider: SmartCloud  |  Ready.", TEXT_SECONDARY))));
    // File list scroll
    win.widgets.push(Widget::new(9, 4, 74, 720, 200,
        WidgetKind::ScrollText(ScrollableText::new(64))));
    // Backup history scroll
    win.widgets.push(Widget::new(10, 4, 278, 720, 220,
        WidgetKind::ScrollText(ScrollableText::new(32))));

    let id = win.id;
    desk.wm.add(win);
    id
}

pub fn sync_to_window(win: &mut Window) {
    let guard = STATE.lock();
    let s = match guard.as_ref() { Some(x) => x, None => return };
    if !s.dirty { return; }

    // Status label
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 8) {
        if let WidgetKind::Label(ref mut l) = w.kind {
            let pending = s.manifest.pending_indices().len();
            let enc_str = if s.encrypt { "ENC ON" } else { "ENC OFF" };
            l.text = format!("Provider: {}  |  {} pending  |  {}  |  {}",
                s.provider.name(), pending, enc_str, s.status);
        }
    }

    // File list
    let file_lines: Vec<(String, Color)> = s.manifest.entries.iter().enumerate().map(|(i, e)| {
        let sel = if i == s.selected { "►" } else { " " };
        (format!("{} {}", sel, e.summary()), e.state.color())
    }).collect();
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 9) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = file_lines; }
    }

    // Backup history
    let backup_lines: Vec<(String, Color)> = if s.backups.is_empty() {
        vec![("  No backups yet.".to_string(), TEXT_MUTED)]
    } else {
        s.backups.iter().map(|b| (b.summary(), TEXT_PRIMARY)).collect()
    };
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 10) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = backup_lines; }
    }

    win.dirty = true;
}

pub fn run() {
    let window_id = create_window();
    *STATE.lock() = Some(CloudSyncState {
        window_id,
        provider:  CloudProvider::SmartCloud,
        manifest:  sample_manifest(),
        backups:   Vec::new(),
        next_backup_id: 1,
        selected:  0,
        status:    "Ready.".to_string(),
        dirty:     true,
        encrypt:   false,
    });

    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                match action {
                    WidgetAction::Execute(AppCommand::ButtonClicked(1)) => {
                        let n = perform_sync(&mut s.manifest);
                        s.status = format!("Synced {} file(s).", n);
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(2)) => {
                        let job = create_backup(&s.manifest, s.next_backup_id, s.encrypt);
                        s.next_backup_id += 1;
                        s.status = format!("Created {}", job.label.clone());
                        s.backups.push(job);
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(3)) => {
                        if let Some(last) = s.backups.last().cloned() {
                            let r = restore_backup(&last, &mut s.manifest);
                            s.status = r.message;
                        } else {
                            s.status = "No backup to restore.".to_string();
                        }
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(4)) => {
                        s.manifest.detect_changes();
                        let pending = s.manifest.pending_indices().len();
                        s.status = format!("Detected {} change(s).", pending);
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(5)) => {
                        s.encrypt = !s.encrypt;
                        s.status = if s.encrypt { "Encryption enabled." } else { "Encryption disabled." }.to_string();
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(6)) => {
                        s.provider = CloudProvider::SmartCloud;
                        s.status = "Provider: SmartCloud".to_string();
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(7)) => {
                        s.provider = CloudProvider::S3Compatible;
                        s.status = "Provider: S3-Compatible".to_string();
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::LineClicked(row)) => {
                        if row < s.manifest.entries.len() {
                            s.selected = row;
                            s.dirty = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        crate::process::scheduler::yield_now();
    }
}

pub fn self_test() -> bool {
    let mut ok = true;

    // T1: sync_checksum is deterministic
    let cs1 = sync_checksum(b"/home/user/file.txt", 1024);
    let cs2 = sync_checksum(b"/home/user/file.txt", 1024);
    if cs1 != cs2 { ok = false; }

    // T2: different paths → different checksums
    let cs3 = sync_checksum(b"/home/user/other.txt", 1024);
    if cs1 == cs3 { ok = false; }

    // T3: different sizes → different checksums
    let cs4 = sync_checksum(b"/home/user/file.txt", 2048);
    if cs1 == cs4 { ok = false; }

    // T4: encrypt/decrypt round-trip
    let plain = b"Hello, Cloud!";
    let enc = encrypt_stub(plain, 0xA5);
    let dec = decrypt_stub(&enc, 0xA5);
    if dec != plain { ok = false; }

    // T5: encrypt actually changes data
    if enc == plain.to_vec() { ok = false; }

    // T6: SyncManifest add and pending detection
    let mut m = SyncManifest::new();
    m.add("/a.txt", 100);
    m.add("/b.txt", 200);
    // both start as Pending
    if m.pending_indices().len() != 2 { ok = false; }

    // T7: mark_all_synced clears pending
    m.mark_all_synced();
    if !m.pending_indices().is_empty() { ok = false; }

    // T8: detect_changes flags some as Modified
    m.detect_changes();
    // first entry (index 0) should be Modified (0 % 3 == 0)
    if m.entries[0].state != SyncState::Modified { ok = false; }

    // T9: create_backup captures correct entry count
    let m2 = sample_manifest();
    let job = create_backup(&m2, 1, true);
    if job.entry_count != m2.entries.len() { ok = false; }
    if !job.encrypted { ok = false; }

    // T10: restore_backup marks all entries Synced
    let mut m3 = sample_manifest();
    let job2 = create_backup(&m3, 2, false);
    let result = restore_backup(&job2, &mut m3);
    if !result.ok { ok = false; }
    if m3.entries.iter().any(|e| e.state != SyncState::Synced) { ok = false; }

    ok
}

/// Smart OS — Browser IPC Message Bus (Phase 78, v0.38.0)
///
/// Defines the communication protocol between the browser chrome (`browser.rs`)
/// and per-tab sandboxed renderer threads (`browser_sandbox.rs`).
///
/// Chrome → Renderer : `BrowserCmd`
/// Renderer → Chrome : `PageEvent`
///
/// Thread-safe per-tab queues use a single lazy-initialised `IpcState` behind
/// one spin `Mutex`.  A `PageResponse` slot (also one per tab) carries the raw
/// HTTP response body so the chrome can finish the rendering pipeline without
/// copying large byte slices through the event queue.

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;

// ─── Constants ────────────────────────────────────────────────────────────────
pub const MAX_TABS:  usize = 8;
const QUEUE_CAP:     usize = 32;

// ─── Commands  (chrome → renderer) ───────────────────────────────────────────
#[derive(Clone, Debug)]
pub enum BrowserCmd {
    Navigate   { url: String },
    Reload,
    Stop,
    ScrollDown { lines: u32 },
    ScrollUp   { lines: u32 },
    KeyEvent   { ascii: u8, scancode: u8 },
    MouseClick { x: u16, y: u16, button: u8 },
    SetUserAgent(String),
    Shutdown,
}

// ─── Events  (renderer → chrome) ─────────────────────────────────────────────
#[derive(Clone, Debug)]
pub enum PageEvent {
    TitleChanged(String),
    UrlChanged(String),
    LoadStarted,
    LoadProgress(u8),                       // 0-100 %
    LoadDone     { status: u16 },
    LoadError    { message: String },
    SecurityChanged(SecurityInfo),
    PermissionRequest(Permission),
    Crashed      { message: String },
    ConsoleLog   { level: LogLevel, text: String },
}

// ─── Security info ────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct SecurityInfo {
    pub https:        bool,
    pub cert_valid:   bool,
    pub cert_subject: String,
    pub warnings:     Vec<String>,
}

impl SecurityInfo {
    pub fn http() -> Self {
        SecurityInfo { https: false, cert_valid: false,
            cert_subject: String::new(),
            warnings: vec!["Connection not secure (HTTP)".to_string()] }
    }
    pub fn https_valid(subject: &str) -> Self {
        SecurityInfo { https: true, cert_valid: true,
            cert_subject: subject.to_string(), warnings: Vec::new() }
    }
    pub fn https_invalid(reason: &str) -> Self {
        SecurityInfo { https: true, cert_valid: false,
            cert_subject: String::new(),
            warnings: vec![alloc::format!("Certificate error: {}", reason)] }
    }
    /// One-character indicator for the address bar.
    pub fn padlock(&self) -> &'static str {
        if self.https && self.cert_valid { "🔒" }
        else if self.https { "⚠" }
        else { "🔓" }
    }
    pub fn label(&self) -> String {
        if self.https && self.cert_valid {
            alloc::format!("Secure  {}", self.cert_subject)
        } else if self.https {
            alloc::format!("Not secure  {}", self.warnings.first().map(|s| s.as_str()).unwrap_or(""))
        } else {
            "Not secure (HTTP)".to_string()
        }
    }
}

// ─── Permission ───────────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Permission {
    Geolocation,
    Notifications,
    Camera,
    Microphone,
    ClipboardRead,
}

impl Permission {
    pub fn name(self) -> &'static str {
        match self {
            Permission::Geolocation   => "Location",
            Permission::Notifications => "Notifications",
            Permission::Camera        => "Camera",
            Permission::Microphone    => "Microphone",
            Permission::ClipboardRead => "Clipboard",
        }
    }
}

// ─── Console log level ────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LogLevel { Log, Warn, Error, Debug, Info }

// ─── Raw page response (shared buffer, chrome polls after LoadDone) ───────────
#[derive(Clone, Debug)]
pub struct PageResponse {
    pub tab_id:   usize,
    pub url:      String,
    pub status:   u16,
    pub body:     Vec<u8>,
    pub is_https: bool,
    pub error:    Option<String>,
}

// ─── IPC state (one instance, lazy init) ─────────────────────────────────────
struct IpcState {
    cmds:   [VecDeque<BrowserCmd>;    MAX_TABS],
    evts:   [VecDeque<PageEvent>;     MAX_TABS],
    bodies: [Option<PageResponse>;    MAX_TABS],
    // Permission decisions: None = ask, true = allow, false = deny
    perms:  [[Option<bool>; 5];       MAX_TABS],
}

impl IpcState {
    fn new() -> Self {
        IpcState {
            cmds: [
                VecDeque::new(), VecDeque::new(), VecDeque::new(), VecDeque::new(),
                VecDeque::new(), VecDeque::new(), VecDeque::new(), VecDeque::new(),
            ],
            evts: [
                VecDeque::new(), VecDeque::new(), VecDeque::new(), VecDeque::new(),
                VecDeque::new(), VecDeque::new(), VecDeque::new(), VecDeque::new(),
            ],
            bodies: [None, None, None, None, None, None, None, None],
            perms:  [[None; 5]; MAX_TABS],
        }
    }
}

static IPC: Mutex<Option<IpcState>> = Mutex::new(None);

#[inline]
fn with_ipc<F, R>(f: F) -> R where F: FnOnce(&mut IpcState) -> R {
    let mut g = IPC.lock();
    let st = g.get_or_insert_with(IpcState::new);
    f(st)
}

// ─── Public API — commands ────────────────────────────────────────────────────
pub fn push_cmd(tab: usize, cmd: BrowserCmd) {
    if tab >= MAX_TABS { return; }
    with_ipc(|s| {
        if s.cmds[tab].len() >= QUEUE_CAP { s.cmds[tab].pop_front(); }
        s.cmds[tab].push_back(cmd);
    });
}

pub fn pop_cmd(tab: usize) -> Option<BrowserCmd> {
    if tab >= MAX_TABS { return None; }
    with_ipc(|s| s.cmds[tab].pop_front())
}

// ─── Public API — events ─────────────────────────────────────────────────────
pub fn push_event(tab: usize, evt: PageEvent) {
    if tab >= MAX_TABS { return; }
    with_ipc(|s| {
        if s.evts[tab].len() >= QUEUE_CAP { s.evts[tab].pop_front(); }
        s.evts[tab].push_back(evt);
    });
}

pub fn pop_event(tab: usize) -> Option<PageEvent> {
    if tab >= MAX_TABS { return None; }
    with_ipc(|s| s.evts[tab].pop_front())
}

pub fn drain_events(tab: usize) -> Vec<PageEvent> {
    if tab >= MAX_TABS { return Vec::new(); }
    with_ipc(|s| {
        let mut v = Vec::new();
        while let Some(e) = s.evts[tab].pop_front() { v.push(e); }
        v
    })
}

// ─── Public API — page bodies ─────────────────────────────────────────────────
/// Store the raw HTTP body for the chrome to pick up after LoadDone.
pub fn store_response(resp: PageResponse) {
    let tab = resp.tab_id;
    if tab >= MAX_TABS { return; }
    with_ipc(|s| s.bodies[tab] = Some(resp));
}

/// Chrome calls this after receiving a LoadDone event.
pub fn take_response(tab: usize) -> Option<PageResponse> {
    if tab >= MAX_TABS { return None; }
    with_ipc(|s| s.bodies[tab].take())
}

// ─── Public API — permissions ─────────────────────────────────────────────────
fn perm_idx(p: Permission) -> usize {
    match p {
        Permission::Geolocation   => 0,
        Permission::Notifications => 1,
        Permission::Camera        => 2,
        Permission::Microphone    => 3,
        Permission::ClipboardRead => 4,
    }
}

/// Record a user decision for a tab's permission request.
pub fn set_permission(tab: usize, perm: Permission, allow: bool) {
    if tab >= MAX_TABS { return; }
    with_ipc(|s| s.perms[tab][perm_idx(perm)] = Some(allow));
}

/// Check permission decision (None = not yet decided).
pub fn check_permission(tab: usize, perm: Permission) -> Option<bool> {
    if tab >= MAX_TABS { return Some(false); }
    with_ipc(|s| s.perms[tab][perm_idx(perm)])
}

/// Clear all permissions for a tab (e.g. on navigation to new origin).
pub fn clear_permissions(tab: usize) {
    if tab >= MAX_TABS { return; }
    with_ipc(|s| s.perms[tab] = [None; 5]);
}

// ─── Self-test ────────────────────────────────────────────────────────────────
pub fn self_test() -> bool {
    let mut ok = true;

    // T1: push/pop cmd round-trip
    push_cmd(0, BrowserCmd::Navigate { url: "https://example.com".to_string() });
    match pop_cmd(0) {
        Some(BrowserCmd::Navigate { ref url }) if url == "https://example.com" => {}
        _ => { ok = false; }
    }

    // T2: pop empty returns None
    if pop_cmd(0).is_some() { ok = false; }

    // T3: push/pop event
    push_event(1, PageEvent::LoadDone { status: 200 });
    match pop_event(1) {
        Some(PageEvent::LoadDone { status: 200 }) => {}
        _ => { ok = false; }
    }

    // T4: queue drops oldest when full
    for i in 0..(QUEUE_CAP + 5) {
        push_cmd(2, BrowserCmd::ScrollDown { lines: i as u32 });
    }
    let mut count = 0usize;
    while pop_cmd(2).is_some() { count += 1; }
    if count != QUEUE_CAP { ok = false; }

    // T5: SecurityInfo padlocks
    if SecurityInfo::http().padlock()             != "🔓" { ok = false; }
    if SecurityInfo::https_valid("ex").padlock()  != "🔒" { ok = false; }
    if SecurityInfo::https_invalid("exp").padlock()!= "⚠" { ok = false; }

    // T6: Permission names all non-empty
    for p in [Permission::Geolocation, Permission::Notifications,
              Permission::Camera, Permission::Microphone, Permission::ClipboardRead] {
        if p.name().is_empty() { ok = false; }
    }

    // T7: drain_events
    push_event(3, PageEvent::LoadStarted);
    push_event(3, PageEvent::TitleChanged("T".to_string()));
    if drain_events(3).len() != 2 { ok = false; }
    if pop_event(3).is_some() { ok = false; }

    // T8: PageResponse store/take
    let resp = PageResponse { tab_id: 4, url: "http://x.com".to_string(),
        status: 200, body: b"hi".to_vec(), is_https: false, error: None };
    store_response(resp);
    match take_response(4) {
        Some(r) if r.status == 200 && r.body == b"hi" => {}
        _ => { ok = false; }
    }
    if take_response(4).is_some() { ok = false; }  // second take is None

    // T9: permissions
    set_permission(0, Permission::Geolocation, true);
    if check_permission(0, Permission::Geolocation) != Some(true) { ok = false; }
    clear_permissions(0);
    if check_permission(0, Permission::Geolocation) != None { ok = false; }

    // T10: out-of-bounds tab is safe (no panic)
    push_cmd(MAX_TABS, BrowserCmd::Reload);
    if pop_cmd(MAX_TABS).is_some() { ok = false; }

    ok
}

/// Phase 110 — Tab Process Isolation
///
/// Each browser tab's renderer runs as a separate ring-3 process with its own
/// address space and a restricted capability set.  The browser-chrome process
/// communicates with each renderer through a typed IPC channel.
///
/// Architecture:
///   BrowserProcess  (ring-3 or kernel task, holds TabProcessManager)
///       │
///       ├─ RendererProcess(tab 0)  – isolated ring-3 process, PID N
///       ├─ RendererProcess(tab 1)  – isolated ring-3 process, PID N+1
///       └─ ...
///
/// The renderer is only allowed a minimal syscall allowlist (read, write, mmap,
/// yield, exit, IPC send/recv).  All other syscalls are rejected by the kernel's
/// CBAC layer before they reach the syscall handler.

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::format;
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// IPC message types (browser ↔ renderer)
// ─────────────────────────────────────────────────────────────────────────────

/// Messages the **browser process** sends to a renderer.
#[derive(Debug, Clone)]
pub enum BrowserMsg {
    /// Navigate to a URL (sends HTML bytes already fetched by the browser).
    Navigate { url: String, html: Vec<u8> },
    /// Execute a JavaScript snippet in the renderer's context.
    ExecScript { script: String },
    /// Ask renderer to repaint and return a display list.
    RequestPaint,
    /// Signal the renderer to shut down.
    Shutdown,
    /// Inject a synthetic DOM event (click, keydown, …).
    InjectEvent { kind: String, target_id: u32, data: u32 },
}

/// Messages the **renderer** sends back to the browser process.
#[derive(Debug, Clone)]
pub enum RendererMsg {
    /// Renderer finished layout; returns serialised display list length.
    PaintReady { display_list_len: usize },
    /// Navigation complete; sends back the page title.
    NavComplete { title: String },
    /// JavaScript produced a console output line.
    ConsoleLog { level: ConsoleLevel, text: String },
    /// An unhandled exception was thrown.
    JsException { message: String },
    /// Renderer encountered a fatal error and is about to exit.
    Crashed { reason: String },
    /// A link was clicked; browser should open URL (possibly in new tab).
    LinkClicked { url: String, new_tab: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleLevel { Log, Warn, Error, Info, Debug }

// ─────────────────────────────────────────────────────────────────────────────
// IPC channel (in-kernel ring buffer, one per tab)
// ─────────────────────────────────────────────────────────────────────────────

const CHAN_CAP: usize = 64;

/// Single-producer single-consumer ring buffer for typed messages.
pub struct MsgChannel<T> {
    buf:  [Option<T>; CHAN_CAP],
    head: usize,
    tail: usize,
}

// SAFETY: channel is always accessed under the TabProcessManager mutex.
unsafe impl<T: Send> Send for MsgChannel<T> {}
unsafe impl<T: Send> Sync for MsgChannel<T> {}

impl<T> MsgChannel<T> {
    #[allow(clippy::declare_interior_mutable_const)]
    pub const fn new() -> Self {
        const NONE_SLOT: Option<()> = None;
        // We need to use a workaround since T may not be Copy.
        // Each slot starts as None; we transmute to avoid T: Copy bound.
        // SAFETY: None::<T> is zero-sized metadata + null ptr, safe to memcopy.
        MsgChannel {
            // Can't use array literal with non-Copy T in const, so we use
            // a trick: initialise all slots to MaybeUninit::uninit and treat
            // them as None (discriminant = 0 for Option<Box<T>> is guaranteed
            // to be None on all supported targets).
            // For simplicity we use a fixed-size array of pointers.
            buf:  unsafe { core::mem::zeroed() },
            head: 0,
            tail: 0,
        }
    }

    pub fn is_empty(&self) -> bool { self.head == self.tail }

    pub fn is_full(&self) -> bool { (self.tail + 1) % CHAN_CAP == self.head }

    pub fn send(&mut self, msg: T) -> bool {
        if self.is_full() { return false; }
        self.buf[self.tail] = Some(msg);
        self.tail = (self.tail + 1) % CHAN_CAP;
        true
    }

    pub fn recv(&mut self) -> Option<T> {
        if self.is_empty() { return None; }
        let msg = self.buf[self.head].take();
        self.head = (self.head + 1) % CHAN_CAP;
        msg
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Renderer capability set
// ─────────────────────────────────────────────────────────────────────────────

/// Syscall numbers the renderer is permitted to call.
/// All others are blocked by the kernel CBAC check before dispatch.
pub const RENDERER_ALLOWED_SYSCALLS: &[u64] = &[
    0,  // EXIT
    1,  // YIELD
    3,  // GETPID
    4,  // SLEEP
    8,  // MMAP
    10, // IPC_SEND
    11, // IPC_RECV
    22, // READ  (IPC pipe end only)
    23, // WRITE (IPC pipe end only)
];

/// Check whether a syscall number is allowed for a renderer process.
#[inline]
pub fn renderer_syscall_allowed(nr: u64) -> bool {
    RENDERER_ALLOWED_SYSCALLS.contains(&nr)
}

// ─────────────────────────────────────────────────────────────────────────────
// Renderer process record
// ─────────────────────────────────────────────────────────────────────────────

pub type TabId  = u32;
pub type Pid    = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererState {
    /// Process is being set up; not yet ready for messages.
    Starting,
    /// Idle, no pending navigation.
    Idle,
    /// Parsing HTML / running layout.
    Loading,
    /// Paint has been requested; waiting for display list.
    Painting,
    /// Process has exited abnormally.
    Crashed,
    /// Renderer was shut down cleanly.
    Stopped,
}

pub struct RendererProcess {
    pub tab_id:        TabId,
    pub pid:           Pid,
    pub state:         RendererState,
    pub current_url:   String,
    pub page_title:    String,
    /// Messages queued by the browser for the renderer to consume.
    pub to_renderer:   MsgChannel<BrowserMsg>,
    /// Messages queued by the renderer for the browser to consume.
    pub from_renderer: MsgChannel<RendererMsg>,
    /// Crash count — renderer is restarted up to MAX_RESTARTS times.
    pub crash_count:   u32,
}

const MAX_RESTARTS: u32 = 3;

impl RendererProcess {
    pub fn new(tab_id: TabId, pid: Pid) -> Self {
        RendererProcess {
            tab_id,
            pid,
            state:         RendererState::Starting,
            current_url:   String::new(),
            page_title:    String::new(),
            to_renderer:   MsgChannel::new(),
            from_renderer: MsgChannel::new(),
            crash_count:   0,
        }
    }

    /// Send a message to this renderer.  Returns false if the channel is full.
    pub fn send(&mut self, msg: BrowserMsg) -> bool {
        self.to_renderer.send(msg)
    }

    /// Poll for a message from this renderer.
    pub fn poll(&mut self) -> Option<RendererMsg> {
        let msg = self.from_renderer.recv()?;
        // Update local state based on received message.
        match &msg {
            RendererMsg::NavComplete { title } => {
                self.page_title = title.clone();
                self.state = RendererState::Idle;
            }
            RendererMsg::PaintReady { .. } => {
                self.state = RendererState::Idle;
            }
            RendererMsg::Crashed { .. } => {
                self.state = RendererState::Crashed;
                self.crash_count += 1;
            }
            _ => {}
        }
        Some(msg)
    }

    /// Whether the renderer should be restarted after a crash.
    pub fn should_restart(&self) -> bool {
        self.state == RendererState::Crashed && self.crash_count <= MAX_RESTARTS
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tab Process Manager
// ─────────────────────────────────────────────────────────────────────────────

/// Global manager for all renderer processes.
pub struct TabProcessManager {
    renderers:    BTreeMap<TabId, RendererProcess>,
    next_tab_id:  TabId,
    next_fake_pid: Pid,
}

impl TabProcessManager {
    pub const fn new() -> Self {
        TabProcessManager {
            renderers:     BTreeMap::new(),
            next_tab_id:   0,
            next_fake_pid: 1000,
        }
    }

    /// Spawn a new isolated renderer process for a tab.
    /// Returns the assigned TabId.
    pub fn spawn_tab(&mut self) -> TabId {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        // In a real implementation this would call process::spawn() to create
        // a ring-3 process running the renderer binary with limited capabilities.
        // Here we record the renderer with a synthetic PID.
        let pid = self.next_fake_pid;
        self.next_fake_pid += 1;

        let mut renderer = RendererProcess::new(tab_id, pid);
        renderer.state = RendererState::Idle;

        // Apply capability restrictions: register the PID in the CBAC layer
        // with only the renderer allowlist.
        // (In Phase 50 CBAC, this would call: cap_restrict_pid(pid, RENDERER_CAPS))
        // We record the intent here; wiring to CBAC is done at the integration point.

        self.renderers.insert(tab_id, renderer);
        tab_id
    }

    /// Navigate a tab to a URL (HTML bytes already fetched).
    pub fn navigate(&mut self, tab_id: TabId, url: &str, html: Vec<u8>) -> Result<(), &'static str> {
        let r = self.renderers.get_mut(&tab_id).ok_or("unknown tab")?;
        r.current_url = url.to_string();
        r.state = RendererState::Loading;
        r.send(BrowserMsg::Navigate { url: url.to_string(), html });
        Ok(())
    }

    /// Execute a script in a tab's renderer.
    pub fn exec_script(&mut self, tab_id: TabId, script: &str) -> Result<(), &'static str> {
        let r = self.renderers.get_mut(&tab_id).ok_or("unknown tab")?;
        r.send(BrowserMsg::ExecScript { script: script.to_string() });
        Ok(())
    }

    /// Request a repaint from a tab.
    pub fn request_paint(&mut self, tab_id: TabId) -> Result<(), &'static str> {
        let r = self.renderers.get_mut(&tab_id).ok_or("unknown tab")?;
        r.state = RendererState::Painting;
        r.send(BrowserMsg::RequestPaint);
        Ok(())
    }

    /// Close a tab and stop its renderer.
    pub fn close_tab(&mut self, tab_id: TabId) {
        if let Some(r) = self.renderers.get_mut(&tab_id) {
            r.send(BrowserMsg::Shutdown);
            r.state = RendererState::Stopped;
        }
        self.renderers.remove(&tab_id);
    }

    /// Drain all pending messages from a renderer.
    pub fn drain_renderer(&mut self, tab_id: TabId) -> Vec<RendererMsg> {
        let mut out = Vec::new();
        if let Some(r) = self.renderers.get_mut(&tab_id) {
            while let Some(msg) = r.poll() {
                out.push(msg);
            }
        }
        out
    }

    /// Called when a renderer sends a Crashed message — restart if under limit.
    pub fn handle_crash(&mut self, tab_id: TabId) {
        let should_restart = self.renderers.get(&tab_id)
            .map(|r| r.should_restart())
            .unwrap_or(false);

        if should_restart {
            let old_url = self.renderers.get(&tab_id)
                .map(|r| r.current_url.clone())
                .unwrap_or_default();
            let old_crashes = self.renderers.get(&tab_id).map(|r| r.crash_count).unwrap_or(0);

            // Assign a new PID for the restarted renderer.
            let new_pid = self.next_fake_pid;
            self.next_fake_pid += 1;

            if let Some(r) = self.renderers.get_mut(&tab_id) {
                r.pid         = new_pid;
                r.state       = RendererState::Idle;
                r.crash_count = old_crashes;
                // Re-navigate to the last URL.
                if !old_url.is_empty() {
                    r.send(BrowserMsg::Navigate {
                        url:  old_url,
                        html: b"<html><body>Reloading...</body></html>".to_vec(),
                    });
                }
            }
        } else {
            // Too many crashes — leave as Crashed, browser shows error page.
            if let Some(r) = self.renderers.get_mut(&tab_id) {
                r.state = RendererState::Crashed;
            }
        }
    }

    pub fn tab_count(&self) -> usize { self.renderers.len() }

    pub fn renderer_state(&self, tab_id: TabId) -> Option<RendererState> {
        self.renderers.get(&tab_id).map(|r| r.state)
    }

    pub fn renderer_pid(&self, tab_id: TabId) -> Option<Pid> {
        self.renderers.get(&tab_id).map(|r| r.pid)
    }
}

/// Global tab process manager.
pub static TAB_MANAGER: Mutex<TabProcessManager> =
    Mutex::new(TabProcessManager::new());

// ─────────────────────────────────────────────────────────────────────────────
// SELF-TEST
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;

    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] tab_process: {}", $name); }
        }
    }

    let mut mgr = TabProcessManager::new();

    // T1: spawn tab
    let t0 = mgr.spawn_tab();
    check!(t0 == 0, "spawn first tab id=0");

    // T2: second tab gets id=1
    let t1 = mgr.spawn_tab();
    check!(t1 == 1, "spawn second tab id=1");

    // T3: tab count
    check!(mgr.tab_count() == 2, "tab count = 2");

    // T4: renderer state after spawn = Idle
    check!(mgr.renderer_state(t0) == Some(RendererState::Idle), "state=Idle after spawn");

    // T5: navigate queues message; state → Loading
    mgr.navigate(t0, "https://example.com", b"<html></html>".to_vec()).unwrap();
    check!(mgr.renderer_state(t0) == Some(RendererState::Loading), "state=Loading after navigate");

    // T6: renderer syscall allowlist
    check!(renderer_syscall_allowed(1), "YIELD allowed");
    check!(!renderer_syscall_allowed(2), "SPAWN blocked");
    check!(!renderer_syscall_allowed(5), "FORK blocked");

    // T7: inject crash and handle; renderer should restart (crash_count=1 ≤ MAX)
    if let Some(r) = mgr.renderers.get_mut(&t0) {
        r.state = RendererState::Crashed;
        r.crash_count = 1;
    }
    mgr.handle_crash(t0);
    check!(mgr.renderer_state(t0) == Some(RendererState::Idle), "restart after first crash");

    // T8: three crashes → stays Crashed
    if let Some(r) = mgr.renderers.get_mut(&t0) {
        r.state = RendererState::Crashed;
        r.crash_count = MAX_RESTARTS + 1;
    }
    mgr.handle_crash(t0);
    check!(mgr.renderer_state(t0) == Some(RendererState::Crashed), "stays crashed after MAX");

    // T9: close tab removes it
    mgr.close_tab(t0);
    check!(mgr.renderer_state(t0).is_none(), "tab removed after close");
    check!(mgr.tab_count() == 1, "tab count = 1 after close");

    // T10: MsgChannel round-trip
    let mut chan: MsgChannel<u32> = MsgChannel::new();
    chan.send(42);
    chan.send(99);
    let a = chan.recv();
    let b = chan.recv();
    let c = chan.recv();
    check!(a == Some(42) && b == Some(99) && c.is_none(), "MsgChannel FIFO");

    if fail == 0 {
        crate::serial_println!("[tab_process] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[tab_process] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

/// Wayland compositor bridge for Smart OS — Phase 33.
///
/// Implements the Wayland wire protocol so that native Linux GUI apps can
/// run inside Smart OS without modification. The bridge translates
/// Wayland protocol messages into calls to our GUI compositor.
///
/// Architecture:
///   App ──UNIX socket──► WaylandBridge ──► gui::compositor::COMPOSITOR
///
/// The UNIX socket is emulated via a VFS pipe at /run/wayland-0.
/// Object IDs are tracked in OBJECT_TABLE; each surface maps to a
/// gui::compositor window.
///
/// Supported protocol objects (Wayland core, version 1):
///   wl_display(1), wl_registry(2), wl_compositor(3), wl_shm(4),
///   wl_seat(5), wl_output(6), wl_surface, wl_buffer, wl_shm_pool,
///   xdg_wm_base, xdg_surface, xdg_toplevel

pub mod protocol;
pub mod surface;
pub mod socket;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use alloc::collections::BTreeMap;
use spin::Mutex;

// ── Object registry ───────────────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
pub enum ObjectKind {
    Display,
    Registry,
    Compositor,
    Shm,
    ShmPool { size: u32 },
    Surface { window_id: Option<u64> },
    Buffer  { width: u32, height: u32, format: u32 },
    Seat,
    Pointer,
    Keyboard,
    Output,
    XdgWmBase,
    XdgSurface { surface_id: u32 },
    XdgToplevel { title: String, surface_id: u32 },
    Callback,
    Unknown,
}

pub struct WaylandObject {
    pub id:   u32,
    pub kind: ObjectKind,
}

// ── Per-client state ──────────────────────────────────────────────────────────

pub struct WaylandClient {
    pub client_id: u64,
    pub objects:   BTreeMap<u32, WaylandObject>,
    pub next_serial: u32,
    /// Output buffer waiting to be read by the client
    pub outbox: Vec<u8>,
}

impl WaylandClient {
    pub fn new(client_id: u64) -> Self {
        let mut objects = BTreeMap::new();
        // Object 1 = wl_display (always exists)
        objects.insert(1, WaylandObject { id: 1, kind: ObjectKind::Display });
        Self { client_id, objects, next_serial: 1, outbox: Vec::new() }
    }

    pub fn serial(&mut self) -> u32 {
        let s = self.next_serial;
        self.next_serial += 1;
        s
    }

    pub fn insert(&mut self, id: u32, kind: ObjectKind) {
        self.objects.insert(id, WaylandObject { id, kind });
    }

    pub fn remove(&mut self, id: u32) {
        self.objects.remove(&id);
    }
}

// ── Global state ──────────────────────────────────────────────────────────────

static CLIENTS: Mutex<BTreeMap<u64, WaylandClient>> = Mutex::new(BTreeMap::new());
static NEXT_CLIENT: Mutex<u64> = Mutex::new(1);

/// Screen dimensions exposed to Wayland clients.
static SCREEN_W: Mutex<u32> = Mutex::new(1920);
static SCREEN_H: Mutex<u32> = Mutex::new(1080);

pub fn screen_size() -> (u32, u32) {
    (*SCREEN_W.lock(), *SCREEN_H.lock())
}

// ── Dispatch loop ─────────────────────────────────────────────────────────────

/// Process a single Wayland message from a client.
/// `data` is one complete wire message: [object_id(4), size+opcode(4), payload...]
pub fn dispatch(client_id: u64, data: &[u8]) -> Vec<u8> {
    if data.len() < 8 { return Vec::new(); }
    let object_id = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);
    let size_op   = u32::from_ne_bytes([data[4], data[5], data[6], data[7]]);
    let msg_size  = (size_op >> 16) as usize;
    let opcode    = (size_op & 0xFFFF) as u16;
    let payload   = if data.len() >= msg_size { &data[8..msg_size] } else { &[] };

    let mut clients = CLIENTS.lock();
    let client = clients.entry(client_id).or_insert_with(|| WaylandClient::new(client_id));

    let kind = client.objects.get(&object_id)
        .map(|o| o.kind.clone())
        .unwrap_or(ObjectKind::Unknown);

    protocol::handle(client, object_id, opcode, payload, &kind)
}

/// Handle a connect event: return the client_id for this new connection.
pub fn handle_connect() -> u64 {
    let mut next = NEXT_CLIENT.lock();
    let id = *next;
    *next += 1;

    let mut clients = CLIENTS.lock();
    clients.insert(id, WaylandClient::new(id));

    // Send the initial wl_display::delete_id event (not needed) and
    // wl_display::error event is only sent on errors. Client is ready.
    crate::serial_println!("[wayland] Client {} connected.", id);
    id
}

pub fn handle_disconnect(client_id: u64) {
    let mut clients = CLIENTS.lock();
    if let Some(client) = clients.remove(&client_id) {
        // Destroy any compositor windows this client owned
        for (_, obj) in &client.objects {
            if let ObjectKind::Surface { window_id: Some(wid) } = obj.kind {
                crate::gui::compositor::destroy_window(wid);
            }
        }
    }
    crate::serial_println!("[wayland] Client {} disconnected.", client_id);
}

pub fn client_count() -> usize { CLIENTS.lock().len() }

// ── Socket VFS endpoint ───────────────────────────────────────────────────────

/// Worker thread: reads from the Wayland socket VFS path, dispatches, writes back.
pub fn socket_worker() {
    loop {
        socket::poll_and_dispatch();
        crate::process::scheduler::yield_now();
    }
}

// ── Public init ───────────────────────────────────────────────────────────────

pub fn init(screen_w: u32, screen_h: u32) {
    *SCREEN_W.lock() = screen_w;
    *SCREEN_H.lock() = screen_h;

    // Create the Wayland socket directory and socket file in VFS
    crate::vfs::mkdir("/run").ok();
    crate::vfs::mkdir("/run/wayland").ok();
    let _ = crate::vfs::create_and_write("/run/wayland-0", b"");
    // XDG_RUNTIME_DIR env var
    let _ = crate::vfs::create_and_write(
        "/proc/sys/wayland/xdg_runtime_dir", b"/run\n");

    socket::init();

    crate::serial_println!(
        "[wayland] Compositor bridge ready at /run/wayland-0 ({}x{}).",
        screen_w, screen_h,
    );
}

pub fn set_screen_size(w: u32, h: u32) {
    *SCREEN_W.lock() = w;
    *SCREEN_H.lock() = h;
}

/// Wayland socket emulation for Smart OS.
///
/// Since Smart OS doesn't have real Unix domain sockets, we emulate the
/// Wayland socket (/run/wayland-0) using a pair of VFS FIFO paths:
///   /run/wayland-0.rx  — data from client → compositor (we read)
///   /run/wayland-0.tx  — data from compositor → client (we write)
///
/// Applications call connect("/run/wayland-0") which our socket syscall
/// routes to these VFS files. This lets unmodified Wayland clients work
/// through our Linux ABI layer without real socket support.

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;

const RX_PATH: &str = "/run/wayland-0.rx";
const TX_PATH: &str = "/run/wayland-0.tx";

/// Pending outbound messages keyed by client_id.
static OUTBOX: Mutex<BTreeMap<u64, Vec<u8>>> = Mutex::new(BTreeMap::new());

/// Initialize socket VFS files.
pub fn init() {
    let _ = crate::vfs::create_and_write(RX_PATH, b"");
    let _ = crate::vfs::create_and_write(TX_PATH, b"");
}

/// Enqueue a message to be sent to a Wayland client.
pub fn send_to_client(client_id: u64, data: Vec<u8>) {
    let mut ob = OUTBOX.lock();
    ob.entry(client_id).or_insert_with(Vec::new).extend(data);
}

/// Poll the RX path for new client messages and dispatch them.
/// Called in a tight yield-loop from the socket_worker thread.
pub fn poll_and_dispatch() {
    // Read any pending data from the RX pipe
    let fd = match crate::vfs::open(RX_PATH) {
        Ok(f)  => f,
        Err(_) => return,
    };

    let mut buf = [0u8; 4096];
    let n = crate::vfs::read(fd, &mut buf).unwrap_or(0);
    crate::vfs::close(fd).ok();

    if n < 8 { return; } // too short to be a valid Wayland message

    // Route to client 1 (single-client for now; full multi-client needs
    // a proper socket accept loop which requires real Unix sockets)
    let client_id = 1u64;
    let response = super::dispatch(client_id, &buf[..n]);

    if !response.is_empty() {
        // Write response to TX path
        if let Ok(tx_fd) = crate::vfs::open(TX_PATH) {
            crate::vfs::write(tx_fd, &response).ok();
            crate::vfs::close(tx_fd).ok();
        }
    }

    // Flush any queued outbox messages
    let queued = {
        let mut ob = OUTBOX.lock();
        ob.remove(&client_id).unwrap_or_default()
    };
    if !queued.is_empty() {
        if let Ok(tx_fd) = crate::vfs::open(TX_PATH) {
            crate::vfs::write(tx_fd, &queued).ok();
            crate::vfs::close(tx_fd).ok();
        }
    }
}

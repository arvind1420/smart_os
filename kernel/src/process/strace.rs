/// Syscall Tracing (strace) for Smart OS.
///
/// Phase 10: Optional per-process syscall event logging.
/// Records syscall name, arguments, return value, and timestamp.
/// Useful for debugging applications running in user-space.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;
use crate::serial_println;
use crate::syscall::table;

/// Maximum trace entries per process.
const MAX_TRACE_ENTRIES: usize = 256;

/// A single syscall trace event.
#[derive(Debug, Clone)]
pub struct TraceEvent {
    /// Syscall number.
    pub syscall_nr: usize,
    /// Syscall name.
    pub name: &'static str,
    /// Arguments (up to 4).
    pub args: [u64; 4],
    /// Return value.
    pub ret: u64,
    /// Timestamp (PIT ticks).
    pub timestamp: u64,
    /// Thread ID.
    pub tid: u64,
}

/// Per-process trace buffer.
struct TraceBuffer {
    events: Vec<TraceEvent>,
    enabled: bool,
}

impl TraceBuffer {
    fn new() -> Self {
        Self {
            events: Vec::with_capacity(64),
            enabled: false,
        }
    }
}

/// Global trace state: pid → TraceBuffer.
static TRACE_BUFFERS: Mutex<BTreeMap<u64, TraceBuffer>> = Mutex::new(BTreeMap::new());

/// Global trace enable (trace all processes).
static GLOBAL_TRACE: AtomicBool = AtomicBool::new(false);

/// Total syscalls traced.
static TRACE_COUNT: AtomicU64 = AtomicU64::new(0);

/// Initialize the syscall tracing subsystem.
pub fn init() {
    serial_println!("[strace] Syscall tracing subsystem initialized.");
}

/// Enable tracing for a specific process.
pub fn enable_for_pid(pid: u64) {
    let mut buffers = TRACE_BUFFERS.lock();
    let buf = buffers.entry(pid).or_insert_with(TraceBuffer::new);
    buf.enabled = true;
    serial_println!("[strace] Tracing enabled for pid={}", pid);
}

/// Disable tracing for a specific process.
pub fn disable_for_pid(pid: u64) {
    let mut buffers = TRACE_BUFFERS.lock();
    if let Some(buf) = buffers.get_mut(&pid) {
        buf.enabled = false;
    }
}

/// Enable global tracing (all processes).
pub fn enable_global() {
    GLOBAL_TRACE.store(true, Ordering::Relaxed);
    serial_println!("[strace] Global syscall tracing enabled.");
}

/// Disable global tracing.
pub fn disable_global() {
    GLOBAL_TRACE.store(false, Ordering::Relaxed);
}

/// Check if tracing is active for a given pid.
fn is_tracing(pid: u64) -> bool {
    if GLOBAL_TRACE.load(Ordering::Relaxed) {
        return true;
    }
    TRACE_BUFFERS.lock()
        .get(&pid)
        .map(|b| b.enabled)
        .unwrap_or(false)
}

/// Record a syscall entry (called from syscall dispatcher).
pub fn record_syscall(
    pid: u64,
    tid: u64,
    syscall_nr: usize,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    ret: u64,
) {
    if !is_tracing(pid) {
        return;
    }

    let name = syscall_name(syscall_nr);
    let timestamp = crate::drivers::timer::uptime_ticks();

    let event = TraceEvent {
        syscall_nr,
        name,
        args: [arg1, arg2, arg3, arg4],
        ret,
        timestamp,
        tid,
    };

    let mut buffers = TRACE_BUFFERS.lock();
    let buf = buffers.entry(pid).or_insert_with(TraceBuffer::new);

    // Circular buffer: remove oldest if full
    if buf.events.len() >= MAX_TRACE_ENTRIES {
        buf.events.remove(0);
    }
    buf.events.push(event);

    TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Get the trace events for a process.
pub fn get_trace(pid: u64) -> Vec<TraceEvent> {
    TRACE_BUFFERS.lock()
        .get(&pid)
        .map(|b| b.events.clone())
        .unwrap_or_default()
}

/// Get the last N trace events for a process.
pub fn get_recent_trace(pid: u64, n: usize) -> Vec<TraceEvent> {
    TRACE_BUFFERS.lock()
        .get(&pid)
        .map(|b| {
            let len = b.events.len();
            let start = len.saturating_sub(n);
            b.events[start..].to_vec()
        })
        .unwrap_or_default()
}

/// Clear trace buffer for a process.
pub fn clear_trace(pid: u64) {
    if let Some(buf) = TRACE_BUFFERS.lock().get_mut(&pid) {
        buf.events.clear();
    }
}

/// Format a trace event as a human-readable string.
pub fn format_event(event: &TraceEvent) -> String {
    alloc::format!(
        "[{}] tid={} {}({:#X}, {:#X}, {:#X}, {:#X}) = {:#X}",
        event.timestamp,
        event.tid,
        event.name,
        event.args[0], event.args[1], event.args[2], event.args[3],
        event.ret,
    )
}

/// Get total traced syscall count.
pub fn trace_count() -> u64 {
    TRACE_COUNT.load(Ordering::Relaxed)
}

/// Is global tracing enabled?
pub fn is_global_enabled() -> bool {
    GLOBAL_TRACE.load(Ordering::Relaxed)
}

/// Syscall number to name mapping.
fn syscall_name(nr: usize) -> &'static str {
    match nr {
        table::SYS_EXIT => "exit",
        table::SYS_YIELD => "yield",
        table::SYS_SPAWN => "spawn",
        table::SYS_GETPID => "getpid",
        table::SYS_SLEEP => "sleep",
        table::SYS_FORK => "fork",
        table::SYS_EXEC => "exec",
        table::SYS_WAITPID => "waitpid",
        table::SYS_MMAP => "mmap",
        table::SYS_PIPE => "pipe",
        table::SYS_IPC_SEND => "ipc_send",
        table::SYS_IPC_RECV => "ipc_recv",
        table::SYS_IPC_CREATE_PORT => "ipc_create_port",
        table::SYS_IPC_LOOKUP_PORT => "ipc_lookup_port",
        table::SYS_OPEN => "open",
        table::SYS_CLOSE => "close",
        table::SYS_READ => "read",
        table::SYS_WRITE => "write",
        table::SYS_STAT => "stat",
        table::SYS_READDIR => "readdir",
        table::SYS_MKDIR => "mkdir",
        table::SYS_NET_SEND => "net_send",
        table::SYS_NET_RECV => "net_recv",
        table::SYS_NET_BIND => "net_bind",
        table::SYS_TCP_CONNECT => "tcp_connect",
        table::SYS_TCP_LISTEN => "tcp_listen",
        table::SYS_TCP_ACCEPT => "tcp_accept",
        table::SYS_TCP_SEND => "tcp_send",
        table::SYS_TCP_RECV => "tcp_recv",
        table::SYS_TCP_CLOSE => "tcp_close",
        table::SYS_TCP_STATUS => "tcp_status",
        table::SYS_KG_INSERT => "kg_insert",
        table::SYS_KG_QUERY => "kg_query",
        table::SYS_KG_LINK => "kg_link",
        table::SYS_KG_DELETE => "kg_delete",
        table::SYS_USB_LIST_DEVICES => "usb_list",
        table::SYS_USB_DEVICE_INFO => "usb_info",
        table::SYS_USB_READ => "usb_read",
        table::SYS_USB_WRITE => "usb_write",
        table::SYS_FAT32_MOUNT => "fat32_mount",
        table::SYS_FAT32_SYNC => "fat32_sync",
        _ => "unknown",
    }
}

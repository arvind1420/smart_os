/// Process namespace isolation for Smart OS.
///
/// Provides lightweight PID namespaces (each namespace has its own local PID view)
/// and mount namespaces (each process has a VFS root override).
///
/// This is a simplified implementation — no actual resource isolation between
/// namespace ID 0 (global) and others, but getpid() returns the namespace-local PID
/// and path resolution respects the per-process VFS root.

use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::string::String;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ── PID Namespace ─────────────────────────────────────────────────────────────

static NEXT_NS: AtomicU32 = AtomicU32::new(1);

/// Global namespace = 0. All processes start here.
pub const GLOBAL_NS: u32 = 0;

/// Per-namespace PID counter and PID mapping.
struct PidNs {
    next_local_pid: u32,
    global_to_local: BTreeMap<u64, u32>,
}

static PID_NAMESPACES: Mutex<BTreeMap<u32, PidNs>> = Mutex::new(BTreeMap::new());

/// Which namespace does each process live in?
static PROC_NS: Mutex<BTreeMap<u64, u32>> = Mutex::new(BTreeMap::new());

/// Create a new PID namespace. Returns its ID.
pub fn create_pid_namespace() -> u32 {
    let id = NEXT_NS.fetch_add(1, Ordering::Relaxed);
    PID_NAMESPACES.lock().insert(id, PidNs { next_local_pid: 1, global_to_local: BTreeMap::new() });
    id
}

/// Assign a process to a namespace (and give it a local PID).
pub fn assign_pid_namespace(global_pid: u64, ns_id: u32) {
    PROC_NS.lock().insert(global_pid, ns_id);
    let mut nss = PID_NAMESPACES.lock();
    let ns = nss.entry(ns_id).or_insert(PidNs { next_local_pid: 2, global_to_local: BTreeMap::new() });
    if !ns.global_to_local.contains_key(&global_pid) {
        let local = ns.next_local_pid;
        ns.next_local_pid += 1;
        ns.global_to_local.insert(global_pid, local);
    }
}

pub fn remove_from_namespace(global_pid: u64) {
    let ns_id = PROC_NS.lock().remove(&global_pid);
    if let Some(id) = ns_id {
        if let Some(ns) = PID_NAMESPACES.lock().get_mut(&id) {
            ns.global_to_local.remove(&global_pid);
        }
    }
}

/// Get the namespace-local PID for a process.
/// Falls back to the global PID if not in a non-global namespace.
pub fn local_pid(global_pid: u64) -> u64 {
    let ns_id = match PROC_NS.lock().get(&global_pid).copied() {
        Some(id) if id != GLOBAL_NS => id,
        _ => return global_pid,
    };
    PID_NAMESPACES.lock()
        .get(&ns_id)
        .and_then(|ns| ns.global_to_local.get(&global_pid).copied())
        .map(|p| p as u64)
        .unwrap_or(global_pid)
}

pub fn process_namespace(pid: u64) -> u32 {
    PROC_NS.lock().get(&pid).copied().unwrap_or(GLOBAL_NS)
}

// ── Mount Namespace ───────────────────────────────────────────────────────────

/// Per-process VFS root override (mount namespace). Defaults to "/".
static PROC_VFS_ROOT: Mutex<BTreeMap<u64, String>> = Mutex::new(BTreeMap::new());

pub fn set_vfs_root(pid: u64, root: &str) {
    PROC_VFS_ROOT.lock().insert(pid, String::from(root));
}

pub fn vfs_root(pid: u64) -> String {
    PROC_VFS_ROOT.lock().get(&pid).cloned().unwrap_or_else(|| String::from("/"))
}

pub fn remove_vfs_root(pid: u64) {
    PROC_VFS_ROOT.lock().remove(&pid);
}

/// Resolve a path relative to a process's VFS root.
pub fn resolve_path(pid: u64, path: &str) -> String {
    let root = vfs_root(pid);
    if root == "/" || path.starts_with('/') {
        // Path is already absolute — strip root prefix to avoid double root
        let root_trim = root.trim_end_matches('/');
        if root_trim.is_empty() { return path.to_owned(); }
        if path.starts_with(root_trim) { return path.to_owned(); }
        // Prepend root
        alloc::format!("{}{}", root_trim, path)
    } else {
        alloc::format!("{}/{}", root.trim_end_matches('/'), path)
    }
}

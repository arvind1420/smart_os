//! cgroup — Phase 107 System Hardening
//!
//! Linux-compatible cgroup v1/v2 style resource control groups for Smart OS.
//! Provides per-group memory limits, CPU shares, and PID counts.
//!
//! **Hierarchy**: a single flat list of named groups (no sub-hierarchies).
//!
//! # APIs
//! - `cgroup_create(name, mem_limit, cpu_shares, pid_limit)` → `CgroupId`
//! - `cgroup_destroy(id)`
//! - `cgroup_add_pid(id, pid)` / `cgroup_remove_pid(id, pid)`
//! - `cgroup_account_alloc(id, bytes)` → `Result<(), CgroupError>`
//! - `cgroup_account_free(id, bytes)`
//! - `cgroup_stat(id)` → `Option<CgroupStat>`

#![allow(dead_code)]

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::process::Pid;

// ─── Types ────────────────────────────────────────────────────────────────────

/// Opaque cgroup identifier.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct CgroupId(pub u64);

impl CgroupId {
    pub const ROOT: CgroupId = CgroupId(0);
}

static NEXT_CGID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq)]
pub enum CgroupError {
    /// Memory limit exceeded.
    OutOfMemory,
    /// PID count limit exceeded.
    PidLimitExceeded,
    /// No such cgroup.
    NotFound,
    /// Cannot destroy non-empty cgroup.
    NotEmpty,
}

/// Limits for a cgroup.
#[derive(Clone, Debug)]
pub struct CgroupLimits {
    /// Maximum memory in bytes (0 = unlimited).
    pub memory_limit:   u64,
    /// Maximum number of tasks (0 = unlimited).
    pub pid_limit:      u32,
    /// CPU weight (1–10000, relative; default = 1024).
    pub cpu_shares:     u32,
}

impl Default for CgroupLimits {
    fn default() -> Self {
        CgroupLimits { memory_limit: 0, pid_limit: 0, cpu_shares: 1024 }
    }
}

/// Live accounting for a cgroup.
#[derive(Clone, Debug)]
pub struct CgroupStat {
    pub name:           String,
    pub id:             CgroupId,
    pub limits:         CgroupLimits,
    /// Total bytes currently charged to this group.
    pub memory_used:    u64,
    /// High watermark of memory_used.
    pub memory_peak:    u64,
    /// Number of allocation failures (OOM events).
    pub oom_count:      u64,
    /// Number of PIDs currently in this group.
    pub pid_count:      u32,
}

struct Cgroup {
    name:         String,
    limits:       CgroupLimits,
    memory_used:  u64,
    memory_peak:  u64,
    oom_count:    u64,
    pids:         Vec<Pid>,
}

impl Cgroup {
    fn new(name: String, limits: CgroupLimits) -> Self {
        Cgroup {
            name,
            limits,
            memory_used: 0,
            memory_peak:  0,
            oom_count:    0,
            pids:         Vec::new(),
        }
    }
}

// ─── Global table ─────────────────────────────────────────────────────────────

static CGROUP_TABLE: Mutex<BTreeMap<CgroupId, Cgroup>> = Mutex::new(BTreeMap::new());

// ─── Public API ───────────────────────────────────────────────────────────────

/// Create a new cgroup and return its id.
pub fn cgroup_create(name: &str, limits: CgroupLimits) -> CgroupId {
    let id = CgroupId(NEXT_CGID.fetch_add(1, Ordering::Relaxed));
    CGROUP_TABLE.lock().insert(id, Cgroup::new(name.to_string(), limits));
    id
}

/// Destroy a cgroup.  Fails if any PIDs are still attached.
pub fn cgroup_destroy(id: CgroupId) -> Result<(), CgroupError> {
    let mut table = CGROUP_TABLE.lock();
    match table.get(&id) {
        None => return Err(CgroupError::NotFound),
        Some(g) if !g.pids.is_empty() => return Err(CgroupError::NotEmpty),
        _ => {}
    }
    table.remove(&id);
    Ok(())
}

/// Add `pid` to `id`'s task list. Enforces `pid_limit`.
pub fn cgroup_add_pid(id: CgroupId, pid: Pid) -> Result<(), CgroupError> {
    let mut table = CGROUP_TABLE.lock();
    let g = table.get_mut(&id).ok_or(CgroupError::NotFound)?;
    if g.limits.pid_limit > 0 && g.pids.len() >= g.limits.pid_limit as usize {
        return Err(CgroupError::PidLimitExceeded);
    }
    if !g.pids.contains(&pid) {
        g.pids.push(pid);
    }
    Ok(())
}

/// Remove `pid` from `id`'s task list.
pub fn cgroup_remove_pid(id: CgroupId, pid: Pid) {
    if let Some(g) = CGROUP_TABLE.lock().get_mut(&id) {
        g.pids.retain(|&p| p != pid);
    }
}

/// Charge `bytes` to `id`.  Returns `Err(OutOfMemory)` if the limit is exceeded.
pub fn cgroup_account_alloc(id: CgroupId, bytes: u64) -> Result<(), CgroupError> {
    let mut table = CGROUP_TABLE.lock();
    let g = table.get_mut(&id).ok_or(CgroupError::NotFound)?;
    let new_used = g.memory_used.saturating_add(bytes);
    if g.limits.memory_limit > 0 && new_used > g.limits.memory_limit {
        g.oom_count += 1;
        return Err(CgroupError::OutOfMemory);
    }
    g.memory_used = new_used;
    if g.memory_used > g.memory_peak {
        g.memory_peak = g.memory_used;
    }
    Ok(())
}

/// Release `bytes` previously charged to `id`.
pub fn cgroup_account_free(id: CgroupId, bytes: u64) {
    if let Some(g) = CGROUP_TABLE.lock().get_mut(&id) {
        g.memory_used = g.memory_used.saturating_sub(bytes);
    }
}

/// Read a snapshot of a cgroup's accounting.
pub fn cgroup_stat(id: CgroupId) -> Option<CgroupStat> {
    let table = CGROUP_TABLE.lock();
    let g = table.get(&id)?;
    Some(CgroupStat {
        name:        g.name.clone(),
        id,
        limits:      g.limits.clone(),
        memory_used: g.memory_used,
        memory_peak: g.memory_peak,
        oom_count:   g.oom_count,
        pid_count:   g.pids.len() as u32,
    })
}

/// Update limits for an existing cgroup.
pub fn cgroup_set_limits(id: CgroupId, limits: CgroupLimits) -> Result<(), CgroupError> {
    let mut table = CGROUP_TABLE.lock();
    let g = table.get_mut(&id).ok_or(CgroupError::NotFound)?;
    g.limits = limits;
    Ok(())
}

/// List all cgroup IDs.
pub fn cgroup_list() -> Vec<CgroupId> {
    CGROUP_TABLE.lock().keys().cloned().collect()
}

/// Find the cgroup a PID belongs to, if any.
pub fn cgroup_of_pid(pid: Pid) -> Option<CgroupId> {
    let table = CGROUP_TABLE.lock();
    for (&id, g) in table.iter() {
        if g.pids.contains(&pid) { return Some(id); }
    }
    None
}

/// Total memory charged across all cgroups (for system-wide accounting).
pub fn cgroup_total_charged() -> u64 {
    CGROUP_TABLE.lock().values().map(|g| g.memory_used).sum()
}

// ─── Init ─────────────────────────────────────────────────────────────────────

pub fn init() {
    // Create a root cgroup for kernel/init processes
    let root_id = CgroupId(0);
    CGROUP_TABLE.lock().insert(root_id, Cgroup::new(
        "root".to_string(),
        CgroupLimits { memory_limit: 0, pid_limit: 0, cpu_shares: 1024 },
    ));
    crate::serial_println!("[cgroup] Phase 107: cgroup resource control ready.");
}

// ─── Self-test ────────────────────────────────────────────────────────────────

pub fn self_test() {
    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! check {
        ($desc:expr, $val:expr) => {
            if $val { passed += 1; }
            else    { failed += 1; crate::serial_println!("[cgroup] FAIL: {}", $desc); }
        };
    }

    // T1: create + stat
    let limits = CgroupLimits { memory_limit: 1024 * 1024, pid_limit: 10, cpu_shares: 512 };
    let id = cgroup_create("test-web", limits.clone());
    let stat = cgroup_stat(id).unwrap();
    check!("name matches",           stat.name == "test-web");
    check!("memory_used = 0",        stat.memory_used == 0);
    check!("memory_limit set",       stat.limits.memory_limit == 1024 * 1024);
    check!("cpu_shares set",         stat.limits.cpu_shares == 512);

    // T2: add PIDs
    check!("add pid 1 ok", cgroup_add_pid(id, 1).is_ok());
    check!("add pid 2 ok", cgroup_add_pid(id, 2).is_ok());
    check!("pid_count = 2", cgroup_stat(id).unwrap().pid_count == 2);
    check!("cgroup_of_pid(1) = id", cgroup_of_pid(1) == Some(id));

    // T3: pid_limit enforced
    for i in 3..=10u64 { cgroup_add_pid(id, i).ok(); }
    check!("pid_limit: 11th add fails", cgroup_add_pid(id, 99).is_err());

    // T4: remove pid
    cgroup_remove_pid(id, 1);
    check!("pid removed", cgroup_of_pid(1).is_none());
    check!("pid_count decremented", cgroup_stat(id).unwrap().pid_count == 9);

    // T5: memory accounting
    check!("alloc 100KB ok",      cgroup_account_alloc(id, 100_000).is_ok());
    check!("memory_used = 100K",  cgroup_stat(id).unwrap().memory_used == 100_000);
    check!("alloc to limit ok",   cgroup_account_alloc(id, 900_000).is_ok());
    check!("memory at limit",     cgroup_stat(id).unwrap().memory_used == 1_000_000);

    // T6: OOM enforcement
    let oom = cgroup_account_alloc(id, 100_000);
    check!("OOM returns Err",     oom.is_err());
    check!("OOM error variant",   oom == Err(CgroupError::OutOfMemory));
    check!("oom_count = 1",       cgroup_stat(id).unwrap().oom_count == 1);

    // T7: free reduces usage
    cgroup_account_free(id, 500_000);
    check!("free reduces usage",  cgroup_stat(id).unwrap().memory_used == 500_000);
    check!("peak unchanged",      cgroup_stat(id).unwrap().memory_peak == 1_000_000);

    // T8: update limits
    let new_limits = CgroupLimits { memory_limit: 2 * 1024 * 1024, pid_limit: 0, cpu_shares: 1024 };
    check!("set_limits ok", cgroup_set_limits(id, new_limits).is_ok());
    check!("new limit applied", cgroup_stat(id).unwrap().limits.memory_limit == 2 * 1024 * 1024);

    // T9: destroy non-empty fails
    check!("destroy non-empty fails", cgroup_destroy(id).is_err());

    // Remove all pids first
    for i in 2..=10u64 { cgroup_remove_pid(id, i); }
    check!("destroy empty ok", cgroup_destroy(id).is_ok());
    check!("stat after destroy = None", cgroup_stat(id).is_none());

    // T10: invalid id
    let bad = CgroupId(99_999);
    check!("alloc on invalid id fails", cgroup_account_alloc(bad, 1).is_err());
    check!("add_pid on invalid id fails", cgroup_add_pid(bad, 1).is_err());
    check!("set_limits on invalid id fails", cgroup_set_limits(bad, CgroupLimits::default()).is_err());

    crate::serial_println!("[cgroup] self_test: {}/{} passed", passed, passed + failed);
}

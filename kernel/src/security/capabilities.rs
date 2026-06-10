/// Phase 50: Capability system for Smart OS.
///
/// Provides a unified capability-based access control (CBAC) layer:
///
///   • SmartOS capability tokens — fine-grained per-operation grants
///   • Per-process grant table — BTreeMap<Pid, CapGrants>
///   • cap_check(pid, cap)     — the single enforcement point
///   • vfs_cap_check(pid, path, op) — enforced at VFS open/read/write
///   • net_cap_check(pid, op, port) — enforced at TCP/UDP connect/bind
///   • syscall_cap_check(pid, nr)   — combines linux_caps + seccomp
///   • Audit logging on every denial
///
/// Builds on linux_caps.rs (bitmask) and seccomp.rs (syscall filter).

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
//  Smart OS capability token set
// ─────────────────────────────────────────────────────────────────────────────

/// A fine-grained capability token.  Each variant maps to one bit in a u64.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SmartCap {
    // ── VFS capabilities ──────────────────────────────────────────────────────
    /// Open or stat any path (incl. protected /system/* and /boot/*).
    VfsReadProtected  =  0,
    /// Write to any path (bypass read-only restrictions).
    VfsWrite          =  1,
    /// Create or remove VFS entries at any path.
    VfsMutate         =  2,
    /// Perform raw block-device I/O via /disk/*.
    VfsRawDisk        =  3,
    /// Mount / unmount filesystems (FAT32, NTFS, …).
    VfsMount          =  4,

    // ── Network capabilities ──────────────────────────────────────────────────
    /// Bind to a privileged port (< 1024).
    NetBindPrivileged =  8,
    /// Send/receive raw IP packets (raw sockets).
    NetRaw            =  9,
    /// Open outbound TCP connections.
    NetConnect        = 10,
    /// Send/receive UDP datagrams.
    NetUdp            = 11,
    /// Perform DNS queries.
    NetDns            = 12,
    /// Modify network interface configuration.
    NetAdmin          = 13,

    // ── System capabilities ───────────────────────────────────────────────────
    /// Execute another binary (exec syscall).
    SysExec           = 16,
    /// Fork a new process.
    SysFork           = 17,
    /// Kill any process (not just own children).
    SysKill           = 18,
    /// Adjust process scheduling parameters.
    SysNice           = 19,
    /// Load/unload kernel modules.
    SysModules        = 20,
    /// Read kernel audit log.
    SysAuditRead      = 21,
    /// Reboot or shutdown.
    SysBoot           = 22,
    /// Access MMAP with arbitrary permissions.
    SysMmap           = 23,

    // ── IPC capabilities ─────────────────────────────────────────────────────
    /// Create and own IPC ports.
    IpcPort           = 24,
    /// Send messages to arbitrary IPC ports.
    IpcSend           = 25,

    // ── Debugging / tracing ───────────────────────────────────────────────────
    /// Attach a debugger / strace to another process.
    DbgTrace          = 32,
    /// Read kernel performance counters.
    DbgPerf           = 33,
}

impl SmartCap {
    pub fn bit(self) -> u64 { 1u64 << (self as u8) }

    pub fn name(self) -> &'static str {
        match self {
            Self::VfsReadProtected  => "VfsReadProtected",
            Self::VfsWrite          => "VfsWrite",
            Self::VfsMutate         => "VfsMutate",
            Self::VfsRawDisk        => "VfsRawDisk",
            Self::VfsMount          => "VfsMount",
            Self::NetBindPrivileged => "NetBindPrivileged",
            Self::NetRaw            => "NetRaw",
            Self::NetConnect        => "NetConnect",
            Self::NetUdp            => "NetUdp",
            Self::NetDns            => "NetDns",
            Self::NetAdmin          => "NetAdmin",
            Self::SysExec           => "SysExec",
            Self::SysFork           => "SysFork",
            Self::SysKill           => "SysKill",
            Self::SysNice           => "SysNice",
            Self::SysModules        => "SysModules",
            Self::SysAuditRead      => "SysAuditRead",
            Self::SysBoot           => "SysBoot",
            Self::SysMmap           => "SysMmap",
            Self::IpcPort           => "IpcPort",
            Self::IpcSend           => "IpcSend",
            Self::DbgTrace          => "DbgTrace",
            Self::DbgPerf           => "DbgPerf",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Per-process capability grants
// ─────────────────────────────────────────────────────────────────────────────

/// Capability grant set for one process.
#[derive(Clone, Default)]
pub struct CapGrants {
    /// Bitmask of granted SmartCap tokens.
    pub granted: u64,
    /// If true, all caps are inherited by child processes on fork.
    pub inheritable: bool,
    /// Process label (for audit).
    pub label: String,
}

impl CapGrants {
    /// All capabilities — kernel / root processes.
    pub fn all() -> Self {
        Self { granted: u64::MAX, inheritable: true, label: String::from("root") }
    }

    /// Standard user process: networking (no raw/admin), exec/fork, IPC.
    pub fn user_default() -> Self {
        use SmartCap::*;
        let mask = NetConnect.bit() | NetUdp.bit() | NetDns.bit()
                 | SysExec.bit()   | SysFork.bit()  | SysNice.bit()
                 | IpcSend.bit()   | SysMmap.bit();
        Self { granted: mask, inheritable: false, label: String::from("user") }
    }

    /// Sandboxed process — minimal rights.
    pub fn sandboxed() -> Self {
        Self { granted: 0, inheritable: false, label: String::from("sandbox") }
    }

    pub fn has(&self, cap: SmartCap) -> bool {
        self.granted & cap.bit() != 0
    }

    pub fn grant(&mut self, cap: SmartCap) {
        self.granted |= cap.bit();
    }

    pub fn revoke(&mut self, cap: SmartCap) {
        self.granted &= !cap.bit();
    }

    pub fn grant_all(&mut self) { self.granted = u64::MAX; }
    pub fn revoke_all(&mut self) { self.granted = 0; }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Global process capability table
// ─────────────────────────────────────────────────────────────────────────────

static GRANTS: Mutex<BTreeMap<u64, CapGrants>> = Mutex::new(BTreeMap::new());

/// Assign capability grants to a process (call at spawn).
pub fn assign(pid: u64, grants: CapGrants) {
    GRANTS.lock().insert(pid, grants);
}

/// Assign default user-space capability grants to a process.
pub fn assign_default(pid: u64) {
    GRANTS.lock().insert(pid, CapGrants::user_default());
}

/// Remove capability tracking when a process exits.
pub fn remove(pid: u64) {
    GRANTS.lock().remove(&pid);
}

/// Grant a single capability to a process.
/// Requires the caller to have `SmartCap::SysModules` or pid == 0 (kernel).
pub fn grant(caller_pid: u64, target_pid: u64, cap: SmartCap) -> Result<(), &'static str> {
    if caller_pid != 0 && !cap_check_raw(caller_pid, SmartCap::SysModules) {
        audit_deny(caller_pid, cap, "grant-requires-SysModules");
        return Err("EPERM");
    }
    let mut table = GRANTS.lock();
    table.entry(target_pid).or_insert_with(CapGrants::user_default).grant(cap);
    crate::serial_println!("[caps] grant pid={} cap={}", target_pid, cap.name());
    Ok(())
}

/// Revoke a single capability from a process.
pub fn revoke(caller_pid: u64, target_pid: u64, cap: SmartCap) -> Result<(), &'static str> {
    if caller_pid != 0 && !cap_check_raw(caller_pid, SmartCap::SysModules) {
        audit_deny(caller_pid, cap, "revoke-requires-SysModules");
        return Err("EPERM");
    }
    let mut table = GRANTS.lock();
    if let Some(g) = table.get_mut(&target_pid) {
        g.revoke(cap);
    }
    crate::serial_println!("[caps] revoke pid={} cap={}", target_pid, cap.name());
    Ok(())
}

/// Read the capability grants for a process (for display / audit).
pub fn get_grants(pid: u64) -> CapGrants {
    if pid == 0 { return CapGrants::all(); }
    GRANTS.lock().get(&pid).cloned().unwrap_or_else(CapGrants::user_default)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Core enforcement: cap_check
// ─────────────────────────────────────────────────────────────────────────────

/// Internal check — no audit emission.
#[inline]
fn cap_check_raw(pid: u64, cap: SmartCap) -> bool {
    if pid == 0 { return true; }
    GRANTS.lock().get(&pid).map(|g| g.has(cap)).unwrap_or(true)
}

/// Check if `pid` holds `cap`. Returns `Ok(())` or `Err("EPERM")`.
/// Logs a denial event to the audit log on failure.
pub fn cap_check(pid: u64, cap: SmartCap) -> Result<(), &'static str> {
    if pid == 0 { return Ok(()); }
    let granted = GRANTS.lock().get(&pid).map(|g| g.has(cap)).unwrap_or(true);
    if granted {
        Ok(())
    } else {
        audit_deny(pid, cap, "cap_check");
        Err("EPERM: capability denied")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  VFS integration
// ─────────────────────────────────────────────────────────────────────────────

/// VFS operation type.
#[derive(Clone, Copy, Debug)]
pub enum VfsOp { Read, Write, Create, Delete, Mount }

/// Protected path prefixes — require elevated capabilities to write.
const PROTECTED_PREFIXES: &[&str] = &[
    "/system/", "/boot/", "/kernel/", "/etc/passwd", "/etc/shadow",
];

/// Called at VFS open (and for write operations).
/// Returns `Ok(())` if the operation is permitted.
pub fn vfs_cap_check(pid: u64, path: &str, op: VfsOp) -> Result<(), &'static str> {
    if pid == 0 { return Ok(()); }

    match op {
        VfsOp::Read => {
            // Protected paths need VfsReadProtected.
            if PROTECTED_PREFIXES.iter().any(|pfx| path.starts_with(pfx)) {
                return cap_check(pid, SmartCap::VfsReadProtected);
            }
            Ok(())
        }
        VfsOp::Write => {
            if PROTECTED_PREFIXES.iter().any(|pfx| path.starts_with(pfx)) {
                cap_check(pid, SmartCap::VfsReadProtected)?;
            }
            cap_check(pid, SmartCap::VfsWrite)
        }
        VfsOp::Create | VfsOp::Delete => {
            cap_check(pid, SmartCap::VfsMutate)
        }
        VfsOp::Mount => {
            cap_check(pid, SmartCap::VfsMount)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Network integration
// ─────────────────────────────────────────────────────────────────────────────

/// Network operation type.
#[derive(Clone, Copy, Debug)]
pub enum NetOp { TcpConnect, UdpSend, UdpBind, Dns, RawSend, AdminConfig }

/// Called before performing a network operation.
pub fn net_cap_check(pid: u64, op: NetOp, port: u16) -> Result<(), &'static str> {
    if pid == 0 { return Ok(()); }

    match op {
        NetOp::TcpConnect => {
            cap_check(pid, SmartCap::NetConnect)
        }
        NetOp::UdpSend => {
            cap_check(pid, SmartCap::NetUdp)
        }
        NetOp::UdpBind => {
            if port < 1024 {
                cap_check(pid, SmartCap::NetBindPrivileged)?;
            }
            cap_check(pid, SmartCap::NetUdp)
        }
        NetOp::Dns => {
            cap_check(pid, SmartCap::NetDns)
        }
        NetOp::RawSend => {
            cap_check(pid, SmartCap::NetRaw)
        }
        NetOp::AdminConfig => {
            cap_check(pid, SmartCap::NetAdmin)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Syscall allowlist integration
// ─────────────────────────────────────────────────────────────────────────────

/// Syscall numbers for Smart OS (matching the syscall table in memory).
pub mod syscall_nr {
    pub const EXIT:        u64 = 0;
    pub const YIELD:       u64 = 1;
    pub const SPAWN:       u64 = 2;
    pub const GETPID:      u64 = 3;
    pub const SLEEP:       u64 = 4;
    pub const FORK:        u64 = 5;
    pub const EXEC:        u64 = 6;
    pub const WAITPID:     u64 = 7;
    pub const MMAP:        u64 = 8;
    pub const PIPE:        u64 = 9;
    pub const IPC_SEND:    u64 = 10;
    pub const IPC_RECV:    u64 = 11;
    pub const CREATE_PORT: u64 = 12;
    pub const LOOKUP_PORT: u64 = 13;
    pub const OPEN:        u64 = 20;
    pub const CLOSE:       u64 = 21;
    pub const READ:        u64 = 22;
    pub const WRITE:       u64 = 23;
    pub const STAT:        u64 = 24;
    pub const READDIR:     u64 = 25;
    pub const MKDIR:       u64 = 26;
    pub const NET_SEND:    u64 = 30;
    pub const NET_RECV:    u64 = 31;
    pub const NET_BIND:    u64 = 32;
    pub const TCP_CONNECT: u64 = 33;
    pub const TCP_LISTEN:  u64 = 34;
    pub const TCP_ACCEPT:  u64 = 35;
    pub const TCP_SEND:    u64 = 36;
    pub const TCP_RECV:    u64 = 37;
    pub const TCP_CLOSE:   u64 = 38;
    pub const TCP_STATUS:  u64 = 39;
    pub const KILL:        u64 = 41;
    pub const USB_LIST:    u64 = 50;
    pub const USB_READ:    u64 = 52;
    pub const USB_WRITE:   u64 = 53;
    pub const DUP2:        u64 = 56;
}

/// Which SmartCap (if any) is required to execute a given syscall number.
/// Returns `None` if the syscall is always allowed (exit, yield, getpid, …).
pub fn required_cap_for_syscall(nr: u64) -> Option<SmartCap> {
    use syscall_nr::*;
    match nr {
        // Always allowed — no capability required.
        EXIT | YIELD | GETPID | SLEEP | CLOSE | READ | WRITE | STAT
        | READDIR | DUP2 | IPC_RECV | LOOKUP_PORT | TCP_STATUS | TCP_RECV
        | TCP_CLOSE => None,

        // Execution / process management.
        FORK           => Some(SmartCap::SysFork),
        EXEC | SPAWN   => Some(SmartCap::SysExec),
        WAITPID        => None,
        KILL           => Some(SmartCap::SysKill),
        MMAP           => Some(SmartCap::SysMmap),
        PIPE           => None,

        // VFS write-side.
        OPEN | MKDIR   => None, // fine-grained check done inside vfs_cap_check
        NET_SEND | NET_RECV => Some(SmartCap::NetUdp),
        NET_BIND       => Some(SmartCap::NetUdp),
        TCP_CONNECT    => Some(SmartCap::NetConnect),
        TCP_LISTEN | TCP_ACCEPT => Some(SmartCap::NetConnect),
        TCP_SEND       => Some(SmartCap::NetConnect),

        // IPC.
        IPC_SEND | CREATE_PORT => Some(SmartCap::IpcSend),

        // USB (raw device access).
        USB_READ | USB_WRITE | USB_LIST => Some(SmartCap::VfsRawDisk),

        // Everything else: be conservative — require SysModules.
        _ => Some(SmartCap::SysModules),
    }
}

/// Full syscall check: seccomp filter first, then capability.
/// Returns `Ok(())` if the syscall is allowed, `Err(reason)` to block it.
pub fn syscall_cap_check(pid: u64, nr: u64) -> Result<(), &'static str> {
    if pid == 0 { return Ok(()); }

    // 1. Seccomp filter (mode-based allowlist).
    if !crate::security::seccomp::check(pid, nr) {
        return Err("EPERM: seccomp blocked");
    }

    // 2. Capability token check.
    if let Some(cap) = required_cap_for_syscall(nr) {
        cap_check(pid, cap)?;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
//  Capability inheritance on fork
// ─────────────────────────────────────────────────────────────────────────────

/// Called after fork: inherit capabilities from parent to child (if inheritable).
pub fn inherit_on_fork(parent_pid: u64, child_pid: u64) {
    let parent_grants = GRANTS.lock().get(&parent_pid).cloned();
    if let Some(g) = parent_grants {
        let child_grants = if g.inheritable {
            CapGrants {
                granted: g.granted,
                inheritable: g.inheritable,
                label: alloc::format!("{}:fork:{}", g.label, child_pid),
            }
        } else {
            // Non-inheritable: child gets default user caps only.
            CapGrants {
                granted: CapGrants::user_default().granted,
                inheritable: false,
                label: alloc::format!("fork:{}", child_pid),
            }
        };
        GRANTS.lock().insert(child_pid, child_grants);
    } else {
        assign_default(child_pid);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Audit helpers
// ─────────────────────────────────────────────────────────────────────────────

fn audit_deny(pid: u64, cap: SmartCap, context: &str) {
    let msg = alloc::format!("CAP_DENY pid={} cap={} ctx={}", pid, cap.name(), context);
    crate::security::audit::log(msg);
}

// ─────────────────────────────────────────────────────────────────────────────
//  Grant listing (for `caps` terminal command)
// ─────────────────────────────────────────────────────────────────────────────

/// Return a list of all active capability grants (pid, label, bit count).
pub fn list_grants() -> Vec<(u64, String, u32)> {
    GRANTS.lock()
        .iter()
        .map(|(pid, g)| (*pid, g.label.clone(), g.granted.count_ones()))
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    // Kernel process (pid=0) always has all caps — no entry needed.
    // Assign full caps to init process (pid=1).
    assign(1, CapGrants::all());
    crate::serial_println!(
        "[caps] Capability system initialised — {} SmartCap tokens, VFS+net enforcement active.",
        core::mem::size_of::<SmartCap>() * 8  // token count implied by bit width
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  Self-test
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // ── Test 1: Sandboxed process is denied NetConnect ────────────────────────
    assign(9001, CapGrants::sandboxed());
    let r = cap_check(9001, SmartCap::NetConnect);
    if r.is_ok() {
        crate::serial_println!("[caps-test] FAIL: sandboxed pid should not have NetConnect");
        ok = false;
    }

    // ── Test 2: User-default process has NetConnect ───────────────────────────
    assign(9002, CapGrants::user_default());
    let r = cap_check(9002, SmartCap::NetConnect);
    if r.is_err() {
        crate::serial_println!("[caps-test] FAIL: user-default pid should have NetConnect");
        ok = false;
    }

    // ── Test 3: VFS protected path denied for sandboxed ──────────────────────
    assign(9003, CapGrants::sandboxed());
    let r = vfs_cap_check(9003, "/system/version", VfsOp::Read);
    if r.is_ok() {
        crate::serial_println!("[caps-test] FAIL: sandboxed pid should not read /system/*");
        ok = false;
    }

    // ── Test 4: Regular path readable by sandboxed ───────────────────────────
    assign(9004, CapGrants::sandboxed());
    let r = vfs_cap_check(9004, "/home/user/notes.txt", VfsOp::Read);
    if r.is_err() {
        crate::serial_println!("[caps-test] FAIL: sandboxed pid should read /home/*");
        ok = false;
    }

    // ── Test 5: Net privileged port denied for user ───────────────────────────
    assign(9005, CapGrants::user_default());
    let r = net_cap_check(9005, NetOp::UdpBind, 80);
    if r.is_ok() {
        crate::serial_println!("[caps-test] FAIL: user pid should not bind port 80");
        ok = false;
    }

    // ── Test 6: Net high port allowed for user ────────────────────────────────
    assign(9006, CapGrants::user_default());
    let r = net_cap_check(9006, NetOp::UdpBind, 8080);
    if r.is_err() {
        crate::serial_println!("[caps-test] FAIL: user pid should bind port 8080");
        ok = false;
    }

    // ── Test 7: Syscall allowlist — FORK denied for sandboxed ────────────────
    assign(9007, CapGrants::sandboxed());
    let r = syscall_cap_check(9007, syscall_nr::FORK);
    if r.is_ok() {
        crate::serial_println!("[caps-test] FAIL: sandboxed pid should not fork");
        ok = false;
    }

    // ── Test 8: Syscall allowlist — EXIT always allowed ──────────────────────
    assign(9008, CapGrants::sandboxed());
    let r = syscall_cap_check(9008, syscall_nr::EXIT);
    if r.is_err() {
        crate::serial_println!("[caps-test] FAIL: exit should always be allowed");
        ok = false;
    }

    // ── Test 9: Fork inheritance — inheritable=false drops to user defaults ───
    assign(9009, CapGrants::sandboxed());
    inherit_on_fork(9009, 9010);
    // Child of sandboxed process gets user_default (inheritable=false in sandboxed)
    let r = cap_check(9010, SmartCap::NetConnect);
    if r.is_err() {
        crate::serial_println!("[caps-test] FAIL: child of sandboxed should get user-default caps");
        ok = false;
    }

    // ── Test 10: grant/revoke round-trip ──────────────────────────────────────
    assign(9011, CapGrants::user_default());
    let _ = grant(0, 9011, SmartCap::NetRaw);
    let r = cap_check(9011, SmartCap::NetRaw);
    if r.is_err() {
        crate::serial_println!("[caps-test] FAIL: grant NetRaw should stick");
        ok = false;
    }
    let _ = revoke(0, 9011, SmartCap::NetRaw);
    let r = cap_check(9011, SmartCap::NetRaw);
    if r.is_ok() {
        crate::serial_println!("[caps-test] FAIL: revoke NetRaw should remove it");
        ok = false;
    }

    // ── Cleanup ───────────────────────────────────────────────────────────────
    for pid in 9001..=9011u64 { remove(pid); }

    if ok {
        crate::serial_println!("[caps-test] All 10 capability tests PASSED");
    }
    ok
}

/// Linux-compatible capability system for Smart OS.
///
/// Implements the 40 POSIX.1e / Linux capabilities as a 64-bit bitmask.
/// Each process has effective/permitted/inheritable sets stored in CAPSETS.
/// capget(125) / capset(126) syscalls read and write these sets.

use alloc::collections::BTreeMap;
use spin::Mutex;

// ── Linux capability constants ─────────────────────────────────────────────────

pub const CAP_CHOWN:            u8 =  0;
pub const CAP_DAC_OVERRIDE:     u8 =  1;
pub const CAP_DAC_READ_SEARCH:  u8 =  2;
pub const CAP_FOWNER:           u8 =  3;
pub const CAP_FSETID:           u8 =  4;
pub const CAP_KILL:             u8 =  5;
pub const CAP_SETGID:           u8 =  6;
pub const CAP_SETUID:           u8 =  7;
pub const CAP_SETPCAP:          u8 =  8;
pub const CAP_NET_BIND_SERVICE: u8 = 10;
pub const CAP_NET_BROADCAST:    u8 = 11;
pub const CAP_NET_ADMIN:        u8 = 12;
pub const CAP_NET_RAW:          u8 = 13;
pub const CAP_SYS_RAWIO:        u8 = 17;
pub const CAP_SYS_CHROOT:       u8 = 18;
pub const CAP_SYS_PTRACE:       u8 = 19;
pub const CAP_SYS_ADMIN:        u8 = 21;
pub const CAP_SYS_BOOT:         u8 = 22;
pub const CAP_SYS_NICE:         u8 = 23;
pub const CAP_SYS_RESOURCE:     u8 = 24;
pub const CAP_SYS_TIME:         u8 = 25;
pub const CAP_MKNOD:            u8 = 27;
pub const CAP_AUDIT_WRITE:      u8 = 29;
pub const CAP_AUDIT_CONTROL:    u8 = 30;
pub const CAP_SETFCAP:          u8 = 31;
pub const CAP_MAC_OVERRIDE:     u8 = 32;
pub const CAP_SYSLOG:           u8 = 34;
pub const CAP_WAKE_ALARM:       u8 = 35;
pub const CAP_BLOCK_SUSPEND:    u8 = 36;
pub const CAP_AUDIT_READ:       u8 = 37;
pub const CAP_PERFMON:          u8 = 38;
pub const CAP_BPF:              u8 = 39;

/// A process capability set (Linux-compatible 3-tuple).
#[derive(Clone, Copy, Default)]
pub struct CapSet {
    pub effective:   u64,
    pub permitted:   u64,
    pub inheritable: u64,
}

impl CapSet {
    /// Full capabilities (root).
    pub const fn all() -> Self {
        Self { effective: u64::MAX, permitted: u64::MAX, inheritable: 0 }
    }
    /// No capabilities (sandboxed / untrusted process).
    pub const fn none() -> Self {
        Self { effective: 0, permitted: 0, inheritable: 0 }
    }
    /// Default for a non-root user: most caps dropped, keep a few benign ones.
    pub fn non_root_default() -> Self {
        let keep = cap_bit(CAP_NET_BIND_SERVICE) | cap_bit(CAP_SYS_NICE);
        Self { effective: keep, permitted: keep, inheritable: 0 }
    }
    pub fn has(&self, cap: u8) -> bool {
        self.effective & cap_bit(cap) != 0
    }
}

pub fn cap_bit(cap: u8) -> u64 { 1u64 << (cap as u64) }

// ── Per-process capability table ──────────────────────────────────────────────

static CAPSETS: Mutex<BTreeMap<u64, CapSet>> = Mutex::new(BTreeMap::new());

/// Assign a capability set to a process (called at spawn).
pub fn assign(pid: u64, caps: CapSet) {
    CAPSETS.lock().insert(pid, caps);
}

/// Remove capability tracking when a process exits.
pub fn remove(pid: u64) {
    CAPSETS.lock().remove(&pid);
}

/// Check if a process has a specific capability.
pub fn has_cap(pid: u64, cap: u8) -> bool {
    // pid=0 (kernel) always has all caps
    if pid == 0 { return true; }
    CAPSETS.lock().get(&pid).map(|cs| cs.has(cap)).unwrap_or(true)
}

/// Read a process's capability set (capget syscall).
pub fn capget(pid: u64) -> CapSet {
    if pid == 0 { return CapSet::all(); }
    CAPSETS.lock().get(&pid).copied().unwrap_or(CapSet::all())
}

/// Write a process's capability set (capset syscall).
/// Only allowed if the caller has CAP_SETPCAP or is root.
pub fn capset(target_pid: u64, caller_pid: u64, caps: CapSet) -> Result<(), &'static str> {
    if caller_pid != 0 && !has_cap(caller_pid, CAP_SETPCAP) {
        crate::security::audit::log_cap_denied(caller_pid, CAP_SETPCAP);
        return Err("EPERM: need CAP_SETPCAP");
    }
    CAPSETS.lock().insert(target_pid, caps);
    Ok(())
}

/// Drop caps for non-root processes (called after exec when uid != 0).
pub fn drop_for_non_root(pid: u64, uid: u32) {
    if uid != 0 {
        let mut table = CAPSETS.lock();
        let cs = table.entry(pid).or_insert(CapSet::all());
        *cs = CapSet::non_root_default();
    }
}

pub fn name(cap: u8) -> &'static str {
    match cap {
        CAP_CHOWN           => "CAP_CHOWN",
        CAP_DAC_OVERRIDE    => "CAP_DAC_OVERRIDE",
        CAP_KILL            => "CAP_KILL",
        CAP_NET_RAW         => "CAP_NET_RAW",
        CAP_NET_ADMIN       => "CAP_NET_ADMIN",
        CAP_SYS_ADMIN       => "CAP_SYS_ADMIN",
        CAP_SYS_BOOT        => "CAP_SYS_BOOT",
        CAP_SETUID          => "CAP_SETUID",
        CAP_SETGID          => "CAP_SETGID",
        CAP_SETPCAP         => "CAP_SETPCAP",
        CAP_AUDIT_WRITE     => "CAP_AUDIT_WRITE",
        CAP_SYSLOG          => "CAP_SYSLOG",
        _                   => "CAP_UNKNOWN",
    }
}

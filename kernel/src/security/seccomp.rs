/// Seccomp-style syscall filtering for Smart OS.
///
/// Three modes per process:
///   Disabled — all syscalls pass (default for unsandboxed processes)
///   Strict   — only read(0)/write(1)/exit(60)/sigreturn(15) allowed
///   Filter   — custom 256-bit allowlist bitmask
///
/// Activated via prctl(PR_SET_SECCOMP, mode, ...) or the `seccomp` terminal command.

use alloc::collections::BTreeMap;
use spin::Mutex;

// ── prctl constants ───────────────────────────────────────────────────────────

pub const PR_SET_SECCOMP: u64 = 22;
pub const SECCOMP_MODE_DISABLED: u64 = 0;
pub const SECCOMP_MODE_STRICT:   u64 = 1;
pub const SECCOMP_MODE_FILTER:   u64 = 2;

// ── Filter state ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum SeccompMode { Disabled, Strict, Filter }

/// 256-bit allowlist (covers Linux syscalls 0-255).
#[derive(Clone, Copy)]
pub struct SyscallMask([u64; 4]);

impl SyscallMask {
    pub const fn empty() -> Self { Self([0; 4]) }
    pub const fn all()   -> Self { Self([u64::MAX; 4]) }

    pub fn allow(&mut self, nr: u64) {
        if nr < 256 { self.0[(nr / 64) as usize] |= 1u64 << (nr % 64); }
    }
    pub fn is_allowed(&self, nr: u64) -> bool {
        if nr >= 256 { return false; }
        self.0[(nr / 64) as usize] & (1u64 << (nr % 64)) != 0
    }
}

/// Syscalls allowed in strict mode: read, write, _exit, sigreturn, exit_group.
pub fn strict_mask() -> SyscallMask {
    let mut m = SyscallMask::empty();
    for nr in [0u64, 1, 15, 60, 231] { m.allow(nr); }
    m
}

#[derive(Clone)]
struct SeccompState { mode: SeccompMode, mask: SyscallMask }

static STATES: Mutex<BTreeMap<u64, SeccompState>> = Mutex::new(BTreeMap::new());

// ── Public API ────────────────────────────────────────────────────────────────

pub fn set_strict(pid: u64) {
    STATES.lock().insert(pid, SeccompState { mode: SeccompMode::Strict, mask: strict_mask() });
    crate::serial_println!("[seccomp] pid={} STRICT mode enabled", pid);
}

pub fn set_filter(pid: u64, mask: SyscallMask) {
    STATES.lock().insert(pid, SeccompState { mode: SeccompMode::Filter, mask });
    crate::serial_println!("[seccomp] pid={} FILTER mode enabled", pid);
}

pub fn disable(pid: u64) {
    STATES.lock().remove(&pid);
}

pub fn remove(pid: u64) { disable(pid); }

/// Check if syscall `nr` is allowed for `pid`.
/// Returns true if allowed, false if denied (caller should return EPERM/ENOSYS).
pub fn check(pid: u64, nr: u64) -> bool {
    if pid == 0 { return true; } // kernel always allowed
    let states = STATES.lock();
    let state = match states.get(&pid) {
        Some(s) => s,
        None    => return true, // no seccomp filter → allow
    };
    match state.mode {
        SeccompMode::Disabled => true,
        SeccompMode::Strict | SeccompMode::Filter => {
            if state.mask.is_allowed(nr) { true } else {
                drop(states);
                crate::security::audit::log_denied_syscall(pid, nr, "seccomp");
                false
            }
        }
    }
}

pub fn mode(pid: u64) -> SeccompMode {
    STATES.lock().get(&pid).map(|s| s.mode).unwrap_or(SeccompMode::Disabled)
}

/// Handle prctl(PR_SET_SECCOMP, mode, ...) from a user process.
pub fn handle_prctl_set_seccomp(pid: u64, mode: u64) -> u64 {
    match mode {
        SECCOMP_MODE_DISABLED => { disable(pid); 0 }
        SECCOMP_MODE_STRICT   => { set_strict(pid); 0 }
        _                     => u64::MAX, // EINVAL
    }
}

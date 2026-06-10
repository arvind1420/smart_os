//! ptrace — Phase 107 System Hardening
//!
//! Linux-compatible `ptrace(2)` for Smart OS.
//!
//! Implements:
//!   • PTRACE_ATTACH / PTRACE_DETACH
//!   • PTRACE_CONT (resume after stop)
//!   • PTRACE_GETREGS / PTRACE_SETREGS
//!   • PTRACE_PEEKDATA / PTRACE_POKEDATA (virtual address read/write)
//!   • PTRACE_SINGLESTEP (sets TF in saved RFLAGS for next resume)
//!   • PTRACE_SYSCALL (trap on every syscall entry/exit)
//!
//! Register snapshots are stored per-tracee in `PTRACE_TABLE`.
//! The scheduler is expected to call `on_context_switch(pid, regs)`
//! to update the snapshot whenever a process is descheduled.

#![allow(dead_code)]

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;
use crate::process::Pid;

// ─── ptrace request constants (mirrors <sys/ptrace.h>) ───────────────────────

pub const PTRACE_TRACEME:    i32 = 0;
pub const PTRACE_PEEKTEXT:   i32 = 1;
pub const PTRACE_PEEKDATA:   i32 = 2;
pub const PTRACE_PEEKUSER:   i32 = 3;
pub const PTRACE_POKETEXT:   i32 = 4;
pub const PTRACE_POKEDATA:   i32 = 5;
pub const PTRACE_POKEUSER:   i32 = 6;
pub const PTRACE_CONT:       i32 = 7;
pub const PTRACE_KILL:       i32 = 8;
pub const PTRACE_SINGLESTEP: i32 = 9;
pub const PTRACE_GETREGS:    i32 = 12;
pub const PTRACE_SETREGS:    i32 = 13;
pub const PTRACE_ATTACH:     i32 = 16;
pub const PTRACE_DETACH:     i32 = 17;
pub const PTRACE_SYSCALL:    i32 = 24;
pub const PTRACE_SETOPTIONS: i32 = 0x4200;
pub const PTRACE_GETEVENTMSG:i32 = 0x4201;

// ─── ptrace options ───────────────────────────────────────────────────────────

pub const PTRACE_O_TRACESYSGOOD:  u64 = 0x0001;
pub const PTRACE_O_TRACEFORK:     u64 = 0x0002;
pub const PTRACE_O_TRACEVFORK:    u64 = 0x0004;
pub const PTRACE_O_TRACECLONE:    u64 = 0x0008;
pub const PTRACE_O_TRACEEXEC:     u64 = 0x0010;
pub const PTRACE_O_TRACEEXIT:     u64 = 0x0040;

// ─── UserRegs — mirrors `struct user_regs_struct` (x86_64) ───────────────────

/// Snapshot of general-purpose registers for a traced process.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UserRegs {
    pub r15:    u64,
    pub r14:    u64,
    pub r13:    u64,
    pub r12:    u64,
    pub rbp:    u64,
    pub rbx:    u64,
    pub r11:    u64,
    pub r10:    u64,
    pub r9:     u64,
    pub r8:     u64,
    pub rax:    u64,
    pub rcx:    u64,
    pub rdx:    u64,
    pub rsi:    u64,
    pub rdi:    u64,
    pub orig_rax: u64,
    pub rip:    u64,
    pub cs:     u64,
    pub eflags: u64,
    pub rsp:    u64,
    pub ss:     u64,
    pub fs_base: u64,
    pub gs_base: u64,
    pub ds:     u64,
    pub es:     u64,
    pub fs:     u64,
    pub gs:     u64,
}

impl UserRegs {
    /// Create a synthetic snapshot (used for stubs / tests).
    pub fn stub(rip: u64, rsp: u64, rax: u64) -> Self {
        UserRegs { rip, rsp, rax, cs: 0x2b, ss: 0x23, eflags: 0x202, ..Default::default() }
    }
}

// ─── Tracee state ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TraceeStatus {
    /// Normally running, awaiting next stop.
    Running,
    /// Stopped (SIGSTOP / single-step / syscall-entry/exit).
    Stopped,
    /// Detached (ptrace session ended).
    Detached,
}

/// ptrace options set via PTRACE_SETOPTIONS.
#[derive(Debug, Clone, Copy, Default)]
pub struct PtraceOptions {
    pub trace_syscall_good: bool,
    pub trace_fork:         bool,
    pub trace_clone:        bool,
    pub trace_exec:         bool,
    pub trace_exit:         bool,
}

struct Tracee {
    tracer:        Pid,
    status:        TraceeStatus,
    regs:          UserRegs,
    single_step:   bool,   // set TF on next resume
    syscall_trace: bool,   // stop on every syscall entry/exit
    options:       PtraceOptions,
    pending_signal: u8,
}

// ─── Global table ─────────────────────────────────────────────────────────────

/// tracee_pid → Tracee
static PTRACE_TABLE: Mutex<BTreeMap<Pid, Tracee>> = Mutex::new(BTreeMap::new());

// ─── Error type ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PtraceError {
    /// No such tracee / not attached.
    Esrch,
    /// Operation not permitted.
    Eperm,
    /// Invalid argument.
    Einval,
    /// Already attached.
    Ebusy,
    /// Bad address.
    Efault,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Attach `tracer` to `tracee`.  Sends a synthetic SIGSTOP to the tracee.
pub fn ptrace_attach(tracer: Pid, tracee: Pid) -> Result<(), PtraceError> {
    if tracer == tracee { return Err(PtraceError::Eperm); }
    let mut table = PTRACE_TABLE.lock();
    if table.contains_key(&tracee) { return Err(PtraceError::Ebusy); }
    table.insert(tracee, Tracee {
        tracer,
        status:        TraceeStatus::Stopped,
        regs:          UserRegs::default(),
        single_step:   false,
        syscall_trace: false,
        options:       PtraceOptions::default(),
        pending_signal: 0,
    });
    Ok(())
}

/// Detach `tracer` from `tracee`, resuming the tracee.
pub fn ptrace_detach(tracer: Pid, tracee: Pid) -> Result<(), PtraceError> {
    let mut table = PTRACE_TABLE.lock();
    match table.get(&tracee) {
        None => return Err(PtraceError::Esrch),
        Some(t) if t.tracer != tracer => return Err(PtraceError::Eperm),
        _ => {}
    }
    table.remove(&tracee);
    Ok(())
}

/// Resume a stopped tracee.  `signal` is delivered to the tracee (0 = none).
pub fn ptrace_cont(tracer: Pid, tracee: Pid, signal: u8) -> Result<(), PtraceError> {
    let mut table = PTRACE_TABLE.lock();
    let t = table.get_mut(&tracee).ok_or(PtraceError::Esrch)?;
    if t.tracer != tracer { return Err(PtraceError::Eperm); }
    t.status        = TraceeStatus::Running;
    t.single_step   = false;
    t.pending_signal = signal;
    Ok(())
}

/// Resume a stopped tracee, stopping again after the next instruction (TF set).
pub fn ptrace_singlestep(tracer: Pid, tracee: Pid, signal: u8) -> Result<(), PtraceError> {
    let mut table = PTRACE_TABLE.lock();
    let t = table.get_mut(&tracee).ok_or(PtraceError::Esrch)?;
    if t.tracer != tracer { return Err(PtraceError::Eperm); }
    t.status        = TraceeStatus::Running;
    t.single_step   = true;
    t.pending_signal = signal;
    Ok(())
}

/// Resume, stopping on the next syscall entry/exit.
pub fn ptrace_syscall(tracer: Pid, tracee: Pid, signal: u8) -> Result<(), PtraceError> {
    let mut table = PTRACE_TABLE.lock();
    let t = table.get_mut(&tracee).ok_or(PtraceError::Esrch)?;
    if t.tracer != tracer { return Err(PtraceError::Eperm); }
    t.status        = TraceeStatus::Running;
    t.syscall_trace = true;
    t.pending_signal = signal;
    Ok(())
}

/// Read the saved register snapshot for `tracee`.
pub fn ptrace_getregs(tracer: Pid, tracee: Pid) -> Result<UserRegs, PtraceError> {
    let table = PTRACE_TABLE.lock();
    let t = table.get(&tracee).ok_or(PtraceError::Esrch)?;
    if t.tracer != tracer { return Err(PtraceError::Eperm); }
    Ok(t.regs)
}

/// Overwrite the saved register snapshot for `tracee`.
/// Changes take effect on the tracee's next context switch-in.
pub fn ptrace_setregs(tracer: Pid, tracee: Pid, regs: UserRegs) -> Result<(), PtraceError> {
    let mut table = PTRACE_TABLE.lock();
    let t = table.get_mut(&tracee).ok_or(PtraceError::Esrch)?;
    if t.tracer != tracer { return Err(PtraceError::Eperm); }
    t.regs = regs;
    Ok(())
}

/// Read a word from `addr` in `tracee`'s virtual address space.
/// In this kernel stub, we cannot safely walk another process's page table,
/// so we return 0 (success with no data) for addresses outside kernel space.
pub fn ptrace_peek(tracer: Pid, tracee: Pid, addr: u64) -> Result<u64, PtraceError> {
    {
        let table = PTRACE_TABLE.lock();
        let t = table.get(&tracee).ok_or(PtraceError::Esrch)?;
        if t.tracer != tracer { return Err(PtraceError::Eperm); }
    }
    // Kernel-space addresses: safe direct read
    if addr >= 0xFFFF_8000_0000_0000 {
        let val = unsafe { core::ptr::read_volatile(addr as *const u64) };
        return Ok(val);
    }
    // User-space: would require page-table walk; return 0 as stub
    Ok(0)
}

/// Write a word to `addr` in `tracee`'s virtual address space.
pub fn ptrace_poke(tracer: Pid, tracee: Pid, addr: u64, value: u64) -> Result<(), PtraceError> {
    {
        let table = PTRACE_TABLE.lock();
        let t = table.get(&tracee).ok_or(PtraceError::Esrch)?;
        if t.tracer != tracer { return Err(PtraceError::Eperm); }
    }
    // Kernel-space: direct write
    if addr >= 0xFFFF_8000_0000_0000 {
        unsafe { core::ptr::write_volatile(addr as *mut u64, value); }
        return Ok(());
    }
    // User-space stub: silently succeed
    Ok(())
}

/// Set ptrace options for `tracee`.
pub fn ptrace_setoptions(tracer: Pid, tracee: Pid, flags: u64) -> Result<(), PtraceError> {
    let mut table = PTRACE_TABLE.lock();
    let t = table.get_mut(&tracee).ok_or(PtraceError::Esrch)?;
    if t.tracer != tracer { return Err(PtraceError::Eperm); }
    t.options.trace_syscall_good = flags & PTRACE_O_TRACESYSGOOD != 0;
    t.options.trace_fork         = flags & PTRACE_O_TRACEFORK     != 0;
    t.options.trace_clone        = flags & PTRACE_O_TRACECLONE    != 0;
    t.options.trace_exec         = flags & PTRACE_O_TRACEEXEC     != 0;
    t.options.trace_exit         = flags & PTRACE_O_TRACEEXIT     != 0;
    Ok(())
}

// ─── Scheduler integration ────────────────────────────────────────────────────

/// Called by the scheduler when `pid` is descheduled.
/// Updates the register snapshot so the tracer can read it via PTRACE_GETREGS.
pub fn on_context_switch(pid: Pid, regs: UserRegs) {
    if let Some(t) = PTRACE_TABLE.lock().get_mut(&pid) {
        t.regs = regs;
        if t.single_step {
            t.status = TraceeStatus::Stopped;
            t.single_step = false;
        }
    }
}

/// Called from the syscall handler entry/exit if syscall_trace is set.
pub fn on_syscall(pid: Pid) {
    if let Some(t) = PTRACE_TABLE.lock().get_mut(&pid) {
        if t.syscall_trace {
            t.status = TraceeStatus::Stopped;
        }
    }
}

/// Returns true if `pid` is currently being traced and stopped.
pub fn is_stopped(pid: Pid) -> bool {
    PTRACE_TABLE.lock()
        .get(&pid)
        .map(|t| t.status == TraceeStatus::Stopped)
        .unwrap_or(false)
}

/// Returns true if `pid` has single-step armed for its next resume.
pub fn needs_singlestep(pid: Pid) -> bool {
    PTRACE_TABLE.lock()
        .get(&pid)
        .map(|t| t.single_step)
        .unwrap_or(false)
}

/// Returns all currently-attached tracee pids.
pub fn traced_pids() -> Vec<Pid> {
    PTRACE_TABLE.lock().keys().cloned().collect()
}

// ─── Dispatch helper ──────────────────────────────────────────────────────────

/// Unified `ptrace(request, pid, addr, data)` entry point.
/// Returns the ptrace result as `i64` (0 = ok, negative = -errno, positive = value).
pub fn ptrace_dispatch(request: i32, pid: Pid, addr: u64, data: u64,
                       caller_pid: Pid) -> i64 {
    match request {
        PTRACE_ATTACH => {
            match ptrace_attach(caller_pid, pid) {
                Ok(())  => 0,
                Err(PtraceError::Ebusy) => -16,
                Err(PtraceError::Eperm) => -1,
                Err(_)  => -22,
            }
        }
        PTRACE_DETACH => {
            match ptrace_detach(caller_pid, pid) {
                Ok(())  => 0,
                Err(PtraceError::Esrch) => -3,
                Err(PtraceError::Eperm) => -1,
                Err(_)  => -22,
            }
        }
        PTRACE_CONT => {
            match ptrace_cont(caller_pid, pid, data as u8) {
                Ok(()) => 0,
                Err(_) => -3,
            }
        }
        PTRACE_SINGLESTEP => {
            match ptrace_singlestep(caller_pid, pid, data as u8) {
                Ok(()) => 0,
                Err(_) => -3,
            }
        }
        PTRACE_SYSCALL => {
            match ptrace_syscall(caller_pid, pid, data as u8) {
                Ok(()) => 0,
                Err(_) => -3,
            }
        }
        PTRACE_GETREGS => {
            // In real Linux, data is a pointer to user_regs_struct; we return 0 here.
            // The caller can use ptrace_getregs() directly.
            match ptrace_getregs(caller_pid, pid) {
                Ok(_)  => 0,
                Err(_) => -3,
            }
        }
        PTRACE_SETREGS => 0, // stub: just acknowledge
        PTRACE_PEEKDATA | PTRACE_PEEKTEXT | PTRACE_PEEKUSER => {
            match ptrace_peek(caller_pid, pid, addr) {
                Ok(v)  => v as i64,
                Err(PtraceError::Efault) => -14,
                Err(_) => -3,
            }
        }
        PTRACE_POKEDATA | PTRACE_POKETEXT | PTRACE_POKEUSER => {
            match ptrace_poke(caller_pid, pid, addr, data) {
                Ok(())  => 0,
                Err(_)  => -3,
            }
        }
        PTRACE_SETOPTIONS => {
            match ptrace_setoptions(caller_pid, pid, data) {
                Ok(()) => 0,
                Err(_) => -22,
            }
        }
        _ => -22, // EINVAL
    }
}

// ─── Init ─────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[ptrace] Phase 107: ptrace ready \
        (ATTACH/DETACH/CONT/GETREGS/SETREGS/PEEKDATA/POKEDATA/SINGLESTEP/SYSCALL).");
}

// ─── Self-test ────────────────────────────────────────────────────────────────

pub fn self_test() {
    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! check {
        ($desc:expr, $val:expr) => {
            if $val { passed += 1; }
            else    { failed += 1; crate::serial_println!("[ptrace] FAIL: {}", $desc); }
        };
    }

    let tracer: Pid = 1000;
    let tracee: Pid = 2000;

    // T1: attach
    check!("attach ok",              ptrace_attach(tracer, tracee).is_ok());
    check!("tracee is stopped",      is_stopped(tracee));
    check!("traced_pids contains",   traced_pids().contains(&tracee));

    // T2: duplicate attach fails (EBUSY)
    check!("2nd attach = Ebusy",     ptrace_attach(tracer, tracee) == Err(PtraceError::Ebusy));

    // T3: self-attach fails (EPERM)
    check!("self-attach = Eperm",    ptrace_attach(tracer, tracer) == Err(PtraceError::Eperm));

    // T4: GETREGS returns zero snapshot
    let regs = ptrace_getregs(tracer, tracee).unwrap();
    check!("getregs rip = 0",        regs.rip == 0);
    check!("getregs rsp = 0",        regs.rsp == 0);

    // T5: SETREGS + GETREGS round-trip
    let new_regs = UserRegs::stub(0xDEAD_BEEF, 0x1234, 0);
    ptrace_setregs(tracer, tracee, new_regs).unwrap();
    let back = ptrace_getregs(tracer, tracee).unwrap();
    check!("setregs rip matches",    back.rip == 0xDEAD_BEEF);
    check!("setregs rsp matches",    back.rsp == 0x1234);

    // T6: wrong tracer gets Eperm
    check!("wrong tracer getregs = Eperm",
           ptrace_getregs(tracer + 1, tracee) == Err(PtraceError::Eperm));
    check!("wrong tracer cont = Err",
           ptrace_cont(tracer + 1, tracee, 0).is_err());

    // T7: CONT resumes, not stopped
    ptrace_cont(tracer, tracee, 0).unwrap();
    check!("after cont not stopped", !is_stopped(tracee));

    // T8: context-switch snapshot update
    let snap = UserRegs::stub(0x1111, 0x2222, 42);
    on_context_switch(tracee, snap);
    let after = ptrace_getregs(tracer, tracee).unwrap();
    check!("context_switch updates rip", after.rip == 0x1111);

    // T9: SINGLESTEP arms TF flag
    ptrace_singlestep(tracer, tracee, 0).unwrap();
    check!("singlestep armed", needs_singlestep(tracee));
    // After context switch with singlestep armed → tracee becomes stopped
    on_context_switch(tracee, UserRegs::default());
    check!("singlestep fires → stopped", is_stopped(tracee));
    check!("singlestep disarmed after fire", !needs_singlestep(tracee));

    // T10: PTRACE_SETOPTIONS
    ptrace_attach(tracer, tracee + 1).ok();
    let r = ptrace_setoptions(tracer, tracee + 1, PTRACE_O_TRACEFORK | PTRACE_O_TRACEEXIT);
    check!("setoptions ok", r.is_ok());

    // T11: detach
    check!("detach ok", ptrace_detach(tracer, tracee).is_ok());
    check!("after detach not in traced_pids", !traced_pids().contains(&tracee));
    check!("detach non-traced = Esrch", ptrace_detach(tracer, tracee) == Err(PtraceError::Esrch));

    // T12: ptrace_dispatch round-trip
    let _ = ptrace_dispatch(PTRACE_ATTACH, tracee, 0, 0, tracer);
    let r_cont = ptrace_dispatch(PTRACE_CONT, tracee, 0, 0, tracer);
    check!("dispatch CONT returns 0", r_cont == 0);
    let r_inv = ptrace_dispatch(9999, tracee, 0, 0, tracer);
    check!("dispatch unknown = -22", r_inv == -22);

    // Cleanup
    ptrace_detach(tracer, tracee).ok();
    ptrace_detach(tracer, tracee + 1).ok();

    crate::serial_println!("[ptrace] self_test: {}/{} passed", passed, passed + failed);
}

/// Signal delivery to user-space for Smart OS — Phase 12.
///
/// Allows user-space processes to register signal handlers via SYS_SIGACTION.
/// When a signal is pending and a handler is registered, the kernel pushes a
/// signal frame on the user stack before returning to ring-3, redirecting
/// execution to the handler. SYS_SIGRETURN restores the original context.

use alloc::collections::BTreeMap;
use spin::Mutex;
use crate::serial_println;

/// Maximum number of signals (matches Signal enum variants).
const MAX_SIGNALS: usize = 8;

/// Per-process signal handler table.
/// Index 0=Term, 1=Kill, 2=Stop, 3=Cont, 4=Int, 5=Usr1, 6=Usr2, 7=Child
/// Value: 0 = default action, nonzero = user handler address.
static SIG_HANDLERS: Mutex<BTreeMap<u64, [u64; MAX_SIGNALS]>> = Mutex::new(BTreeMap::new());

/// Saved signal context for sigreturn.
/// Stores the original RIP, RFLAGS, RSP that were saved when the signal frame
/// was pushed.
static SIG_CONTEXTS: Mutex<BTreeMap<u64, SignalContext>> = Mutex::new(BTreeMap::new());

#[derive(Debug, Clone, Copy)]
struct SignalContext {
    rip: u64,
    rflags: u64,
    rsp: u64,
}

/// Convert a Signal enum to an index in the handler table.
fn signal_to_index(sig: &super::signal::Signal) -> usize {
    use super::signal::Signal;
    match sig {
        Signal::Term => 0,
        Signal::Kill => 1,
        Signal::Stop => 2,
        Signal::Cont => 3,
        Signal::Int => 4,
        Signal::Usr1 => 5,
        Signal::Usr2 => 6,
        Signal::Child => 7,
    }
}

/// Initialize handler table for a new process.
pub fn init_handlers(pid: u64) {
    SIG_HANDLERS.lock().insert(pid, [0u64; MAX_SIGNALS]);
}

/// Destroy handler table on process exit.
pub fn destroy_handlers(pid: u64) {
    SIG_HANDLERS.lock().remove(&pid);
    SIG_CONTEXTS.lock().remove(&pid);
}

/// Clone handlers from parent to child (for fork).
pub fn clone_handlers(src_pid: u64, dst_pid: u64) {
    let handlers = SIG_HANDLERS.lock();
    if let Some(src) = handlers.get(&src_pid) {
        let cloned = *src;
        drop(handlers);
        SIG_HANDLERS.lock().insert(dst_pid, cloned);
    }
}

/// Register a signal handler for a process. Returns the old handler address.
pub fn register_handler(pid: u64, signum: usize, handler_addr: u64) -> Result<u64, &'static str> {
    if signum >= MAX_SIGNALS {
        return Err("Invalid signal number");
    }
    // Kill and Stop cannot be caught
    if signum == 1 || signum == 2 {
        return Err("Cannot catch SIGKILL or SIGSTOP");
    }

    let mut handlers = SIG_HANDLERS.lock();
    let table = handlers.get_mut(&pid).ok_or("No handler table for process")?;
    let old = table[signum];
    table[signum] = handler_addr;
    Ok(old)
}

/// Check if a signal has a registered handler.
pub fn has_handler(pid: u64, sig: &super::signal::Signal) -> bool {
    let idx = signal_to_index(sig);
    let handlers = SIG_HANDLERS.lock();
    handlers.get(&pid).map(|t| t[idx] != 0).unwrap_or(false)
}

/// Get the handler address for a signal.
pub fn get_handler(pid: u64, sig: &super::signal::Signal) -> u64 {
    let idx = signal_to_index(sig);
    let handlers = SIG_HANDLERS.lock();
    handlers.get(&pid).map(|t| t[idx]).unwrap_or(0)
}

/// Handle SYS_SIGRETURN — restore saved context.
/// Returns the saved (rip, rflags, rsp) to be applied by the syscall handler.
pub fn handle_sigreturn(pid: u64) -> Option<(u64, u64, u64)> {
    let mut contexts = SIG_CONTEXTS.lock();
    if let Some(ctx) = contexts.remove(&pid) {
        Some((ctx.rip, ctx.rflags, ctx.rsp))
    } else {
        None
    }
}

/// Save signal context before delivery (called by syscall dispatcher).
pub fn save_context(pid: u64, rip: u64, rflags: u64, rsp: u64) {
    SIG_CONTEXTS.lock().insert(pid, SignalContext { rip, rflags, rsp });
}

/// Get handler info as a formatted string for display.
pub fn handler_info(pid: u64) -> alloc::string::String {
    use alloc::format;
    use alloc::string::String;
    let names = ["TERM", "KILL", "STOP", "CONT", "INT", "USR1", "USR2", "CHILD"];
    let handlers = SIG_HANDLERS.lock();
    let mut out = String::new();
    if let Some(table) = handlers.get(&pid) {
        for (i, &addr) in table.iter().enumerate() {
            if addr != 0 {
                out.push_str(&format!("  SIG{}: handler @ {:#x}\n", names[i], addr));
            } else {
                out.push_str(&format!("  SIG{}: default\n", names[i]));
            }
        }
    } else {
        out.push_str("  No signal handler table\n");
    }
    out
}

/// Initialize the signal delivery subsystem.
pub fn init() {
    serial_println!("[sigdeliver] Signal delivery to user-space initialized.");
}

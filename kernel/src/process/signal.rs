/// Signal delivery subsystem for Smart OS.
///
/// Phase 9: POSIX-inspired signal model with cooperative delivery
/// and immediate Kill handling.

use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use spin::Mutex;

/// Signal types supported by Smart OS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    Term,
    Kill,
    Stop,
    Cont,
    Int,
    Usr1,
    Usr2,
    Child,
}

/// Per-thread signal queues. Keyed by thread ID (u64).
static SIGNAL_QUEUES: Mutex<BTreeMap<u64, VecDeque<Signal>>> = Mutex::new(BTreeMap::new());

/// Initialize the signal subsystem.
pub fn init() {
    crate::serial_println!("[signal] Signal subsystem initialized.");
}

/// Send a signal to a thread.
///
/// For Kill signals, the thread is marked for termination immediately
/// (once kill_thread is available in the scheduler). For all other signals,
/// the signal is queued for cooperative retrieval via `pending_signal()`.
///
/// Returns `Err` if the target thread ID does not exist.
pub fn send_signal(tid: u64, sig: Signal) -> Result<(), &'static str> {
    // Verify the target thread exists by checking the scheduler thread list.
    let threads = super::scheduler::list_threads();
    let exists = threads.iter().any(|(id, _, _, _)| *id == tid);
    if !exists {
        return Err("thread not found");
    }

    if sig == Signal::Kill {
        // Kill is intended to be immediate and non-catchable.
        // The scheduler does not yet expose kill_thread(), so we queue
        // the signal for now. When kill_thread() is added, this branch
        // should call it directly.
        crate::serial_println!("[signal] Signal {:?} sent to tid {} (queued, immediate kill TBD)", sig, tid);
        let mut queues = SIGNAL_QUEUES.lock();
        queues.entry(tid).or_insert_with(VecDeque::new).push_back(sig);
    } else {
        crate::serial_println!("[signal] Signal {:?} sent to tid {}", sig, tid);
        let mut queues = SIGNAL_QUEUES.lock();
        queues.entry(tid).or_insert_with(VecDeque::new).push_back(sig);
    }

    Ok(())
}

/// Pop and return the next pending signal for a thread, if any.
pub fn pending_signal(tid: u64) -> Option<Signal> {
    let mut queues = SIGNAL_QUEUES.lock();
    queues.get_mut(&tid).and_then(|q| q.pop_front())
}

/// Clear all pending signals for a thread.
pub fn clear_signals(tid: u64) {
    let mut queues = SIGNAL_QUEUES.lock();
    queues.remove(&tid);
}

/// Return the conventional signal name string for a signal.
pub fn signal_name(sig: &Signal) -> &'static str {
    match sig {
        Signal::Term  => "SIGTERM",
        Signal::Kill  => "SIGKILL",
        Signal::Stop  => "SIGSTOP",
        Signal::Cont  => "SIGCONT",
        Signal::Int   => "SIGINT",
        Signal::Usr1  => "SIGUSR1",
        Signal::Usr2  => "SIGUSR2",
        Signal::Child => "SIGCHLD",
    }
}

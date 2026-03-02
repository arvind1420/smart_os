/// Wait queues for Smart OS.
///
/// Phase 11: Provides the ability to block threads on various conditions
/// (sleep timer, waitpid, pipe read) and wake them when conditions are met.
/// Used by the timer interrupt to periodically check for expired sleeps,
/// and by process exit to wake parents blocked on waitpid.

use alloc::vec::Vec;
use spin::Mutex;
use crate::serial_println;

/// Reason a thread is blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    /// Sleeping until a specific timer tick.
    Sleep { wake_at_tick: u64 },
    /// Waiting for a child process to exit.
    WaitPid { target_pid: u64 },
    /// Waiting for data on a pipe.
    PipeRead { pipe_id: u64 },
}

/// An entry in the wait queue.
#[derive(Debug, Clone)]
pub struct WaitEntry {
    pub tid: u64,
    pub reason: WaitReason,
}

/// Global wait queue — threads waiting on various conditions.
static WAIT_QUEUE: Mutex<Vec<WaitEntry>> = Mutex::new(Vec::new());

/// Add a thread to the wait queue with a given reason.
pub fn sleep_thread(tid: u64, reason: WaitReason) {
    WAIT_QUEUE.lock().push(WaitEntry { tid, reason });
}

/// Check for threads whose sleep timer has expired.
/// Returns a list of thread IDs that should be woken.
/// Called from the timer interrupt handler.
pub fn check_wakeups() -> Vec<u64> {
    let current_tick = crate::drivers::timer::ticks();
    let mut queue = WAIT_QUEUE.lock();
    let mut woken = Vec::new();

    queue.retain(|entry| {
        if let WaitReason::Sleep { wake_at_tick } = entry.reason {
            if current_tick >= wake_at_tick {
                woken.push(entry.tid);
                return false; // remove from queue
            }
        }
        true // keep in queue
    });

    woken
}

/// Wake all threads waiting for a specific process to exit.
/// Returns the list of woken thread IDs.
pub fn wake_waiters_for_pid(target_pid: u64) -> Vec<u64> {
    let mut queue = WAIT_QUEUE.lock();
    let mut woken = Vec::new();

    queue.retain(|entry| {
        if let WaitReason::WaitPid { target_pid: tp } = entry.reason {
            if tp == target_pid || tp == 0 {
                // tp == 0 means "wait for any child"
                woken.push(entry.tid);
                return false;
            }
        }
        true
    });

    woken
}

/// Wake all threads waiting for data on a specific pipe.
/// Returns the list of woken thread IDs.
pub fn wake_pipe_readers(pipe_id: u64) -> Vec<u64> {
    let mut queue = WAIT_QUEUE.lock();
    let mut woken = Vec::new();

    queue.retain(|entry| {
        if let WaitReason::PipeRead { pipe_id: pid } = entry.reason {
            if pid == pipe_id {
                woken.push(entry.tid);
                return false;
            }
        }
        true
    });

    woken
}

/// Get the number of entries in the wait queue.
pub fn wait_queue_count() -> usize {
    WAIT_QUEUE.lock().len()
}

/// Get a snapshot of the wait queue for debugging.
pub fn wait_queue_snapshot() -> Vec<(u64, WaitReason)> {
    WAIT_QUEUE.lock().iter().map(|e| (e.tid, e.reason)).collect()
}

/// Initialize the wait queue subsystem.
pub fn init() {
    serial_println!("[wait] Wait queue subsystem initialized.");
}

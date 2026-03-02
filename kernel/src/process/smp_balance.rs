/// SMP Load Balancing for Smart OS.
///
/// Phase 10: Distributes threads across available CPUs using a
/// work-stealing scheduler. Each CPU has a local run queue.
/// When a CPU goes idle, it steals work from the busiest CPU.
///
/// Currently SMP has 4 CPUs booted (Phase 6) but APs are in HLT loop.
/// This module provides per-CPU queues and the load balancer logic.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;
use crate::serial_println;

/// Maximum CPUs supported.
pub const MAX_CPUS: usize = 4;

/// Per-CPU run queue.
pub struct CpuRunQueue {
    /// CPU ID.
    pub cpu_id: usize,
    /// Threads assigned to this CPU (by TID).
    pub thread_tids: VecDeque<u64>,
    /// Total tasks executed.
    pub tasks_executed: u64,
    /// Whether this CPU is active (not in HLT).
    pub active: bool,
}

impl CpuRunQueue {
    const fn new(cpu_id: usize) -> Self {
        Self {
            cpu_id,
            thread_tids: VecDeque::new(),
            tasks_executed: 0,
            active: false,
        }
    }
}

/// Per-CPU run queues (indexed by CPU ID).
static CPU_QUEUES: [Mutex<CpuRunQueue>; MAX_CPUS] = [
    Mutex::new(CpuRunQueue::new(0)),
    Mutex::new(CpuRunQueue::new(1)),
    Mutex::new(CpuRunQueue::new(2)),
    Mutex::new(CpuRunQueue::new(3)),
];

/// Number of active CPUs.
static ACTIVE_CPUS: AtomicUsize = AtomicUsize::new(1); // BSP always active

/// Load balance interval counter.
static BALANCE_TICK: AtomicU64 = AtomicU64::new(0);

/// Balance interval (every N scheduler ticks).
const BALANCE_INTERVAL: u64 = 100;

/// Total steal operations performed.
static STEAL_COUNT: AtomicU64 = AtomicU64::new(0);

/// Initialize SMP load balancing.
pub fn init() {
    // Mark BSP (CPU 0) as active
    CPU_QUEUES[0].lock().active = true;

    // Check how many APs booted
    let cpu_count = crate::arch::x86_64::smp::cpu_count() as usize;
    ACTIVE_CPUS.store(cpu_count, Ordering::Relaxed);

    // Mark booted APs as available (even though they're in HLT,
    // we can assign work to them for when they wake)
    for i in 1..cpu_count {
        if i < MAX_CPUS {
            CPU_QUEUES[i].lock().active = true;
        }
    }

    serial_println!(
        "[smp-balance] Load balancer initialized ({} CPUs active).",
        cpu_count,
    );
}

/// Assign a thread to the least-loaded CPU.
///
/// Returns the CPU ID the thread was assigned to.
pub fn assign_thread(tid: u64) -> usize {
    let active = ACTIVE_CPUS.load(Ordering::Relaxed).min(MAX_CPUS);
    if active <= 1 {
        // Single CPU — everything goes to CPU 0
        CPU_QUEUES[0].lock().thread_tids.push_back(tid);
        return 0;
    }

    // Find the CPU with the shortest queue
    let mut min_len = usize::MAX;
    let mut min_cpu = 0;

    for i in 0..active {
        let q = CPU_QUEUES[i].lock();
        if q.active && q.thread_tids.len() < min_len {
            min_len = q.thread_tids.len();
            min_cpu = i;
        }
    }

    CPU_QUEUES[min_cpu].lock().thread_tids.push_back(tid);
    min_cpu
}

/// Remove a thread from its CPU queue (when it exits).
pub fn remove_thread(tid: u64) {
    for i in 0..MAX_CPUS {
        let mut q = CPU_QUEUES[i].lock();
        if let Some(pos) = q.thread_tids.iter().position(|&t| t == tid) {
            q.thread_tids.remove(pos);
            return;
        }
    }
}

/// Perform work-stealing load balance.
///
/// Called periodically from the scheduler.
/// Moves threads from the busiest CPU to the least-busy CPU.
pub fn balance() {
    let tick = BALANCE_TICK.fetch_add(1, Ordering::Relaxed);
    if tick % BALANCE_INTERVAL != 0 {
        return;
    }

    let active = ACTIVE_CPUS.load(Ordering::Relaxed).min(MAX_CPUS);
    if active <= 1 {
        return; // Nothing to balance
    }

    // Find most-loaded and least-loaded CPUs
    let mut max_len = 0;
    let mut max_cpu = 0;
    let mut min_len = usize::MAX;
    let mut min_cpu = 0;

    for i in 0..active {
        let q = CPU_QUEUES[i].lock();
        if !q.active { continue; }
        let len = q.thread_tids.len();
        if len > max_len {
            max_len = len;
            max_cpu = i;
        }
        if len < min_len {
            min_len = len;
            min_cpu = i;
        }
    }

    // Steal if imbalance is significant (difference > 2)
    if max_cpu != min_cpu && max_len > min_len + 2 {
        let steal_count = (max_len - min_len) / 2;
        let steal_count = steal_count.min(4); // Don't steal too many at once

        let mut stolen = Vec::new();
        {
            let mut src = CPU_QUEUES[max_cpu].lock();
            for _ in 0..steal_count {
                if let Some(tid) = src.thread_tids.pop_back() {
                    stolen.push(tid);
                }
            }
        }

        {
            let mut dst = CPU_QUEUES[min_cpu].lock();
            for tid in &stolen {
                dst.thread_tids.push_back(*tid);
            }
        }

        if !stolen.is_empty() {
            STEAL_COUNT.fetch_add(stolen.len() as u64, Ordering::Relaxed);
        }
    }
}

/// Set CPU affinity for a thread (pin to specific CPU).
pub fn set_affinity(tid: u64, target_cpu: usize) -> bool {
    if target_cpu >= MAX_CPUS {
        return false;
    }

    // Remove from current queue
    remove_thread(tid);

    // Add to target queue
    CPU_QUEUES[target_cpu].lock().thread_tids.push_back(tid);
    true
}

/// Get load statistics for each CPU: Vec<(cpu_id, queue_length, tasks_executed, active)>.
pub fn cpu_load_stats() -> Vec<(usize, usize, u64, bool)> {
    let active = ACTIVE_CPUS.load(Ordering::Relaxed).min(MAX_CPUS);
    let mut stats = Vec::new();
    for i in 0..active {
        let q = CPU_QUEUES[i].lock();
        stats.push((q.cpu_id, q.thread_tids.len(), q.tasks_executed, q.active));
    }
    stats
}

/// Get total steal operations.
pub fn steal_count() -> u64 {
    STEAL_COUNT.load(Ordering::Relaxed)
}

/// Get total active CPU count.
pub fn active_cpu_count() -> usize {
    ACTIVE_CPUS.load(Ordering::Relaxed)
}

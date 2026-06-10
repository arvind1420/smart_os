/// Per-process syscall monitor with AI-driven anomaly detection.
///
/// Tracks syscall sequences in a ring buffer per process. Every 32 syscalls,
/// extracts a 16-element feature vector and runs the anomaly detection model.
/// If the anomaly score exceeds the threshold AND mass file operations are
/// detected, the process is frozen.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use crate::process::Pid;

/// Queue for offloading anomaly detection to background cores.
pub static ANOMALY_QUEUE: Mutex<VecDeque<(Pid, Vec<i8>)>> = Mutex::new(VecDeque::new());

/// Ring buffer size for syscall history per process.
const SYSCALL_HISTORY_SIZE: usize = 64;

/// Anomaly score threshold for freezing (0.0 = normal, 1.0 = anomaly).
const FREEZE_THRESHOLD: f32 = 0.85;

/// Minimum number of syscalls before anomaly detection activates.
const MIN_CALLS_FOR_DETECTION: usize = 16;

/// Threshold for mass file opens in a window (ransomware pattern).
const MASS_OPEN_THRESHOLD: u32 = 20;

/// Interval (in syscalls) between AI anomaly checks.
const ANOMALY_CHECK_INTERVAL: u64 = 32;

// ── Per-Process Monitor ─────────────────────────────────────────────

/// Per-process syscall monitoring state.
#[derive(Debug)]
pub struct ProcessMonitor {
    /// Circular buffer of recent syscall numbers.
    syscall_ring: [u8; SYSCALL_HISTORY_SIZE],
    /// Current write position in ring buffer.
    ring_pos: usize,
    /// Total syscalls observed for this process.
    total_calls: u64,
    /// Counter: file open syscalls in current window.
    file_open_count: u32,
    /// Counter: file write syscalls in current window.
    file_write_count: u32,
    /// Counter: file read syscalls in current window.
    file_read_count: u32,
    /// Last anomaly score from the AI model.
    pub last_score: f32,
    /// Whether this process has been flagged as suspicious.
    pub flagged: bool,
}

impl ProcessMonitor {
    /// Create a new monitor for a process.
    pub fn new() -> Self {
        Self {
            syscall_ring: [0u8; SYSCALL_HISTORY_SIZE],
            ring_pos: 0,
            total_calls: 0,
            file_open_count: 0,
            file_write_count: 0,
            file_read_count: 0,
            last_score: 0.0,
            flagged: false,
        }
    }

    /// Record a syscall. Returns true if the process should be frozen.
    pub fn record_syscall(&mut self, pid: Pid, nr: u8) -> bool {
        // Write to ring buffer
        self.syscall_ring[self.ring_pos % SYSCALL_HISTORY_SIZE] = nr;
        self.ring_pos += 1;
        self.total_calls += 1;

        // Update counters for specific syscall types
        use crate::syscall::table::*;
        match nr as usize {
            SYS_OPEN => self.file_open_count += 1,
            SYS_WRITE => self.file_write_count += 1,
            SYS_READ => self.file_read_count += 1,
            _ => {}
        }

        // Only analyze after sufficient history
        if self.total_calls < MIN_CALLS_FOR_DETECTION as u64 {
            return false;
        }

        // Heuristic check: mass file operations (ransomware pattern)
        if self.file_open_count > MASS_OPEN_THRESHOLD
            && self.file_write_count > MASS_OPEN_THRESHOLD
        {
            self.flagged = true;
            crate::security::log_audit(
                crate::security::AuditEventType::SyscallAnomaly,
                pid as u32,
                "Mass file operations detected (Heuristic Ransomware Pattern)"
            );
            crate::serial_println!(
                "[security] HEURISTIC ALERT: Mass file operations detected \
                 (opens={}, writes={})",
                self.file_open_count,
                self.file_write_count,
            );
            return true;
        }

        // AI anomaly detection every ANOMALY_CHECK_INTERVAL syscalls
        // Offload to background core instead of blocking the user thread
        if self.total_calls % ANOMALY_CHECK_INTERVAL == 0 {
            let features = self.extract_features();
            ANOMALY_QUEUE.lock().push_back((pid, features));
        }

        false
    }

    /// Extract a 16-element INT8 feature vector from syscall patterns.
    ///
    /// Features:
    ///   [0..8]:  Syscall type distribution (8 buckets, ratio-scaled)
    ///   [8]:     File open rate (opens / total)
    ///   [9]:     File write rate (writes / total)
    ///   [10]:    Write-to-read ratio
    ///   [11]:    Syscall entropy (diversity of different syscall numbers)
    ///   [12]:    Max consecutive same-syscall run length
    ///   [13]:    Recent window file-heavy indicator
    ///   [14..15]: Reserved (zero)
    fn extract_features(&self) -> Vec<i8> {
        let mut features = vec![0i8; 16];
        let ring_len = self.ring_pos.min(SYSCALL_HISTORY_SIZE);
        if ring_len == 0 {
            return features;
        }

        // [0..8]: Syscall type distribution in 8 buckets
        let mut counts = [0u32; 8];
        for i in 0..ring_len {
            let nr = self.syscall_ring[i % SYSCALL_HISTORY_SIZE];
            let bucket = (nr as usize / 5).min(7); // 0-4=bucket0, 5-9=bucket1, ...
            counts[bucket] += 1;
        }
        for i in 0..8 {
            let ratio = counts[i] as f32 / ring_len as f32;
            features[i] = (ratio * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;
        }

        // [8]: File open rate
        let total = self.total_calls.max(1) as f32;
        let open_rate = self.file_open_count as f32 / total;
        features[8] = (open_rate * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;

        // [9]: File write rate
        let write_rate = self.file_write_count as f32 / total;
        features[9] = (write_rate * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;

        // [10]: Write-to-read ratio (high = suspicious)
        let wr_ratio = if self.file_read_count > 0 {
            self.file_write_count as f32 / self.file_read_count as f32
        } else if self.file_write_count > 0 {
            1.0
        } else {
            0.0
        };
        features[10] = (wr_ratio.min(4.0) / 4.0 * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;

        // [11]: Syscall entropy (unique syscall numbers / total)
        let mut nr_freq = [0u32; 64];
        for i in 0..ring_len {
            let nr = self.syscall_ring[i % SYSCALL_HISTORY_SIZE] as usize;
            if nr < 64 {
                nr_freq[nr] += 1;
            }
        }
        let unique = nr_freq.iter().filter(|&&c| c > 0).count();
        features[11] = ((unique as f32 / 44.0) * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;

        // [12]: Max consecutive same-syscall run
        let mut max_run = 1u32;
        let mut current_run = 1u32;
        for i in 1..ring_len {
            let curr = self.syscall_ring[i % SYSCALL_HISTORY_SIZE];
            let prev = self.syscall_ring[(i - 1) % SYSCALL_HISTORY_SIZE];
            if curr == prev {
                current_run += 1;
                max_run = max_run.max(current_run);
            } else {
                current_run = 1;
            }
        }
        features[12] = ((max_run.min(64) as f32 / 64.0) * 254.0 - 127.0)
            .clamp(-128.0, 127.0) as i8;

        // [13]: Recent window file-heavy indicator
        // Count file-related syscalls in last 16 entries
        let recent_start = if ring_len > 16 { ring_len - 16 } else { 0 };
        let mut file_ops = 0u32;
        for i in recent_start..ring_len {
            let nr = self.syscall_ring[i % SYSCALL_HISTORY_SIZE] as usize;
            if (20..=26).contains(&nr) {
                // SYS_OPEN..SYS_MKDIR range
                file_ops += 1;
            }
        }
        let file_ratio = file_ops as f32 / 16.0;
        features[13] = (file_ratio * 254.0 - 127.0).clamp(-128.0, 127.0) as i8;

        features
    }
}

// ── Global Monitor Table ────────────────────────────────────────────

/// Per-process monitor table.
pub static MONITORS: Mutex<BTreeMap<Pid, ProcessMonitor>> = Mutex::new(BTreeMap::new());

/// Initialize the security monitor.
/// Registers the anomaly detection AI model and spawns background worker.
pub fn init() {
    // Register the anomaly detection model with the AI engine
    let model = create_anomaly_model();
    let mut engine = crate::ai::inference::ENGINE.lock();
    if let Some(eng) = engine.as_mut() {
        eng.register_model(model);
    }
    drop(engine); // drop lock before spawning thread
    
    // Spawn background worker thread
    crate::process::scheduler::spawn("anomaly-worker", anomaly_worker_thread, 4);
}

/// Worker thread that pops features from the queue and runs the AI model.
fn anomaly_worker_thread() {
    loop {
        let job = {
            let mut q = ANOMALY_QUEUE.lock();
            q.pop_front()
        };
        
        if let Some((pid, features)) = job {
            let score = run_anomaly_model(&features);
            
            let mut monitors = MONITORS.lock();
            if let Some(monitor) = monitors.get_mut(&pid) {
                monitor.last_score = score;
                if score > FREEZE_THRESHOLD {
                    monitor.flagged = true;
                    crate::security::log_audit(
                        crate::security::AuditEventType::SyscallAnomaly,
                        pid as u32,
                        &alloc::format!("AI Anomaly detected (score={:.2})", score)
                    );
                    crate::serial_println!(
                        "[security] AI ALERT: Background monitor flagged pid {} with score {:.2}",
                        pid, score
                    );
                    drop(monitors); // Drop before freezing
                    freeze_process(pid);
                }
            }
        } else {
            // Wait for work
            crate::process::scheduler::yield_now();
        }
    }
}

/// Called from syscall_dispatcher on every syscall for user processes.
/// Returns true if the process should be frozen.
pub fn on_syscall(pid: Pid, syscall_nr: u8) -> bool {
    if pid == 0 {
        return false; // Never monitor kernel
    }

    let mut monitors = MONITORS.lock();
    let monitor = monitors.entry(pid).or_insert_with(ProcessMonitor::new);
    monitor.record_syscall(pid, syscall_nr)
}

/// Freeze a process: mark all its threads as Blocked in the scheduler.
pub fn freeze_process(pid: Pid) {
    crate::serial_println!(
        "[security] FREEZING suspicious process pid={} — all threads blocked.",
        pid,
    );

    // Set threads to blocked state in scheduler
    let mut sched = crate::process::scheduler::SCHEDULER.lock();
    for thread in sched.ready_queue.iter_mut() {
        if thread.pid == pid {
            thread.state = crate::process::ThreadState::Dead;
        }
    }
}

/// Get monitoring statistics.
pub fn monitor_stats() -> (usize, usize) {
    let monitors = MONITORS.lock();
    let total = monitors.len();
    let flagged = monitors.values().filter(|m| m.flagged).count();
    (total, flagged)
}

/// Get detailed status of all monitors.
pub fn monitor_details() -> Vec<(Pid, u64, f32, bool)> {
    let monitors = MONITORS.lock();
    monitors
        .iter()
        .map(|(&pid, m)| (pid, m.total_calls, m.last_score, m.flagged))
        .collect()
}

// ── Anomaly Detection Model ─────────────────────────────────────────

/// Create the anomaly detection model: 16→8→2 (normal, anomaly).
fn create_anomaly_model() -> crate::ai::model::FeedForwardModel {
    use crate::ai::model::*;
    use crate::ai::tensor::Tensor;

    let mut model = FeedForwardModel::new("anomaly-detector", 16);
    model.set_labels(alloc::vec![
        String::from("normal"),
        String::from("anomaly"),
    ]);

    // Layer 1: 16→8, ReLU
    // Weights emphasize file operation rates and unusual patterns
    #[rustfmt::skip]
    let w1_data: Vec<i8> = vec![
        // Neuron 0: overall syscall distribution
         20, 15, 10, 10,  5,  5,  5,  5,  10,  10,   5,  15,   5,  10,  0,  0,
        // Neuron 1: file operation emphasis
          5,  5,  5,  5,  5,  5,  5,  5,  30,  30,  25,  10,   5,  25,  0,  0,
        // Neuron 2: write-heavy pattern detector
          5,  5,  5,  5,  5,  5,  5,  5,  10,  35,  30,   5,  10,  20,  0,  0,
        // Neuron 3: entropy/diversity detector
         10, 10, 10, 10, 10, 10, 10, 10,   5,   5,   5,  30,   5,   5,  0,  0,
        // Neuron 4: repetitive behavior detector
          5,  5,  5,  5,  5,  5,  5,  5,   5,   5,   5,  -20, 35,  15,  0,  0,
        // Neuron 5: open+write combo
          5,  5,  5,  5,  5,  5,  5,  5,  25,  25,  20,   5,  10,  20,  0,  0,
        // Neuron 6: low-diversity high-volume
         -5, -5, -5, -5, -5, -5, -5, -5,  15,  15,  10, -25,  20,  15,  0,  0,
        // Neuron 7: balanced (normal baseline)
         10, 10, 10, 10, 10, 10, 10, 10,  -5,  -5,  -5,  10,  -5,  -5,  0,  0,
    ];
    let w1 = Tensor::from_data(w1_data, 8, 16, 0.02, 0);
    let b1 = alloc::vec![0i32; 8];
    let layer1 = DenseLayer::new(w1, b1, Activation::ReLU, 0.02, 0);

    // Layer 2: 8→2, None (softmax applied externally)
    #[rustfmt::skip]
    let w2_data: Vec<i8> = vec![
        // normal: strong from balanced neuron, weak from file-heavy
         -5, -15, -20, 10, -15, -15, -20, 30,
        // anomaly: strong from file-heavy, write-heavy, repetitive
          5,  25,  30, -10, 25,  25,  25, -20,
    ];
    let w2 = Tensor::from_data(w2_data, 2, 8, 0.02, 0);
    let b2 = alloc::vec![5i32, -5]; // Slight bias toward normal
    let layer2 = DenseLayer::new(w2, b2, Activation::None, 0.02, 0);

    model.add_layer(layer1);
    model.add_layer(layer2);

    model
}

/// Run the anomaly model on features.
/// Returns anomaly probability (0.0 = normal, 1.0 = anomaly).
fn run_anomaly_model(features: &[i8]) -> f32 {
    let tensor = crate::ai::tensor::Tensor::from_vec(features.to_vec(), 0.01, 0);
    let engine = crate::ai::inference::ENGINE.lock();
    if let Some(eng) = engine.as_ref() {
        if let Ok(result) = eng.infer("anomaly-detector", &tensor) {
            // Second class is "anomaly"
            if result.all_scores.len() >= 2 {
                return result.all_scores[1].1;
            }
        }
    }
    0.0 // Default to normal if inference fails
}

/// Process management for Smart OS.
///
/// A process is an isolated address space with one or more threads,
/// a file descriptor table, and a lifecycle state.
/// Phase 5: Kernel process (pid=0) + user processes with page tables.
/// Phase 11: Children tracking, full exit cleanup, zombie reaping.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::structures::paging::{PhysFrame, Size4KiB};
use super::{Pid, Tid};

/// Process state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    Zombie,
}

/// A process — an isolated address space with threads and metadata.
pub struct Process {
    /// Unique process ID.
    pub pid: Pid,
    /// Process name.
    pub name: String,
    /// Physical frame containing the Level 4 page table (PML4).
    /// `None` for kernel processes that share the kernel page table.
    pub page_table: Option<PhysFrame<Size4KiB>>,
    /// Thread IDs belonging to this process.
    pub threads: Vec<Tid>,
    /// Process state.
    pub state: ProcessState,
    /// Parent process ID (0 for kernel).
    pub parent_pid: Pid,
    /// Exit code (set when process exits).
    pub exit_code: Option<i32>,
    /// Child process IDs.
    pub children: Vec<Pid>,
}

/// Global process table.
pub static PROCESS_TABLE: Mutex<BTreeMap<Pid, Process>> = Mutex::new(BTreeMap::new());

static NEXT_PID: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(1);

pub fn alloc_pid() -> Pid {
    NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

impl Process {
    /// Create a new kernel process (no separate address space).
    pub fn new_kernel(name: &str) -> Self {
        Self {
            pid: 0, // Kernel is always PID 0
            name: String::from(name),
            page_table: None,
            threads: Vec::new(),
            state: ProcessState::Running,
            parent_pid: 0,
            exit_code: None,
            children: Vec::new(),
        }
    }

    /// Create a new user process with its own address space.
    pub fn new_user(name: &str, pml4: PhysFrame<Size4KiB>) -> Self {
        let pid = alloc_pid();
        // Create per-process FD table
        super::fd::create_fd_table(pid);
        super::posix::init_cwd(pid);
        super::sigdeliver::init_handlers(pid);
        Self {
            pid,
            name: String::from(name),
            page_table: Some(pml4),
            threads: Vec::new(),
            state: ProcessState::Running,
            parent_pid: 0,
            exit_code: None,
            children: Vec::new(),
        }
    }
}

/// Register PID=0 kernel process for existing kernel threads.
pub fn init() {
    let kernel_proc = Process::new_kernel("kernel");
    PROCESS_TABLE.lock().insert(0, kernel_proc);
    crate::serial_println!("[process] Kernel process (pid=0) registered.");
}

/// Mark a process as zombie with the given exit code (simple version).
pub fn exit_process(pid: Pid, code: i32) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(proc) = table.get_mut(&pid) {
        proc.state = ProcessState::Zombie;
        proc.exit_code = Some(code);
        crate::serial_println!("[process] Process {} exited with code {}.", pid, code);

        // Free the user page table if present.
        if let Some(pml4) = proc.page_table.take() {
            crate::memory::paging::free_user_page_table(pml4);
        }
    }
}

/// Full process exit with cleanup (Phase 11).
///
/// 1. Destroy FD table
/// 2. Mark Zombie with exit code
/// 3. Free page table
/// 4. Reparent children to pid 0
/// 5. Wake parent's waitpid
/// 6. Send SIGCHLD to parent
pub fn exit_process_full(pid: Pid, code: i32) {
    // 1. Destroy per-process tables
    super::fd::destroy_fd_table(pid);
    super::posix::destroy_cwd(pid);
    super::sigdeliver::destroy_handlers(pid);
    crate::gui::display_server::cleanup_process(pid);

    let mut table = PROCESS_TABLE.lock();
    let (parent_pid, children) = if let Some(proc) = table.get_mut(&pid) {
        // 2. Mark as zombie
        proc.state = ProcessState::Zombie;
        proc.exit_code = Some(code);

        // 3. Free the user page table
        if let Some(pml4) = proc.page_table.take() {
            crate::memory::paging::free_user_page_table(pml4);
        }

        let parent = proc.parent_pid;
        let children = core::mem::take(&mut proc.children);
        (parent, children)
    } else {
        return;
    };

    // 4. Reparent children to pid 0 (kernel)
    for child_pid in &children {
        if let Some(child) = table.get_mut(child_pid) {
            child.parent_pid = 0;
        }
    }
    // Add children to pid 0's children list
    if let Some(kernel) = table.get_mut(&0) {
        kernel.children.extend(children);
    }

    drop(table);

    crate::serial_println!("[process] Process {} exited with code {} (full cleanup).", pid, code);

    // 5. Wake any threads waiting on this pid
    let woken = super::wait::wake_waiters_for_pid(pid);
    for tid in woken {
        super::scheduler::wake_thread(tid);
    }

    // 6. Send SIGCHLD to parent
    if parent_pid != 0 {
        let _ = super::signal::send_signal(parent_pid, super::signal::Signal::Child);
    }
}

/// Reap a zombie process — remove it from the process table entirely.
pub fn reap_zombie(pid: Pid) -> Option<i32> {
    let mut table = PROCESS_TABLE.lock();
    let info = table.get(&pid).and_then(|proc| {
        if proc.state == ProcessState::Zombie {
            Some((proc.exit_code, proc.parent_pid))
        } else {
            None
        }
    });
    if let Some((code, parent_pid)) = info {
        table.remove(&pid);
        if let Some(parent) = table.get_mut(&parent_pid) {
            parent.children.retain(|&c| c != pid);
        }
        return code;
    }
    None
}

/// Get a snapshot of the process table for debugging.
pub fn process_list() -> Vec<(Pid, String, ProcessState, Pid, Option<i32>, Vec<Pid>)> {
    let table = PROCESS_TABLE.lock();
    table.values().map(|p| {
        (p.pid, p.name.clone(), p.state, p.parent_pid, p.exit_code, p.children.clone())
    }).collect()
}

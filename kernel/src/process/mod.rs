/// Process and thread management for Smart OS.
///
/// Phase 2: Kernel-space cooperative threads with round-robin scheduling.
/// Phase 5: User-space processes with address space isolation, preemptive scheduling.

pub mod thread;
pub mod scheduler;
pub mod process;
pub mod elf;
pub mod userspace;
pub mod pipe;
pub mod env;
pub mod signal;
pub mod smp_balance;
pub mod strace;
pub mod wait;
pub mod container;
pub mod fd;
pub mod userprogs;
pub mod posix;
pub mod sigdeliver;
pub mod asm_builder;
pub mod kmod;
pub mod pe;
pub mod migration;
pub mod dynlink;
pub mod ptrace;

use core::sync::atomic::{AtomicU64, Ordering};

/// Unique process ID.
pub type Pid = u64;
/// Unique thread ID.
pub type Tid = u64;

static NEXT_TID: AtomicU64 = AtomicU64::new(1);

/// Allocate a new unique thread ID.
pub fn alloc_tid() -> Tid {
    NEXT_TID.fetch_add(1, Ordering::Relaxed)
}

/// Thread state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Ready,
    Running,
    Blocked,
    Dead,
}

/// Initialize the process subsystem.
pub fn init() {
    process::init();
    scheduler::init();
    migration::init();
    crate::serial_println!("[process] Process subsystem initialized.");
}

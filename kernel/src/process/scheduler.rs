/// Round-robin scheduler for Smart OS.
///
/// Phase 2: Cooperative scheduling for kernel threads.
/// Phase 5: Adds user-space thread support with CR3 switching,
///           preemptive scheduling via timer interrupt, and
///           dual context-switch paths (kernel vs user threads).

use alloc::collections::{VecDeque, BTreeMap};
use spin::Mutex;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;
use super::thread::{Thread, thread_switch, switch_to_user};
use super::ThreadState;
use super::wait::WaitReason;

/// The global scheduler.
pub static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());

pub struct Scheduler {
    /// Queue of ready threads.
    pub ready_queue: VecDeque<Thread>,
    /// The currently running thread.
    pub current: Option<Thread>,
    /// Threads blocked on wait conditions (keyed by TID).
    pub blocked_threads: BTreeMap<u64, Thread>,
    /// Whether the scheduler has been initialized.
    pub initialized: bool,
    /// Whether gaming performance mode is active (Phase 47).
    pub is_game_mode: bool,
}

impl Scheduler {
    const fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
            current: None,
            blocked_threads: BTreeMap::new(),
            initialized: false,
            is_game_mode: false,
        }
    }
}

/// Initialize the scheduler with the boot thread.
pub fn init() {
    let mut sched = SCHEDULER.lock();
    let boot_thread = Thread::boot_thread();
    sched.current = Some(boot_thread);
    sched.initialized = true;
    crate::serial_println!("[scheduler] Initialized with boot thread.");
}

/// Spawn a new kernel thread.
pub fn spawn(name: &str, entry: fn(), priority: u8) {
    let thread = Thread::new(name, entry, priority);
    crate::serial_println!("[scheduler] Spawned thread '{}' (tid={})", thread.name, thread.tid);
    SCHEDULER.lock().ready_queue.push_back(thread);

    // Record app launch for predictive scheduling
    crate::ai::predictor::record_app_launch(name);
}

/// Spawn a kernel thread with a 512 KiB stack.
/// Use this for threads that do deep HTTPS call chains (browser, net daemon).
pub fn spawn_large(name: &str, entry: fn(), priority: u8) {
    let thread = Thread::new_large(name, entry, priority);
    crate::serial_println!("[scheduler] Spawned thread '{}' (tid={}, stack=512K)", thread.name, thread.tid);
    SCHEDULER.lock().ready_queue.push_back(thread);
    crate::ai::predictor::record_app_launch(name);
}

/// Spawn a user-space process from an ELF binary stored in the VFS.
pub fn spawn_user_process(name: &str, elf_path: &str) -> Result<super::Pid, &'static str> {
    // 1. Read ELF from VFS.
    let fd = crate::vfs::open(elf_path).map_err(|_| "Failed to open ELF file")?;
    let mut elf_data = alloc::vec![0u8; 64 * 1024]; // 64 KB max
    let n = crate::vfs::read(fd, &mut elf_data).map_err(|_| "Failed to read ELF")?;
    crate::vfs::close(fd).ok();
    elf_data.truncate(n);

    // 2. Create user page table (copies kernel half).
    let pml4 = crate::memory::paging::create_user_page_table()
        .ok_or("Failed to create user page table")?;

    // 3. Load ELF segments into the user page table.
    let loaded = super::elf::load_elf(&elf_data, pml4)?;
    crate::serial_println!(
        "[scheduler] ELF loaded: entry={:#X}, highest={:#X}",
        loaded.entry_point, loaded.highest_addr
    );

    // 4. Set up user stack with ASLR
    let user_stack_top = crate::memory::aslr::randomize_stack_top();
    let user_stack_pages = 16usize;
    let user_stack_bottom = user_stack_top - (user_stack_pages as u64) * 4096;
    let stack_flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;
    crate::memory::paging::map_range(pml4, user_stack_bottom, user_stack_pages, stack_flags)?;

    // 5. Create process.
    let mut proc = super::process::Process::new_user(name, pml4, loaded.is_linux);
    let pid = proc.pid;

    // 6. Create user thread.
    let cr3 = pml4.start_address().as_u64();
    let thread = Thread::new_user(name, pid, loaded.entry_point, user_stack_top, cr3, 5);
    proc.threads.push(thread.tid);

    // 7. Register process and enqueue thread.
    super::process::PROCESS_TABLE.lock().insert(pid, proc);
    let tid = thread.tid;
    SCHEDULER.lock().ready_queue.push_back(thread);
    
    // Create strict sandbox (CAP_MINIMAL by default)
    let caps = if name == "sh" || name == "guihello" || name == "httpd" {
        crate::security::sandbox::caps::CAP_ALL // Legacy bypass for built-ins
    } else {
        crate::security::sandbox::caps::CAP_MINIMAL
    };
    crate::security::sandbox::create_sandbox(pid, name, caps);

    // SMP distribution
    super::smp_balance::assign_thread(tid);

    crate::serial_println!("[scheduler] User process '{}' (pid={}) spawned.", name, pid);
    Ok(pid)
}

/// Voluntarily yield the CPU to the next ready thread.
///
/// Works for both kernel threads and user threads. The context switch
/// path depends on whether the next thread is a user thread.
pub fn yield_now() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        if !sched.initialized || sched.ready_queue.is_empty() {
            return;
        }

        // Take the current thread out
        let mut old_thread = match sched.current.take() {
            Some(t) => t,
            None => return,
        };

        // Pick the next thread
        let mut new_thread = match sched.ready_queue.pop_front() {
            Some(t) => t,
            None => {
                sched.current = Some(old_thread);
                return;
            }
        };

        // If old thread is still alive, put it back
        if old_thread.state != ThreadState::Dead {
            old_thread.state = ThreadState::Ready;
            sched.ready_queue.push_back(old_thread);
        }

        new_thread.state = ThreadState::Running;

        // Get pointers for context switch
        let old_sp_ptr: *mut u64;
        let old_idx = sched.ready_queue.len() - 1;
        // Only valid if old thread was pushed back (not dead)
        let dead = sched.ready_queue.get(old_idx)
            .map(|t| t.state == ThreadState::Dead)
            .unwrap_or(true);

        let new_sp = new_thread.stack_ptr;
        let new_is_user = new_thread.is_user;
        let new_cr3 = new_thread.cr3;
        let new_kernel_rsp = new_thread.kernel_stack_ptr;

        if !dead {
            old_sp_ptr = &mut sched.ready_queue[old_idx].stack_ptr as *mut u64;
        } else {
            // Thread is dead — use a dummy save location
            static mut DUMMY_SP: u64 = 0;
            old_sp_ptr = &raw mut DUMMY_SP;
        }

        sched.current = Some(new_thread);
        drop(sched);

        if new_is_user {
            // Switching to a user thread: update TSS RSP0 and SYSCALL kernel RSP,
            // then do switch_to_user which ends with iretq to ring 3.
            unsafe {
                crate::arch::x86_64::gdt::set_tss_rsp0(VirtAddr::new(new_kernel_rsp));
                crate::arch::x86_64::syscall_entry::set_kernel_rsp(new_kernel_rsp);
                switch_to_user(old_sp_ptr, new_sp, new_cr3);
            }
        } else {
            // Switching to a kernel thread: normal callee-saved context switch.
            unsafe {
                thread_switch(old_sp_ptr, new_sp);
            }
        }
    });
}

/// Mark the current thread as dead and switch to the next one.
pub fn exit_current_thread() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        if let Some(ref mut current) = sched.current {
            current.state = ThreadState::Dead;
        }

        if let Some(mut next) = sched.ready_queue.pop_front() {
            next.state = ThreadState::Running;
            let new_sp = next.stack_ptr;
            let new_is_user = next.is_user;
            let new_cr3 = next.cr3;
            let new_kernel_rsp = next.kernel_stack_ptr;

            let mut dummy_sp: u64 = 0;
            let dummy_ptr = &mut dummy_sp as *mut u64;

            sched.current = Some(next);
            drop(sched);

            if new_is_user {
                unsafe {
                    crate::arch::x86_64::gdt::set_tss_rsp0(VirtAddr::new(new_kernel_rsp));
                    crate::arch::x86_64::syscall_entry::set_kernel_rsp(new_kernel_rsp);
                    switch_to_user(dummy_ptr, new_sp, new_cr3);
                }
            } else {
                unsafe {
                    thread_switch(dummy_ptr, new_sp);
                }
            }
        }
    });

    loop {
        x86_64::instructions::hlt();
    }
}

/// Called from the timer interrupt to preemptively switch threads.
///
/// `interrupted_rsp` is the stack pointer at the point of interruption.
/// For user threads, this points to the InterruptContext pushed by our timer stub.
/// For kernel threads, this saves the kernel stack state.
///
/// Returns true if a context switch happened.
pub fn preempt(interrupted_rsp: u64) -> bool {
    // Use try_lock to avoid deadlock if scheduler was already locked.
    let mut sched = match SCHEDULER.try_lock() {
        Some(s) => s,
        None => return false,
    };

    if !sched.initialized || sched.ready_queue.is_empty() {
        return false;
    }

    // Save the interrupted thread's RSP.
    if let Some(ref mut current) = sched.current {
        current.stack_ptr = interrupted_rsp;
        current.state = ThreadState::Ready;
    }

    let mut old_thread = match sched.current.take() {
        Some(t) => t,
        None => return false,
    };

    // The timer ISR preemption path uses "pop 15 GPRs + iretq", which only
    // works for user threads that have the full 20-item interrupt-context
    // stack layout.  Kernel threads yield cooperatively; skip them here.
    let new_thread_opt = sched.ready_queue.iter().position(|t| t.is_user)
        .map(|i| sched.ready_queue.remove(i).unwrap());

    let mut new_thread = match new_thread_opt {
        Some(t) => t,
        None => {
            old_thread.state = ThreadState::Running;
            sched.current = Some(old_thread);
            return false;
        }
    };

    // Push old thread to back of queue.
    sched.ready_queue.push_back(old_thread);

    new_thread.state = ThreadState::Running;
    let new_is_user = new_thread.is_user;
    let new_kernel_rsp = new_thread.kernel_stack_ptr;
    let new_cr3 = new_thread.cr3;

    // If switching to a user thread, update TSS + syscall RSP.
    if new_is_user {
        unsafe {
            crate::arch::x86_64::gdt::set_tss_rsp0(VirtAddr::new(new_kernel_rsp));
            crate::arch::x86_64::syscall_entry::set_kernel_rsp(new_kernel_rsp);
            if new_cr3 != 0 {
                // We'll switch CR3 in the assembly return path.
            }
        }
    }

    // The timer interrupt handler will use the returned new_thread's stack_ptr
    // to switch. We store it in a global and handle it from the asm stub.
    // For simplicity in our approach, preemption re-uses the cooperative switch:
    // we modify the interrupted RSP on the stack to point to the new thread.
    // However, this is complex. Instead, we just record the switch target
    // and let the timer stub handle it.

    // For MVP: store the preemption target so the timer stub can use it.
    let new_sp = new_thread.stack_ptr;
    sched.current = Some(new_thread);

    // Store preemption info for the timer stub.
    unsafe {
        PREEMPT_TARGET.new_sp = new_sp;
        PREEMPT_TARGET.new_cr3 = new_cr3;
        PREEMPT_TARGET.is_user = new_is_user;
        PREEMPT_TARGET.active = true;
    }

    true
}

/// Preemption target info, written by `preempt()`, read by timer stub.
#[repr(C)]
pub struct PreemptTarget {
    pub new_sp: u64,
    pub new_cr3: u64,
    pub is_user: bool,
    pub active: bool,
}

#[unsafe(no_mangle)]
pub static mut PREEMPT_TARGET: PreemptTarget = PreemptTarget {
    new_sp: 0,
    new_cr3: 0,
    is_user: false,
    active: false,
};

/// Get the current thread's TID.
pub fn current_tid() -> Option<u64> {
    let sched = SCHEDULER.lock();
    sched.current.as_ref().map(|t| t.tid)
}

/// Get the current thread's PID.
pub fn current_pid() -> Option<u64> {
    let sched = SCHEDULER.lock();
    sched.current.as_ref().map(|t| t.pid)
}

/// Get the current thread's name.
pub fn current_name() -> Option<alloc::string::String> {
    let sched = SCHEDULER.lock();
    sched.current.as_ref().map(|t| t.name.clone())
}

/// Number of ready threads.
pub fn ready_count() -> usize {
    SCHEDULER.lock().ready_queue.len()
}

/// List all threads with their (tid, name, state, priority).
pub fn list_threads() -> alloc::vec::Vec<(u64, alloc::string::String, ThreadState, u8)> {
    let sched = SCHEDULER.lock();
    let mut result = alloc::vec::Vec::new();
    if let Some(ref current) = sched.current {
        result.push((current.tid, current.name.clone(), current.state, current.priority));
    }
    for thread in &sched.ready_queue {
        result.push((thread.tid, thread.name.clone(), thread.state, thread.priority));
    }
    for thread in sched.blocked_threads.values() {
        result.push((thread.tid, thread.name.clone(), ThreadState::Blocked, thread.priority));
    }
    result
}

/// Block the current thread with a wait reason.
/// Removes the current thread from running, stores it in blocked_threads,
/// registers it with the wait queue, then switches to the next ready thread.
///
/// MUST be called from the syscall path (on kernel stack), NOT from interrupt context.
pub fn block_current_thread(reason: WaitReason) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        if !sched.initialized {
            return;
        }

        // Take the current thread out
        let mut thread = match sched.current.take() {
            Some(t) => t,
            None => return,
        };

        let tid = thread.tid;
        thread.state = ThreadState::Blocked;

        // Register with the wait queue
        super::wait::sleep_thread(tid, reason);

        // Store in blocked_threads map
        sched.blocked_threads.insert(tid, thread);

        // Switch to the next ready thread
        if let Some(mut next) = sched.ready_queue.pop_front() {
            next.state = ThreadState::Running;
            let new_sp = next.stack_ptr;
            let new_is_user = next.is_user;
            let new_cr3 = next.cr3;
            let new_kernel_rsp = next.kernel_stack_ptr;

            let mut dummy_sp: u64 = 0;
            let dummy_ptr = &mut dummy_sp as *mut u64;

            sched.current = Some(next);
            drop(sched);

            if new_is_user {
                unsafe {
                    crate::arch::x86_64::gdt::set_tss_rsp0(VirtAddr::new(new_kernel_rsp));
                    crate::arch::x86_64::syscall_entry::set_kernel_rsp(new_kernel_rsp);
                    switch_to_user(dummy_ptr, new_sp, new_cr3);
                }
            } else {
                unsafe {
                    thread_switch(dummy_ptr, new_sp);
                }
            }
        } else {
            // No threads to run — this shouldn't happen in a real system
            drop(sched);
        }
    });
}

/// Wake a blocked thread by TID — moves it from blocked_threads to ready_queue.
pub fn wake_thread(tid: u64) {
    let mut sched = match SCHEDULER.try_lock() {
        Some(s) => s,
        None => return, // Can't lock, will retry on next tick
    };

    if let Some(mut thread) = sched.blocked_threads.remove(&tid) {
        thread.state = ThreadState::Ready;
        sched.ready_queue.push_back(thread);
    }
}

/// Get the number of blocked threads.
pub fn blocked_count() -> usize {
    SCHEDULER.lock().blocked_threads.len()
}

/// Fork the current thread for a child process.
///
/// Creates a child thread that is a copy of the current thread's user-mode state,
/// but with RAX=0 (the return value for the child).
/// The child thread is placed in the ready queue.
///
/// Returns the child's TID.
pub fn fork_current_thread(child_pid: u64, child_cr3: u64) -> u64 {
    use super::thread::{USER_CODE_SEL, USER_DATA_SEL};

    // Read the parent's saved user state from PerCpu via GS
    let user_rsp: u64;
    let kernel_rsp: u64;
    unsafe {
        let pcpu: *const crate::arch::x86_64::percpu::PerCpu;
        core::arch::asm!("mov {}, gs:[0]", out(reg) pcpu);
        user_rsp = (*pcpu).user_rsp;
        kernel_rsp = (*pcpu).kernel_rsp;
    }

    // We need to get the parent's saved RCX (user RIP) and R11 (user RFLAGS)
    // from the syscall entry stub's stack frame. The kernel stack currently has:
    //   [SyscallFrame, r15, r14, r13, r12, rbp, rbx, r11, rcx, ...]
    // But we can't easily read those here. Instead, we reconstruct the child's
    // context as a fresh InterruptContext on a new kernel stack.
    //
    // The parent's user RIP was in RCX and user RFLAGS in R11 when SYSCALL entered.
    // After sysret, the parent continues at RCX with RFLAGS=R11.
    // For the child, we need to create an InterruptContext that iretq will pop.
    //
    // We read the saved RCX/R11 from the current kernel stack.
    // The layout on the kernel stack (from SYSCALL entry stub) is:
    //   [SyscallFrame(7 qwords), r15, r14, r13, r12, rbp, rbx, r11, rcx]
    //   ^-- rsp after pushes
    // So rcx is at kernel_rsp - 8 (top), r11 at kernel_rsp - 16.
    // But actually after "call dispatcher", the return address is pushed,
    // then stack adjustments happen. This is complex.
    //
    // Simpler approach: we know the parent entered via SYSCALL for SYS_FORK.
    // The child should return to the instruction after the parent's syscall.
    // We need the parent's user RIP. In the SYSCALL convention, RCX = user RIP.
    // The entry stub saved RCX on the kernel stack. Let's read it.
    //
    // Actually, the simplest approach: the kernel_rsp in SYSCALL_CPU_DATA points
    // to the TOP of the kernel stack. The entry stub pushes down from there:
    //   push rcx, push r11, push rbp, push rbx, push r12..r15,
    //   push r9, push r8, push r10, push rdx, push rsi, push rdi, push rax
    //   (total 15 pushes = 15 * 8 = 120 bytes)
    // Then "call dispatcher" pushes return addr (another 8 bytes).
    // So rcx is at kernel_rsp - 8 and r11 at kernel_rsp - 16.

    let parent_user_rip: u64;
    let parent_user_rflags: u64;
    unsafe {
        let stack_top = kernel_rsp;
        // From the syscall stub: first push is rcx (user RIP), second is r11 (user RFLAGS)
        parent_user_rip = *((stack_top - 8) as *const u64);
        parent_user_rflags = *((stack_top - 16) as *const u64);
    }

    // Allocate child's kernel stack and set up an InterruptContext
    let tid = super::alloc_tid();
    let kernel_stack = alloc::vec![0u8; 4096 * 4]; // 16 KiB
    let kernel_stack_top = kernel_stack.as_ptr() as u64 + kernel_stack.len() as u64;

    // Build InterruptContext: 20 qwords (15 GPRs + 5 iretq frame)
    let initial_sp = kernel_stack_top - 20 * 8;
    unsafe {
        let sp = initial_sp as *mut u64;
        // GPRs (all zero initially except RAX=0 for child return value)
        core::ptr::write(sp.add(0), 0);   // R15
        core::ptr::write(sp.add(1), 0);   // R14
        core::ptr::write(sp.add(2), 0);   // R13
        core::ptr::write(sp.add(3), 0);   // R12
        core::ptr::write(sp.add(4), 0);   // R11
        core::ptr::write(sp.add(5), 0);   // R10
        core::ptr::write(sp.add(6), 0);   // R9
        core::ptr::write(sp.add(7), 0);   // R8
        core::ptr::write(sp.add(8), 0);   // RBP
        core::ptr::write(sp.add(9), 0);   // RDI
        core::ptr::write(sp.add(10), 0);  // RSI
        core::ptr::write(sp.add(11), 0);  // RDX
        core::ptr::write(sp.add(12), 0);  // RCX
        core::ptr::write(sp.add(13), 0);  // RBX
        core::ptr::write(sp.add(14), 0);  // RAX = 0 (child gets 0 from fork)
        // iretq frame
        core::ptr::write(sp.add(15), parent_user_rip);    // RIP
        core::ptr::write(sp.add(16), USER_CODE_SEL);      // CS (0x23)
        core::ptr::write(sp.add(17), parent_user_rflags | 0x200); // RFLAGS with IF set
        core::ptr::write(sp.add(18), user_rsp);           // RSP (same user stack, CoW protected)
        core::ptr::write(sp.add(19), USER_DATA_SEL);      // SS (0x1B)
    }

    let child_thread = Thread {
        tid,
        pid: child_pid,
        name: alloc::string::String::from("forked"),
        state: ThreadState::Ready,
        stack_ptr: initial_sp,
        kernel_stack_ptr: kernel_stack_top,
        _stack: Some(kernel_stack),
        priority: 5,
        is_user: true,
        cr3: child_cr3,
        canary: crate::process::thread::gen_canary(tid),
    };

    SCHEDULER.lock().ready_queue.push_back(child_thread);
    tid
}

/// Replace the current thread's execution context for exec().
///
/// Builds a fresh InterruptContext on the current thread's kernel stack,
/// then jumps to the new program via switch_to_user. Never returns.
pub fn exec_replace_context(entry: u64, stack_top: u64, cr3: u64) -> ! {
    use super::thread::USER_CODE_SEL;
    use super::thread::USER_DATA_SEL;

    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        if let Some(ref mut current) = sched.current {
            current.cr3 = cr3;

            // Use the current thread's kernel stack top
            let kstack_top = current.kernel_stack_ptr;

            // Build fresh InterruptContext: 20 qwords
            let new_sp = kstack_top - 20 * 8;
            unsafe {
                let sp = new_sp as *mut u64;
                // GPRs all zero
                for i in 0..15 {
                    core::ptr::write(sp.add(i), 0);
                }
                // iretq frame
                core::ptr::write(sp.add(15), entry);         // RIP
                core::ptr::write(sp.add(16), USER_CODE_SEL); // CS
                core::ptr::write(sp.add(17), 0x202);          // RFLAGS (IF)
                core::ptr::write(sp.add(18), stack_top);      // RSP
                core::ptr::write(sp.add(19), USER_DATA_SEL); // SS
            }

            current.stack_ptr = new_sp;

            // Update TSS and syscall kernel RSP
            unsafe {
                crate::arch::x86_64::gdt::set_tss_rsp0(VirtAddr::new(kstack_top));
                crate::arch::x86_64::syscall_entry::set_kernel_rsp(kstack_top);
            }

            // We need a dummy old_sp pointer since we won't return
            let mut dummy_sp: u64 = 0;
            let dummy_ptr = &mut dummy_sp as *mut u64;

            drop(sched);

            // Switch to the new user context — never returns
            unsafe {
                switch_to_user(dummy_ptr, new_sp, cr3);
            }
        }
    });

    // Should never reach here
    loop { x86_64::instructions::hlt(); }
}

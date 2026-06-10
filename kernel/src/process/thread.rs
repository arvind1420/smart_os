/// Kernel and user thread management.
///
/// Each thread has its own stack and a saved register context.
/// Phase 5 adds: user-space threads with full InterruptContext save/restore,
/// per-thread kernel stack, and pid association.

use alloc::string::String;
use alloc::vec::Vec;
use super::{Pid, Tid, ThreadState, alloc_tid};

/// Default kernel thread stack (128 KiB).
const THREAD_STACK_SIZE: usize = 4096 * 32;

/// Large stack for threads that do deep networking (512 KiB).
///
/// The browser thread makes two sequential HTTPS connections per page load
/// (one for HTML, one per image).  TLS 1.3 alone allocates two 4 KB stack
/// buffers + dozens of 32-byte crypto arrays; the HTTP client and TCP layer
/// add more.  128 KiB is too tight; 512 KiB gives ample headroom.
pub const THREAD_STACK_SIZE_LARGE: usize = 4096 * 128;

/// User-mode code segment selector (GDT index 4, RPL=3).
pub const USER_CODE_SEL: u64 = (4 << 3) | 3; // 0x23
/// User-mode data segment selector (GDT index 3, RPL=3).
pub const USER_DATA_SEL: u64 = (3 << 3) | 3; // 0x1B

/// A kernel or user thread.
pub struct Thread {
    /// Unique thread identifier.
    pub tid: Tid,
    /// Owning process ID (0 = kernel).
    pub pid: Pid,
    /// Human-readable name.
    pub name: String,
    /// Current state.
    pub state: ThreadState,
    /// Stack pointer (saved during context switch).
    pub stack_ptr: u64,
    /// Kernel stack top pointer (for syscall entry / TSS RSP0).
    /// Only meaningful for user threads.
    pub kernel_stack_ptr: u64,
    /// The thread's stack memory (kernel stack for user threads, main stack for kernel threads).
    pub(super) _stack: Option<Vec<u8>>,
    /// Priority (lower = higher priority).
    pub priority: u8,
    /// Whether this thread runs in ring 3.
    pub is_user: bool,
    /// CR3 value for this thread's address space.
    /// 0 means use the kernel page table.
    pub cr3: u64,
    /// Stack canary — random value placed at stack bottom, checked on exit.
    pub canary: u64,
}

impl Thread {
    /// Create a new kernel thread that will execute the given function.
    pub fn new(name: &str, entry: fn(), priority: u8) -> Self {
        let tid = alloc_tid();

        // Allocate a stack. Vec<u8> has 1-byte alignment, so we must align
        // stack_top down to 16 bytes to satisfy the x86-64 ABI (RSP must be
        // 16-byte aligned before any `call` instruction).
        let stack = alloc::vec![0u8; THREAD_STACK_SIZE];
        let raw_top = stack.as_ptr() as u64 + THREAD_STACK_SIZE as u64;
        let stack_top = raw_top & !15u64; // align down to 16-byte boundary

        // Set up the initial stack frame so that when we "return" to this
        // thread, it starts executing at `entry`.
        // We push a fake context that thread_switch will pop.
        // Stack layout must match thread_switch's pop order:
        //   thread_switch pops: rbx, rbp, r12, r13, r14, r15, then ret.
        // So sp+0 = rbx (first pop), sp+48 = return address (ret target).
        let initial_sp = stack_top - 8 * 7;
        unsafe {
            let sp = initial_sp as *mut u64;
            core::ptr::write(sp.add(0), entry as *const () as u64); // rbx = fn_ptr
            core::ptr::write(sp.add(1), 0u64); // rbp
            core::ptr::write(sp.add(2), 0u64); // r12
            core::ptr::write(sp.add(3), 0u64); // r13
            core::ptr::write(sp.add(4), 0u64); // r14
            core::ptr::write(sp.add(5), 0u64); // r15
            core::ptr::write(sp.add(6), thread_entry_trampoline as *const () as u64); // ret
        }

        Thread {
            tid,
            pid: 0,
            name: String::from(name),
            state: ThreadState::Ready,
            stack_ptr: initial_sp,
            kernel_stack_ptr: 0,
            _stack: Some(stack),
            priority,
            is_user: false,
            cr3: 0,
            canary: gen_canary(tid),
        }
    }

    /// Like `new` but allocates a 512 KiB stack for threads that need deep
    /// network call chains (TLS + HTTP + image decode stacked on top of each
    /// other can consume > 200 KiB of stack).
    pub fn new_large(name: &str, entry: fn(), priority: u8) -> Self {
        let tid = alloc_tid();
        let stack = alloc::vec![0u8; THREAD_STACK_SIZE_LARGE];
        let raw_top = stack.as_ptr() as u64 + THREAD_STACK_SIZE_LARGE as u64;
        let stack_top = raw_top & !15u64;
        let initial_sp = stack_top - 8 * 7;
        unsafe {
            let sp = initial_sp as *mut u64;
            core::ptr::write(sp.add(0), entry as *const () as u64);
            core::ptr::write(sp.add(1), 0u64);
            core::ptr::write(sp.add(2), 0u64);
            core::ptr::write(sp.add(3), 0u64);
            core::ptr::write(sp.add(4), 0u64);
            core::ptr::write(sp.add(5), 0u64);
            core::ptr::write(sp.add(6), thread_entry_trampoline as *const () as u64);
        }
        Thread {
            tid,
            pid: 0,
            name: String::from(name),
            state: ThreadState::Ready,
            stack_ptr: initial_sp,
            kernel_stack_ptr: 0,
            _stack: Some(stack),
            priority,
            is_user: false,
            cr3: 0,
            canary: gen_canary(tid),
        }
    }

    /// Create a thread representing the current (boot) execution context.
    pub fn boot_thread() -> Self {
        Thread {
            tid: 0,
            pid: 0,
            name: String::from("kernel_main"),
            state: ThreadState::Running,
            stack_ptr: 0,
            kernel_stack_ptr: 0,
            _stack: None,
            priority: 0,
            is_user: false,
            cr3: 0,
            canary: 0xCAFEBABEDEAD_0000,
        }
    }

    /// Create a new user-space thread with its own kernel stack.
    ///
    /// The kernel stack is set up with a full InterruptContext frame so that
    /// when the scheduler switches to this thread, it pops all GPRs and does
    /// `iretq` to enter ring 3.
    ///
    /// Layout on kernel stack (20 qwords, from low to high address):
    ///   [R15, R14, R13, R12, R11, R10, R9, R8, RBP, RDI, RSI, RDX, RCX, RBX, RAX,
    ///    RIP, CS, RFLAGS, RSP, SS]
    pub fn new_user(
        name: &str,
        pid: Pid,
        entry_rip: u64,
        user_rsp: u64,
        cr3: u64,
        priority: u8,
    ) -> Self {
        let tid = alloc_tid();

        // Allocate a kernel stack for this thread (used during syscalls/interrupts).
        let kernel_stack = alloc::vec![0u8; THREAD_STACK_SIZE];
        let raw_top = kernel_stack.as_ptr() as u64 + THREAD_STACK_SIZE as u64;
        let kernel_stack_top = raw_top & !15u64; // align down to 16-byte boundary

        // Set up the full InterruptContext frame on the kernel stack.
        // 20 qwords: 15 GPRs + 5 iretq frame.
        let initial_sp = kernel_stack_top - 20 * 8;
        unsafe {
            let sp = initial_sp as *mut u64;
            // GPRs (all zero initially) — offsets 0..14
            core::ptr::write(sp.add(0), 0);  // R15
            core::ptr::write(sp.add(1), 0);  // R14
            core::ptr::write(sp.add(2), 0);  // R13
            core::ptr::write(sp.add(3), 0);  // R12
            core::ptr::write(sp.add(4), 0);  // R11
            core::ptr::write(sp.add(5), 0);  // R10
            core::ptr::write(sp.add(6), 0);  // R9
            core::ptr::write(sp.add(7), 0);  // R8
            core::ptr::write(sp.add(8), 0);  // RBP
            core::ptr::write(sp.add(9), 0);  // RDI
            core::ptr::write(sp.add(10), 0); // RSI
            core::ptr::write(sp.add(11), 0); // RDX
            core::ptr::write(sp.add(12), 0); // RCX
            core::ptr::write(sp.add(13), 0); // RBX
            core::ptr::write(sp.add(14), 0); // RAX
            // iretq frame — offsets 15..19
            core::ptr::write(sp.add(15), entry_rip);        // RIP
            core::ptr::write(sp.add(16), USER_CODE_SEL);    // CS (0x23)
            core::ptr::write(sp.add(17), 0x202);             // RFLAGS (IF set)
            core::ptr::write(sp.add(18), user_rsp);          // RSP
            core::ptr::write(sp.add(19), USER_DATA_SEL);    // SS (0x1B)
        }

        Thread {
            tid,
            pid,
            name: String::from(name),
            state: ThreadState::Ready,
            stack_ptr: initial_sp,
            kernel_stack_ptr: kernel_stack_top,
            _stack: Some(kernel_stack),
            priority,
            is_user: true,
            cr3,
            canary: gen_canary(tid),
        }
    }
}

pub fn gen_canary(tid: u64) -> u64 {
    let t = crate::drivers::timer::ticks();
    // Mix tid + ticks with a constant to get a pseudo-random canary
    t.wrapping_mul(0x9e3779b97f4a7c15).wrapping_add(tid).wrapping_mul(0x6c62272e07bb0142)
}

/// Trampoline function that new kernel threads start in.
/// RBX contains the entry function pointer (set up in Thread::new).
#[unsafe(naked)]
unsafe extern "C" fn thread_entry_trampoline() {
    core::arch::naked_asm!(
        "call rbx",      // Call the actual entry function (pointer in RBX)
        "call {exit}",   // If it returns, mark thread as dead
        "2: hlt",
        "jmp 2b",
        exit = sym thread_exit,
    );
}

/// Called when a kernel thread's entry function returns.
fn thread_exit() {
    crate::serial_println!("[thread] Thread exited.");
    super::scheduler::exit_current_thread();
}

/// Low-level context switch for kernel threads: save callee-saved registers,
/// swap stack pointer.
///
/// Arguments: old_sp: *mut u64, new_sp: u64
#[unsafe(naked)]
pub unsafe extern "C" fn thread_switch(old_sp: *mut u64, new_sp: u64) {
    core::arch::naked_asm!(
        // Save callee-saved registers on the old stack
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push rbp",
        "push rbx",
        // Save the old stack pointer
        "mov [rdi], rsp",
        // Load the new stack pointer
        "mov rsp, rsi",
        // Restore callee-saved registers from the new stack
        "pop rbx",
        "pop rbp",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        // Return to the new thread (address is on the new stack)
        "ret",
    );
}

/// Context switch to a user thread: save old kernel thread's callee-saved regs,
/// switch CR3 if needed, then pop full InterruptContext + iretq to ring 3.
///
/// Arguments: old_sp: *mut u64, new_sp: u64, new_cr3: u64
#[unsafe(naked)]
pub unsafe extern "C" fn switch_to_user(
    _old_sp: *mut u64,
    _new_sp: u64,
    _new_cr3: u64,
) {
    core::arch::naked_asm!(
        // Save callee-saved registers on the old (kernel) stack
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push rbp",
        "push rbx",
        // Save old stack pointer
        "mov [rdi], rsp",

        // Switch to the user thread's kernel stack
        "mov rsp, rsi",

        // Switch CR3 if new_cr3 != 0
        "test rdx, rdx",
        "jz 2f",
        "mov cr3, rdx",
        "2:",

        // Pop full InterruptContext: 15 GPRs
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",

        // iretq: pops [RIP, CS, RFLAGS, RSP, SS] → jumps to ring 3
        "iretq",
    );
}

// Note: When returning from a user thread (preempted by timer), the interrupt
// stub pushes a full InterruptContext onto the kernel stack and saves the RSP
// as the thread's stack_ptr. The scheduler then uses the normal thread_switch
// to pick the next thread.

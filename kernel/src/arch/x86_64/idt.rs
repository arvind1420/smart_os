/// Interrupt Descriptor Table (IDT) for Smart OS.
///
/// Handles CPU exceptions and hardware interrupts (timer, keyboard, mouse).
/// Phase 5: Timer interrupt uses raw stub for full register save/restore
/// to support preemptive scheduling of user-space threads.

use spin::Lazy;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::VirtAddr;

use crate::serial_println;
use super::gdt;

/// Hardware interrupt vector numbers.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = 32,       // IRQ0
    Keyboard = 33,    // IRQ1
    Mouse = 44,       // IRQ12 (PIC2 line 4)
}

/// Full register context saved by the timer interrupt stub.
/// Matches the push order in `timer_interrupt_stub`.
#[repr(C)]
#[allow(dead_code)]
pub struct InterruptContext {
    // Pushed by our stub (in this order, low address first)
    pub r15: u64, pub r14: u64, pub r13: u64, pub r12: u64,
    pub r11: u64, pub r10: u64, pub r9: u64,  pub r8: u64,
    pub rbp: u64, pub rdi: u64, pub rsi: u64, pub rdx: u64,
    pub rcx: u64, pub rbx: u64, pub rax: u64,
    // Pushed by CPU on interrupt entry
    pub rip: u64, pub cs: u64, pub rflags: u64, pub rsp: u64, pub ss: u64,
}

/// The IDT, lazily initialized.
static IDT: Lazy<InterruptDescriptorTable> = Lazy::new(|| {
    let mut idt = InterruptDescriptorTable::new();

    // ── CPU Exceptions ──
    idt.breakpoint.set_handler_fn(breakpoint_handler);
    unsafe {
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
    }
    idt.page_fault.set_handler_fn(page_fault_handler);
    idt.general_protection_fault.set_handler_fn(general_protection_fault_handler);
    idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
    idt.divide_error.set_handler_fn(divide_error_handler);
    idt.stack_segment_fault.set_handler_fn(stack_segment_fault_handler);

    // ── Hardware Interrupts ──
    // Timer: use raw naked stub for full context save (preemptive scheduling).
    unsafe {
        idt[InterruptIndex::Timer as u8]
            .set_handler_addr(VirtAddr::new(
                timer_interrupt_stub as *const () as u64
            ));
    }
    idt[InterruptIndex::Keyboard as u8].set_handler_fn(keyboard_interrupt_handler);
    idt[InterruptIndex::Mouse as u8].set_handler_fn(mouse_interrupt_handler);

    idt
});

/// Load the IDT into the CPU.
pub fn init() {
    IDT.load();
}

// ─── Timer Interrupt (raw stub for preemptive scheduling) ────────

/// Raw timer interrupt stub. Saves all 15 GPRs, calls the Rust handler,
/// then checks if a preemptive context switch should happen.
///
/// Stack layout after pushes (InterruptContext):
///   [R15, R14, R13, R12, R11, R10, R9, R8, RBP, RDI, RSI, RDX, RCX, RBX, RAX,
///    RIP, CS, RFLAGS, RSP, SS]  ← CPU pushed these 5
#[unsafe(naked)]
unsafe extern "C" fn timer_interrupt_stub() {
    core::arch::naked_asm!(
        // Save all general-purpose registers
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // Call the Rust handler with RSP as argument
        // (points to our InterruptContext)
        "mov rdi, rsp",
        "call {handler}",

        // Check if preemption wants to switch to a different thread.
        // Use RIP-relative LEA for PIE-compatible addressing.
        "lea rbx, [rip + {preempt_active}]",
        "cmp byte ptr [rbx], 1",
        "jne 2f",

        // Preemption active — save current RSP into old thread (already done
        // by scheduler::preempt), then switch to new thread's stack.
        "mov byte ptr [rbx], 0",  // clear flag

        // Load new CR3 if non-zero
        "lea rbx, [rip + {preempt_cr3}]",
        "mov rax, [rbx]",
        "test rax, rax",
        "jz 3f",
        "mov cr3, rax",
        "3:",

        // Load new thread's stack pointer
        "lea rbx, [rip + {preempt_sp}]",
        "mov rsp, [rbx]",

        // Check if new thread is a user thread
        "lea rbx, [rip + {preempt_is_user}]",
        "cmp byte ptr [rbx], 1",
        "jne 2f",

        // User thread: RSP points to InterruptContext (15 GPRs + iretq frame).
        // Fall through to pop all GPRs and iretq.

        "2:",
        // Restore all registers
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

        "iretq",

        handler = sym timer_interrupt_inner,
        preempt_active = sym PREEMPT_ACTIVE_ADDR,
        preempt_sp = sym PREEMPT_SP_ADDR,
        preempt_cr3 = sym PREEMPT_CR3_ADDR,
        preempt_is_user = sym PREEMPT_IS_USER_ADDR,
    );
}

// These statics provide direct access to the PREEMPT_TARGET fields
// from the assembly stub. We use separate statics because inline asm
// can't easily do struct field offsets.
#[unsafe(no_mangle)]
static mut PREEMPT_ACTIVE_ADDR: u8 = 0;
#[unsafe(no_mangle)]
static mut PREEMPT_SP_ADDR: u64 = 0;
#[unsafe(no_mangle)]
static mut PREEMPT_CR3_ADDR: u64 = 0;
#[unsafe(no_mangle)]
static mut PREEMPT_IS_USER_ADDR: u8 = 0;

/// Rust part of the timer interrupt handler.
extern "C" fn timer_interrupt_inner(ctx_rsp: u64) {
    crate::drivers::timer::tick();

    // Check wait queue for expired sleeps and wake blocked threads
    {
        let woken = crate::process::wait::check_wakeups();
        for tid in woken {
            crate::process::scheduler::wake_thread(tid);
        }
    }

    // Preemptive scheduling: every 10 ticks (~100ms at 100Hz)
    let ticks = crate::drivers::timer::ticks();
    if ticks % 10 == 0 {
        if crate::process::scheduler::preempt(ctx_rsp) {
            // Scheduler decided to switch. Copy preempt target to our asm-visible statics.
            unsafe {
                let target_ptr = &raw const crate::process::scheduler::PREEMPT_TARGET;
                PREEMPT_ACTIVE_ADDR = 1;
                PREEMPT_SP_ADDR = (*target_ptr).new_sp;
                PREEMPT_CR3_ADDR = (*target_ptr).new_cr3;
                PREEMPT_IS_USER_ADDR = if (*target_ptr).is_user { 1 } else { 0 };
                // Clear the scheduler's flag
                let target_mut = &raw mut crate::process::scheduler::PREEMPT_TARGET;
                (*target_mut).active = false;
            }
        }
    }

    // Send EOI
    unsafe {
        crate::drivers::pic::PICS.lock().end_of_interrupt(InterruptIndex::Timer as u8);
    }
}

// ─── Other Hardware Interrupt Handlers ────────────────────────────

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::drivers::keyboard::handle_scancode();
    unsafe {
        crate::drivers::pic::PICS.lock().end_of_interrupt(InterruptIndex::Keyboard as u8);
    }
}

extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::drivers::mouse::handle_packet();
    unsafe {
        crate::drivers::pic::PICS.lock().end_of_interrupt(InterruptIndex::Mouse as u8);
    }
}

// ─── Exception Handlers ────────────────────────────────────────

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    serial_println!("[EXCEPTION] Breakpoint");
    serial_println!("  {:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    serial_println!("!!! DOUBLE FAULT !!! (error code: {})", error_code);
    serial_println!("  {:#?}", stack_frame);
    panic!("Double fault — cannot recover");
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;
    let fault_addr = Cr2::read().expect("invalid CR2 address");

    // Check for CoW fault: write to a present page from user mode
    let is_write = error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE);
    let is_user = error_code.contains(PageFaultErrorCode::USER_MODE);
    let is_protection = error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION);

    if is_write && is_user && is_protection {
        // Potential CoW fault — try to handle it
        let pid = crate::process::scheduler::current_pid().unwrap_or(0);
        if pid != 0 {
            let table = crate::process::process::PROCESS_TABLE.lock();
            if let Some(proc) = table.get(&pid) {
                if let Some(pml4) = proc.page_table {
                    drop(table);
                    if crate::memory::cow::handle_cow_fault(pml4, fault_addr) {
                        return; // CoW fault resolved, resume user code
                    }
                }
            }
        }
    }

    if is_user {
        // User-mode page fault that wasn't CoW — kill the process
        let pid = crate::process::scheduler::current_pid().unwrap_or(0);
        serial_println!(
            "[page_fault] Killing user process {} (addr={:?}, error={:?})",
            pid, fault_addr, error_code
        );
        if pid != 0 {
            crate::process::process::exit_process_full(pid, -11); // SIGSEGV
            crate::process::scheduler::exit_current_thread();
        }
    }

    // Kernel-mode page fault — unrecoverable
    serial_println!("[EXCEPTION] Page Fault");
    serial_println!("  Accessed address: {:?}", fault_addr);
    serial_println!("  Error code: {:?}", error_code);
    serial_println!("  {:#?}", stack_frame);
    panic!("Page fault");
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    serial_println!("[EXCEPTION] GPF (error code: {})", error_code);
    serial_println!("  {:#?}", stack_frame);
    panic!("General protection fault");
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    serial_println!("[EXCEPTION] Invalid Opcode");
    serial_println!("  {:#?}", stack_frame);
    panic!("Invalid opcode");
}

extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    serial_println!("[EXCEPTION] Divide Error");
    serial_println!("  {:#?}", stack_frame);
    panic!("Division by zero");
}

extern "x86-interrupt" fn stack_segment_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    serial_println!("[EXCEPTION] Stack Segment Fault (error code: {})", error_code);
    serial_println!("  {:#?}", stack_frame);
    panic!("Stack segment fault");
}

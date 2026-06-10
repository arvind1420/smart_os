/// Interrupt Descriptor Table (IDT) for Smart OS.

use spin::Lazy;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::VirtAddr;

use crate::serial_println;
use super::gdt;

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = 32,
    Keyboard = 33,
    Serial2 = 35, // IRQ 3 (COM2/4)
    Serial1 = 36, // IRQ 4 (COM1/3)
    Mouse = 44,
}

#[repr(C)]
pub struct InterruptContext {
// ... (rest of the struct)
    pub r15: u64, pub r14: u64, pub r13: u64, pub r12: u64,
    pub r11: u64, pub r10: u64, pub r9: u64,  pub r8: u64,
    pub rbp: u64, pub rdi: u64, pub rsi: u64, pub rdx: u64,
    pub rcx: u64, pub rbx: u64, pub rax: u64,
    pub rip: u64, pub cs: u64, pub rflags: u64, pub rsp: u64, pub ss: u64,
}

static IDT: Lazy<InterruptDescriptorTable> = Lazy::new(|| {
    let mut idt = InterruptDescriptorTable::new();

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

    unsafe {
        idt[InterruptIndex::Timer as u8].set_handler_addr(VirtAddr::new(timer_interrupt_stub as *const () as u64));
    }
    idt[InterruptIndex::Keyboard as u8].set_handler_fn(keyboard_interrupt_handler);
    idt[InterruptIndex::Serial1 as u8].set_handler_fn(serial1_interrupt_handler);
    idt[InterruptIndex::Serial2 as u8].set_handler_fn(serial2_interrupt_handler);
    idt[InterruptIndex::Mouse as u8].set_handler_fn(mouse_interrupt_handler);

    idt
});

pub fn init() {
    IDT.load();
}

#[unsafe(naked)]
unsafe extern "C" fn timer_interrupt_stub() {
    core::arch::naked_asm!(
        // swapgs only when interrupted from ring-3 (user mode).
        // Before any push: [rsp+8] = CS from the interrupt frame.
        "test byte ptr [rsp + 8], 3",
        "jz 1f",
        "swapgs",  // ring-3 → ring-0: activate kernel GS
        "1:",

        // Save all registers
        "push rax", "push rbx", "push rcx", "push rdx", "push rsi", "push rdi", "push rbp",
        "push r8", "push r9", "push r10", "push r11", "push r12", "push r13", "push r14", "push r15",

        // Call Rust handler (GS is always kernel GS here)
        "mov rdi, rsp",
        "call {handler}",

        // Check preemption via GS (safe: kernel GS is active)
        "cmp byte ptr gs:[49], 1", // PerCpu.preempt_active
        "jne 2f",

        // Preemption active — clear flag, optionally switch CR3, switch stack
        "mov byte ptr gs:[49], 0",
        "mov rax, gs:[40]", // PerCpu.preempt_cr3
        "test rax, rax",
        "jz 3f",
        "mov cr3, rax",
        "3:",
        "mov rsp, gs:[32]", // PerCpu.preempt_sp

        "2:",
        // Restore all registers
        "pop r15", "pop r14", "pop r13", "pop r12", "pop r11", "pop r10", "pop r9", "pop r8",
        "pop rbp", "pop rdi", "pop rsi", "pop rdx", "pop rcx", "pop rbx", "pop rax",

        // swapgs only when returning to ring-3.
        // After pops: [rsp+8] = CS from the interrupt frame.
        "test byte ptr [rsp + 8], 3",
        "jz 4f",
        "swapgs",  // ring-0 → ring-3: restore user GS
        "4:",

        "iretq",

        handler = sym timer_interrupt_inner,
    );
}

extern "C" fn timer_interrupt_inner(ctx_rsp: u64) {
    // Phase 51: profiling sample (fast path, no-op when disabled).
    crate::profiler::sample(ctx_rsp);
    crate::drivers::timer::tick();

    // Waking logic
    {
        let woken = crate::process::wait::check_wakeups();
        for tid in woken {
            crate::process::scheduler::wake_thread(tid);
        }
    }

    // Preemptive scheduling
    let ticks = crate::drivers::timer::ticks();
    if ticks % 10 == 0 {
        if crate::process::scheduler::preempt(ctx_rsp) {
            // Scheduler decided to switch. Update the CURRENT core's PerCpu.
            unsafe {
                let pcpu: *mut super::percpu::PerCpu;
                core::arch::asm!("mov {}, gs:[0]", out(reg) pcpu);
                
                let target_ptr = &raw const crate::process::scheduler::PREEMPT_TARGET;
                (*pcpu).preempt_active = 1;
                (*pcpu).preempt_sp = (*target_ptr).new_sp;
                (*pcpu).preempt_cr3 = (*target_ptr).new_cr3;
                (*pcpu).preempt_is_user = if (*target_ptr).is_user { 1 } else { 0 };
                
                // Clear the global scheduler's flag
                let target_mut = &raw mut crate::process::scheduler::PREEMPT_TARGET;
                (*target_mut).active = false;
            }
        }
    }

    unsafe {
        crate::arch::x86_64::lapic::eoi();
    }
}

// ... Other handlers remain unchanged
extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::serial_println!("[kbd-irq] keyboard interrupt fired");
    crate::drivers::keyboard::handle_scancode();
    // Send EOI to PIC (keyboard IRQ1 comes through PIC, not LAPIC)
    unsafe {
        crate::drivers::pic::PICS.lock().end_of_interrupt(
            crate::drivers::pic::PIC1_OFFSET + 1  // IRQ1 = keyboard
        );
    }
}

extern "x86-interrupt" fn serial1_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::drivers::uart::dispatch_interrupt(4); // IRQ 4
    unsafe { crate::arch::x86_64::lapic::eoi(); }
}

extern "x86-interrupt" fn serial2_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::drivers::uart::dispatch_interrupt(3); // IRQ 3
    unsafe { crate::arch::x86_64::lapic::eoi(); }
}

extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::drivers::mouse::handle_packet();
    unsafe { crate::arch::x86_64::lapic::eoi(); }
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    serial_println!("[EXCEPTION] Breakpoint at {:#?}", stack_frame.instruction_pointer);
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) -> ! {
    serial_println!("!!! DOUBLE FAULT !!! at {:#?}", stack_frame.instruction_pointer);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn page_fault_handler(stack_frame: InterruptStackFrame, error_code: PageFaultErrorCode) {
    use x86_64::registers::control::Cr2;
    x86_64::instructions::interrupts::disable();
    let fault_addr = Cr2::read();
    serial_println!(
        "[EXCEPTION] Page Fault! RIP={:#X} CR2={:?} err={:?}",
        stack_frame.instruction_pointer.as_u64(),
        fault_addr,
        error_code,
    );
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn general_protection_fault_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    x86_64::instructions::interrupts::disable();
    serial_println!(
        "[EXCEPTION] GPF! RIP={:#X} error_code={:#X}",
        stack_frame.instruction_pointer.as_u64(),
        error_code,
    );
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::disable();
    serial_println!("[EXCEPTION] Invalid Opcode at RIP={:#X}", stack_frame.instruction_pointer.as_u64());
    // Walk the stack to find the return addresses leading to this crash.
    // stack_frame.stack_pointer is RSP at the moment of the ud2/invalid opcode.
    let rsp = stack_frame.stack_pointer.as_u64();
    serial_println!("[EXCEPTION] RSP at crash: {:#X}", rsp);
    unsafe {
        for i in 0..16usize {
            let addr = rsp + (i as u64) * 8;
            let val = core::ptr::read_unaligned(addr as *const u64);
            // Only print if value looks like a kernel text address
            if val >= 0x8000_0000_0000 && val < 0x8100_0000_0000 {
                serial_println!("  [RSP+{:#X}] = {:#X}  ← possible return addr (kernel offset: {:#X})",
                    i * 8, val, val - 0x8000_0000_0000);
            } else {
                serial_println!("  [RSP+{:#X}] = {:#X}", i * 8, val);
            }
        }
    }
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    serial_println!("[EXCEPTION] Divide Error");
    panic!("Div0");
}

extern "x86-interrupt" fn stack_segment_fault_handler(stack_frame: InterruptStackFrame, code: u64) {
    serial_println!("[EXCEPTION] Stack Segment Fault ({})", code);
    panic!("SSF");
}

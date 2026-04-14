/// Per-CPU data for Smart OS.
///
/// Each core has its own PerCpu structure, pointed to by the GS_BASE MSR.

use super::gdt::Selectors;
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;
use alloc::boxed::Box;

/// Static double-fault IST stack for the BSP — allocated before the heap exists.
static mut BSP_DF_STACK: [u8; 16384] = [0u8; 16384];

/// Per-CPU storage.
#[repr(C)]
pub struct PerCpu {
    /// Pointer to this PerCpu structure (for self-reference in asm). Offset 0.
    pub self_ptr: *const PerCpu,
    /// CPU ID. Offset 8.
    pub cpu_id: u32,
    /// LAPIC ID. Offset 12.
    pub lapic_id: u8,
    /// Saved user RSP during syscall. Offset 16.
    pub user_rsp: u64,
    /// Kernel stack pointer for syscall entry. Offset 24.
    pub kernel_rsp: u64,
    
    /// Preemption target: new stack pointer. Offset 32.
    pub preempt_sp: u64,
    /// Preemption target: new CR3. Offset 40.
    pub preempt_cr3: u64,
    /// Preemption target: is user thread? Offset 48.
    pub preempt_is_user: u8,
    /// Preemption target: is active? Offset 49.
    pub preempt_active: u8,

    /// Per-CPU TSS. Offset 56.
    pub tss: TaskStateSegment,
    /// Per-CPU GDT selectors.
    pub selectors: Option<Selectors>,
}

impl PerCpu {
    pub const fn new(cpu_id: u32) -> Self {
        Self {
            self_ptr: core::ptr::null(),
            cpu_id,
            lapic_id: 0,
            user_rsp: 0,
            kernel_rsp: 0,
            preempt_sp: 0,
            preempt_cr3: 0,
            preempt_is_user: 0,
            preempt_active: 0,
            tss: TaskStateSegment::new(),
            selectors: None,
        }
    }

    /// Initialize the per-CPU TSS and stacks.
    pub fn init(&mut self, kernel_stack_top: u64) {
        self.self_ptr = self as *const _;
        self.kernel_rsp = kernel_stack_top;
        
        // Setup TSS privilege stack (RSP0)
        self.tss.privilege_stack_table[0] = VirtAddr::new(kernel_stack_top);
        
        // Setup Double Fault IST.
        // BSP (cpu_id == 0) boots before the heap is initialized, so use a
        // pre-allocated static stack.  APs boot after heap init, so Box is fine.
        let df_stack_top = if self.cpu_id == 0 {
            unsafe { (&raw const BSP_DF_STACK as u64) + 16384 }
        } else {
            let df_stack = Box::leak(Box::new([0u8; 16384]));
            df_stack.as_ptr() as u64 + 16384
        };
        self.tss.interrupt_stack_table[0] = VirtAddr::new(df_stack_top);
    }
}

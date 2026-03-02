/// Global Descriptor Table (GDT) setup for Smart OS.
///
/// The GDT defines memory segments for the CPU. In long mode (64-bit),
/// segmentation is mostly unused, but we still need:
/// - Kernel code segment (ring 0) — index 1, selector 0x08
/// - Kernel data segment (ring 0) — index 2, selector 0x10
/// - User data segment (ring 3)   — index 3, selector 0x18
/// - User code segment (ring 3)   — index 4, selector 0x20
/// - Task State Segment (TSS)     — index 5-6, selector 0x28
///
/// The ordering (user_data before user_code) is required for SYSRET:
///   STAR[63:48] = 0x10 → SYSRET loads SS = 0x10+8|3 = 0x1B, CS = 0x10+16|3 = 0x23

use spin::Lazy;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// Index of the double-fault handler stack in the IST (Interrupt Stack Table).
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Size of the interrupt/privilege stacks (16 KiB each).
const STACK_SIZE: usize = 4096 * 4;

/// Static storage for the TSS. We need mutable access to update RSP0
/// when switching to user-mode threads, so we use `static mut` instead
/// of wrapping in Lazy<>.
static mut TSS_STORAGE: TaskStateSegment = TaskStateSegment::new();

/// Whether TSS has been initialized (one-time setup).
static TSS_INIT: Lazy<()> = Lazy::new(|| {
    // SAFETY: This runs exactly once during lazy init, before any other
    // access to TSS_STORAGE. We use raw pointers to avoid shared-reference-
    // to-static-mut issues in Rust 2024.
    unsafe {
        let tss_ptr = &raw mut TSS_STORAGE;

        // Double-fault IST entry.
        (*tss_ptr).interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            let stack_start = VirtAddr::from_ptr(&raw const STACK);
            stack_start + STACK_SIZE as u64
        };

        // Privilege stack for ring 0 (used when interrupts fire from ring 3).
        (*tss_ptr).privilege_stack_table[0] = {
            static mut PRIV_STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            let stack_start = VirtAddr::from_ptr(&raw const PRIV_STACK);
            stack_start + STACK_SIZE as u64
        };
    }
});

/// The GDT and its segment selectors.
static GDT: Lazy<(GlobalDescriptorTable, Selectors)> = Lazy::new(|| {
    // Ensure TSS is initialized before creating the GDT.
    Lazy::force(&TSS_INIT);

    let mut gdt = GlobalDescriptorTable::new();

    let kernel_code = gdt.append(Descriptor::kernel_code_segment()); // 0x08
    let kernel_data = gdt.append(Descriptor::kernel_data_segment()); // 0x10
    let user_data = gdt.append(Descriptor::user_data_segment());     // 0x18
    let user_code = gdt.append(Descriptor::user_code_segment());     // 0x20
    // SAFETY: TSS_STORAGE was initialized by TSS_INIT above, and we only
    // mutate privilege_stack_table[0] with interrupts disabled (single CPU).
    let tss_ref = unsafe { &*(&raw const TSS_STORAGE) };
    let tss = gdt.append(Descriptor::tss_segment(tss_ref)); // 0x28

    (
        gdt,
        Selectors {
            kernel_code,
            kernel_data,
            user_code,
            user_data,
            tss,
        },
    )
});

/// Segment selectors for the loaded GDT.
#[allow(dead_code)]
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_code: SegmentSelector,
    pub user_data: SegmentSelector,
    pub tss: SegmentSelector,
}

/// Initialize the GDT, load it into the CPU, and set segment registers.
pub fn init() {
    use x86_64::instructions::segmentation::{CS, DS, Segment};
    use x86_64::instructions::tables::load_tss;

    GDT.0.load();

    unsafe {
        CS::set_reg(GDT.1.kernel_code);
        DS::set_reg(GDT.1.kernel_data);
        load_tss(GDT.1.tss);
    }
}

/// Update the TSS RSP0 (privilege level 0 stack pointer).
///
/// This must be called before switching to a user-mode thread so that
/// when an interrupt fires in ring 3, the CPU loads this kernel stack.
///
/// # Safety
/// Must be called with interrupts disabled (during context switch).
pub unsafe fn set_tss_rsp0(stack_top: VirtAddr) {
    let tss_ptr = &raw mut TSS_STORAGE;
    unsafe {
        (*tss_ptr).privilege_stack_table[0] = stack_top;
    }
}

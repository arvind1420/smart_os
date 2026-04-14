/// Global Descriptor Table (GDT) setup for Smart OS.
///
/// Each core has its own GDT and TSS to support SMP context switching.

use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;
use super::percpu::PerCpu;

/// Index of the double-fault handler stack in the IST (Interrupt Stack Table).
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Size of the interrupt/privilege stacks (16 KiB each).
pub const STACK_SIZE: usize = 4096 * 4;

/// Segment selectors for the loaded GDT.
#[derive(Debug, Clone, Copy)]
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_code: SegmentSelector,
    pub user_data: SegmentSelector,
    pub tss: SegmentSelector,
}

/// Create a new GDT for the given TSS.
pub fn create_gdt(tss: &'static TaskStateSegment) -> (GlobalDescriptorTable, Selectors) {
    let mut gdt = GlobalDescriptorTable::new();
    let kernel_code = gdt.append(Descriptor::kernel_code_segment());
    let kernel_data = gdt.append(Descriptor::kernel_data_segment());
    let user_data = gdt.append(Descriptor::user_data_segment());
    let user_code = gdt.append(Descriptor::user_code_segment());
    let tss_sel = gdt.append(Descriptor::tss_segment(tss));

    (
        gdt,
        Selectors {
            kernel_code,
            kernel_data,
            user_code,
            user_data,
            tss: tss_sel,
        },
    )
}

/// Global BSP PerCpu structure.
pub static mut BSP_PER_CPU: Option<PerCpu> = None;
/// Global BSP GDT.
pub static mut BSP_GDT: Option<GlobalDescriptorTable> = None;

/// Initialize the GDT for the BSP (Boot Strap Processor).
pub fn init() {
    use x86_64::instructions::segmentation::{CS, DS, Segment};
    use x86_64::instructions::tables::load_tss;

    unsafe {
        let mut pcpu = PerCpu::new(0);
        // Temporary stack for BSP initialization until scheduler takes over
        static mut BSP_STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
        let stack_top = (&raw const BSP_STACK as u64) + STACK_SIZE as u64;
        
        pcpu.init(stack_top);
        let pcpu_ptr = &raw mut BSP_PER_CPU;
        (*pcpu_ptr) = Some(pcpu);
        
        let pcpu_ref = (*pcpu_ptr).as_mut().unwrap();
        let (gdt, selectors) = create_gdt(&pcpu_ref.tss);
        
        let gdt_ptr = &raw mut BSP_GDT;
        (*gdt_ptr) = Some(gdt);
        (*gdt_ptr).as_ref().unwrap().load();
        
        CS::set_reg(selectors.kernel_code);
        DS::set_reg(selectors.kernel_data);
        load_tss(selectors.tss);
        
        // Store selectors in PerCpu
        pcpu_ref.selectors = Some(selectors);
        
        // Set GS_BASE to point to PerCpu
        set_percpu_base(pcpu_ref as *const _ as u64);
    }
}

/// Initialize GDT for an AP (Application Processor).
/// This is called from the AP entry point.
pub fn init_ap(pcpu: &'static mut PerCpu, gdt: &'static mut GlobalDescriptorTable) {
    use x86_64::instructions::segmentation::{CS, DS, Segment};
    use x86_64::instructions::tables::load_tss;

    gdt.load();
    
    let selectors = pcpu.selectors.expect("AP selectors not initialized");
    
    unsafe {
        CS::set_reg(selectors.kernel_code);
        DS::set_reg(selectors.kernel_data);
        load_tss(selectors.tss);
        
        set_percpu_base(pcpu as *const _ as u64);
    }
}

/// Set the GS_BASE MSR to point to the PerCpu structure.
pub unsafe fn set_percpu_base(addr: u64) {
    let low = (addr & 0xFFFF_FFFF) as u32;
    let high = (addr >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") 0xC000_0101u32, // GS_BASE
        in("eax") low,
        in("edx") high,
    );
}

/// Update the TSS RSP0 for the CURRENT CPU.
pub unsafe fn set_tss_rsp0(stack_top: VirtAddr) {
    // We can access the current PerCpu via GS
    let pcpu: *mut PerCpu;
    core::arch::asm!(
        "mov {}, gs:[0]", // GS:[0] is self_ptr
        out(reg) pcpu,
    );
    (*pcpu).tss.privilege_stack_table[0] = stack_top;
}

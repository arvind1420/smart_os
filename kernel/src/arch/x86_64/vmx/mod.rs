/// Intel VT-x (VMX) Native Hypervisor for Smart OS.
///
/// Phase 18: Ring -1 Support.
/// Provides the foundation for running unmodified guest operating systems
/// (like Windows or Linux) inside isolated Virtual Machines directly
/// on the Smart OS kernel.

use core::arch::asm;
use crate::serial_println;

const CR4_VMXE_BIT: u64 = 1 << 13;
const IA32_FEATURE_CONTROL: u32 = 0x3A;

/// Initialize VT-x on the current processor.
pub fn init() -> Result<(), &'static str> {
    if !is_vmx_supported() {
        return Err("Intel VT-x not supported or disabled in BIOS");
    }

    // Enable VMX in CR4
    unsafe {
        let mut cr4: u64;
        asm!("mov {}, cr4", out(reg) cr4);
        cr4 |= CR4_VMXE_BIT;
        asm!("mov cr4, {}", in(reg) cr4);
    }

    // Enable VMX in IA32_FEATURE_CONTROL MSR
    unsafe {
        let mut eax: u32;
        let mut edx: u32;
        asm!("rdmsr", in("ecx") IA32_FEATURE_CONTROL, out("eax") eax, out("edx") edx);
        
        // Bit 0 is lock bit. If locked and Bit 2 (VMX outside SMX) is 0, VMX cannot be enabled.
        if (eax & 1) != 0 && (eax & (1 << 2)) == 0 {
            return Err("VT-x is locked off in BIOS");
        }

        if (eax & 1) == 0 {
            eax |= (1 << 2) | 1; // Enable VMX outside SMX + Lock
            asm!("wrmsr", in("ecx") IA32_FEATURE_CONTROL, in("eax") eax, in("edx") edx);
        }
    }

    serial_println!("[vmx] Intel VT-x enabled on current core.");
    Ok(())
}

fn is_vmx_supported() -> bool {
    let mut ecx: u32;
    unsafe {
        // CPUID EAX=1 returns features in ECX. Bit 5 is VMX.
        asm!(
            "push rbx",      // Save rbx since LLVM uses it internally
            "cpuid",
            "pop rbx",       // Restore rbx
            inout("eax") 1 => _,
            lateout("ecx") ecx,
            out("edx") _,
            options(preserves_flags),
        );
    }
    (ecx & (1 << 5)) != 0
}

/// A placeholder for a Virtual Machine Control Structure (VMCS).
pub struct Vm {
    pub id: u32,
    pub vmcs_phys_addr: u64,
}

impl Vm {
    /// Boot a guest VM.
    pub fn launch(&mut self) -> Result<(), &'static str> {
        serial_println!("[vmx] Launching VM {}...", self.id);
        // In a real implementation:
        // 1. VMCLEAR
        // 2. VMPTRLD
        // 3. VMWRITE (setup guest state, host state, execution controls)
        // 4. VMLAUNCH / VMRESUME
        Err("VMLAUNCH not implemented")
    }
}

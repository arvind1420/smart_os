/// SSE/SSE2 initialization for Smart OS.
///
/// All x86_64 CPUs mandate SSE2. We enable it here so kernel code (including
/// RustCrypto software implementations) can use XMM registers and 128-bit ops.
/// Must be called before any SSE instructions execute.

pub fn init() {
    unsafe {
        // CR0: clear EM (emulate FPU) bit [2], set MP (monitor co-processor) bit [1].
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        cr0 &= !(1 << 2); // clear CR0.EM
        cr0 |=   1 << 1;  // set   CR0.MP
        core::arch::asm!("mov cr0, {}", in(reg) cr0);

        // CR4: set OSFXSR bit [9] (enable FXSAVE/FXRSTOR for SSE state)
        //       set OSXMMEXCPT bit [10] (enable #XF exceptions).
        //       Also dynamically enable SMEP (bit 20) and SMAP (bit 21) if supported by CPU.
        let cpuid = core::arch::x86_64::__cpuid_count(7, 0);
        let smep_supported = (cpuid.ebx & (1 << 7)) != 0;
        let smap_supported = (cpuid.ebx & (1 << 20)) != 0;

        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        cr4 |= (1 << 9) | (1 << 10);
        if smep_supported {
            cr4 |= 1 << 20;
        }
        if smap_supported {
            cr4 |= 1 << 21;
        }
        core::arch::asm!("mov cr4, {}", in(reg) cr4);

        if smep_supported || smap_supported {
            crate::serial_println!(
                "[sse] CPU security features enabled: SMEP={}, SMAP={}",
                smep_supported, smap_supported
            );
        }
    }
    crate::serial_println!("[sse] SSE2 enabled (CR0.EM cleared, CR4.OSFXSR+OSXMMEXCPT set).");
}

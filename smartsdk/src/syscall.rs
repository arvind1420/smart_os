use core::arch::asm;

pub const SYS_EXIT: u64 = 0;
pub const SYS_WRITE: u64 = 23;
pub const SYS_TCP_CONNECT: u64 = 33;
pub const SYS_TCP_SEND: u64 = 36;
pub const SYS_TCP_RECV: u64 = 37;
pub const SYS_TCP_CLOSE: u64 = 38;
pub const SYS_TLS_CONNECT: u64 = 84;
pub const SYS_TLS_SEND: u64 = 85;
pub const SYS_TLS_RECV: u64 = 86;
pub const SYS_TLS_CLOSE: u64 = 87;
pub const SYS_GETHOSTBYNAME: u64 = 69;
pub const SYS_SYSINFO: u64 = 70;
pub const SYS_KMOD_LOAD: u64 = 71;
pub const SYS_AI_INFER: u64 = 72;
pub const SYS_AI_SEARCH: u64 = 73;
pub const SYS_AUDIT_READ: u64 = 74;
pub const SYS_LICENSE_CHECK: u64 = 75;

pub const SYS_UI_TRAVERSE: u64 = 77;
pub const SYS_GAMEPAD_STATE: u64 = 78;
pub const SYS_SET_GAME_MODE: u64 = 79;

// Tensor API
// pub const SYS_TENSOR_CREATE: u64 = 80;
// pub const SYS_TENSOR_OP: u64 = 81;
// pub const SYS_TENSOR_DESTROY: u64 = 82;

pub const SYS_FORK: u64 = 5;
pub const SYS_EXEC: u64 = 6;
pub const SYS_WAITPID: u64 = 7;
pub const SYS_SPAWN: u64 = 2;
pub const SYS_MMAP: u64 = 8;

pub const SYS_IPC_SEND: u64 = 10;
pub const SYS_IPC_RECV: u64 = 11;
pub const SYS_IPC_CREATE_PORT: u64 = 12;
pub const SYS_IPC_LOOKUP_PORT: u64 = 13;

pub const SYS_READ: u64 = 22;
pub const SYS_OPEN: u64 = 20;
pub const SYS_CLOSE: u64 = 21;
pub const SYS_STAT: u64 = 24;
pub const SYS_READDIR: u64 = 25;
pub const SYS_MKDIR: u64 = 26;

pub const SYS_DISPLAY_CMD: u64 = 63;
pub const SYS_DISPLAY_EVENT: u64 = 64;

#[inline(always)]
pub fn syscall0(number: u64) -> u64 {
    let mut ret: u64;
    unsafe {
        #[cfg(target_arch = "x86_64")]
        asm!("syscall", in("rax") number, out("rcx") _, out("r11") _, lateout("rax") ret, options(nostack));
        
        #[cfg(target_arch = "aarch64")]
        asm!("svc #0", in("x8") number, lateout("x0") ret, options(nostack));
    }
    ret
}

#[inline(always)]
pub fn syscall1(number: u64, arg1: u64) -> u64 {
    let mut ret: u64;
    unsafe {
        #[cfg(target_arch = "x86_64")]
        asm!("syscall", in("rax") number, in("rdi") arg1, out("rcx") _, out("r11") _, lateout("rax") ret, options(nostack));
        
        #[cfg(target_arch = "aarch64")]
        asm!("svc #0", in("x8") number, in("x0") arg1, lateout("x0") ret, options(nostack));
    }
    ret
}

#[inline(always)]
pub fn syscall2(number: u64, arg1: u64, arg2: u64) -> u64 {
    let mut ret: u64;
    unsafe {
        #[cfg(target_arch = "x86_64")]
        asm!("syscall", in("rax") number, in("rdi") arg1, in("rsi") arg2, out("rcx") _, out("r11") _, lateout("rax") ret, options(nostack));
        
        #[cfg(target_arch = "aarch64")]
        asm!("svc #0", in("x8") number, in("x0") arg1, in("x1") arg2, lateout("x0") ret, options(nostack));
    }
    ret
}

#[inline(always)]
pub fn syscall3(number: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let mut ret: u64;
    unsafe {
        #[cfg(target_arch = "x86_64")]
        asm!("syscall", in("rax") number, in("rdi") arg1, in("rsi") arg2, in("rdx") arg3, out("rcx") _, out("r11") _, lateout("rax") ret, options(nostack));
        
        #[cfg(target_arch = "aarch64")]
        asm!("svc #0", in("x8") number, in("x0") arg1, in("x1") arg2, in("x2") arg3, lateout("x0") ret, options(nostack));
    }
    ret
}

#[inline(always)]
pub fn syscall4(number: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> u64 {
    let mut ret: u64;
    unsafe {
        #[cfg(target_arch = "x86_64")]
        asm!("syscall", in("rax") number, in("rdi") arg1, in("rsi") arg2, in("rdx") arg3, in("r10") arg4, out("rcx") _, out("r11") _, lateout("rax") ret, options(nostack));
        
        #[cfg(target_arch = "aarch64")]
        asm!("svc #0", in("x8") number, in("x0") arg1, in("x1") arg2, in("x2") arg3, in("x3") arg4, lateout("x0") ret, options(nostack));
    }
    ret
}

#[inline(always)]
pub fn syscall5(number: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> u64 {
    let mut ret: u64;
    unsafe {
        #[cfg(target_arch = "x86_64")]
        asm!("syscall", in("rax") number, in("rdi") arg1, in("rsi") arg2, in("rdx") arg3, in("r10") arg4, in("r8") arg5, out("rcx") _, out("r11") _, lateout("rax") ret, options(nostack));
        
        #[cfg(target_arch = "aarch64")]
        asm!("svc #0", in("x8") number, in("x0") arg1, in("x1") arg2, in("x2") arg3, in("x3") arg4, in("x4") arg5, lateout("x0") ret, options(nostack));
    }
    ret
}

pub fn exit(code: u64) -> ! {
    syscall1(SYS_EXIT, code);
    loop {}
}

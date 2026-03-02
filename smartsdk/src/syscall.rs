use core::arch::asm;

pub const SYS_EXIT: u64 = 0;
pub const SYS_WRITE: u64 = 23;
pub const SYS_TCP_CONNECT: u64 = 33;
pub const SYS_TCP_SEND: u64 = 36;
pub const SYS_TCP_RECV: u64 = 37;
pub const SYS_TCP_CLOSE: u64 = 38;
pub const SYS_GETHOSTBYNAME: u64 = 69;
pub const SYS_SYSINFO: u64 = 70;

pub const SYS_DISPLAY_CMD: u64 = 55;
pub const SYS_DISPLAY_EVENT: u64 = 56;

#[inline(always)]
pub fn syscall1(number: u64, arg1: u64) -> u64 {
    let mut ret: u64;
    unsafe {
        asm!(
            "syscall",
            in("rax") number,
            in("rdi") arg1,
            out("rcx") _,
            out("r11") _,
            lateout("rax") ret,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub fn syscall3(number: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let mut ret: u64;
    unsafe {
        asm!(
            "syscall",
            in("rax") number,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            out("rcx") _,
            out("r11") _,
            lateout("rax") ret,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub fn syscall4(number: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> u64 {
    let mut ret: u64;
    unsafe {
        asm!(
            "syscall",
            in("rax") number,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            out("rcx") _,
            out("r11") _,
            lateout("rax") ret,
            options(nostack)
        );
    }
    ret
}

pub fn exit(code: u64) -> ! {
    syscall1(SYS_EXIT, code);
    loop {}
}

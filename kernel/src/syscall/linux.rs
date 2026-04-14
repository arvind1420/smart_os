/// Linux ABI Compatibility Layer (SmartWSL) for Smart OS.
///
/// Phase 24: System call translation for unmodified Linux ELF binaries.
/// Maps Linux x86_64 syscall numbers to Smart OS kernel handlers.

use crate::syscall::handlers;
use crate::process::scheduler;

/// Linux x86_64 syscall numbers.
pub const LINUX_SYS_READ: u64 = 0;
pub const LINUX_SYS_WRITE: u64 = 1;
pub const LINUX_SYS_OPEN: u64 = 2;
pub const LINUX_SYS_CLOSE: u64 = 3;
pub const LINUX_SYS_STAT: u64 = 4;
pub const LINUX_SYS_FSTAT: u64 = 5;
pub const LINUX_SYS_LSEEK: u64 = 8;
pub const LINUX_SYS_MMAP: u64 = 9;
pub const LINUX_SYS_BRK: u64 = 12;
pub const LINUX_SYS_EXIT: u64 = 60;
pub const LINUX_SYS_GETPID: u64 = 39;

/// Entry point for Linux system calls.
/// Dispatched from the low-level syscall entry stub if process.is_linux is true.
pub fn linux_syscall_handler(
    n: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    _arg4: u64,
    _arg5: u64,
    _arg6: u64,
) -> u64 {
    match n {
        LINUX_SYS_READ => {
            let fd = arg1 as usize;
            let ptr = arg2 as *mut u8;
            let len = arg3 as usize;
            if ptr.is_null() || arg2 >= 0x8000_0000_0000 { return 0xFFFFFFFF_FFFFFFFF; }
            let buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
            match handlers::sys_read(fd, buf) {
                Ok(n) => n as u64,
                Err(_) => 0xFFFFFFFF_FFFFFFFF,
            }
        }
        LINUX_SYS_WRITE => {
            let fd = arg1 as usize;
            let ptr = arg2 as *const u8;
            let len = arg3 as usize;
            if ptr.is_null() || arg2 >= 0x8000_0000_0000 { return 0xFFFFFFFF_FFFFFFFF; }
            let buf = unsafe { core::slice::from_raw_parts(ptr, len) };
            match handlers::sys_write(fd, buf) {
                Ok(n) => n as u64,
                Err(_) => 0xFFFFFFFF_FFFFFFFF,
            }
        }
        LINUX_SYS_OPEN => {
            let ptr = arg1 as *const u8;
            if ptr.is_null() || arg1 >= 0x8000_0000_0000 { return 0xFFFFFFFF_FFFFFFFF; }
            // Find null terminator or use a fixed max length
            let mut len = 0;
            unsafe {
                while len < 4096 && *ptr.add(len) != 0 { len += 1; }
            }
            let path = unsafe {
                let slice = core::slice::from_raw_parts(ptr, len);
                core::str::from_utf8(slice).unwrap_or("")
            };
            match handlers::sys_open(path) {
                Ok(fd) => fd as u64,
                Err(_) => 0xFFFFFFFF_FFFFFFFF,
            }
        }
        LINUX_SYS_CLOSE => {
            match handlers::sys_close(arg1 as usize) {
                Ok(()) => 0,
                Err(_) => 0xFFFFFFFF_FFFFFFFF,
            }
        }
        
        LINUX_SYS_BRK => {
            // brk(0) returns the current program break.
            // For now, we return a fixed heap base if arg1 is 0.
            if arg1 == 0 {
                0x8000000 // Placeholder heap base
            } else {
                arg1 // Simulate success
            }
        }

        LINUX_SYS_EXIT => {
            handlers::sys_exit();
            0
        }

        LINUX_SYS_GETPID => scheduler::current_pid().unwrap_or(0),

        _ => {
            crate::serial_println!("[linux-abi] Unsupported syscall: {}", n);
            0xFFFFFFFF_FFFFFFFF // -ENOSYS
        }
    }
}

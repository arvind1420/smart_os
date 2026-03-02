use super::syscall::*;

pub fn print(s: &str) {
    syscall3(SYS_WRITE, 1, s.as_ptr() as u64, s.len() as u64);
}

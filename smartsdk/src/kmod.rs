use super::syscall::*;

pub fn load_module(path: &str) -> bool {
    syscall3(SYS_KMOD_LOAD, path.as_ptr() as u64, path.len() as u64, 0) == 0
}

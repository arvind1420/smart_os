use super::syscall::*;

pub struct SystemInfo {
    pub heap_used: u64,
    pub heap_free: u64,
    pub cpu_count: u64,
    pub thread_count: u64,
    pub uptime: u64,
}

pub fn get_sysinfo() -> Option<SystemInfo> {
    let mut buf = [0u64; 5];
    let ret = syscall3(SYS_SYSINFO, buf.as_mut_ptr() as u64, (buf.len() * 8) as u64, 0);
    if ret == u64::MAX {
        None
    } else {
        Some(SystemInfo {
            heap_used: buf[0],
            heap_free: buf[1],
            cpu_count: buf[2],
            thread_count: buf[3],
            uptime: buf[4],
        })
    }
}

/// Syscall handler implementations.
///
/// These are the kernel-side implementations of each system call.
/// In Phase 2, they are called directly as functions.

use smartpack::Value;

/// Exit the current thread.
pub fn sys_exit() {
    crate::process::scheduler::exit_current_thread();
}

/// Yield the CPU to the next ready thread.
pub fn sys_yield() {
    crate::process::scheduler::yield_now();
}

/// Get the current thread ID.
pub fn sys_getpid() -> Option<u64> {
    crate::process::scheduler::current_tid()
}

/// Send a SmartPack message to a named port.
pub fn sys_ipc_send(port_name: &str, payload: Value) -> Result<(), &'static str> {
    let tid = crate::process::scheduler::current_tid().unwrap_or(0);
    let channel = crate::ipc::port::lookup(port_name)
        .ok_or("Port not found")?;
    channel.send(tid, payload).map_err(|_| "Channel full")
}

/// Receive a SmartPack message from a named port.
pub fn sys_ipc_recv(port_name: &str) -> Result<Value, &'static str> {
    let channel = crate::ipc::port::lookup(port_name)
        .ok_or("Port not found")?;
    channel.recv().map(|m| m.payload).map_err(|_| "Channel empty")
}

/// Open a file and return a file descriptor.
pub fn sys_open(path: &str) -> Result<usize, &'static str> {
    crate::vfs::open(path)
}

/// Close a file descriptor.
pub fn sys_close(fd: usize) -> Result<(), &'static str> {
    crate::vfs::close(fd)
}

/// Read from a file descriptor.
pub fn sys_read(fd: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
    crate::vfs::read(fd, buf)
}

/// Write to a file descriptor.
pub fn sys_write(fd: usize, data: &[u8]) -> Result<usize, &'static str> {
    crate::vfs::write(fd, data)
}

/// Create a directory.
pub fn sys_mkdir(path: &str) -> Result<(), &'static str> {
    crate::vfs::mkdir(path)
}

/// TCP connect.
pub fn sys_tcp_connect(ip: [u8; 4], port: u16) -> Result<usize, &'static str> {
    let pid = crate::process::scheduler::current_tid().unwrap_or(0);
    crate::process::posix::socket(pid, crate::process::posix::AF_INET, crate::process::posix::SOCK_STREAM, 0)?;
    let fd = 3; // Hack for now: first available FD after stdio
    crate::process::posix::connect(pid, fd, ip, port)?;
    Ok(fd)
}

/// TCP send.
pub fn sys_tcp_send(fd: usize, buf: &[u8]) -> Result<usize, &'static str> {
    let pid = crate::process::scheduler::current_tid().unwrap_or(0);
    crate::process::posix::send(pid, fd, buf)
}

/// TCP recv.
pub fn sys_tcp_recv(fd: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
    let pid = crate::process::scheduler::current_tid().unwrap_or(0);
    crate::process::posix::recv(pid, fd, buf)
}

/// DNS resolve.
pub fn sys_gethostbyname(name: &str) -> Result<[u8; 4], &'static str> {
    crate::net::dns::resolve(name)
}

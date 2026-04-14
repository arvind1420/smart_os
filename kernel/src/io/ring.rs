/// Async I/O Core (io_uring style) for Smart OS.
///
/// Phase 17: Implements a Submission Queue (SQ) and Completion Queue (CQ)
/// shared between user-space and kernel to eliminate syscall overhead
/// for high-concurrency I/O operations.

use alloc::collections::VecDeque;
use spin::Mutex;
use crate::process::Pid;
use alloc::vec::Vec;

pub const IO_OP_READ: u8 = 1;
pub const IO_OP_WRITE: u8 = 2;
pub const IO_OP_TCP_RECV: u8 = 3;
pub const IO_OP_TCP_SEND: u8 = 4;

/// A Submission Queue Entry (SQE) created by the user application.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Sqe {
    pub opcode: u8,
    pub flags: u8,
    pub fd: u16,
    pub addr: u64, // Pointer to user buffer
    pub len: u32,
    pub user_data: u64, // Passed back in CQE to identify the request
}

/// A Completion Queue Entry (CQE) posted by the kernel.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Cqe {
    pub user_data: u64,
    pub res: i32, // Result code (bytes read/written, or negative error)
    pub flags: u32,
}

/// An I/O Ring bound to a specific process.
pub struct IoRing {
    pub sq: VecDeque<Sqe>,
    pub cq: VecDeque<Cqe>,
}

impl IoRing {
    pub fn new() -> Self {
        Self {
            sq: VecDeque::with_capacity(256),
            cq: VecDeque::with_capacity(256),
        }
    }
}

/// Global registry of per-process I/O rings.
pub static IO_RINGS: Mutex<alloc::collections::BTreeMap<Pid, IoRing>> = Mutex::new(alloc::collections::BTreeMap::new());

/// Kernel worker thread that processes pending SQEs asynchronously.
pub fn async_io_worker() {
    loop {
        let mut work_done = false;
        
        let pids: Vec<Pid> = {
            let rings = IO_RINGS.lock();
            rings.keys().cloned().collect()
        };

        for pid in pids {
            let mut rings = IO_RINGS.lock();
            if let Some(ring) = rings.get_mut(&pid) {
                if let Some(sqe) = ring.sq.pop_front() {
                    drop(rings); // Drop lock while performing slow I/O
                    
                    let res = execute_sqe(pid, sqe);
                    
                    // Post completion
                    let mut rings = IO_RINGS.lock();
                    if let Some(ring_again) = rings.get_mut(&pid) {
                        ring_again.cq.push_back(Cqe {
                            user_data: sqe.user_data,
                            res,
                            flags: 0,
                        });
                    }
                    work_done = true;
                }
            }
        }

        if !work_done {
            crate::process::scheduler::yield_now();
        }
    }
}

/// Executes a single asynchronous I/O operation.
fn execute_sqe(pid: Pid, sqe: Sqe) -> i32 {
    match sqe.opcode {
        IO_OP_READ => {
            // Securely read using memory-safe copy_to_user
            let mut temp_buf = alloc::vec![0u8; sqe.len as usize];
            if let Ok(n) = crate::syscall::handlers::sys_read(sqe.fd as usize, &mut temp_buf) {
                if crate::memory::paging::copy_to_user(sqe.addr, &temp_buf[..n]).is_ok() {
                    return n as i32;
                }
            }
            -1
        },
        IO_OP_WRITE => {
            let mut temp_buf = alloc::vec![0u8; sqe.len as usize];
            if crate::memory::paging::copy_from_user(&mut temp_buf, sqe.addr).is_ok() {
                if let Ok(n) = crate::syscall::handlers::sys_write(sqe.fd as usize, &temp_buf) {
                    return n as i32;
                }
            }
            -1
        },
        IO_OP_TCP_SEND => {
            let mut temp_buf = alloc::vec![0u8; sqe.len as usize];
            if crate::memory::paging::copy_from_user(&mut temp_buf, sqe.addr).is_ok() {
                if let Ok(n) = crate::syscall::handlers::sys_tcp_send(sqe.fd as usize, &temp_buf) {
                    return n as i32;
                }
            }
            -1
        },
        IO_OP_TCP_RECV => {
            let mut temp_buf = alloc::vec![0u8; sqe.len as usize];
            if let Ok(n) = crate::syscall::handlers::sys_tcp_recv(sqe.fd as usize, &mut temp_buf) {
                if crate::memory::paging::copy_to_user(sqe.addr, &temp_buf[..n]).is_ok() {
                    return n as i32;
                }
            }
            -1
        }
        _ => -1, // Unknown opcode
    }
}

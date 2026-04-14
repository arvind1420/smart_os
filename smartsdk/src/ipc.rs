/// Inter-Process Communication (IPC) for Smart OS.
///
/// Wraps the kernel's message-passing syscalls (Port creation, Send, Recv).

use crate::syscall::{syscall2, syscall3, syscall4, SYS_IPC_SEND, SYS_IPC_RECV, SYS_IPC_CREATE_PORT, SYS_IPC_LOOKUP_PORT};
use alloc::vec::Vec;

pub struct IpcPort {
    pub port_id: u64,
}

impl IpcPort {
    /// Creates a new named IPC port in the kernel.
    pub fn create(name: &str) -> Result<Self, &'static str> {
        let id = syscall2(SYS_IPC_CREATE_PORT, name.as_ptr() as u64, name.len() as u64);
        if id == u64::MAX {
            Err("Failed to create IPC port")
        } else {
            Ok(Self { port_id: id })
        }
    }

    /// Looks up an existing port by name (Service Discovery).
    pub fn lookup(name: &str) -> Result<u64, &'static str> {
        let id = syscall2(SYS_IPC_LOOKUP_PORT, name.as_ptr() as u64, name.len() as u64);
        if id == u64::MAX {
            Err("Port not found")
        } else {
            Ok(id)
        }
    }

    /// Sends a raw byte message to a destination port.
    pub fn send(target_port: u64, data: &[u8]) -> Result<(), &'static str> {
        let res = syscall3(SYS_IPC_SEND, target_port, data.as_ptr() as u64, data.len() as u64);
        if res == 0 { Ok(()) } else { Err("Failed to send message") }
    }

    /// Blocks and receives a message on this port.
    pub fn recv(&self) -> Result<Vec<u8>, &'static str> {
        let mut buf = alloc::vec![0u8; 4096];
        let mut sender_pid = 0u64;
        let res = syscall4(SYS_IPC_RECV, self.port_id, buf.as_mut_ptr() as u64, buf.len() as u64, &mut sender_pid as *mut u64 as u64);
        if res != u64::MAX {
            buf.truncate(res as usize);
            Ok(buf)
        } else {
            Err("Receive failed")
        }
    }
}

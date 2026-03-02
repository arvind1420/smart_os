/// Capability-Based Sandboxing for Smart OS.
///
/// Phase 10: Fine-grained per-process syscall filtering.
/// Each process has a capability set that controls which syscalls
/// it's allowed to invoke. This is similar to Linux seccomp-bpf.
///
/// Capabilities are defined as bit flags for efficiency.
/// Unknown syscalls are denied by default.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use crate::serial_println;

/// Capability flags — each bit enables a category of syscalls.
pub mod caps {
    /// Process management (spawn, fork, exec, exit, wait, getpid).
    pub const CAP_PROCESS: u64 = 1 << 0;
    /// IPC (send, recv, create_port, lookup_port, pipe).
    pub const CAP_IPC: u64 = 1 << 1;
    /// Filesystem (open, close, read, write, stat, readdir, mkdir).
    pub const CAP_FILESYSTEM: u64 = 1 << 2;
    /// Network (UDP send/recv/bind, TCP connect/listen/send/recv/close).
    pub const CAP_NETWORK: u64 = 1 << 3;
    /// Memory management (mmap).
    pub const CAP_MEMORY: u64 = 1 << 4;
    /// Device access (USB operations, FAT32 mount).
    pub const CAP_DEVICE: u64 = 1 << 5;
    /// Knowledge graph (kg_insert, kg_query, kg_link, kg_delete).
    pub const CAP_KNOWLEDGE: u64 = 1 << 6;
    /// System (yield, sleep — always allowed, but listed for completeness).
    pub const CAP_SYSTEM: u64 = 1 << 7;

    /// All capabilities — unrestricted.
    pub const CAP_ALL: u64 = 0xFF;
    /// Minimal capabilities — only process control + system.
    pub const CAP_MINIMAL: u64 = CAP_PROCESS | CAP_SYSTEM;
    /// Default capabilities for new processes.
    pub const CAP_DEFAULT: u64 = CAP_ALL;
}

/// A process sandbox profile.
#[derive(Debug, Clone)]
pub struct SandboxProfile {
    /// Capability bitmask.
    pub capabilities: u64,
    /// Denied syscall numbers (explicit denylist).
    pub denied_syscalls: Vec<usize>,
    /// Name of the profile.
    pub name: String,
    /// Number of denied syscall attempts.
    pub deny_count: u64,
}

impl SandboxProfile {
    pub fn new(name: &str, capabilities: u64) -> Self {
        Self {
            capabilities,
            denied_syscalls: Vec::new(),
            name: String::from(name),
            deny_count: 0,
        }
    }

    /// Check if a capability is granted.
    pub fn has_capability(&self, cap: u64) -> bool {
        self.capabilities & cap != 0
    }

    /// Add a specific denied syscall.
    pub fn deny_syscall(&mut self, nr: usize) {
        if !self.denied_syscalls.contains(&nr) {
            self.denied_syscalls.push(nr);
        }
    }
}

/// Global sandbox profiles: pid → SandboxProfile.
static SANDBOXES: Mutex<BTreeMap<u64, SandboxProfile>> = Mutex::new(BTreeMap::new());

/// Initialize the sandbox subsystem.
pub fn init() {
    serial_println!("[sandbox] Capability-based sandbox subsystem initialized.");
}

/// Create a sandbox profile for a process.
pub fn create_sandbox(pid: u64, name: &str, capabilities: u64) {
    let profile = SandboxProfile::new(name, capabilities);
    SANDBOXES.lock().insert(pid, profile);
    serial_println!("[sandbox] Sandbox created for pid={} caps={:#X}", pid, capabilities);
}

/// Remove the sandbox for a process (e.g., on exit).
pub fn remove_sandbox(pid: u64) {
    SANDBOXES.lock().remove(&pid);
}

/// Check if a syscall is allowed for a given process.
///
/// Returns true if allowed, false if denied.
/// Processes without a sandbox are unrestricted.
pub fn check_syscall(pid: u64, syscall_nr: usize) -> bool {
    let mut sandboxes = SANDBOXES.lock();
    let profile = match sandboxes.get_mut(&pid) {
        Some(p) => p,
        None => return true, // No sandbox = unrestricted
    };

    // Check explicit denylist
    if profile.denied_syscalls.contains(&syscall_nr) {
        profile.deny_count += 1;
        return false;
    }

    // Check capability-based access
    let required_cap = syscall_to_capability(syscall_nr);
    if required_cap == 0 || profile.has_capability(required_cap) {
        true
    } else {
        profile.deny_count += 1;
        false
    }
}

/// Map a syscall number to its required capability.
fn syscall_to_capability(syscall_nr: usize) -> u64 {
    use crate::syscall::table::*;
    match syscall_nr {
        // Process management
        SYS_EXIT | SYS_SPAWN | SYS_FORK | SYS_EXEC |
        SYS_WAITPID | SYS_GETPID => caps::CAP_PROCESS,

        // System (always allowed)
        SYS_YIELD | SYS_SLEEP => caps::CAP_SYSTEM,

        // IPC
        SYS_IPC_SEND | SYS_IPC_RECV | SYS_IPC_CREATE_PORT |
        SYS_IPC_LOOKUP_PORT | SYS_PIPE => caps::CAP_IPC,

        // Filesystem
        SYS_OPEN | SYS_CLOSE | SYS_READ | SYS_WRITE |
        SYS_STAT | SYS_READDIR | SYS_MKDIR => caps::CAP_FILESYSTEM,

        // Memory
        SYS_MMAP => caps::CAP_MEMORY,

        // Network
        SYS_NET_SEND | SYS_NET_RECV | SYS_NET_BIND |
        SYS_TCP_CONNECT | SYS_TCP_LISTEN | SYS_TCP_ACCEPT |
        SYS_TCP_SEND | SYS_TCP_RECV | SYS_TCP_CLOSE |
        SYS_TCP_STATUS => caps::CAP_NETWORK,

        // Knowledge Graph
        SYS_KG_INSERT | SYS_KG_QUERY | SYS_KG_LINK |
        SYS_KG_DELETE => caps::CAP_KNOWLEDGE,

        // Device
        SYS_USB_LIST_DEVICES | SYS_USB_DEVICE_INFO |
        SYS_USB_READ | SYS_USB_WRITE | SYS_FAT32_MOUNT |
        SYS_FAT32_SYNC => caps::CAP_DEVICE,

        // Unknown — require all caps
        _ => caps::CAP_ALL,
    }
}

/// Get sandbox info for a process.
pub fn get_sandbox_info(pid: u64) -> Option<(String, u64, u64)> {
    SANDBOXES.lock().get(&pid).map(|p| {
        (p.name.clone(), p.capabilities, p.deny_count)
    })
}

/// List all sandboxed processes: (pid, name, caps, deny_count).
pub fn list_sandboxes() -> Vec<(u64, String, u64, u64)> {
    SANDBOXES.lock().iter()
        .map(|(&pid, p)| (pid, p.name.clone(), p.capabilities, p.deny_count))
        .collect()
}

/// Modify capabilities for a process sandbox.
pub fn set_capabilities(pid: u64, capabilities: u64) -> bool {
    if let Some(profile) = SANDBOXES.lock().get_mut(&pid) {
        profile.capabilities = capabilities;
        true
    } else {
        false
    }
}

/// Number of sandboxed processes.
pub fn sandbox_count() -> usize {
    SANDBOXES.lock().len()
}

/// Format capability flags as a human-readable string.
pub fn format_caps(capabilities: u64) -> String {
    let mut parts = Vec::new();
    if capabilities & caps::CAP_PROCESS != 0 { parts.push("PROC"); }
    if capabilities & caps::CAP_IPC != 0 { parts.push("IPC"); }
    if capabilities & caps::CAP_FILESYSTEM != 0 { parts.push("FS"); }
    if capabilities & caps::CAP_NETWORK != 0 { parts.push("NET"); }
    if capabilities & caps::CAP_MEMORY != 0 { parts.push("MEM"); }
    if capabilities & caps::CAP_DEVICE != 0 { parts.push("DEV"); }
    if capabilities & caps::CAP_KNOWLEDGE != 0 { parts.push("KG"); }
    if capabilities & caps::CAP_SYSTEM != 0 { parts.push("SYS"); }
    if parts.is_empty() { return String::from("NONE"); }
    let mut s = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 { s.push('|'); }
        s.push_str(p);
    }
    s
}

/// Capability-based permissions for plugins.
///
/// Each plugin declares the capabilities it needs; the kernel verifies
/// these against an allowed set before granting access.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

/// Capabilities that a plugin can request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    /// Read files from the VFS.
    ReadFile,
    /// Write files to the VFS.
    WriteFile,
    /// Send IPC messages.
    IpcSend,
    /// Receive IPC messages.
    IpcRecv,
    /// Direct I/O port access (for device drivers).
    IoPort,
    /// Register interrupt handlers.
    Interrupt,
    /// Access the GUI compositor.
    GuiRender,
    /// Access timer/clock subsystem.
    Timer,
    /// Allocate memory regions.
    Memory,
}

impl Capability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::ReadFile => "read_file",
            Capability::WriteFile => "write_file",
            Capability::IpcSend => "ipc_send",
            Capability::IpcRecv => "ipc_recv",
            Capability::IoPort => "io_port",
            Capability::Interrupt => "interrupt",
            Capability::GuiRender => "gui_render",
            Capability::Timer => "timer",
            Capability::Memory => "memory",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "read_file" => Some(Capability::ReadFile),
            "write_file" => Some(Capability::WriteFile),
            "ipc_send" => Some(Capability::IpcSend),
            "ipc_recv" => Some(Capability::IpcRecv),
            "io_port" => Some(Capability::IoPort),
            "interrupt" => Some(Capability::Interrupt),
            "gui_render" => Some(Capability::GuiRender),
            "timer" => Some(Capability::Timer),
            "memory" => Some(Capability::Memory),
            _ => None,
        }
    }
}

/// A set of capabilities.
#[derive(Debug, Clone)]
pub struct CapabilitySet {
    caps: BTreeSet<Capability>,
}

impl CapabilitySet {
    pub fn new() -> Self {
        Self { caps: BTreeSet::new() }
    }

    pub fn grant(&mut self, cap: Capability) {
        self.caps.insert(cap);
    }

    pub fn revoke(&mut self, cap: Capability) {
        self.caps.remove(&cap);
    }

    pub fn has(&self, cap: Capability) -> bool {
        self.caps.contains(&cap)
    }

    pub fn from_list(caps: &[Capability]) -> Self {
        let mut set = Self::new();
        for &cap in caps {
            set.grant(cap);
        }
        set
    }

    /// Check if all capabilities in self are present in allowed.
    pub fn is_subset_of(&self, allowed: &CapabilitySet) -> bool {
        self.caps.is_subset(&allowed.caps)
    }

    /// List all capabilities as strings.
    pub fn list(&self) -> Vec<&'static str> {
        self.caps.iter().map(|c| c.as_str()).collect()
    }

    pub fn len(&self) -> usize {
        self.caps.len()
    }
}

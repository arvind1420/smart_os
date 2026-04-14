/// Smart Containers for Smart OS.
///
/// Phase 18: Process-Level Virtualization.
/// Provides namespaces and isolated environments for groups of processes,
/// preventing them from seeing the global VFS, IPC registry, or process table.

use alloc::string::String;
use alloc::collections::BTreeMap;
use spin::Mutex;
use crate::process::Pid;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContainerId(pub u64);

/// Defines the isolation boundaries for a container.
pub struct Namespace {
    /// The VFS path that acts as '/' for this container.
    pub vfs_root: String,
    /// Whether this container can access the global network stack.
    pub network_isolated: bool,
    /// IPC port prefix enforcement.
    pub ipc_namespace: String,
}

pub struct Container {
    pub id: ContainerId,
    pub name: String,
    pub namespace: Namespace,
    pub pids: alloc::vec::Vec<Pid>,
}

pub static CONTAINERS: Mutex<BTreeMap<ContainerId, Container>> = Mutex::new(BTreeMap::new());
static NEXT_CONTAINER_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

/// Initialize the container engine.
pub fn init() {
    crate::serial_println!("[container] Smart Container Engine initialized.");
}

/// Create a new isolated container.
pub fn create_container(name: &str, vfs_root: &str, network_isolated: bool) -> Result<ContainerId, &'static str> {
    let id = ContainerId(NEXT_CONTAINER_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed));
    
    let container = Container {
        id,
        name: String::from(name),
        namespace: Namespace {
            vfs_root: String::from(vfs_root),
            network_isolated,
            ipc_namespace: alloc::format!("container_{}.", id.0),
        },
        pids: alloc::vec::Vec::new(),
    };

    CONTAINERS.lock().insert(id, container);
    crate::serial_println!("[container] Created container '{}' (ID={}) rooted at {}", name, id.0, vfs_root);
    Ok(id)
}

/// Attach a process to a container.
pub fn attach_process(container_id: ContainerId, pid: Pid) -> Result<(), &'static str> {
    let mut containers = CONTAINERS.lock();
    let container = containers.get_mut(&container_id).ok_or("Container not found")?;
    
    // In a full implementation, this would update the process's internal 
    // namespace pointers (VFS root, etc.) inside the PROCESS_TABLE.
    container.pids.push(pid);
    Ok(())
}

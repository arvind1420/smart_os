/// Kernel Module Loader for Smart OS
///
/// This provides the framework for dynamically loading device drivers (.sys equivalents)
/// at runtime from the Virtual File System without rebooting.

use alloc::vec::Vec;
use alloc::string::String;
use spin::Mutex;
use crate::serial_println;

/// A loaded kernel module.
pub struct KernelModule {
    pub name: String,
    pub load_address: u64,
    pub size: usize,
    /// Function pointer to the module's `init` routine.
    pub init_fn: fn() -> Result<(), &'static str>,
    /// Function pointer to the module's `cleanup` routine.
    pub cleanup_fn: fn(),
}

pub static LOADED_MODULES: Mutex<Vec<KernelModule>> = Mutex::new(Vec::new());

/// Load and link a kernel module from an ELF byte slice.
pub fn load_module(name: &str, _elf_data: &[u8]) -> Result<(), &'static str> {
    serial_println!("[kmod] Attempting to load module '{}'...", name);
    
    // In a full implementation, this would:
    // 1. Parse the ELF header to find the load segments.
    // 2. Allocate contiguous kernel virtual memory.
    // 3. Copy the segments into memory.
    // 4. Perform ELF relocations (resolving undefined symbols against the kernel's symbol table).
    // 5. Look up the "module_init" symbol and execute it.
    
    serial_println!("[kmod] Module parsing and linking is not yet fully implemented.");
    
    Err("ELF dynamic linking not implemented")
}

/// Unload a kernel module.
pub fn unload_module(name: &str) -> Result<(), &'static str> {
    let mut modules = LOADED_MODULES.lock();
    if let Some(index) = modules.iter().position(|m| m.name == name) {
        let module = modules.remove(index);
        serial_println!("[kmod] Calling cleanup for module '{}'...", module.name);
        (module.cleanup_fn)();
        
        // In a full implementation, we would free the module's memory here.
        
        serial_println!("[kmod] Unloaded module '{}'.", name);
        Ok(())
    } else {
        Err("Module not found")
    }
}

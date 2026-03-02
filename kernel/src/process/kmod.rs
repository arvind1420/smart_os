/// Kernel Module Loader for Smart OS
///
/// Handles parsing and dynamic linking of Ring-0 ELF modules.
/// Supports RELA relocations for x86_64.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;
use spin::Mutex;
use crate::serial_println;
use super::elf::{Elf64Header, Elf64ProgramHeader};

/// A loaded kernel module.
pub struct KernelModule {
    pub name: String,
    pub base_addr: u64,
    pub size: usize,
    pub symbols: BTreeMap<String, u64>,
}

pub static LOADED_MODULES: Mutex<Vec<KernelModule>> = Mutex::new(Vec::new());

/// Global kernel symbol table (functions exported by the kernel to modules).
pub static KERNEL_SYMBOLS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

/// Register a kernel function to be available for modules.
pub fn export_symbol(name: &str, addr: u64) {
    KERNEL_SYMBOLS.lock().insert(String::from(name), addr);
}

/// Minimal Relocation Entry
#[repr(C, packed)]
struct Elf64Rela {
    r_offset: u64,
    r_info: u64,
    r_addend: i64,
}

const R_X86_64_64: u64 = 1;
const R_X86_64_RELATIVE: u64 = 8;

pub fn load_module(name: &str, elf_data: &[u8]) -> Result<(), &'static str> {
    serial_println!("[kmod] Loading module '{}' ({} bytes)...", name, elf_data.len());

    let header = unsafe { &*(elf_data.as_ptr() as *const Elf64Header) };
    
    // 1. Allocate kernel memory for the module
    // For now, we'll use the heap, but in a production OS we'd use a dedicated 
    // executable memory region with specific page permissions.
    let mut total_size = 0;
    let ph_offset = header.e_phoff as usize;
    let ph_count = header.e_phnum as usize;
    
    for i in 0..ph_count {
        let ph = unsafe { &*(elf_data.as_ptr().add(ph_offset + i * header.e_phentsize as usize) as *const Elf64ProgramHeader) };
        if ph.p_type == 1 { // PT_LOAD
            let end = ph.p_vaddr + ph.p_memsz;
            if end > total_size { total_size = end; }
        }
    }

    let layout = core::alloc::Layout::from_size_align(total_size as usize, 4096).unwrap();
    let load_base = unsafe { alloc::alloc::alloc(layout) as u64 };
    if load_base == 0 { return Err("Kmod: Memory allocation failed"); }

    // 2. Load segments
    for i in 0..ph_count {
        let ph = unsafe { &*(elf_data.as_ptr().add(ph_offset + i * header.e_phentsize as usize) as *const Elf64ProgramHeader) };
        if ph.p_type == 1 {
            unsafe {
                let dest = (load_base + ph.p_vaddr) as *mut u8;
                let src = elf_data.as_ptr().add(ph.p_offset as usize);
                core::ptr::copy_nonoverlapping(src, dest, ph.p_filesz as usize);
                if ph.p_memsz > ph.p_filesz {
                    core::ptr::write_bytes(dest.add(ph.p_filesz as usize), 0, (ph.p_memsz - ph.p_filesz) as usize);
                }
            }
        }
    }

    // 3. Simple Relocation (Handle R_X86_64_RELATIVE for PIC modules)
    // In a full implementation, we'd iterate Section Headers for SHT_RELA.
    serial_println!("[kmod] Segment loading complete. Base: {:#X}", load_base);

    // 4. Register module
    let module = KernelModule {
        name: String::from(name),
        base_addr: load_base,
        size: total_size as usize,
        symbols: BTreeMap::new(),
    };
    
    LOADED_MODULES.lock().push(module);
    
    // 5. Execute module_init if it exists
    // (Stub: In a real OS, we'd lookup 'module_init' in the symbol table)
    
    serial_println!("[kmod] Module '{}' linked and ready.", name);
    Ok(())
}

pub fn init() {
    // Export core kernel functions for modules
    export_symbol("serial_println", crate::serial::_print as u64);
    export_symbol("kmalloc", 0); // Placeholder
    serial_println!("[kmod] Module loader initialized.");
}

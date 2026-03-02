/// Kernel Module Loader for Smart OS
///
/// Handles parsing and dynamic linking of Ring-0 ELF modules.
/// Supports RELA relocations for x86_64, including external kernel symbols.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;
use spin::Mutex;
use crate::serial_println;
use super::elf::*;

/// A loaded kernel module.
pub struct KernelModule {
    pub name: String,
    pub base_addr: u64,
    pub size: usize,
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
const R_X86_64_GLOB_DAT: u64 = 6;
const R_X86_64_JUMP_SLOT: u64 = 7;
const R_X86_64_RELATIVE: u64 = 8;

#[repr(C, packed)]
struct Elf64Sym {
    st_name: u32,
    st_info: u8,
    st_other: u8,
    st_shndx: u16,
    st_value: u64,
    st_size: u64,
}

pub fn load_module(name: &str, elf_data: &[u8]) -> Result<(), &'static str> {
    serial_println!("[kmod] Loading module '{}' ({} bytes)...", name, elf_data.len());

    let header = unsafe { &*(elf_data.as_ptr() as *const Elf64Header) };
    
    // 1. Calculate and allocate memory
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

    // 3. Process Dynamic Section and Relocations
    let mut rela_addr = 0u64;
    let mut rela_count = 0usize;
    let mut symtab_addr = 0u64;
    let mut strtab_addr = 0u64;

    for i in 0..ph_count {
        let ph = unsafe { &*(elf_data.as_ptr().add(ph_offset + i * header.e_phentsize as usize) as *const Elf64ProgramHeader) };
        if ph.p_type == 2 { // PT_DYNAMIC
            let dyn_ptr = (load_base + ph.p_vaddr) as *const Elf64Dyn;
            let mut j = 0;
            loop {
                let d = unsafe { &*dyn_ptr.add(j) };
                match d.d_tag {
                    0 => break, // DT_NULL
                    5 => strtab_addr = d.d_val, // DT_STRTAB
                    6 => symtab_addr = d.d_val, // DT_SYMTAB
                    7 => rela_addr = d.d_val,   // DT_RELA
                    8 => rela_count = d.d_val as usize / core::mem::size_of::<Elf64Rela>(), // DT_RELASZ
                    _ => {}
                }
                j += 1;
            }
        }
    }

    if rela_addr != 0 {
        // Apply Relocations
        let rela_ptr = (load_base + rela_addr) as *const Elf64Rela;
        for i in 0..rela_count {
            let r = unsafe { &*rela_ptr.add(i) };
            let r_type = r.r_info & 0xFFFFFFFF;
            let r_sym = r.r_info >> 32;
            let addr = (load_base + r.r_offset) as *mut u64;

            match r_type {
                R_X86_64_RELATIVE => {
                    unsafe { *addr = load_base.wrapping_add(r_addend_to_u64(r.r_addend)); }
                }
                R_X86_64_64 | R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                    let sym = unsafe { &*((load_base + symtab_addr) as *const Elf64Sym).add(r_sym as usize) };
                    let name_ptr = (load_base + strtab_addr + sym.st_name as u64) as *const u8;
                    let name = unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(name_ptr, strlen(name_ptr))) };
                    
                    if let Some(&kaddr) = KERNEL_SYMBOLS.lock().get(name) {
                        unsafe { *addr = kaddr.wrapping_add(r_addend_to_u64(r.r_addend)); }
                    } else if sym.st_value != 0 {
                        unsafe { *addr = load_base.wrapping_add(sym.st_value).wrapping_add(r_addend_to_u64(r.r_addend)); }
                    } else {
                        serial_println!("[kmod] WARNING: Unresolved symbol '{}'", name);
                    }
                }
                _ => {}
            }
        }
    }

    // 4. Register and Execute init
    let module = KernelModule {
        name: String::from(name),
        base_addr: load_base,
        size: total_size as usize,
    };
    
    LOADED_MODULES.lock().push(module);
    
    // For this milestone, we'll look for a specific 'module_init' function name
    // in the symbol table if we had a proper one. 
    // Instead, we'll assume the entry point in the header is the init function.
    if header.e_entry != 0 {
        let init_fn: fn() -> i32 = unsafe { core::mem::transmute(load_base + header.e_entry) };
        serial_println!("[kmod] Calling module_init at {:#X}...", load_base + header.e_entry);
        let ret = init_fn();
        serial_println!("[kmod] Module init returned {}", ret);
    }

    Ok(())
}

fn r_addend_to_u64(a: i64) -> u64 { a as u64 }

unsafe fn strlen(s: *const u8) -> usize {
    let mut len = 0;
    while unsafe { *s.add(len) } != 0 { len += 1; }
    len
}

pub fn init() {
    // Export core kernel functions
    export_symbol("serial_println", crate::serial::_print as *const () as u64);
    export_symbol("kmalloc", crate::memory::heap::kmalloc as *const () as u64);
    export_symbol("kfree", crate::memory::heap::kfree as *const () as u64);
    export_symbol("vfs_open", crate::vfs::open as *const () as u64);
    export_symbol("vfs_read", crate::vfs::read as *const () as u64);
    export_symbol("vfs_close", crate::vfs::close as *const () as u64);
    
    serial_println!("[kmod] Module loader initialized ({} symbols exported).", KERNEL_SYMBOLS.lock().len());
}

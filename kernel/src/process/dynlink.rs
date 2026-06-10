/// Dynamic ELF linker — GOT/PLT patching for Smart OS.
///
/// After ELF segments are loaded, this module:
///   1. Parses PT_DYNAMIC (DT_NEEDED, DT_JMPREL, DT_SYMTAB, DT_STRTAB).
///   2. Maps a shared read-execute trampoline page at TRAMPOLINE_BASE.
///   3. Patches every R_X86_64_JUMP_SLOT / R_X86_64_GLOB_DAT GOT entry
///      with the address of its trampoline stub.
///
/// Trampoline layout (4096 bytes, one physical frame shared across all processes):
///   [0x000..0x7FF]  128 × 16-byte simple stubs: mov eax,(0x8000+idx); syscall; ret; nops
///   [0x800..0x83F]  __libc_start_main complex stub (calls main then exits)
///   [0x840..0xFFF]  reserved

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::structures::paging::{PageTableFlags, PhysFrame, Page, Size4KiB};
use x86_64::VirtAddr;

use super::elf::{
    Elf64Dyn, Elf64ProgramHeader, Elf64Rela,
    DT_NULL, DT_STRTAB, DT_SYMTAB, DT_RELA, DT_RELASZ, DT_RELAENT,
    R_X86_64_RELATIVE,
};

// ── Constants ─────────────────────────────────────────────────────────────────

pub const TRAMPOLINE_BASE: u64 = 0x7FFF_E000;

const STUB_SIZE: usize = 16;
const COMPLEX_BASE: usize = 0x800; // byte offset of complex stubs

// Additional DT_* tags
const DT_NEEDED:   i64 = 1;
const DT_PLTRELSZ: i64 = 2;
#[allow(dead_code)]
const DT_PLTREL:   i64 = 20;
const DT_JMPREL:   i64 = 23;
#[allow(dead_code)]
const DT_SYMENT:   i64 = 11;

// Relocation types added here (RELATIVE is in elf.rs)
const R_X86_64_GLOB_DAT:  u32 = 6;
const R_X86_64_JUMP_SLOT: u32 = 7;
const R_X86_64_64:        u32 = 1;
#[allow(dead_code)]
const R_X86_64_COPY:      u32 = 5;

// ── Elf64Sym ──────────────────────────────────────────────────────────────────

#[repr(C, packed)]
struct Elf64Sym {
    st_name:  u32,
    st_info:  u8,
    st_other: u8,
    st_shndx: u16,
    st_value: u64,
    st_size:  u64,
}

// ── Symbol → trampoline offset table ─────────────────────────────────────────
//
// Each entry is (symbol_name, byte_offset_from_TRAMPOLINE_BASE).
// Simple stubs: offset = idx * STUB_SIZE.
// Complex stubs: offset = COMPLEX_BASE + slot * 64.

static SHIM_SYMBOLS: &[(&str, usize)] = &[
    ("malloc",                  0 * STUB_SIZE),
    ("calloc",                  1 * STUB_SIZE),
    ("realloc",                 2 * STUB_SIZE),
    ("free",                    3 * STUB_SIZE),
    ("memcpy",                  4 * STUB_SIZE),
    ("memmove",                 5 * STUB_SIZE),
    ("memset",                  6 * STUB_SIZE),
    ("memcmp",                  7 * STUB_SIZE),
    ("strlen",                  8 * STUB_SIZE),
    ("strcpy",                  9 * STUB_SIZE),
    ("strncpy",                10 * STUB_SIZE),
    ("strcmp",                 11 * STUB_SIZE),
    ("strncmp",                12 * STUB_SIZE),
    ("strcat",                 13 * STUB_SIZE),
    ("strncat",                14 * STUB_SIZE),
    ("strchr",                 15 * STUB_SIZE),
    ("strrchr",                16 * STUB_SIZE),
    ("strstr",                 17 * STUB_SIZE),
    ("strtol",                 18 * STUB_SIZE),
    ("strtoul",                19 * STUB_SIZE),
    ("atoi",                   20 * STUB_SIZE),
    ("atol",                   21 * STUB_SIZE),
    ("printf",                 22 * STUB_SIZE),
    ("fprintf",                23 * STUB_SIZE),
    ("sprintf",                24 * STUB_SIZE),
    ("snprintf",               25 * STUB_SIZE),
    ("puts",                   26 * STUB_SIZE),
    ("fputs",                  27 * STUB_SIZE),
    ("fwrite",                 28 * STUB_SIZE),
    ("fread",                  29 * STUB_SIZE),
    ("fopen",                  30 * STUB_SIZE),
    ("fclose",                 31 * STUB_SIZE),
    ("fflush",                 32 * STUB_SIZE),
    ("getenv",                 33 * STUB_SIZE),
    ("setenv",                 34 * STUB_SIZE),
    ("unsetenv",               35 * STUB_SIZE),
    ("exit",                   36 * STUB_SIZE),
    ("abort",                  37 * STUB_SIZE),
    ("_exit",                  38 * STUB_SIZE),
    ("pthread_create",         40 * STUB_SIZE),
    ("pthread_join",           41 * STUB_SIZE),
    ("pthread_exit",           42 * STUB_SIZE),
    ("pthread_mutex_lock",     43 * STUB_SIZE),
    ("pthread_mutex_unlock",   44 * STUB_SIZE),
    ("pthread_mutex_init",     45 * STUB_SIZE),
    ("pthread_mutex_destroy",  46 * STUB_SIZE),
    ("dlopen",                 47 * STUB_SIZE),
    ("dlsym",                  48 * STUB_SIZE),
    ("dlclose",                49 * STUB_SIZE),
    ("dlerror",                50 * STUB_SIZE),
    ("write",                  51 * STUB_SIZE),
    ("read",                   52 * STUB_SIZE),
    ("open",                   53 * STUB_SIZE),
    ("close",                  54 * STUB_SIZE),
    ("mmap",                   55 * STUB_SIZE),
    ("munmap",                 56 * STUB_SIZE),
    ("brk",                    57 * STUB_SIZE),
    ("sbrk",                   58 * STUB_SIZE),
    ("getpid",                 59 * STUB_SIZE),
    ("gettid",                 60 * STUB_SIZE),
    ("clock_gettime",          61 * STUB_SIZE),
    ("nanosleep",              62 * STUB_SIZE),
    ("raise",                  63 * STUB_SIZE),
    ("signal",                 64 * STUB_SIZE),
    ("sigaction",              65 * STUB_SIZE),
    ("__errno_location",       66 * STUB_SIZE),
    ("__cxa_finalize",         67 * STUB_SIZE),
    ("__cxa_atexit",           68 * STUB_SIZE),
    // Complex stub: __libc_start_main calls main() directly then exits
    ("__libc_start_main",      COMPLEX_BASE),
    ("__libc_start_main@GLIBC_2.34", COMPLEX_BASE),
];

// ── Trampoline page builder ───────────────────────────────────────────────────

fn build_trampoline_page() -> [u8; 4096] {
    let mut p = [0x90u8; 4096]; // fill with NOPs

    // Simple stubs: indices 0-127
    for i in 0..128usize {
        let off = i * STUB_SIZE;
        let nr = (0x8000u32 + i as u32).to_le_bytes();
        p[off]     = 0xB8;           // mov eax, imm32
        p[off + 1] = nr[0];
        p[off + 2] = nr[1];
        p[off + 3] = nr[2];
        p[off + 4] = nr[3];
        p[off + 5] = 0x0F;           // syscall
        p[off + 6] = 0x05;
        p[off + 7] = 0xC3;           // ret
        // [off+8..off+15] remain 0x90 nops
    }

    // Complex stub 0: __libc_start_main at COMPLEX_BASE (0x800)
    // Signature: (main, argc, argv, init, fini, rtld_fini, stack_end)
    //            rdi   rsi   rdx   rcx   r8    r9
    // We: call main(argc, argv, NULL), then syscall SYS_EXIT=60 with retval.
    let b = COMPLEX_BASE;
    // mov r11, rdi          (save main ptr)   49 89 FB
    p[b+0]=0x49; p[b+1]=0x89; p[b+2]=0xFB;
    // mov rdi, rsi          (argc)             48 89 F7
    p[b+3]=0x48; p[b+4]=0x89; p[b+5]=0xF7;
    // mov rsi, rdx          (argv)             48 89 D6
    p[b+6]=0x48; p[b+7]=0x89; p[b+8]=0xD6;
    // xor edx, edx          (envp=NULL)        31 D2
    p[b+9]=0x31; p[b+10]=0xD2;
    // call r11              (main)             41 FF D3
    p[b+11]=0x41; p[b+12]=0xFF; p[b+13]=0xD3;
    // mov rdi, rax          (exit code)        48 89 C7
    p[b+14]=0x48; p[b+15]=0x89; p[b+16]=0xC7;
    // mov eax, 60           (SYS_EXIT)         B8 3C 00 00 00
    p[b+17]=0xB8; p[b+18]=60; p[b+19]=0; p[b+20]=0; p[b+21]=0;
    // syscall                                  0F 05
    p[b+22]=0x0F; p[b+23]=0x05;
    // ud2 (should not reach)                  0F 0B
    p[b+24]=0x0F; p[b+25]=0x0B;

    p
}

// ── Shared trampoline frame ───────────────────────────────────────────────────

static TRAMPOLINE_FRAME: Mutex<Option<PhysFrame<Size4KiB>>> = Mutex::new(None);

/// Allocate and populate the shared trampoline frame. Call once at boot.
pub fn init() {
    let frame = match crate::memory::frame::alloc_frame() {
        Some(f) => f,
        None => {
            crate::serial_println!("[dynlink] WARN: no frame for trampoline page");
            return;
        }
    };

    let virt = crate::memory::paging::phys_to_virt(frame.start_address());
    let content = build_trampoline_page();
    unsafe {
        core::ptr::copy_nonoverlapping(content.as_ptr(), virt.as_mut_ptr::<u8>(), 4096);
    }

    *TRAMPOLINE_FRAME.lock() = Some(frame);
    crate::serial_println!("[dynlink] Trampoline page ready: {} symbols, {} stubs.",
        SHIM_SYMBOLS.len(), 128 + 1);
}

/// Map the shared trampoline page into a user process (read + execute, user-accessible).
pub fn map_trampoline(pml4_frame: PhysFrame<Size4KiB>) {
    let frame = match *TRAMPOLINE_FRAME.lock() {
        Some(f) => f,
        None => return,
    };
    let flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(TRAMPOLINE_BASE));
    let _ = crate::memory::paging::map_page(pml4_frame, page, frame, flags);
}

/// Return the user-space address of the trampoline stub for `symbol`, or `None`.
pub fn trampoline_addr(symbol: &str) -> Option<u64> {
    for &(name, offset) in SHIM_SYMBOLS {
        if name == symbol {
            return Some(TRAMPOLINE_BASE + offset as u64);
        }
    }
    None
}

// ── File-offset resolver ──────────────────────────────────────────────────────

fn vaddr_to_file_off(
    elf_data: &[u8],
    ph_offset: usize,
    ph_size: usize,
    ph_count: usize,
    vaddr: u64,
) -> Option<usize> {
    for i in 0..ph_count {
        let start = ph_offset + i * ph_size;
        if start + ph_size > elf_data.len() { break; }
        let ph: Elf64ProgramHeader = unsafe {
            core::ptr::read_unaligned(elf_data[start..].as_ptr() as *const Elf64ProgramHeader)
        };
        if ph.p_type == 1 && vaddr >= ph.p_vaddr && vaddr < ph.p_vaddr + ph.p_memsz {
            return Some((ph.p_offset + (vaddr - ph.p_vaddr)) as usize);
        }
    }
    None
}

// ── String table reader ───────────────────────────────────────────────────────

fn read_strtab(elf_data: &[u8], strtab_file_off: usize, name_off: usize) -> String {
    let start = strtab_file_off + name_off;
    if start >= elf_data.len() { return String::new(); }
    let slice = &elf_data[start..];
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len().min(256));
    core::str::from_utf8(&slice[..end]).unwrap_or("").into()
}

// ── Core: apply dynamic relocations ──────────────────────────────────────────

/// Parse PT_DYNAMIC and patch all JUMP_SLOT / GLOB_DAT GOT entries.
/// Called from `elf::load_elf()` after all PT_LOAD segments are mapped.
pub fn apply(
    pml4_frame: PhysFrame<Size4KiB>,
    elf_data: &[u8],
    load_bias: u64,
    dyn_vaddr: u64,
    dyn_size: u64,
    ph_offset: usize,
    ph_size: usize,
    ph_count: usize,
) {
    map_trampoline(pml4_frame);

    let dyn_file_off = match vaddr_to_file_off(elf_data, ph_offset, ph_size, ph_count, dyn_vaddr) {
        Some(o) => o,
        None => {
            crate::serial_println!("[dynlink] PT_DYNAMIC not found in file");
            return;
        }
    };

    let mut dt_strtab:   Option<u64> = None;
    let mut dt_symtab:   Option<u64> = None;
    let mut dt_jmprel:   Option<u64> = None;
    let mut dt_pltrelsz: Option<u64> = None;
    let mut dt_rela:     Option<u64> = None;
    let mut dt_relasz:   Option<u64> = None;
    let mut dt_relaent:  Option<u64> = None;
    let mut dt_needed:   Vec<u32>    = Vec::new();

    let mut off = 0usize;
    loop {
        if off + core::mem::size_of::<Elf64Dyn>() > dyn_size as usize { break; }
        if dyn_file_off + off + core::mem::size_of::<Elf64Dyn>() > elf_data.len() { break; }
        let d: Elf64Dyn = unsafe {
            core::ptr::read_unaligned(
                elf_data.as_ptr().add(dyn_file_off + off) as *const Elf64Dyn
            )
        };
        if d.d_tag == DT_NULL { break; }
        match d.d_tag {
            DT_NEEDED   => dt_needed.push(d.d_val as u32),
            DT_STRTAB   => dt_strtab = Some(d.d_val),
            DT_SYMTAB   => dt_symtab = Some(d.d_val),
            DT_JMPREL   => dt_jmprel = Some(d.d_val),
            DT_PLTRELSZ => dt_pltrelsz = Some(d.d_val),
            DT_RELA     => dt_rela = Some(d.d_val),
            DT_RELASZ   => dt_relasz = Some(d.d_val),
            DT_RELAENT  => dt_relaent = Some(d.d_val),
            _ => {}
        }
        off += core::mem::size_of::<Elf64Dyn>();
    }

    // Log DT_NEEDED entries
    if let Some(strtab_va) = dt_strtab {
        if let Some(str_off) = vaddr_to_file_off(elf_data, ph_offset, ph_size, ph_count, strtab_va) {
            for &name_off in &dt_needed {
                let lib = read_strtab(elf_data, str_off, name_off as usize);
                crate::serial_println!("[dynlink] DT_NEEDED: {}", lib);
            }
        }
    }

    // PLT relocations (R_X86_64_JUMP_SLOT)
    if let (Some(jmprel_va), Some(pltrelsz)) = (dt_jmprel, dt_pltrelsz) {
        apply_rela_table(pml4_frame, elf_data, load_bias,
            ph_offset, ph_size, ph_count,
            jmprel_va, pltrelsz as usize, 24,
            dt_strtab, dt_symtab);
    }

    // Regular relocations (R_X86_64_GLOB_DAT etc.)
    if let (Some(rela_va), Some(relasz), Some(relaent)) = (dt_rela, dt_relasz, dt_relaent) {
        apply_rela_table(pml4_frame, elf_data, load_bias,
            ph_offset, ph_size, ph_count,
            rela_va, relasz as usize, relaent as usize,
            dt_strtab, dt_symtab);
    }
}

fn apply_rela_table(
    pml4_frame: PhysFrame<Size4KiB>,
    elf_data: &[u8],
    load_bias: u64,
    ph_offset: usize,
    ph_size: usize,
    ph_count: usize,
    table_va: u64,
    table_sz: usize,
    entry_sz: usize,
    strtab_va: Option<u64>,
    symtab_va: Option<u64>,
) {
    let table_off = match vaddr_to_file_off(elf_data, ph_offset, ph_size, ph_count, table_va) {
        Some(o) => o,
        None => return,
    };

    let strtab_off = strtab_va.and_then(|va|
        vaddr_to_file_off(elf_data, ph_offset, ph_size, ph_count, va));
    let symtab_off = symtab_va.and_then(|va|
        vaddr_to_file_off(elf_data, ph_offset, ph_size, ph_count, va));

    let mut off = 0usize;
    while off + 24 <= table_sz {
        if table_off + off + 24 > elf_data.len() { break; }

        let rela: Elf64Rela = unsafe {
            core::ptr::read_unaligned(
                elf_data.as_ptr().add(table_off + off) as *const Elf64Rela
            )
        };

        let r_type = (rela.r_info & 0xFFFF_FFFF) as u32;
        let r_sym  = (rela.r_info >> 32) as usize;

        let (sym_name, value) = resolve(
            elf_data, load_bias, r_type, r_sym, &rela, strtab_off, symtab_off,
        );

        let got_va = rela.r_offset + load_bias;
        if let Some(phys) = crate::memory::paging::translate_in_pml4(pml4_frame, VirtAddr::new(got_va)) {
            let kvirt = crate::memory::paging::phys_to_virt(phys);
            unsafe { core::ptr::write_unaligned(kvirt.as_mut_ptr::<u64>(), value); }
            if !sym_name.is_empty() {
                crate::serial_println!("[dynlink] GOT[{:#x}] = {:#x}  ({})", got_va, value, sym_name);
            }
        }

        off += entry_sz.max(24);
    }
}

fn resolve(
    elf_data: &[u8],
    load_bias: u64,
    r_type: u32,
    sym_idx: usize,
    rela: &Elf64Rela,
    strtab_off: Option<usize>,
    symtab_off: Option<usize>,
) -> (String, u64) {
    if r_type == R_X86_64_RELATIVE {
        return (String::new(), load_bias.wrapping_add(rela.r_addend as u64));
    }

    if sym_idx == 0 {
        return (String::new(), 0);
    }

    let mut sym_name = String::new();
    if let (Some(st_off), Some(str_off)) = (symtab_off, strtab_off) {
        let sym_off = st_off + sym_idx * core::mem::size_of::<Elf64Sym>();
        if sym_off + core::mem::size_of::<Elf64Sym>() <= elf_data.len() {
            let sym: Elf64Sym = unsafe {
                core::ptr::read_unaligned(elf_data.as_ptr().add(sym_off) as *const Elf64Sym)
            };
            sym_name = read_strtab(elf_data, str_off, sym.st_name as usize);
        }
    }

    let value = match r_type {
        R_X86_64_JUMP_SLOT | R_X86_64_GLOB_DAT => {
            trampoline_addr(&sym_name)
                .or_else(|| crate::posix::linker::lookup_symbol(&sym_name))
                .unwrap_or_else(|| {
                    crate::serial_println!("[dynlink] WARN: unresolved symbol '{}'", sym_name);
                    0
                })
        }
        R_X86_64_64 => {
            trampoline_addr(&sym_name)
                .or_else(|| crate::posix::linker::lookup_symbol(&sym_name))
                .unwrap_or(0)
                .wrapping_add(rela.r_addend as u64)
        }
        _ => 0,
    };

    (sym_name, value)
}

// ── Shim dispatch (called from syscall/linux.rs for rax >= 0x8000) ────────────

/// Dispatch a libc shim call. `idx` = rax - 0x8000.
/// Arguments follow the System V AMD64 ABI (rdi, rsi, rdx, r10, r8, r9).
pub fn dispatch_shim(idx: usize, a1: u64, a2: u64, a3: u64, _a4: u64, _a5: u64, _a6: u64) -> u64 {
    match idx {
        // malloc(size)
        0 => shim_malloc(a1 as usize),
        // calloc(nmemb, size)
        1 => shim_malloc((a1 as usize).saturating_mul(a2 as usize)),
        // realloc(ptr, size) — simplified: alloc new, copy, return new
        2 => shim_realloc(a1, a2 as usize),
        // free(ptr)
        3 => { shim_free(a1); 0 }
        // memcpy(dst, src, n) → dst
        4 => {
            if a1 != 0 && a2 != 0 && a3 != 0 {
                unsafe { core::ptr::copy_nonoverlapping(a2 as *const u8, a1 as *mut u8, a3 as usize); }
            }
            a1
        }
        // memmove(dst, src, n) → dst
        5 => {
            if a1 != 0 && a2 != 0 && a3 != 0 {
                unsafe { core::ptr::copy(a2 as *const u8, a1 as *mut u8, a3 as usize); }
            }
            a1
        }
        // memset(dst, val, n) → dst
        6 => {
            if a1 != 0 && a3 != 0 {
                unsafe { core::ptr::write_bytes(a1 as *mut u8, a2 as u8, a3 as usize); }
            }
            a1
        }
        // memcmp(s1, s2, n) → i32
        7 => {
            if a1 == 0 || a2 == 0 { return 0; }
            let n = a3 as usize;
            let s1 = unsafe { core::slice::from_raw_parts(a1 as *const u8, n) };
            let s2 = unsafe { core::slice::from_raw_parts(a2 as *const u8, n) };
            s1.cmp(s2) as i8 as i64 as u64
        }
        // strlen(s)
        8 => {
            if a1 == 0 { return 0; }
            let p = a1 as *const u8;
            let mut len = 0usize;
            while unsafe { *p.add(len) } != 0 && len < 65536 { len += 1; }
            len as u64
        }
        // strcpy(dst, src) → dst
        9 => {
            if a1 == 0 || a2 == 0 { return a1; }
            let mut i = 0usize;
            loop {
                let b = unsafe { *(a2 as *const u8).add(i) };
                unsafe { *(a1 as *mut u8).add(i) = b; }
                if b == 0 { break; }
                i += 1;
            }
            a1
        }
        // strncpy(dst, src, n) → dst
        10 => {
            if a1 == 0 || a2 == 0 { return a1; }
            let n = a3 as usize;
            for i in 0..n {
                let b = unsafe { *(a2 as *const u8).add(i) };
                unsafe { *(a1 as *mut u8).add(i) = b; }
                if b == 0 { break; }
            }
            a1
        }
        // strcmp(s1, s2) → i32
        11 => {
            if a1 == 0 || a2 == 0 { return 0; }
            let mut i = 0usize;
            loop {
                let c1 = unsafe { *(a1 as *const u8).add(i) };
                let c2 = unsafe { *(a2 as *const u8).add(i) };
                if c1 != c2 { return (c1 as i8 - c2 as i8) as i64 as u64; }
                if c1 == 0 { return 0; }
                i += 1;
                if i > 65536 { return 0; }
            }
        }
        // strncmp(s1, s2, n) → i32
        12 => {
            if a1 == 0 || a2 == 0 { return 0; }
            let n = a3 as usize;
            for i in 0..n {
                let c1 = unsafe { *(a1 as *const u8).add(i) };
                let c2 = unsafe { *(a2 as *const u8).add(i) };
                if c1 != c2 { return (c1 as i8 - c2 as i8) as i64 as u64; }
                if c1 == 0 { return 0; }
            }
            0
        }
        // strcat(dst, src) → dst  (simplified)
        13 => {
            if a1 == 0 || a2 == 0 { return a1; }
            // find end of dst
            let mut end = 0usize;
            while unsafe { *(a1 as *const u8).add(end) } != 0 && end < 65536 { end += 1; }
            let mut i = 0usize;
            loop {
                let b = unsafe { *(a2 as *const u8).add(i) };
                unsafe { *(a1 as *mut u8).add(end + i) = b; }
                if b == 0 { break; }
                i += 1;
            }
            a1
        }
        // strncat(dst, src, n) → dst
        14 => {
            if a1 == 0 || a2 == 0 { return a1; }
            let n = a3 as usize;
            let mut end = 0usize;
            while unsafe { *(a1 as *const u8).add(end) } != 0 && end < 65536 { end += 1; }
            for i in 0..n {
                let b = unsafe { *(a2 as *const u8).add(i) };
                unsafe { *(a1 as *mut u8).add(end + i) = b; }
                if b == 0 { return a1; }
            }
            unsafe { *(a1 as *mut u8).add(end + n) = 0; }
            a1
        }
        // strchr(s, c) → ptr or NULL
        15 => {
            if a1 == 0 { return 0; }
            let c = a2 as u8;
            let mut i = 0usize;
            loop {
                let b = unsafe { *(a1 as *const u8).add(i) };
                if b == c { return a1 + i as u64; }
                if b == 0 { return 0; }
                i += 1;
                if i > 65536 { return 0; }
            }
        }
        // strrchr(s, c) → last occurrence or NULL
        16 => {
            if a1 == 0 { return 0; }
            let c = a2 as u8;
            let mut last = 0u64;
            let mut i = 0usize;
            loop {
                let b = unsafe { *(a1 as *const u8).add(i) };
                if b == c { last = a1 + i as u64; }
                if b == 0 { break; }
                i += 1;
                if i > 65536 { break; }
            }
            last
        }
        // strstr(hay, needle) → ptr or NULL
        17 => {
            if a1 == 0 || a2 == 0 { return 0; }
            let hay_len = dispatch_shim(8, a1, 0, 0, 0, 0, 0) as usize;
            let needle_len = dispatch_shim(8, a2, 0, 0, 0, 0, 0) as usize;
            if needle_len == 0 { return a1; }
            if needle_len > hay_len { return 0; }
            for i in 0..=(hay_len - needle_len) {
                if dispatch_shim(12, a1 + i as u64, a2, needle_len as u64, 0, 0, 0) == 0 {
                    return a1 + i as u64;
                }
            }
            0
        }
        // strtol(s, endptr, base) → i64
        18 => {
            if a1 == 0 { return 0; }
            let s = unsafe { core::str::from_utf8_unchecked(
                core::slice::from_raw_parts(a1 as *const u8, 64.min(
                    dispatch_shim(8, a1, 0, 0, 0, 0, 0) as usize))
            )};
            let base = if a3 == 0 { 10 } else { a3 as u32 };
            i64::from_str_radix(s.trim(), base).unwrap_or(0) as u64
        }
        // strtoul
        19 => {
            if a1 == 0 { return 0; }
            let len = dispatch_shim(8, a1, 0, 0, 0, 0, 0) as usize;
            let s = unsafe { core::str::from_utf8_unchecked(
                core::slice::from_raw_parts(a1 as *const u8, len.min(64)))
            };
            let base = if a3 == 0 { 10 } else { a3 as u32 };
            u64::from_str_radix(s.trim(), base).unwrap_or(0)
        }
        // atoi / atol
        20 | 21 => dispatch_shim(18, a1, 0, 10, 0, 0, 0),

        // printf(fmt, ...) → simplified: write fmt string to stdout
        22 => {
            if a1 == 0 { return 0; }
            let len = dispatch_shim(8, a1, 0, 0, 0, 0, 0) as usize;
            let buf = unsafe { core::slice::from_raw_parts(a1 as *const u8, len) };
            crate::syscall::handlers::sys_write(1, buf).unwrap_or(0) as u64
        }
        // fprintf(stream, fmt) → write to fd (treat stream as fd)
        23 => {
            if a2 == 0 { return 0; }
            let fd = if a1 <= 2 { a1 as usize } else { 2 };
            let len = dispatch_shim(8, a2, 0, 0, 0, 0, 0) as usize;
            let buf = unsafe { core::slice::from_raw_parts(a2 as *const u8, len) };
            crate::syscall::handlers::sys_write(fd, buf).unwrap_or(0) as u64
        }
        // sprintf(buf, fmt) → copy fmt into buf
        24 => { dispatch_shim(9, a1, a2, 0, 0, 0, 0) }
        // snprintf(buf, n, fmt) → copy up to n bytes of fmt into buf
        25 => { dispatch_shim(10, a1, a3, a2, 0, 0, 0) }
        // puts(s) → write s + newline
        26 => {
            if a1 == 0 { return 0; }
            let len = dispatch_shim(8, a1, 0, 0, 0, 0, 0) as usize;
            let buf = unsafe { core::slice::from_raw_parts(a1 as *const u8, len) };
            let _ = crate::syscall::handlers::sys_write(1, buf);
            let _ = crate::syscall::handlers::sys_write(1, b"\n");
            (len + 1) as u64
        }
        // fputs(s, stream) → write s
        27 => { dispatch_shim(22, a1, 0, 0, 0, 0, 0) }
        // fwrite(ptr, size, count, stream) → write size*count bytes
        28 => {
            if a1 == 0 { return 0; }
            let total = (a2 as usize).saturating_mul(a3 as usize);
            if total == 0 { return 0; }
            let buf = unsafe { core::slice::from_raw_parts(a1 as *const u8, total) };
            let _ = crate::syscall::handlers::sys_write(1, buf);
            a3
        }
        // fread → 0 (stub)
        29 => 0,
        // fopen(path, mode) → fd (we use fd as fake FILE*)
        30 => {
            if a1 == 0 { return 0; }
            let len = dispatch_shim(8, a1, 0, 0, 0, 0, 0) as usize;
            let path = unsafe { core::str::from_utf8_unchecked(
                core::slice::from_raw_parts(a1 as *const u8, len.min(4096)))
            };
            match crate::vfs::open(path) {
                Ok(fd) => fd as u64,
                Err(_) => 0,
            }
        }
        // fflush → 0
        32 => 0,
        // getenv(name)
        33 => {
            if a1 == 0 { return 0; }
            let len = dispatch_shim(8, a1, 0, 0, 0, 0, 0) as usize;
            let key = unsafe { core::str::from_utf8_unchecked(
                core::slice::from_raw_parts(a1 as *const u8, len.min(256)))
            };
            match crate::process::env::get(key) {
                Some(_) => a1, // return key ptr as non-NULL (simplified)
                None => 0,
            }
        }
        // setenv(name, val, overwrite)
        34 => {
            if a1 == 0 || a2 == 0 { return 0; }
            let klen = dispatch_shim(8, a1, 0, 0, 0, 0, 0) as usize;
            let vlen = dispatch_shim(8, a2, 0, 0, 0, 0, 0) as usize;
            let key = unsafe { core::str::from_utf8_unchecked(
                core::slice::from_raw_parts(a1 as *const u8, klen.min(256)))
            };
            let val = unsafe { core::str::from_utf8_unchecked(
                core::slice::from_raw_parts(a2 as *const u8, vlen.min(4096)))
            };
            crate::process::env::set(key, val);
            0
        }
        // unsetenv(name)
        35 => {
            if a1 == 0 { return 0; }
            let len = dispatch_shim(8, a1, 0, 0, 0, 0, 0) as usize;
            let key = unsafe { core::str::from_utf8_unchecked(
                core::slice::from_raw_parts(a1 as *const u8, len.min(256)))
            };
            crate::process::env::remove(key);
            0
        }
        // exit / abort / _exit
        36 | 37 | 38 => {
            crate::process::scheduler::exit_current_thread();
            loop { x86_64::instructions::hlt(); }
        }
        // pthread_* → stub 0 (single-threaded for now)
        40 | 41 | 42 | 43 | 44 | 45 | 46 => 0,
        // dlopen / dlsym / dlclose / dlerror → stub
        47 | 48 | 49 | 50 => 0,
        // write(fd, buf, count) → forward to sys_write
        51 => {
            if a2 == 0 || a3 == 0 { return 0; }
            let buf = unsafe { core::slice::from_raw_parts(a2 as *const u8, a3 as usize) };
            crate::syscall::handlers::sys_write(a1 as usize, buf).unwrap_or(0) as u64
        }
        // read(fd, buf, count) → forward to sys_read
        52 => {
            if a2 == 0 || a3 == 0 { return 0; }
            let buf = unsafe { core::slice::from_raw_parts_mut(a2 as *mut u8, a3 as usize) };
            crate::syscall::handlers::sys_read(a1 as usize, buf).unwrap_or(0) as u64
        }
        // open(path, flags, mode)
        53 => {
            if a1 == 0 { return u64::MAX; }
            let len = dispatch_shim(8, a1, 0, 0, 0, 0, 0) as usize;
            let path = unsafe { core::str::from_utf8_unchecked(
                core::slice::from_raw_parts(a1 as *const u8, len.min(4096)))
            };
            match crate::vfs::open(path) {
                Ok(fd) => fd as u64,
                Err(_) => u64::MAX,
            }
        }
        // close(fd) / fclose
        31 | 54 => { let _ = crate::syscall::handlers::sys_close(a1 as usize); 0 }
        // mmap(addr, len, prot, flags, fd, off)
        55 => {
            let size = a2 as usize;
            if size == 0 { return 0; }
            let layout = alloc::alloc::Layout::from_size_align(size, 4096)
                .unwrap_or(alloc::alloc::Layout::new::<u8>());
            let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
            if ptr.is_null() { 0 } else { ptr as u64 }
        }
        // munmap(addr, len) → just free
        56 => 0,  // leak; real munmap needs size tracking
        // brk / sbrk → return 0 (programs usually fall back to mmap)
        57 | 58 => 0,
        // getpid
        59 => crate::process::scheduler::current_pid().unwrap_or(0),
        // gettid
        60 => crate::process::scheduler::current_tid().unwrap_or(0),
        // clock_gettime → 0
        61 => 0,
        // nanosleep → 0 (yield)
        62 => { crate::process::scheduler::yield_now(); 0 }
        // raise / signal / sigaction → 0
        63 | 64 | 65 => 0,
        // __errno_location → pointer to a per-shim errno (stub: return 0 addr)
        66 => {
            // Return address of a static errno cell
            static ERRNO_CELL: spin::Mutex<i32> = spin::Mutex::new(0);
            &*ERRNO_CELL.lock() as *const i32 as u64
        }
        // __cxa_finalize / __cxa_atexit → 0
        67 | 68 => 0,

        _ => {
            crate::serial_println!("[dynlink] unhandled shim idx {}", idx);
            0
        }
    }
}

// ── malloc / free helpers ─────────────────────────────────────────────────────

fn shim_malloc(size: usize) -> u64 {
    if size == 0 { return 0; }
    let total = size.saturating_add(8);
    let layout = match alloc::alloc::Layout::from_size_align(total, 8) {
        Ok(l) => l,
        Err(_) => return 0,
    };
    let raw = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if raw.is_null() { return 0; }
    unsafe { *(raw as *mut usize) = size; }
    raw as u64 + 8
}

fn shim_free(ptr: u64) {
    if ptr < 8 { return; }
    let header = (ptr - 8) as *mut usize;
    let size = unsafe { *header };
    if size == 0 || size > 0x1000_0000 { return; }
    let total = size + 8;
    if let Ok(layout) = alloc::alloc::Layout::from_size_align(total, 8) {
        unsafe { alloc::alloc::dealloc((ptr - 8) as *mut u8, layout); }
    }
}

fn shim_realloc(ptr: u64, new_size: usize) -> u64 {
    if ptr == 0 { return shim_malloc(new_size); }
    if new_size == 0 { shim_free(ptr); return 0; }
    let new_ptr = shim_malloc(new_size);
    if new_ptr == 0 { return 0; }
    let old_size = if ptr >= 8 { unsafe { *((ptr - 8) as *const usize) } } else { 0 };
    let copy_len = old_size.min(new_size);
    if copy_len > 0 {
        unsafe { core::ptr::copy_nonoverlapping(ptr as *const u8, new_ptr as *mut u8, copy_len); }
    }
    shim_free(ptr);
    new_ptr
}

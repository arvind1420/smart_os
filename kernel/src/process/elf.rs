/// Minimal ELF64 loader for Smart OS.
///
/// Parses ELF64 headers, loads PT_LOAD segments into a user-space page table,
/// and returns the entry point address. No external crate needed.

use x86_64::structures::paging::{PageTableFlags, PhysFrame, Page, Size4KiB};
use x86_64::VirtAddr;

/// ELF64 magic bytes.
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// ELF64 file header.
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct Elf64Header {
    pub e_ident: [u8; 16],
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

/// ELF64 program header.
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct Elf64ProgramHeader {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

/// ELF64 section header.
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct Elf64SectionHeader {
    pub sh_name: u32,
    pub sh_type: u32,
    pub sh_flags: u64,
    pub sh_addr: u64,
    pub sh_offset: u64,
    pub sh_size: u64,
    pub sh_link: u32,
    pub sh_info: u32,
    pub sh_addralign: u64,
    pub sh_entsize: u64,
}

pub const SHT_SYMTAB: u32 = 2;
pub const SHT_STRTAB: u32 = 3;
pub const SHT_RELA: u32 = 4;
pub const SHT_DYNSYM: u32 = 11;

/// ELF64 Dynamic entry.
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct Elf64Dyn {
    pub d_tag: i64,
    pub d_val: u64,
}

pub const DT_NULL:    i64 = 0;
pub const DT_NEEDED:  i64 = 1;
pub const DT_STRTAB:  i64 = 5;
pub const DT_SYMTAB:  i64 = 6;
pub const DT_RELA:    i64 = 7;
pub const DT_RELASZ:  i64 = 8;
pub const DT_RELAENT: i64 = 9;
pub const DT_SYMENT:  i64 = 11;
pub const DT_PLTREL:  i64 = 20;
pub const DT_JMPREL:  i64 = 23;
pub const DT_PLTRELSZ:i64 = 2;

pub const R_X86_64_RELATIVE:  u32 = 8;
pub const R_X86_64_GLOB_DAT:  u32 = 6;
pub const R_X86_64_JUMP_SLOT: u32 = 7;
pub const R_X86_64_64:        u32 = 1;

/// ELF64 Relocation with Addend.
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct Elf64Rela {
    pub r_offset: u64,
    pub r_info: u64,
    pub r_addend: i64,
}

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
#[allow(dead_code)]
const PF_R: u32 = 4;

/// Result of loading an ELF.
pub struct LoadedElf {
    pub entry_point: u64,
    pub highest_addr: u64,
    pub is_linux: bool,
}

/// Load an ELF64 binary into a user-space page table.
///
/// `elf_data` is the raw ELF file bytes (from VFS).
/// `pml4_frame` is the user process's Level 4 page table.
pub fn load_elf(
    elf_data: &[u8],
    pml4_frame: PhysFrame<Size4KiB>,
) -> Result<LoadedElf, &'static str> {
    // Validate header size
    if elf_data.len() < core::mem::size_of::<Elf64Header>() {
        return Err("ELF too small");
    }

    // Read header (packed, so use read_unaligned for safety)
    let header: Elf64Header = unsafe {
        core::ptr::read_unaligned(elf_data.as_ptr() as *const Elf64Header)
    };

    // Validate magic
    if header.e_ident[0..4] != ELF_MAGIC {
        return Err("Not an ELF file");
    }
    if header.e_ident[4] != 2 {
        return Err("Not ELF64");
    }
    if header.e_machine != 0x3E {
        return Err("Not x86_64 ELF");
    }

    // ABI Detection: 0 = System V (usually Linux), 3 = Linux
    let is_linux = header.e_ident[7] == 0 || header.e_ident[7] == 3;

    // e_type 3 is ET_DYN (PIE or shared library). 
    // For PIE, we generate a random load bias (ASLR - Phase 38).
    let is_pie = header.e_type == 3;
    let mut load_bias = 0u64;

    if is_pie {
        if let Some(tpm) = crate::drivers::tpm::TPM.lock().as_ref() {
            let mut rand_bytes = [0u8; 8];
            if tpm.get_random(&mut rand_bytes).is_ok() {
                let rand_val = u64::from_le_bytes(rand_bytes);
                // Create a page-aligned bias in the range [0x100000, 0xFF000000]
                load_bias = (rand_val & 0xFFF_F000) + 0x100_000;
                crate::serial_println!("[elf] PIE detected, applied ASLR load bias: {:#X}", load_bias);
            }
        }
    }

    let entry = header.e_entry + load_bias;
    let mut highest = 0u64;

    let ph_offset = header.e_phoff as usize;
    let ph_size = header.e_phentsize as usize;
    let ph_count = header.e_phnum as usize;

    let mut dynamic_ptr: Option<u64> = None;
    let mut dynamic_size: Option<u64> = None;

    // Iterate program headers, load PT_LOAD segments
    for i in 0..ph_count {
        let ph_start = ph_offset + i * ph_size;
        if ph_start + ph_size > elf_data.len() {
            return Err("Program header out of bounds");
        }

        let ph: Elf64ProgramHeader = unsafe {
            core::ptr::read_unaligned(
                elf_data[ph_start..].as_ptr() as *const Elf64ProgramHeader
            )
        };

        if ph.p_type == PT_DYNAMIC {
            dynamic_ptr = Some(ph.p_vaddr); // raw ELF vaddr — bias not applied here; used only for file-offset lookup
            dynamic_size = Some(ph.p_memsz);
            continue;
        }

        if ph.p_type != PT_LOAD {
            continue;
        }

        let seg_vaddr = ph.p_vaddr + load_bias;
        let seg_memsz = ph.p_memsz;
        let seg_filesz = ph.p_filesz;
        let seg_offset = ph.p_offset;
        let seg_end = seg_vaddr + seg_memsz;

        if seg_end > highest {
            highest = seg_end;
        }

        // Page-aligned range
        let page_start = seg_vaddr & !0xFFF;
        let page_end = (seg_end + 0xFFF) & !0xFFF;
        let page_count = ((page_end - page_start) / 4096) as usize;

        // Determine page flags
        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if ph.p_flags & PF_W != 0 {
            flags |= PageTableFlags::WRITABLE;
        }
        if ph.p_flags & PF_X == 0 {
            flags |= PageTableFlags::NO_EXECUTE;
        }

        // Allocate and map each page
        for j in 0..page_count {
            let page_base = page_start + (j as u64) * 4096;
            let vaddr = VirtAddr::new(page_base);
            let page = Page::<Size4KiB>::containing_address(vaddr);
            let frame = crate::memory::frame::alloc_frame()
                .ok_or("Out of frames for ELF segment")?;

            crate::memory::paging::map_page(pml4_frame, page, frame, flags)?;

            // Write data into the mapped frame via physical offset.
            let frame_virt = crate::memory::paging::phys_to_virt(
                frame.start_address(),
            );
            let dest = unsafe {
                core::slice::from_raw_parts_mut(frame_virt.as_mut_ptr::<u8>(), 4096)
            };

            // Zero the entire page first (handles BSS and partial pages).
            dest.fill(0);

            // Copy file data that overlaps with this page.
            if seg_filesz > 0 {
                let copy_vstart = page_base.max(seg_vaddr);
                let copy_vend = (page_base + 4096).min(seg_vaddr + seg_filesz);

                if copy_vstart < copy_vend {
                    let dest_off = (copy_vstart - page_base) as usize;
                    let src_off = (seg_offset + (copy_vstart - seg_vaddr)) as usize;
                    let copy_len = (copy_vend - copy_vstart) as usize;

                    if src_off + copy_len <= elf_data.len() {
                        dest[dest_off..dest_off + copy_len]
                            .copy_from_slice(&elf_data[src_off..src_off + copy_len]);
                    }
                }
            }
        }
    }

    // Handle relocations if this is a dynamic executable (PIE)
    if is_pie {
        if let Some(dyn_vaddr) = dynamic_ptr {
            let mut rela_ptr: Option<u64> = None;
            let mut rela_size: Option<u64> = None;
            let mut rela_ent: Option<u64> = None;

            // Find dynamic entries
            let mut offset = 0;
            while let Some(size) = dynamic_size {
                if offset >= size { break; }
                // This is a bit tricky because dyn_vaddr is a virtual address in the *user* process.
                // We need to find the file offset for this virtual address.
                // For simplicity, we assume PT_DYNAMIC is in a PT_LOAD segment we just loaded.
                
                // Let's find the file offset for dyn_vaddr
                let mut dyn_file_off = 0;
                for i in 0..ph_count {
                    let ph_start = ph_offset + i * ph_size;
                    let ph: Elf64ProgramHeader = unsafe {
                        core::ptr::read_unaligned(elf_data[ph_start..].as_ptr() as *const Elf64ProgramHeader)
                    };
                    if ph.p_type == PT_LOAD && dyn_vaddr >= ph.p_vaddr && dyn_vaddr < ph.p_vaddr + ph.p_memsz {
                        dyn_file_off = ph.p_offset + (dyn_vaddr - ph.p_vaddr);
                        break;
                    }
                }

                let d_ptr = elf_data.as_ptr().wrapping_add(dyn_file_off as usize + offset as usize) as *const Elf64Dyn;
                let d = unsafe { core::ptr::read_unaligned(d_ptr) };
                if d.d_tag == DT_NULL { break; }
                if d.d_tag == DT_RELA { rela_ptr = Some(d.d_val); }
                if d.d_tag == DT_RELASZ { rela_size = Some(d.d_val); }
                if d.d_tag == DT_RELAENT { rela_ent = Some(d.d_val); }
                offset += core::mem::size_of::<Elf64Dyn>() as u64;
            }

            // Perform relocations
            if let (Some(r_ptr), Some(r_sz), Some(r_ent)) = (rela_ptr, rela_size, rela_ent) {
                let mut r_off = 0;
                while r_off < r_sz {
                    // Find file offset for relocation table
                    let mut r_file_off = 0;
                    for i in 0..ph_count {
                        let ph_start = ph_offset + i * ph_size;
                        let ph: Elf64ProgramHeader = unsafe {
                            core::ptr::read_unaligned(elf_data[ph_start..].as_ptr() as *const Elf64ProgramHeader)
                        };
                        if ph.p_type == PT_LOAD && r_ptr >= ph.p_vaddr && r_ptr < ph.p_vaddr + ph.p_memsz {
                            r_file_off = ph.p_offset + (r_ptr - ph.p_vaddr);
                            break;
                        }
                    }

                    let rela_entry_ptr = elf_data.as_ptr().wrapping_add(r_file_off as usize + r_off as usize) as *const Elf64Rela;
                    let rela = unsafe { core::ptr::read_unaligned(rela_entry_ptr) };
                    
                    let r_type = (rela.r_info & 0xFFFFFFFF) as u32;
                    if r_type == R_X86_64_RELATIVE {
                        // In Smart OS, R_X86_64_RELATIVE is: *offset = base + addend.
                        // For PIE, 'base' is the load_bias.
                        let target_vaddr = rela.r_offset + load_bias;
                        let vaddr = VirtAddr::new(target_vaddr);
                        if let Some(phys) = crate::memory::paging::translate_in_pml4(pml4_frame, vaddr) {
                            let kernel_virt = crate::memory::paging::phys_to_virt(phys);
                            unsafe {
                                let ptr = kernel_virt.as_mut_ptr::<u64>();
                                core::ptr::write_unaligned(ptr, load_bias + rela.r_addend as u64);
                            }
                        }
                    }
                    r_off += r_ent;
                }
            }
        }
    }

    // Apply dynamic relocations (JUMP_SLOT, GLOB_DAT) if PT_DYNAMIC present.
    if let (Some(dyn_vaddr), Some(dyn_sz)) = (dynamic_ptr, dynamic_size) {
        super::dynlink::apply(
            pml4_frame, elf_data, load_bias,
            dyn_vaddr, dyn_sz,
            ph_offset, ph_size, ph_count,
        );
    }

    Ok(LoadedElf {
        entry_point: entry,
        highest_addr: highest,
        is_linux,
    })
}


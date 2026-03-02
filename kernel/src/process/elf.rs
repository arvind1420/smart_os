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

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
#[allow(dead_code)]
const PF_R: u32 = 4;

/// Result of loading an ELF.
pub struct LoadedElf {
    pub entry_point: u64,
    pub highest_addr: u64,
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

    let entry = header.e_entry;
    let mut highest = 0u64;

    let ph_offset = header.e_phoff as usize;
    let ph_size = header.e_phentsize as usize;
    let ph_count = header.e_phnum as usize;

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

        if ph.p_type != PT_LOAD {
            continue;
        }

        let seg_vaddr = ph.p_vaddr;
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
            // The segment occupies virtual [seg_vaddr, seg_vaddr + seg_filesz) in the file
            // and [seg_vaddr, seg_vaddr + seg_memsz) in memory (filesz <= memsz).
            if seg_filesz > 0 {
                // What range of the segment falls on this page?
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

    Ok(LoadedElf {
        entry_point: entry,
        highest_addr: highest,
    })
}

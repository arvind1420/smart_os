/// User-space program builder for Smart OS.
///
/// Constructs minimal ELF64 binaries in memory for demo user-space processes.
/// These are stored in the VFS and loaded by the ELF loader.

use alloc::vec;
use alloc::vec::Vec;

/// Create a minimal "hello" ELF64 binary.
///
/// The program does:
///   1. syscall(SYS_WRITE=23, fd=1, "Hello from user space!\n", 23)
///   2. syscall(SYS_EXIT=0, code=0)
///
/// Returns the raw ELF bytes suitable for storing in VFS.
pub fn create_hello_elf() -> Vec<u8> {
    // Machine code for the hello program.
    // Loaded at virtual address 0x400000.
    //
    // Layout:
    //   0x00: mov rax, 23          ; SYS_WRITE
    //   0x07: mov rdi, 1           ; fd = stdout
    //   0x0E: lea rsi, [rip+msg]   ; buf = message string
    //   0x15: mov rdx, 23          ; len = 23
    //   0x1C: syscall
    //   0x1E: mov rax, 0           ; SYS_EXIT
    //   0x25: mov rdi, 0           ; exit code = 0
    //   0x2C: syscall
    //   0x2E: jmp $                ; safety loop
    //   0x30: msg: "Hello from user space!\n"
    //
    // Note: lea rsi, [rip+offset] — offset is from end of LEA instruction (0x15) to msg (0x30) = 0x1B

    let code: Vec<u8> = vec![
        // mov rax, 23 (SYS_WRITE)
        0x48, 0xC7, 0xC0, 23, 0, 0, 0,
        // mov rdi, 1 (fd=stdout)
        0x48, 0xC7, 0xC7, 1, 0, 0, 0,
        // lea rsi, [rip + 0x1B]  (points to message at offset 0x30)
        0x48, 0x8D, 0x35, 0x1B, 0x00, 0x00, 0x00,
        // mov rdx, 23 (len)
        0x48, 0xC7, 0xC2, 23, 0, 0, 0,
        // syscall
        0x0F, 0x05,
        // mov rax, 0 (SYS_EXIT)
        0x48, 0xC7, 0xC0, 0, 0, 0, 0,
        // mov rdi, 0 (exit code = 0)
        0x48, 0xC7, 0xC7, 0, 0, 0, 0,
        // syscall
        0x0F, 0x05,
        // jmp $ (infinite loop as safety net)
        0xEB, 0xFE,
        // Message: "Hello from user space!\n"
        b'H', b'e', b'l', b'l', b'o', b' ', b'f', b'r',
        b'o', b'm', b' ', b'u', b's', b'e', b'r', b' ',
        b's', b'p', b'a', b'c', b'e', b'!', b'\n',
    ];

    build_elf64(0x400000, &code)
}

/// Build a minimal valid ELF64 binary with a single PT_LOAD segment.
///
/// `load_vaddr` is the virtual address where the code will be loaded.
/// `code` is the raw machine code bytes.
fn build_elf64(load_vaddr: u64, code: &[u8]) -> Vec<u8> {
    const EHDR_SIZE: usize = 64;  // ELF64 header
    const PHDR_SIZE: usize = 56;  // Program header entry

    let code_offset = EHDR_SIZE + PHDR_SIZE;
    let total_size = code_offset + code.len();
    let mut elf = vec![0u8; total_size];

    // ── ELF Header ──
    // e_ident
    elf[0] = 0x7F;
    elf[1] = b'E';
    elf[2] = b'L';
    elf[3] = b'F';
    elf[4] = 2;     // ELFCLASS64
    elf[5] = 1;     // ELFDATA2LSB (little-endian)
    elf[6] = 1;     // EV_CURRENT
    elf[7] = 0;     // ELFOSABI_NONE
    // e_type = ET_EXEC (2)
    elf[16..18].copy_from_slice(&2u16.to_le_bytes());
    // e_machine = EM_X86_64 (0x3E)
    elf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes());
    // e_version = EV_CURRENT (1)
    elf[20..24].copy_from_slice(&1u32.to_le_bytes());
    // e_entry = load_vaddr (entry point)
    elf[24..32].copy_from_slice(&load_vaddr.to_le_bytes());
    // e_phoff = EHDR_SIZE (program header offset)
    elf[32..40].copy_from_slice(&(EHDR_SIZE as u64).to_le_bytes());
    // e_shoff = 0 (no section headers)
    // e_flags = 0
    // e_ehsize = EHDR_SIZE
    elf[52..54].copy_from_slice(&(EHDR_SIZE as u16).to_le_bytes());
    // e_phentsize = PHDR_SIZE
    elf[54..56].copy_from_slice(&(PHDR_SIZE as u16).to_le_bytes());
    // e_phnum = 1
    elf[56..58].copy_from_slice(&1u16.to_le_bytes());

    // ── Program Header (at offset EHDR_SIZE) ──
    let ph = &mut elf[EHDR_SIZE..EHDR_SIZE + PHDR_SIZE];
    // p_type = PT_LOAD (1)
    ph[0..4].copy_from_slice(&1u32.to_le_bytes());
    // p_flags = PF_R | PF_X (5) — readable + executable
    ph[4..8].copy_from_slice(&5u32.to_le_bytes());
    // p_offset = code_offset (offset in file)
    ph[8..16].copy_from_slice(&(code_offset as u64).to_le_bytes());
    // p_vaddr = load_vaddr
    ph[16..24].copy_from_slice(&load_vaddr.to_le_bytes());
    // p_paddr = load_vaddr (same for simplicity)
    ph[24..32].copy_from_slice(&load_vaddr.to_le_bytes());
    // p_filesz = code length
    ph[32..40].copy_from_slice(&(code.len() as u64).to_le_bytes());
    // p_memsz = code length (no BSS)
    ph[40..48].copy_from_slice(&(code.len() as u64).to_le_bytes());
    // p_align = 0x1000 (page aligned)
    ph[48..56].copy_from_slice(&0x1000u64.to_le_bytes());

    // ── Code ──
    elf[code_offset..code_offset + code.len()].copy_from_slice(code);

    elf
}

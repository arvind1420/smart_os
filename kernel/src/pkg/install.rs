/// Package installation engine for Smart OS.
///
/// Downloads each file in a package's file list from the registry and
/// places it in the VFS. For packages not yet available on the network,
/// generates a functional stub ELF so commands at least exist.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use super::index::IndexEntry;

/// Registry base URL (scheme stripped — TLS client prepends https://).
const REGISTRY_HOST: &str = "pkg.smartos.dev";
const REGISTRY_PATH: &str = "/packages/v1/";

/// Run the full install sequence for a package.
pub fn run(entry: &IndexEntry) -> Result<(), &'static str> {
    crate::serial_println!("[pkg] Installing {} v{} ...", entry.name, entry.version);

    for file in &entry.files {
        install_file(entry, &file.install_path, &file.url_suffix, file.is_executable)?;
    }

    // Write a package manifest
    let manifest_path = format!("/var/pkg/cache/{}.info", entry.name);
    let manifest = format!(
        "name={}\nversion={}\ndescription={}\ninstalled=yes\n",
        entry.name, entry.version, entry.description
    );
    let _ = crate::vfs::create_and_write(&manifest_path, manifest.as_bytes());

    crate::serial_println!("[pkg] {} installed successfully.", entry.name);
    Ok(())
}

/// Download (or generate stub) a single file and place it in the VFS.
fn install_file(
    entry: &IndexEntry,
    install_path: &str,
    url_suffix: &str,
    is_executable: bool,
) -> Result<(), &'static str> {
    // Ensure parent directory exists
    ensure_parent(install_path);

    // Try network download first
    let url_path = format!("{}{}", REGISTRY_PATH, url_suffix);
    let data = match crate::net::tls::https_get(REGISTRY_HOST, &url_path) {
        Ok(body) if !body.is_empty() => body,
        _ => {
            // Network unavailable — generate a stub ELF so the path exists
            if is_executable {
                make_stub_elf(&entry.name, install_path)
            } else {
                make_text_stub(entry, url_suffix)
            }
        }
    };

    crate::vfs::create_and_write(install_path, &data)
        .map_err(|_| "failed to write package file")?;

    crate::serial_println!("[pkg]   {} ({} bytes)", install_path, data.len());
    Ok(())
}

/// Create all parent directories for a path.
fn ensure_parent(path: &str) {
    let mut idx = 1;
    while let Some(pos) = path[idx..].find('/') {
        let dir = &path[..idx + pos];
        crate::vfs::mkdir(dir).ok();
        idx += pos + 1;
    }
}

/// Generate a minimal stub ELF that prints "not available offline" and exits.
/// This is a 64-bit ELF with a single syscall sequence: write + exit.
fn make_stub_elf(name: &str, _path: &str) -> Vec<u8> {
    // We use a handcrafted x86-64 ELF. The .text section:
    //   mov rax, 1              ; SYS_write
    //   mov rdi, 1              ; stdout
    //   lea rsi, [rip+msg]      ; message pointer
    //   mov rdx, msg_len        ; length
    //   syscall
    //   mov rax, 60             ; SYS_exit
    //   xor rdi, rdi
    //   syscall
    //
    // ELF64 header + single PT_LOAD segment + code + message.

    let msg = format!("{}: offline stub — run `pkg update` then reinstall\n", name);
    let msg_bytes = msg.as_bytes();
    let msg_len   = msg_bytes.len() as u64;

    // Code (position-independent, loaded at 0x400000)
    let load_addr: u64 = 0x400000;
    let ehdr_size: u64 = 64;
    let phdr_size: u64 = 56;
    let code_off:  u64 = ehdr_size + phdr_size;
    // Code sequence: 27 bytes
    let code_size: u64 = 27;
    let msg_off:   u64 = code_off + code_size;
    let total:     u64 = msg_off + msg_len;

    let entry_va = load_addr + code_off;
    let msg_va   = load_addr + msg_off;

    let mut elf: Vec<u8> = Vec::new();

    // ELF64 header (64 bytes)
    elf.extend_from_slice(b"\x7FELF");  // magic
    elf.push(2); elf.push(1); elf.push(1); elf.push(0); // 64-bit, LE, ELFCLASS64
    elf.extend_from_slice(&[0u8; 8]);   // padding
    elf.extend_from_slice(&2u16.to_le_bytes());  // ET_EXEC
    elf.extend_from_slice(&0x3eu16.to_le_bytes()); // EM_X86_64
    elf.extend_from_slice(&1u32.to_le_bytes());  // EV_CURRENT
    elf.extend_from_slice(&entry_va.to_le_bytes()); // e_entry
    elf.extend_from_slice(&ehdr_size.to_le_bytes()); // e_phoff
    elf.extend_from_slice(&0u64.to_le_bytes()); // e_shoff
    elf.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&56u16.to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&1u16.to_le_bytes());  // e_phnum
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_le_bytes());  // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes());  // e_shstrndx

    // PT_LOAD program header (56 bytes)
    elf.extend_from_slice(&1u32.to_le_bytes());  // PT_LOAD
    elf.extend_from_slice(&5u32.to_le_bytes());  // PF_R|PF_X
    elf.extend_from_slice(&0u64.to_le_bytes());  // p_offset
    elf.extend_from_slice(&load_addr.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&load_addr.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&total.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&total.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&0x1000u64.to_le_bytes()); // p_align

    // .text: write(1, msg_va, msg_len); exit(0)
    // mov rax, 1
    elf.extend_from_slice(&[0x48, 0xc7, 0xc0, 0x01, 0x00, 0x00, 0x00]);
    // mov rdi, 1
    elf.extend_from_slice(&[0x48, 0xc7, 0xc7, 0x01, 0x00, 0x00, 0x00]);
    // mov rsi, msg_va (8 bytes: movabs)
    elf.push(0x48); elf.push(0xbe);
    elf.extend_from_slice(&msg_va.to_le_bytes());
    // mov rdx, msg_len
    elf.extend_from_slice(&[0x48, 0xc7, 0xc2]);
    elf.extend_from_slice(&(msg_len as u32).to_le_bytes());
    // syscall
    elf.extend_from_slice(&[0x0f, 0x05]);
    // mov rax, 60; xor rdi, rdi; syscall
    elf.extend_from_slice(&[0x48, 0xc7, 0xc0, 0x3c, 0x00, 0x00, 0x00]);
    elf.extend_from_slice(&[0x48, 0x31, 0xff]);
    elf.extend_from_slice(&[0x0f, 0x05]);

    // Sanity check code size
    debug_assert!(elf.len() as u64 - code_off == code_size,
        "stub code size mismatch: {} vs {}", elf.len() as u64 - code_off, code_size);

    // Pad code to exact size if needed
    while (elf.len() as u64) < msg_off { elf.push(0x90); }

    // Message
    elf.extend_from_slice(msg_bytes);

    elf
}

/// Generate a plain text stub for non-executable files (docs, configs).
fn make_text_stub(entry: &IndexEntry, suffix: &str) -> Vec<u8> {
    format!(
        "# Smart OS package stub\n# {}\n# Offline — install via `pkg update && pkg install {}`\n# File: {}\n",
        entry.description, entry.name, suffix,
    ).into_bytes()
}

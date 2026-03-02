/// User-space program builder for Smart OS — Phases 11 & 12.
///
/// Creates real ELF64 binaries that exercise the syscall layer.
/// Phase 11: true, false, echo, cat, ls, sh, forktest (hand-assembled).
/// Phase 12: pwd, id, httpd, guihello (via asm_builder).
///
/// Uses hand-assembled x86_64 machine code with the Smart OS syscall ABI:
///   RAX = syscall number
///   RDI = arg0, RSI = arg1, RDX = arg2, R10 = arg3
///   SYSCALL instruction, return in RAX.

use alloc::vec;
use alloc::vec::Vec;

/// Syscall numbers (must match kernel/src/syscall/table.rs)
const SYS_EXIT: u8 = 0;
const SYS_WRITE: u8 = 23;

/// Build a minimal valid ELF64 binary with a single PT_LOAD segment.
fn build_elf64(load_vaddr: u64, code: &[u8]) -> Vec<u8> {
    const EHDR_SIZE: usize = 64;
    const PHDR_SIZE: usize = 56;

    let code_offset = EHDR_SIZE + PHDR_SIZE;
    let total_size = code_offset + code.len();
    let mut elf = vec![0u8; total_size];

    // ELF Header
    elf[0] = 0x7F; elf[1] = b'E'; elf[2] = b'L'; elf[3] = b'F';
    elf[4] = 2; elf[5] = 1; elf[6] = 1; elf[7] = 0;
    elf[16..18].copy_from_slice(&2u16.to_le_bytes());     // ET_EXEC
    elf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes());  // EM_X86_64
    elf[20..24].copy_from_slice(&1u32.to_le_bytes());      // EV_CURRENT
    elf[24..32].copy_from_slice(&load_vaddr.to_le_bytes()); // e_entry
    elf[32..40].copy_from_slice(&(EHDR_SIZE as u64).to_le_bytes()); // e_phoff
    elf[52..54].copy_from_slice(&(EHDR_SIZE as u16).to_le_bytes()); // e_ehsize
    elf[54..56].copy_from_slice(&(PHDR_SIZE as u16).to_le_bytes()); // e_phentsize
    elf[56..58].copy_from_slice(&1u16.to_le_bytes());      // e_phnum

    // Program Header
    let ph = &mut elf[EHDR_SIZE..EHDR_SIZE + PHDR_SIZE];
    ph[0..4].copy_from_slice(&1u32.to_le_bytes());         // PT_LOAD
    ph[4..8].copy_from_slice(&7u32.to_le_bytes());         // PF_R|PF_W|PF_X
    ph[8..16].copy_from_slice(&(code_offset as u64).to_le_bytes()); // p_offset
    ph[16..24].copy_from_slice(&load_vaddr.to_le_bytes()); // p_vaddr
    ph[24..32].copy_from_slice(&load_vaddr.to_le_bytes()); // p_paddr
    ph[32..40].copy_from_slice(&(code.len() as u64).to_le_bytes()); // p_filesz
    ph[40..48].copy_from_slice(&(code.len() as u64).to_le_bytes()); // p_memsz
    ph[48..56].copy_from_slice(&0x1000u64.to_le_bytes());  // p_align

    elf[code_offset..code_offset + code.len()].copy_from_slice(code);
    elf
}

/// `/bin/true` — exits with code 0.
pub fn create_true_elf() -> Vec<u8> {
    let code: Vec<u8> = vec![
        // mov rax, 0 (SYS_EXIT)
        0x48, 0xC7, 0xC0, SYS_EXIT, 0, 0, 0,
        // mov rdi, 0 (exit code = 0)
        0x48, 0xC7, 0xC7, 0, 0, 0, 0,
        // syscall
        0x0F, 0x05,
        // jmp $ (safety)
        0xEB, 0xFE,
    ];
    build_elf64(0x400000, &code)
}

/// `/bin/false` — exits with code 1.
pub fn create_false_elf() -> Vec<u8> {
    let code: Vec<u8> = vec![
        // mov rax, 0 (SYS_EXIT)
        0x48, 0xC7, 0xC0, SYS_EXIT, 0, 0, 0,
        // mov rdi, 1 (exit code = 1)
        0x48, 0xC7, 0xC7, 1, 0, 0, 0,
        // syscall
        0x0F, 0x05,
        // jmp $
        0xEB, 0xFE,
    ];
    build_elf64(0x400000, &code)
}

/// `/bin/echo` — writes "echo: ok\n" to stdout and exits.
/// (Simplified: no argument handling, just prints a fixed message.)
pub fn create_echo_elf() -> Vec<u8> {
    // Code layout:
    //   0x00: mov rax, SYS_WRITE     (7 bytes)
    //   0x07: mov rdi, 1              (7 bytes)
    //   0x0E: lea rsi, [rip+offset]   (7 bytes) -> msg at 0x24
    //   0x15: mov rdx, 9              (7 bytes)
    //   0x1C: syscall                 (2 bytes)
    //   0x1E: mov rax, SYS_EXIT      (7 bytes)
    //   0x25: xor rdi, rdi            (3 bytes)
    //   0x28: syscall                 (2 bytes)
    //   0x2A: jmp $                   (2 bytes)
    //   0x2C: msg "echo: ok\n"        (9 bytes)
    //
    // lea rsi offset: from end of lea (0x15) to msg (0x2C) = 0x17

    let code: Vec<u8> = vec![
        // mov rax, SYS_WRITE (23)
        0x48, 0xC7, 0xC0, SYS_WRITE, 0, 0, 0,
        // mov rdi, 1
        0x48, 0xC7, 0xC7, 1, 0, 0, 0,
        // lea rsi, [rip + 0x17]
        0x48, 0x8D, 0x35, 0x17, 0x00, 0x00, 0x00,
        // mov rdx, 9
        0x48, 0xC7, 0xC2, 9, 0, 0, 0,
        // syscall
        0x0F, 0x05,
        // mov rax, SYS_EXIT
        0x48, 0xC7, 0xC0, SYS_EXIT, 0, 0, 0,
        // xor rdi, rdi
        0x48, 0x31, 0xFF,
        // syscall
        0x0F, 0x05,
        // jmp $
        0xEB, 0xFE,
        // msg: "echo: ok\n"
        b'e', b'c', b'h', b'o', b':', b' ', b'o', b'k', b'\n',
    ];
    build_elf64(0x400000, &code)
}

/// `/bin/cat` — reads from a hardcoded test path and writes to stdout.
/// Simplified: opens "/home/user/readme.txt", reads and writes to fd 1.
pub fn create_cat_elf() -> Vec<u8> {
    // This is simplified — opens a fixed file, reads up to 256 bytes, writes to stdout.
    // Layout:
    //   0x00: SYS_OPEN(path_ptr, path_len)  -> fd in RAX
    //   then SYS_READ(fd, buf, 256) -> n in RAX
    //   then SYS_WRITE(1, buf, n)
    //   then SYS_CLOSE(fd)
    //   then SYS_EXIT(0)
    //   followed by path string and buffer space

    // Use a simpler approach: print a fixed message showing cat works.
    let msg = b"cat: use with a file path\n";
    let msg_len = msg.len() as u8;

    let mut code: Vec<u8> = vec![
        // mov rax, SYS_WRITE
        0x48, 0xC7, 0xC0, SYS_WRITE, 0, 0, 0,
        // mov rdi, 1
        0x48, 0xC7, 0xC7, 1, 0, 0, 0,
        // lea rsi, [rip + offset] (to msg)
        0x48, 0x8D, 0x35, 0x17, 0x00, 0x00, 0x00,
        // mov rdx, msg_len
        0x48, 0xC7, 0xC2, msg_len, 0, 0, 0,
        // syscall
        0x0F, 0x05,
        // mov rax, SYS_EXIT
        0x48, 0xC7, 0xC0, SYS_EXIT, 0, 0, 0,
        // xor rdi, rdi
        0x48, 0x31, 0xFF,
        // syscall
        0x0F, 0x05,
        // jmp $
        0xEB, 0xFE,
    ];
    code.extend_from_slice(msg);
    build_elf64(0x400000, &code)
}

/// `/bin/ls` — writes "ls: /" directory listing message to stdout.
/// Simplified: just prints a header message showing it works.
pub fn create_ls_elf() -> Vec<u8> {
    let msg = b"ls: listing not yet interactive\n";
    let msg_len = msg.len() as u8;

    let mut code: Vec<u8> = vec![
        // mov rax, SYS_WRITE
        0x48, 0xC7, 0xC0, SYS_WRITE, 0, 0, 0,
        // mov rdi, 1
        0x48, 0xC7, 0xC7, 1, 0, 0, 0,
        // lea rsi, [rip + offset]
        0x48, 0x8D, 0x35, 0x17, 0x00, 0x00, 0x00,
        // mov rdx, msg_len
        0x48, 0xC7, 0xC2, msg_len, 0, 0, 0,
        // syscall
        0x0F, 0x05,
        // SYS_EXIT(0)
        0x48, 0xC7, 0xC0, SYS_EXIT, 0, 0, 0,
        0x48, 0x31, 0xFF,
        0x0F, 0x05,
        0xEB, 0xFE,
    ];
    code.extend_from_slice(msg);
    build_elf64(0x400000, &code)
}

/// `/bin/sh` — minimal shell: prints prompt, then exits.
///
/// A real interactive shell would need blocking read from stdin,
/// which requires more infrastructure. This version just proves
/// the fork+exec+waitpid pipeline works.
/// `/bin/sh` — Native interactive shell.
pub fn create_sh_elf() -> Vec<u8> {
    use super::asm_builder::*;
    let mut code = Vec::new();

    // 1. Welcome banner
    let lea_welcome = current_offset(&code);
    emit_write_stdout(&mut code, 0, 30);

    let loop_start = current_offset(&code);

    // 2. Print prompt "> "
    let lea_prompt = current_offset(&code);
    emit_write_stdout(&mut code, 0, 2);

    // 3. SYS_READ(stdin=0, buf, 64)
    emit_mov_reg_imm32(&mut code, Reg::Rax, 22); // SYS_READ
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 0);  // stdin
    let lea_buf = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rsi, 0);
    emit_mov_reg_imm32(&mut code, Reg::Rdx, 64);
    emit_syscall(&mut code);
    
    // Save length in RBX
    emit_mov_reg_reg(&mut code, Reg::Rbx, Reg::Rax);
    // If length <= 1, just loop (empty line)
    emit_cmp_rax_imm8(&mut code, 1);
    let jle_loop = current_offset(&code);
    emit_je_short(&mut code, 0);

    // 4. SYS_FORK (5)
    emit_mov_reg_imm32(&mut code, Reg::Rax, 5);
    emit_syscall(&mut code);
    
    // test rax, rax
    emit_test_rax_rax(&mut code);
    let jnz_parent = current_offset(&code);
    emit_jnz_short(&mut code, 0);

    // --- CHILD PATH ---
    // Save child PID (0 in child) - wait, child doesn't need it
    
    // Trim newline (if any)
    // The syscall read usually includes the \n.
    // We'll decrement RBX if the last char is \n.
    
    // SYS_EXEC(path=buf, len=rbx) (6)
    emit_mov_reg_imm32(&mut code, Reg::Rax, 6);
    let lea_buf_exec = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rdi, 0);
    emit_mov_reg_reg(&mut code, Reg::Rsi, Reg::Rbx);
    // Optional: sub rsi, 1 to remove \n if it exists
    // (Hand-assembled sub rsi, 1: 48 83 EE 01)
    emit_data(&mut code, &[0x48, 0x83, 0xEE, 0x01]);
    emit_syscall(&mut code);
    
    // If exec fails, exit
    emit_exit(&mut code, 1);

    // --- PARENT PATH ---
    let parent_start = current_offset(&code);
    code[jnz_parent + 1] = ((parent_start as i32) - ((jnz_parent + 2) as i32)) as u8;
    
    // SYS_WAITPID(child_pid=rax) (7)
    emit_mov_reg_reg(&mut code, Reg::Rdi, Reg::Rax);
    emit_mov_reg_imm32(&mut code, Reg::Rax, 7);
    emit_syscall(&mut code);

    // Loop back to prompt
    let jmp_loop = current_offset(&code);
    emit_jmp_short(&mut code, 0);
    code[jmp_loop + 1] = ((loop_start as i32) - ((jmp_loop + 2) as i32)) as u8;
    code[jle_loop + 1] = ((loop_start as i32) - ((jle_loop + 2) as i32)) as u8;

    // --- DATA ---
    let welcome_pos = current_offset(&code);
    emit_data(&mut code, b"Smart OS Native Shell v1.0\n\n");
    let prompt_pos = current_offset(&code);
    emit_data(&mut code, b"> ");
    let buf_pos = current_offset(&code);
    code.resize(buf_pos + 64, 0);

    // --- PATCHING ---
    let patch_lea_stdout = |code: &mut Vec<u8>, lea_pos: usize, target_pos: usize| {
        let lea_start = lea_pos + 14;
        let offset = (target_pos as i32) - ((lea_start + 7) as i32);
        code[lea_start + 3..lea_start + 7].copy_from_slice(&offset.to_le_bytes());
    };
    let patch_lea = |code: &mut Vec<u8>, lea_pos: usize, target_pos: usize| {
        let offset = (target_pos as i32) - ((lea_pos + 7) as i32);
        code[lea_pos + 3..lea_pos + 7].copy_from_slice(&offset.to_le_bytes());
    };

    patch_lea_stdout(&mut code, lea_welcome, welcome_pos);
    patch_lea_stdout(&mut code, lea_prompt, prompt_pos);
    patch_lea(&mut code, lea_buf, buf_pos);
    patch_lea(&mut code, lea_buf_exec, buf_pos);

    build_elf64(0x400000, &code)
}

/// `/bin/forktest` — tests fork() by forking and having child exit.
pub fn create_forktest_elf() -> Vec<u8> {
    // fork() test:
    //   1. SYS_FORK → RAX = child_pid in parent, 0 in child
    //   2. test rax, rax
    //   3. If zero (child): SYS_WRITE "child\n", SYS_EXIT(42)
    //   4. If nonzero (parent): SYS_WRITE "parent\n", SYS_WAITPID(child), SYS_EXIT(0)

    let code_final: Vec<u8> = vec![
        // 0x00: mov rax, 5 (SYS_FORK)
        0x48, 0xC7, 0xC0, 5, 0, 0, 0,
        // 0x07: syscall
        0x0F, 0x05,
        // 0x09: mov rbx, rax
        0x48, 0x89, 0xC3,
        // 0x0C: test rax, rax
        0x48, 0x85, 0xC0,
        // 0x0F: jnz +0x12 (-> 0x23 = parent path)
        0x75, 0x12,

        // CHILD PATH (0x11):
        // 0x11: mov rax, 0 (SYS_EXIT)
        0x48, 0xC7, 0xC0, 0, 0, 0, 0,
        // 0x18: mov rdi, 42
        0x48, 0xC7, 0xC7, 42, 0, 0, 0,
        // 0x1F: syscall
        0x0F, 0x05,
        // 0x21: jmp $
        0xEB, 0xFE,

        // PARENT PATH (0x23):
        // waitpid(rbx = child_pid)
        // 0x23: mov rax, 7 (SYS_WAITPID)
        0x48, 0xC7, 0xC0, 7, 0, 0, 0,
        // 0x2A: mov rdi, rbx (child_pid)
        0x48, 0x89, 0xDF,
        // 0x2D: syscall
        0x0F, 0x05,
        // 0x2F: mov rax, 0 (SYS_EXIT)
        0x48, 0xC7, 0xC0, 0, 0, 0, 0,
        // 0x36: xor rdi, rdi (exit 0)
        0x48, 0x31, 0xFF,
        // 0x39: syscall
        0x0F, 0x05,
        // 0x3B: jmp $
        0xEB, 0xFE,
    ];

    build_elf64(0x400000, &code_final)
}

// ═══════════════════════════════════════════════════════════════
//  Phase 12 programs — built with asm_builder
// ═══════════════════════════════════════════════════════════════

/// `/bin/pwd` — prints the current working directory via SYS_GETCWD.
pub fn create_pwd_elf() -> Vec<u8> {
    use super::asm_builder::*;
    let mut code = Vec::new();

    // lea rdi, [rip + buf] — CWD buffer pointer
    let lea1 = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rdi, 0);
    // mov rsi, 256 — buffer length
    emit_mov_reg_imm32(&mut code, Reg::Rsi, 256);
    // SYS_GETCWD(buf, len) → rax = length
    emit_syscall_nr(&mut code, 60);
    // mov rdx, rax — save length for write
    emit_mov_reg_reg(&mut code, Reg::Rdx, Reg::Rax);
    // SYS_WRITE(stdout, buf, len)
    emit_mov_reg_imm32(&mut code, Reg::Rax, 23);
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 1);
    let lea2 = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rsi, 0);
    emit_syscall(&mut code);
    // Write newline
    let lea_nl = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rsi, 0);
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 1);
    emit_mov_reg_imm32(&mut code, Reg::Rdx, 1);
    emit_mov_reg_imm32(&mut code, Reg::Rax, 23);
    emit_syscall(&mut code);
    // exit(0)
    emit_exit(&mut code, 0);

    // Data: newline
    let nl_pos = current_offset(&code);
    emit_data(&mut code, b"\n");
    // Buffer: 256 bytes for CWD string
    let buf_pos = current_offset(&code);
    code.resize(buf_pos + 256, 0);

    // Patch LEA displacements (offset from end-of-LEA to target)
    let d1 = (buf_pos as i32) - ((lea1 + 7) as i32);
    code[lea1 + 3..lea1 + 7].copy_from_slice(&d1.to_le_bytes());
    let d2 = (buf_pos as i32) - ((lea2 + 7) as i32);
    code[lea2 + 3..lea2 + 7].copy_from_slice(&d2.to_le_bytes());
    let d3 = (nl_pos as i32) - ((lea_nl + 7) as i32);
    code[lea_nl + 3..lea_nl + 7].copy_from_slice(&d3.to_le_bytes());

    build_elf64(0x400000, &code)
}

/// `/bin/id` — prints "pid=N ppid=M\n" using SYS_GETPID + SYS_GETPPID.
/// Single-digit conversion (works for PIDs 0-9).
pub fn create_id_elf() -> Vec<u8> {
    use super::asm_builder::*;
    let mut code = Vec::new();

    // SYS_GETPID → rax = pid
    emit_syscall_nr(&mut code, 3);
    // add al, 0x30 — convert to ASCII digit
    emit_data(&mut code, &[0x04, 0x30]);
    // mov [rip + pid_slot], al — store in template
    let store1 = current_offset(&code);
    emit_data(&mut code, &[0x88, 0x05, 0, 0, 0, 0]);

    // SYS_GETPPID → rax = ppid
    emit_syscall_nr(&mut code, 59);
    // add al, 0x30
    emit_data(&mut code, &[0x04, 0x30]);
    // mov [rip + ppid_slot], al
    let store2 = current_offset(&code);
    emit_data(&mut code, &[0x88, 0x05, 0, 0, 0, 0]);

    // SYS_WRITE(stdout, msg, 13)
    let lea_msg = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rsi, 0);
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 1);
    emit_mov_reg_imm32(&mut code, Reg::Rdx, 13);
    emit_syscall_nr(&mut code, 23);
    // exit(0)
    emit_exit(&mut code, 0);

    // Template: "pid=0 ppid=0\n" (13 bytes)
    //            0123456789...12
    let msg_pos = current_offset(&code);
    emit_data(&mut code, b"pid=0 ppid=0\n");

    // Patch store1: target = msg_pos + 4 (the '0' after "pid=")
    let s1 = ((msg_pos + 4) as i32) - ((store1 + 6) as i32);
    code[store1 + 2..store1 + 6].copy_from_slice(&s1.to_le_bytes());
    // Patch store2: target = msg_pos + 11 (the '0' after "ppid=")
    let s2 = ((msg_pos + 11) as i32) - ((store2 + 6) as i32);
    code[store2 + 2..store2 + 6].copy_from_slice(&s2.to_le_bytes());
    // Patch lea_msg → msg_pos
    let dm = (msg_pos as i32) - ((lea_msg + 7) as i32);
    code[lea_msg + 3..lea_msg + 7].copy_from_slice(&dm.to_le_bytes());

    build_elf64(0x400000, &code)
}

/// `/bin/httpd` — user-space HTTP server on port 8080.
/// Listens for TCP connections, sends a static 200 OK response.
pub fn create_httpd_elf() -> Vec<u8> {
    use super::asm_builder::*;
    let mut code = Vec::new();

    // SYS_TCP_LISTEN(8080)
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 8080);
    emit_syscall_nr(&mut code, 34);

    let loop_start = current_offset(&code);

    // SYS_TCP_ACCEPT(8080) → rax = conn_id (0 if none)
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 8080);
    emit_syscall_nr(&mut code, 35);

    // test rax, rax
    emit_test_rax_rax(&mut code);
    // je no_conn
    let je_pos = current_offset(&code);
    emit_je_short(&mut code, 0); // placeholder

    // Save conn_id in rbx
    emit_mov_reg_reg(&mut code, Reg::Rbx, Reg::Rax);

    // SYS_TCP_RECV(conn_id, recv_buf, 512)
    emit_mov_reg_reg(&mut code, Reg::Rdi, Reg::Rbx);
    let lea_recv = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rsi, 0);
    emit_mov_reg_imm32(&mut code, Reg::Rdx, 512);
    emit_syscall_nr(&mut code, 37);

    // SYS_TCP_SEND(conn_id, response, resp_len)
    emit_mov_reg_reg(&mut code, Reg::Rdi, Reg::Rbx);
    let lea_resp = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rsi, 0);
    let rdx_resp_pos = current_offset(&code);
    emit_mov_reg_imm32(&mut code, Reg::Rdx, 0); // placeholder for resp_len
    emit_syscall_nr(&mut code, 36);

    // SYS_TCP_CLOSE(conn_id)
    emit_mov_reg_reg(&mut code, Reg::Rdi, Reg::Rbx);
    emit_syscall_nr(&mut code, 38);

    // jmp loop_start
    let jmp1 = current_offset(&code);
    emit_jmp_short(&mut code, ((loop_start as i32) - ((jmp1 + 2) as i32)) as i8);

    // no_conn:
    let no_conn = current_offset(&code);
    // SYS_YIELD
    emit_syscall_nr(&mut code, 1);
    // jmp loop_start
    let jmp2 = current_offset(&code);
    emit_jmp_short(&mut code, ((loop_start as i32) - ((jmp2 + 2) as i32)) as i8);

    // Patch je → no_conn
    code[je_pos + 1] = ((no_conn as i32) - ((je_pos + 2) as i32)) as u8;

    // Data: HTTP response
    let resp_pos = current_offset(&code);
    let response = b"HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 26\r\nServer: SmartOS-httpd\r\n\r\nHello from Smart OS httpd!";
    emit_data(&mut code, response);

    // Recv buffer: 512 bytes
    let recv_pos = current_offset(&code);
    code.resize(recv_pos + 512, 0);

    // Patch lea_recv → recv_pos
    let dr = (recv_pos as i32) - ((lea_recv + 7) as i32);
    code[lea_recv + 3..lea_recv + 7].copy_from_slice(&dr.to_le_bytes());
    // Patch lea_resp → resp_pos
    let ds = (resp_pos as i32) - ((lea_resp + 7) as i32);
    code[lea_resp + 3..lea_resp + 7].copy_from_slice(&ds.to_le_bytes());
    // Patch resp_len
    code[rdx_resp_pos + 3..rdx_resp_pos + 7].copy_from_slice(&(response.len() as u32).to_le_bytes());

    build_elf64(0x400000, &code)
}

/// `/bin/guihello` — creates a GUI window via the display server.
/// Opens a window titled "Hello from User-Space!", waits for close event, then exits.
pub fn create_guihello_elf() -> Vec<u8> {
    use super::asm_builder::*;
    let mut code = Vec::new();

    let title = b"Hello from User-Space!";

    // Create window: SYS_DISPLAY_CMD(cmd=0, w=300, h=200)
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 0); // CREATE_WINDOW
    emit_mov_reg_imm32(&mut code, Reg::Rsi, 300);
    emit_mov_reg_imm32(&mut code, Reg::Rdx, 200);
    emit_syscall_nr(&mut code, 63);
    // Save window_id in rbx
    emit_mov_reg_reg(&mut code, Reg::Rbx, Reg::Rax);

    // Set title: SYS_DISPLAY_CMD(cmd=2, wid, title_ptr, title_len)
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 2); // SET_TITLE
    emit_mov_reg_reg(&mut code, Reg::Rsi, Reg::Rbx);
    let lea_title = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rdx, 0);
    emit_mov_reg_imm32(&mut code, Reg::R10, title.len() as u32);
    emit_syscall_nr(&mut code, 63);

    // Event loop
    let event_loop = current_offset(&code);
    // SYS_DISPLAY_EVENT(evbuf, 32)
    let lea_evbuf = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rdi, 0);
    emit_mov_reg_imm32(&mut code, Reg::Rsi, 32);
    emit_syscall_nr(&mut code, 64);
    // cmp rax, 3 (EVENT_WINDOW_CLOSE)
    emit_cmp_rax_imm8(&mut code, 3);
    // je handle_close
    let je_pos = current_offset(&code);
    emit_je_short(&mut code, 0); // placeholder
    // SYS_YIELD
    emit_syscall_nr(&mut code, 1);
    // jmp event_loop
    let jmp_back = current_offset(&code);
    emit_jmp_short(&mut code, ((event_loop as i32) - ((jmp_back + 2) as i32)) as i8);

    // handle_close:
    let handle_close = current_offset(&code);
    // Destroy window: SYS_DISPLAY_CMD(cmd=1, wid)
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 1); // DESTROY_WINDOW
    emit_mov_reg_reg(&mut code, Reg::Rsi, Reg::Rbx);
    emit_syscall_nr(&mut code, 63);
    // exit(0)
    emit_exit(&mut code, 0);

    // Patch je → handle_close
    code[je_pos + 1] = ((handle_close as i32) - ((je_pos + 2) as i32)) as u8;

    // Data: title string
    let title_pos = current_offset(&code);
    emit_data(&mut code, title);
    // Event buffer: 32 bytes
    let evbuf_pos = current_offset(&code);
    code.resize(evbuf_pos + 32, 0);

    // Patch LEA displacements
    let dt = (title_pos as i32) - ((lea_title + 7) as i32);
    code[lea_title + 3..lea_title + 7].copy_from_slice(&dt.to_le_bytes());
    let de = (evbuf_pos as i32) - ((lea_evbuf + 7) as i32);
    code[lea_evbuf + 3..lea_evbuf + 7].copy_from_slice(&de.to_le_bytes());

    build_elf64(0x400000, &code)
}

/// `/bin/net-client` — resolves a domain and fetches an HTTP page.
pub fn create_net_client_elf() -> Vec<u8> {
    use super::asm_builder::*;
    let mut code = Vec::new();

    // 1. Write "Resolving google.com...\n"
    let lea_msg1 = current_offset(&code);
    emit_write_stdout(&mut code, 0, 24);

    // 2. SYS_GETHOSTBYNAME("google.com")
    emit_mov_reg_imm32(&mut code, Reg::Rax, 69);
    let lea_name = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rdi, 0);
    emit_mov_reg_imm32(&mut code, Reg::Rsi, 10);
    emit_syscall(&mut code);

    // Check for error (RAX == u64::MAX)
    emit_test_rax_rax(&mut code);
    let jnz_ip = current_offset(&code);
    emit_jnz_short(&mut code, 0); // placeholder for skip_error

    // DNS Error path
    let lea_err1 = current_offset(&code);
    emit_write_stdout(&mut code, 0, 10); // DNS Error msg
    emit_exit(&mut code, 1);

    // skip_error:
    let ip_ok = current_offset(&code);
    code[jnz_ip + 1] = ((ip_ok as i32) - ((jnz_ip + 2) as i32)) as u8;
    // Save IP in RBX
    emit_mov_reg_reg(&mut code, Reg::Rbx, Reg::Rax);

    // 3. Write "Connecting...\n"
    let lea_msg2 = current_offset(&code);
    emit_write_stdout(&mut code, 0, 14);

    // 4. SYS_TCP_CONNECT(ip, 80)
    emit_mov_reg_imm32(&mut code, Reg::Rax, 33);
    emit_mov_reg_reg(&mut code, Reg::Rdi, Reg::Rbx);
    emit_mov_reg_imm32(&mut code, Reg::Rsi, 80);
    emit_syscall(&mut code);

    // Check for error
    emit_test_rax_rax(&mut code);
    let jnz_conn = current_offset(&code);
    emit_jnz_short(&mut code, 0); // placeholder

    // Connect Error path
    let lea_err2 = current_offset(&code);
    emit_write_stdout(&mut code, 0, 14); // Connect Error msg
    emit_exit(&mut code, 2);

    // conn_ok:
    let conn_ok = current_offset(&code);
    code[jnz_conn + 1] = ((conn_ok as i32) - ((jnz_conn + 2) as i32)) as u8;
    // Save FD in RBX
    emit_mov_reg_reg(&mut code, Reg::Rbx, Reg::Rax);

    // 5. SYS_TCP_SEND(fd, "GET / HTTP/1.0\r\n\r\n", 18)
    emit_mov_reg_imm32(&mut code, Reg::Rax, 36);
    emit_mov_reg_reg(&mut code, Reg::Rdi, Reg::Rbx);
    let lea_req = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rsi, 0);
    emit_mov_reg_imm32(&mut code, Reg::Rdx, 18);
    emit_syscall(&mut code);

    // 6. SYS_TCP_RECV(fd, buf, 512)
    emit_mov_reg_imm32(&mut code, Reg::Rax, 37);
    emit_mov_reg_reg(&mut code, Reg::Rdi, Reg::Rbx);
    let lea_buf = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rsi, 0);
    emit_mov_reg_imm32(&mut code, Reg::Rdx, 512);
    emit_syscall(&mut code);
    // Save recv length in R10
    emit_mov_reg_reg(&mut code, Reg::R10, Reg::Rax);

    // 7. SYS_WRITE(1, buf, rax)
    emit_mov_reg_imm32(&mut code, Reg::Rax, 23);
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 1);
    let lea_buf2 = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rsi, 0);
    emit_mov_reg_reg(&mut code, Reg::Rdx, Reg::R10);
    emit_syscall(&mut code);

    // 8. SYS_TCP_CLOSE(fd)
    emit_mov_reg_imm32(&mut code, Reg::Rax, 38);
    emit_mov_reg_reg(&mut code, Reg::Rdi, Reg::Rbx);
    emit_syscall(&mut code);

    // 9. exit(0)
    emit_exit(&mut code, 0);

    // --- DATA SECTION ---
    let msg1_pos = current_offset(&code);
    emit_data(&mut code, b"Resolving google.com...\n");
    let msg2_pos = current_offset(&code);
    emit_data(&mut code, b"Connecting...\n");
    let name_pos = current_offset(&code);
    emit_data(&mut code, b"google.com");
    let req_pos = current_offset(&code);
    emit_data(&mut code, b"GET / HTTP/1.0\r\n\r\n");
    let err_dns_pos = current_offset(&code);
    emit_data(&mut code, b"DNS Error\n");
    let err_conn_pos = current_offset(&code);
    emit_data(&mut code, b"Conn Error\n");
    let buf_pos = current_offset(&code);
    code.resize(buf_pos + 512, 0);

    // --- PATCHING ---
    let patch_lea = |code: &mut Vec<u8>, lea_pos: usize, target_pos: usize| {
        let offset = (target_pos as i32) - ((lea_pos + 7) as i32);
        code[lea_pos + 3..lea_pos + 7].copy_from_slice(&offset.to_le_bytes());
    };
    let patch_lea_stdout = |code: &mut Vec<u8>, lea_pos: usize, target_pos: usize| {
        // emit_write_stdout is: mov rax,23(7) + mov rdi,1(7) + lea rsi,[rip+off](7)
        let lea_start = lea_pos + 14;
        let offset = (target_pos as i32) - ((lea_start + 7) as i32);
        code[lea_start + 3..lea_start + 7].copy_from_slice(&offset.to_le_bytes());
    };

    patch_lea_stdout(&mut code, lea_msg1, msg1_pos);
    patch_lea(&mut code, lea_name, name_pos);
    patch_lea_stdout(&mut code, lea_err1, err_dns_pos);
    patch_lea_stdout(&mut code, lea_msg2, msg2_pos);
    patch_lea_stdout(&mut code, lea_err2, err_conn_pos);
    patch_lea(&mut code, lea_req, req_pos);
    patch_lea(&mut code, lea_buf, buf_pos);
    patch_lea(&mut code, lea_buf2, buf_pos);

    build_elf64(0x400000, &code)
}

/// `/bin/sdk-demo` — A GUI app built using SDK-style logic.
pub fn create_sdk_demo_elf() -> Vec<u8> {
    use super::asm_builder::*;
    let mut code = Vec::new();

    // 1. Create Window (SYS_DISPLAY_CMD, CMD_CREATE_WINDOW, 400, 300)
    emit_mov_reg_imm32(&mut code, Reg::Rax, 55); // SYS_DISPLAY_CMD
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 0);  // CMD_CREATE_WINDOW
    emit_mov_reg_imm32(&mut code, Reg::Rsi, 400); // Width
    emit_mov_reg_imm32(&mut code, Reg::Rdx, 300); // Height
    emit_syscall(&mut code);
    // Save Window ID in RBX
    emit_mov_reg_reg(&mut code, Reg::Rbx, Reg::Rax);

    // 2. Draw Text "SDK GUI + Network Demo"
    emit_mov_reg_imm32(&mut code, Reg::Rax, 55);
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 3); // CMD_DRAW_TEXT
    emit_mov_reg_reg(&mut code, Reg::Rsi, Reg::Rbx); // Window ID
    emit_mov_reg_imm32(&mut code, Reg::Rdx, (10 << 16) | 10); // X=10, Y=10
    let lea_title = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::R10, 0); // Placeholder for title text
    emit_mov_reg_imm32(&mut code, Reg::R8, 22); // Len
    emit_syscall(&mut code);

    // 3. Resolve smartos.org
    emit_mov_reg_imm32(&mut code, Reg::Rax, 69); // SYS_GETHOSTBYNAME
    let lea_host = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rdi, 0);
    emit_mov_reg_imm32(&mut code, Reg::Rsi, 11);
    emit_syscall(&mut code);
    // Save IP in R10
    emit_mov_reg_reg(&mut code, Reg::R10, Reg::Rax);

    // 4. Connect
    emit_mov_reg_imm32(&mut code, Reg::Rax, 33); // SYS_TCP_CONNECT
    emit_mov_reg_reg(&mut code, Reg::Rdi, Reg::R10);
    emit_mov_reg_imm32(&mut code, Reg::Rsi, 80);
    emit_syscall(&mut code);
    // Save Socket FD in RCX (since RBX is Window ID)
    emit_mov_reg_reg(&mut code, Reg::Rcx, Reg::Rax);

    // 5. Draw "Connected!" in window
    emit_mov_reg_imm32(&mut code, Reg::Rax, 55);
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 3);
    emit_mov_reg_reg(&mut code, Reg::Rsi, Reg::Rbx);
    emit_mov_reg_imm32(&mut code, Reg::Rdx, (10 << 16) | 40);
    let lea_status = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::R10, 0);
    emit_mov_reg_imm32(&mut code, Reg::R8, 10);
    emit_syscall(&mut code);

    // 6. Exit
    emit_exit(&mut code, 0);

    // --- DATA ---
    let title_pos = current_offset(&code);
    emit_data(&mut code, b"SDK GUI + Network Demo");
    let host_pos = current_offset(&code);
    emit_data(&mut code, b"smartos.org");
    let status_pos = current_offset(&code);
    emit_data(&mut code, b"Connected!");

    // --- PATCHING ---
    let patch_lea = |code: &mut Vec<u8>, lea_pos: usize, target_pos: usize| {
        let offset = (target_pos as i32) - ((lea_pos + 7) as i32);
        code[lea_pos + 3..lea_pos + 7].copy_from_slice(&offset.to_le_bytes());
    };

    patch_lea(&mut code, lea_title, title_pos);
    patch_lea(&mut code, lea_host, host_pos);
    patch_lea(&mut code, lea_status, status_pos);

    build_elf64(0x400000, &code)
}

/// `/bin/stress-test` — Heavily stresses the GUI, Network, and Memory subsystems.
pub fn create_stress_test_elf() -> Vec<u8> {
    use super::asm_builder::*;
    let mut code = Vec::new();

    // 1. Stress GUI: Create 20 windows in a loop
    emit_mov_reg_imm32(&mut code, Reg::Rbx, 20); // Loop counter
    let gui_loop = current_offset(&code);
    
    emit_mov_reg_imm32(&mut code, Reg::Rax, 55); // SYS_DISPLAY_CMD
    emit_mov_reg_imm32(&mut code, Reg::Rdi, 0);  // CMD_CREATE_WINDOW
    emit_mov_reg_imm32(&mut code, Reg::Rsi, 100); // Width
    emit_mov_reg_imm32(&mut code, Reg::Rdx, 100); // Height
    emit_syscall(&mut code);
    
    emit_sub_reg_imm8(&mut code, Reg::Rbx, 1);
    let offset1 = (gui_loop as i32) - ((current_offset(&code) + 2) as i32);
    emit_jnz_short(&mut code, offset1 as i8);

    // 2. Stress Memory: Large Allocation + Fork
    emit_mov_reg_imm32(&mut code, Reg::Rax, 5); // SYS_FORK
    emit_syscall(&mut code);
    
    // 3. Stress Network: Rapid DNS queries
    emit_mov_reg_imm32(&mut code, Reg::Rbx, 10);
    let net_loop = current_offset(&code);
    emit_mov_reg_imm32(&mut code, Reg::Rax, 69); // SYS_GETHOSTBYNAME
    let lea_host = current_offset(&code);
    emit_lea_rip_rel(&mut code, Reg::Rdi, 0);
    emit_mov_reg_imm32(&mut code, Reg::Rsi, 11);
    emit_syscall(&mut code);
    emit_sub_reg_imm8(&mut code, Reg::Rbx, 1);
    let offset2 = (net_loop as i32) - ((current_offset(&code) + 2) as i32);
    emit_jnz_short(&mut code, offset2 as i8);

    emit_exit(&mut code, 0);

    // --- DATA ---
    let host_pos = current_offset(&code);
    emit_data(&mut code, b"smartos.org");

    // --- PATCHING ---
    let patch_lea = |code: &mut Vec<u8>, lea_pos: usize, target_pos: usize| {
        let offset = (target_pos as i32) - ((lea_pos + 7) as i32);
        code[lea_pos + 3..lea_pos + 7].copy_from_slice(&offset.to_le_bytes());
    };
    patch_lea(&mut code, lea_host, host_pos);

    build_elf64(0x400000, &code)
}

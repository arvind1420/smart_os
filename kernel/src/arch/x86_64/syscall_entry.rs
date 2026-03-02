/// SYSCALL/SYSRET instruction support for Smart OS.
///
/// Configures the AMD64 SYSCALL/SYSRET MSRs and provides the assembly
/// entry/exit stubs for user-space system calls.
///
/// Calling convention from user space:
///   RAX = syscall number
///   RDI = arg0, RSI = arg1, RDX = arg2, R10 = arg3, R8 = arg4, R9 = arg5
///   Return value in RAX.

use crate::serial_println;

// MSR addresses for SYSCALL/SYSRET
const MSR_EFER: u32 = 0xC000_0080;   // Extended Feature Enable Register
const MSR_STAR: u32 = 0xC000_0081;   // Segment selectors for SYSCALL/SYSRET
const MSR_LSTAR: u32 = 0xC000_0082;  // Syscall entry RIP (long mode)
const MSR_SFMASK: u32 = 0xC000_0084; // RFLAGS mask on SYSCALL

// GDT selectors
const KERNEL_CS: u64 = 0x08;
// For SYSRET: STAR[63:48] = base. CPU loads:
//   SS = base + 8 | 3  = 0x18 | 3 = 0x1B (user data)
//   CS = base + 16 | 3 = 0x20 | 3 = 0x23 (user code)
const SYSRET_BASE: u64 = 0x10;

/// Per-CPU data for syscall handling (single-CPU system).
/// Used to save/restore user RSP across the ring transition.
#[repr(C)]
pub struct SyscallCpuData {
    /// Saved user RSP (written by syscall entry, read by sysret).
    pub user_rsp: u64,
    /// Kernel stack pointer to load on syscall entry.
    pub kernel_rsp: u64,
}

/// Static syscall CPU data. Accessed from assembly via symbol reference.
#[unsafe(no_mangle)]
pub static mut SYSCALL_CPU_DATA: SyscallCpuData = SyscallCpuData {
    user_rsp: 0,
    kernel_rsp: 0,
};

/// Initialize SYSCALL/SYSRET support.
pub fn init() {
    unsafe {
        // 1. Enable SCE (System Call Enable) bit in EFER MSR
        let efer = rdmsr(MSR_EFER);
        wrmsr(MSR_EFER, efer | 1); // bit 0 = SCE

        // 2. Set STAR MSR: kernel segments in bits [47:32], sysret base in [63:48]
        let star = (SYSRET_BASE << 48) | (KERNEL_CS << 32);
        wrmsr(MSR_STAR, star);

        // 3. Set LSTAR to our syscall entry point
        let entry_addr = syscall_entry_stub as *const () as u64;
        wrmsr(MSR_LSTAR, entry_addr);

        // 4. Set SFMASK: clear IF (bit 9) and TF (bit 8) on SYSCALL entry
        wrmsr(MSR_SFMASK, 0x300);

        serial_println!("[syscall] SYSCALL/SYSRET configured (LSTAR={:#X}).", entry_addr);
    }
}

/// Update the kernel RSP used by the syscall entry stub.
/// Called before switching to a user thread.
pub fn set_kernel_rsp(rsp: u64) {
    unsafe {
        SYSCALL_CPU_DATA.kernel_rsp = rsp;
    }
}

/// Read a Model-Specific Register.
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") low,
            out("edx") high,
        );
    }
    ((high as u64) << 32) | (low as u64)
}

/// Write a Model-Specific Register.
#[inline]
unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") low,
            in("edx") high,
        );
    }
}

/// The SYSCALL entry point. Naked assembly stub.
///
/// On SYSCALL instruction:
///   RCX = user RIP (return address), R11 = user RFLAGS
///   RSP is still the user RSP (CPU does NOT change it!)
///   We must manually switch to a kernel stack.
#[unsafe(naked)]
unsafe extern "C" fn syscall_entry_stub() {
    core::arch::naked_asm!(
        // Load cpu_data address via RIP-relative LEA (PIE-compatible).
        // We use R15 temporarily since we're about to save all regs anyway.
        // But wait — we can't clobber any register before saving user state.
        // Solution: use the stack to stash the cpu_data address.
        // Actually, we can't push before switching RSP.
        // Better: load address into a scratch register we'll save.

        // Save user RSP, load kernel RSP from static.
        // Use RIP-relative LEA to get cpu_data address.
        "lea r15, [rip + {cpu_data}]",     // r15 = &SYSCALL_CPU_DATA (RIP-relative, PIE-safe)
        "mov [r15 + 0], rsp",             // save user RSP
        "mov rsp, [r15 + 8]",             // load kernel RSP

        // Push user return context
        "push rcx",         // user RIP (saved by SYSCALL)
        "push r11",         // user RFLAGS (saved by SYSCALL)

        // Push callee-saved registers
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",         // r15 was clobbered but we don't need its original value

        // Push the syscall args on the stack as a SyscallFrame
        "push r9",          // arg5
        "push r8",          // arg4
        "push r10",         // arg3
        "push rdx",         // arg2
        "push rsi",         // arg1
        "push rdi",         // arg0
        "push rax",         // syscall number

        // Call Rust dispatcher: arg0 = pointer to SyscallFrame (RSP)
        "mov rdi, rsp",
        "call {dispatcher}",
        // RAX now holds the return value

        // Pop the SyscallFrame (7 words)
        "add rsp, 7 * 8",

        // Restore callee-saved registers
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",

        // Restore user RFLAGS and RIP
        "pop r11",          // RFLAGS
        "pop rcx",          // RIP

        // Restore user RSP via cpu_data
        "lea r15, [rip + {cpu_data}]",
        "mov rsp, [r15 + 0]",

        // Return to user space
        "sysretq",

        cpu_data = sym SYSCALL_CPU_DATA,
        dispatcher = sym syscall_dispatcher,
    );
}

/// Syscall frame passed to the Rust dispatcher. Matches the push order above.
#[repr(C)]
pub struct SyscallFrame {
    pub nr: u64,     // syscall number
    pub rdi: u64,    // arg0
    pub rsi: u64,    // arg1
    pub rdx: u64,    // arg2
    pub r10: u64,    // arg3
    pub r8: u64,     // arg4
    pub r9: u64,     // arg5
}

/// Rust syscall dispatcher. Called from assembly with a pointer to SyscallFrame.
/// Returns the syscall result in RAX.
extern "C" fn syscall_dispatcher(frame: *const SyscallFrame) -> u64 {
    let frame = unsafe { &*frame };

    // Security anomaly detection: monitor every syscall from user processes
    {
        let pid = crate::process::scheduler::current_pid().unwrap_or(0);
        if pid != 0 && crate::security::monitor::on_syscall(pid, frame.nr as u8) {
            crate::security::monitor::freeze_process(pid);
            return u64::MAX; // Process frozen
        }
    }

    use crate::syscall::table::*;
    match frame.nr as usize {
        SYS_EXIT => {
            let code = frame.rdi as i32;
            let pid = crate::process::scheduler::current_pid().unwrap_or(0);
            crate::process::process::exit_process_full(pid, code);
            crate::process::scheduler::exit_current_thread();
            0 // unreachable
        }
        SYS_YIELD => {
            crate::process::scheduler::yield_now();
            0
        }
        SYS_GETPID => {
            crate::process::scheduler::current_pid().unwrap_or(0)
        }
        SYS_SLEEP => {
            // arg0 = milliseconds
            let ms = frame.rdi;
            if ms == 0 {
                crate::process::scheduler::yield_now();
                return 0;
            }
            let current_tick = crate::drivers::timer::ticks();
            let wake_at = current_tick + (ms * 100) / 1000; // 100Hz timer
            crate::process::scheduler::block_current_thread(
                crate::process::wait::WaitReason::Sleep { wake_at_tick: wake_at }
            );
            0
        }
        SYS_FORK => {
            handle_fork()
        }
        SYS_EXEC => {
            handle_exec(frame)
        }
        SYS_WAITPID => {
            handle_waitpid(frame)
        }
        SYS_SPAWN => {
            // arg0 = path_ptr, arg1 = path_len
            let path_ptr = frame.rdi;
            let path_len = frame.rsi as usize;
            if path_ptr >= 0x0000_8000_0000_0000 || path_len > 256 {
                return u64::MAX;
            }
            let path_bytes = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, path_len) };
            if let Ok(path) = core::str::from_utf8(path_bytes) {
                let name = path.rsplit('/').next().unwrap_or(path);
                match crate::process::scheduler::spawn_user_process(name, path) {
                    Ok(child_pid) => {
                        // Set parent_pid on child
                        let parent_pid = crate::process::scheduler::current_pid().unwrap_or(0);
                        let mut table = crate::process::process::PROCESS_TABLE.lock();
                        if let Some(child) = table.get_mut(&child_pid) {
                            child.parent_pid = parent_pid;
                        }
                        if let Some(parent) = table.get_mut(&parent_pid) {
                            parent.children.push(child_pid);
                        }
                        child_pid
                    }
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_WRITE => {
            let fd = frame.rdi as usize;
            let buf_ptr = frame.rsi;
            let len = frame.rdx as usize;
            if buf_ptr >= 0x0000_8000_0000_0000 || len > 0x10000 {
                return u64::MAX;
            }
            let buf = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, len) };
            if fd == 1 || fd == 2 {
                if let Ok(s) = core::str::from_utf8(buf) {
                    crate::serial_print!("{}", s);
                }
                len as u64
            } else {
                match crate::syscall::handlers::sys_write(fd, buf) {
                    Ok(n) => n as u64,
                    Err(_) => u64::MAX,
                }
            }
        }
        SYS_READ => {
            let fd = frame.rdi as usize;
            let buf_ptr = frame.rsi;
            let len = frame.rdx as usize;
            if buf_ptr >= 0x0000_8000_0000_0000 || len > 0x10000 {
                return u64::MAX;
            }
            let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, len) };
            match crate::syscall::handlers::sys_read(fd, buf) {
                Ok(n) => n as u64,
                Err(_) => u64::MAX,
            }
        }
        SYS_OPEN => {
            let path_ptr = frame.rdi;
            let path_len = frame.rsi as usize;
            if path_ptr >= 0x0000_8000_0000_0000 || path_len > 256 {
                return u64::MAX;
            }
            let path_bytes = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, path_len) };
            if let Ok(path) = core::str::from_utf8(path_bytes) {
                match crate::vfs::open(path) {
                    Ok(fd) => fd as u64,
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_CLOSE => {
            let fd = frame.rdi as usize;
            match crate::vfs::close(fd) {
                Ok(()) => 0,
                Err(_) => u64::MAX,
            }
        }
        SYS_STAT => {
            // arg0 = path_ptr, arg1 = path_len, arg2 = out_buf_ptr
            let path_ptr = frame.rdi;
            let path_len = frame.rsi as usize;
            let out_ptr = frame.rdx;
            if path_ptr >= 0x0000_8000_0000_0000 || path_len > 256 {
                return u64::MAX;
            }
            let path_bytes = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, path_len) };
            if let Ok(path) = core::str::from_utf8(path_bytes) {
                match crate::vfs::stat(path) {
                    Ok(val) => {
                        // Write file size to output buffer as u64
                        let size = val.as_u64().unwrap_or(0);
                        if out_ptr != 0 && out_ptr < 0x0000_8000_0000_0000 {
                            unsafe { *(out_ptr as *mut u64) = size; }
                        }
                        size
                    }
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_READDIR => {
            // arg0 = path_ptr, arg1 = path_len, arg2 = out_buf_ptr, arg3 = out_buf_len
            let path_ptr = frame.rdi;
            let path_len = frame.rsi as usize;
            let out_ptr = frame.rdx;
            let out_len = frame.r10 as usize;
            if path_ptr >= 0x0000_8000_0000_0000 || path_len > 256 {
                return u64::MAX;
            }
            if out_ptr >= 0x0000_8000_0000_0000 || out_len > 0x10000 {
                return u64::MAX;
            }
            let path_bytes = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, path_len) };
            if let Ok(path) = core::str::from_utf8(path_bytes) {
                match crate::vfs::readdir(path) {
                    Ok(entries) => {
                        // Write null-separated names to output buffer
                        let out_buf = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len) };
                        let mut pos = 0;
                        for name in &entries {
                            let bytes = name.as_bytes();
                            if pos + bytes.len() + 1 > out_len { break; }
                            out_buf[pos..pos + bytes.len()].copy_from_slice(bytes);
                            pos += bytes.len();
                            out_buf[pos] = 0; // null separator
                            pos += 1;
                        }
                        pos as u64
                    }
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_MKDIR => {
            let path_ptr = frame.rdi;
            let path_len = frame.rsi as usize;
            if path_ptr >= 0x0000_8000_0000_0000 || path_len > 256 {
                return u64::MAX;
            }
            let path_bytes = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, path_len) };
            if let Ok(path) = core::str::from_utf8(path_bytes) {
                match crate::vfs::mkdir(path) {
                    Ok(()) => 0,
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_TCP_CONNECT => {
            let ip_packed = (frame.rdi as u32).to_be_bytes();
            let port = frame.rsi as u16;
            match crate::syscall::handlers::sys_tcp_connect(ip_packed, port) {
                Ok(fd) => fd as u64,
                Err(_) => u64::MAX,
            }
        }
        SYS_TCP_SEND => {
            let fd = frame.rdi as usize;
            let buf_ptr = frame.rsi;
            let len = frame.rdx as usize;
            if buf_ptr >= 0x0000_8000_0000_0000 || len > 0x10000 {
                return u64::MAX;
            }
            let buf = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, len) };
            match crate::syscall::handlers::sys_tcp_send(fd, buf) {
                Ok(n) => n as u64,
                Err(_) => u64::MAX,
            }
        }
        SYS_TCP_RECV => {
            let fd = frame.rdi as usize;
            let buf_ptr = frame.rsi;
            let len = frame.rdx as usize;
            if buf_ptr >= 0x0000_8000_0000_0000 || len > 0x10000 {
                return u64::MAX;
            }
            let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, len) };
            match crate::syscall::handlers::sys_tcp_recv(fd, buf) {
                Ok(n) => n as u64,
                Err(_) => u64::MAX,
            }
        }
        SYS_TCP_CLOSE => {
            let fd = frame.rdi as usize;
            match crate::vfs::close(fd) {
                Ok(()) => 0,
                Err(_) => u64::MAX,
            }
        }
        SYS_GETHOSTBYNAME => {
            let name_ptr = frame.rdi;
            let name_len = frame.rsi as usize;
            if name_ptr >= 0x0000_8000_0000_0000 || name_len > 256 {
                return u64::MAX;
            }
            let name_bytes = unsafe { core::slice::from_raw_parts(name_ptr as *const u8, name_len) };
            if let Ok(name) = core::str::from_utf8(name_bytes) {
                match crate::syscall::handlers::sys_gethostbyname(name) {
                    Ok(ip) => u32::from_be_bytes(ip) as u64,
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_DUP2 => {
            // arg0 = old_fd, arg1 = new_fd
            let old_fd = frame.rdi as usize;
            let new_fd = frame.rsi as usize;
            let pid = crate::process::scheduler::current_pid().unwrap_or(0);
            match crate::process::fd::with_fd_table(pid, |t| t.dup2(old_fd, new_fd)) {
                Ok(Ok(())) => 0,
                _ => u64::MAX,
            }
        }
        SYS_MMAP => {
            let hint = frame.rdi;
            let length = frame.rsi as usize;
            let num_pages = (length + 4095) / 4096;
            if num_pages == 0 || num_pages > 256 {
                return u64::MAX;
            }
            let pid = crate::process::scheduler::current_pid().unwrap_or(0);
            let table = crate::process::process::PROCESS_TABLE.lock();
            if let Some(proc) = table.get(&pid) {
                if let Some(pml4) = proc.page_table {
                    drop(table);
                    match crate::memory::paging::mmap_anonymous(pml4, hint, num_pages) {
                        Ok(addr) => addr,
                        Err(_) => u64::MAX,
                    }
                } else {
                    u64::MAX
                }
            } else {
                u64::MAX
            }
        }
        SYS_PIPE => {
            let pipe_id = crate::process::pipe::create_pipe();
            pipe_id
        }
        SYS_NET_BIND => {
            let port = frame.rdi as u16;
            match crate::net::udp::bind(port) {
                Ok(()) => 0,
                Err(_) => u64::MAX,
            }
        }
        SYS_NET_SEND => {
            let ip_packed = (frame.rdi as u32).to_be_bytes();
            let dst_port = frame.rsi as u16;
            let buf_ptr = frame.rdx;
            let len = frame.r10 as usize;
            if buf_ptr >= 0x0000_8000_0000_0000 || len > 0x10000 {
                return u64::MAX;
            }
            let buf = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, len) };
            match crate::net::udp::send(ip_packed, dst_port, 0, buf) {
                Ok(()) => len as u64,
                Err(_) => u64::MAX,
            }
        }
        SYS_NET_RECV => {
            let port = frame.rdi as u16;
            let buf_ptr = frame.rsi;
            let max_len = frame.rdx as usize;
            if buf_ptr >= 0x0000_8000_0000_0000 || max_len > 0x10000 {
                return u64::MAX;
            }
            match crate::net::udp::recv(port) {
                Some((_, _, data)) => {
                    let copy_len = data.len().min(max_len);
                    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, copy_len) };
                    buf.copy_from_slice(&data[..copy_len]);
                    copy_len as u64
                }
                None => 0,
            }
        }
        SYS_KG_INSERT => {
            let type_ptr = frame.rdi;
            let type_len = frame.rsi as usize;
            if type_ptr >= 0x0000_8000_0000_0000 || type_len > 256 {
                return u64::MAX;
            }
            let type_slice = unsafe { core::slice::from_raw_parts(type_ptr as *const u8, type_len) };
            let type_str = core::str::from_utf8(type_slice).unwrap_or("unknown");
            match crate::knowledge::query::kg_insert(type_str, smartpack::Value::Null) {
                Ok(id) => id,
                Err(_) => u64::MAX,
            }
        }
        SYS_KG_QUERY => {
            let type_ptr = frame.rdi;
            let type_len = frame.rsi as usize;
            if type_ptr >= 0x0000_8000_0000_0000 || type_len > 256 {
                return u64::MAX;
            }
            let type_slice = unsafe { core::slice::from_raw_parts(type_ptr as *const u8, type_len) };
            let type_str = core::str::from_utf8(type_slice).unwrap_or("unknown");
            match crate::knowledge::query::kg_query(type_str) {
                Ok(smartpack::Value::Array(arr)) => arr.len() as u64,
                _ => 0,
            }
        }
        SYS_KG_LINK => {
            let from = frame.rdi;
            let to = frame.rsi;
            match crate::knowledge::query::kg_link(from, to, "related", smartpack::Value::Null) {
                Ok(eid) => eid,
                Err(_) => u64::MAX,
            }
        }
        SYS_KG_DELETE => {
            let node_id = frame.rdi;
            match crate::knowledge::query::kg_delete(node_id) {
                Ok(()) => 0,
                Err(_) => u64::MAX,
            }
        }
        _ => {
            serial_println!("[syscall] Unknown syscall: {}", frame.nr);
            u64::MAX // -ENOSYS
        }
    }
}

/// Handle SYS_FORK: create a child process using CoW.
fn handle_fork() -> u64 {
    let pid = crate::process::scheduler::current_pid().unwrap_or(0);
    if pid == 0 {
        return u64::MAX; // Can't fork the kernel
    }

    // Get parent's PML4
    let pml4 = {
        let table = crate::process::process::PROCESS_TABLE.lock();
        match table.get(&pid) {
            Some(proc) => match proc.page_table {
                Some(p) => p,
                None => return u64::MAX,
            },
            None => return u64::MAX,
        }
    };

    // CoW fork: create child page table sharing parent's pages
    let child_pml4 = match crate::memory::cow::cow_fork(pml4) {
        Some(p) => p,
        None => return u64::MAX,
    };

    // Create child process
    let mut child_proc = crate::process::process::Process::new_user("forked", child_pml4);
    let child_pid = child_proc.pid;
    child_proc.parent_pid = pid;

    // Clone FD table, CWD, and signal handlers
    crate::process::fd::clone_fd_table(pid, child_pid);
    crate::process::sigdeliver::clone_handlers(pid, child_pid);

    // Create child thread (copy of parent's user-mode context with RAX=0)
    let child_cr3 = child_pml4.start_address().as_u64();
    let child_tid = crate::process::scheduler::fork_current_thread(child_pid, child_cr3);
    child_proc.threads.push(child_tid);

    // Register child in process table
    let mut table = crate::process::process::PROCESS_TABLE.lock();
    table.insert(child_pid, child_proc);

    // Add child to parent's children list
    if let Some(parent) = table.get_mut(&pid) {
        parent.children.push(child_pid);
    }

    serial_println!("[fork] pid={} forked child pid={}", pid, child_pid);
    child_pid // Parent gets child PID
}

/// Handle SYS_EXEC: replace current process image with new ELF.
fn handle_exec(frame: &SyscallFrame) -> u64 {
    use x86_64::structures::paging::PageTableFlags;

    // arg0 = path_ptr, arg1 = path_len
    let path_ptr = frame.rdi;
    let path_len = frame.rsi as usize;
    if path_ptr >= 0x0000_8000_0000_0000 || path_len > 256 {
        return u64::MAX;
    }

    let path_bytes = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, path_len) };
    let path = match core::str::from_utf8(path_bytes) {
        Ok(s) => s,
        Err(_) => return u64::MAX,
    };

    // Read ELF from VFS (we must read it BEFORE clearing the page table!)
    let fd = match crate::vfs::open(path) {
        Ok(f) => f,
        Err(_) => return u64::MAX,
    };
    let mut elf_data = alloc::vec![0u8; 64 * 1024];
    let n = match crate::vfs::read(fd, &mut elf_data) {
        Ok(n) => n,
        Err(_) => return u64::MAX,
    };
    crate::vfs::close(fd).ok();
    elf_data.truncate(n);

    let pid = crate::process::scheduler::current_pid().unwrap_or(0);

    // Get PML4
    let pml4 = {
        let table = crate::process::process::PROCESS_TABLE.lock();
        match table.get(&pid) {
            Some(proc) => match proc.page_table {
                Some(p) => p,
                None => return u64::MAX,
            },
            None => return u64::MAX,
        }
    };

    // Clear user-space mappings (keep PML4 frame + kernel half)
    crate::memory::paging::free_user_pages_only(pml4);

    // Load new ELF segments
    let loaded = match crate::process::elf::load_elf(&elf_data, pml4) {
        Ok(l) => l,
        Err(_) => {
            // exec failed after clearing pages — kill the process
            crate::process::process::exit_process_full(pid, -1);
            crate::process::scheduler::exit_current_thread();
            return u64::MAX;
        }
    };

    // Map new user stack: 16 pages at 0x7FFF_FFFF_0000
    let user_stack_top = 0x7FFF_FFFF_0000u64;
    let user_stack_pages = 16usize;
    let user_stack_bottom = user_stack_top - (user_stack_pages as u64) * 4096;
    let stack_flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;
    if crate::memory::paging::map_range(pml4, user_stack_bottom, user_stack_pages, stack_flags).is_err() {
        crate::process::process::exit_process_full(pid, -1);
        crate::process::scheduler::exit_current_thread();
        return u64::MAX;
    }

    // Update process name
    {
        let name = path.rsplit('/').next().unwrap_or(path);
        let mut table = crate::process::process::PROCESS_TABLE.lock();
        if let Some(proc) = table.get_mut(&pid) {
            proc.name = alloc::string::String::from(name);
        }
    }

    let cr3 = pml4.start_address().as_u64();
    serial_println!("[exec] pid={} exec'd '{}' (entry={:#X})", pid, path, loaded.entry_point);

    // Replace execution context — never returns
    crate::process::scheduler::exec_replace_context(loaded.entry_point, user_stack_top, cr3);
    // unreachable
}

/// Handle SYS_WAITPID: wait for a child process to exit.
fn handle_waitpid(frame: &SyscallFrame) -> u64 {
    // arg0 = target child pid (0 = any child)
    let target = frame.rdi;
    let pid = crate::process::scheduler::current_pid().unwrap_or(0);

    // Check if target is already a zombie
    {
        let table = crate::process::process::PROCESS_TABLE.lock();
        if target != 0 {
            // Wait for specific child
            if let Some(child) = table.get(&target) {
                if child.state == crate::process::process::ProcessState::Zombie {
                    let code = child.exit_code.unwrap_or(0);
                    drop(table);
                    crate::process::process::reap_zombie(target);
                    return (target << 32) | (code as u32 as u64);
                }
            }
        } else {
            // Wait for any child
            if let Some(parent) = table.get(&pid) {
                for &child_pid in &parent.children {
                    if let Some(child) = table.get(&child_pid) {
                        if child.state == crate::process::process::ProcessState::Zombie {
                            let code = child.exit_code.unwrap_or(0);
                            drop(table);
                            crate::process::process::reap_zombie(child_pid);
                            return (child_pid << 32) | (code as u32 as u64);
                        }
                    }
                }
            }
        }
    }

    // Not zombie yet — block and wait
    crate::process::scheduler::block_current_thread(
        crate::process::wait::WaitReason::WaitPid { target_pid: target }
    );

    // When we're woken up, check again for zombie
    {
        let table = crate::process::process::PROCESS_TABLE.lock();
        if target != 0 {
            if let Some(child) = table.get(&target) {
                if child.state == crate::process::process::ProcessState::Zombie {
                    let code = child.exit_code.unwrap_or(0);
                    drop(table);
                    crate::process::process::reap_zombie(target);
                    return (target << 32) | (code as u32 as u64);
                }
            }
        } else {
            if let Some(parent) = table.get(&pid) {
                for &child_pid in &parent.children {
                    if let Some(child) = table.get(&child_pid) {
                        if child.state == crate::process::process::ProcessState::Zombie {
                            let code = child.exit_code.unwrap_or(0);
                            drop(table);
                            crate::process::process::reap_zombie(child_pid);
                            return (child_pid << 32) | (code as u32 as u64);
                        }
                    }
                }
            }
        }
    }

    0 // No child exited (spurious wakeup)
}

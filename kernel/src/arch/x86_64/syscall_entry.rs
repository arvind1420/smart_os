/// SYSCALL/SYSRET instruction support for Smart OS.
///
/// Each core has its own PerCpu structure accessed via GS.
/// We use SWAPGS on entry to access kernel-side per-CPU data.

use crate::serial_println;

// MSR addresses for SYSCALL/SYSRET
const MSR_EFER: u32 = 0xC000_0080;   
const MSR_STAR: u32 = 0xC000_0081;   
const MSR_LSTAR: u32 = 0xC000_0082;  
const MSR_SFMASK: u32 = 0xC000_0084; 

// GDT selectors
const KERNEL_CS: u64 = 0x08;
const SYSRET_BASE: u64 = 0x10;

/// Initialize SYSCALL/SYSRET support for the current CPU.
pub fn init() {
    unsafe {
        // 1. Enable SCE (System Call Enable) bit in EFER MSR
        let efer = rdmsr(MSR_EFER);
        wrmsr(MSR_EFER, efer | 1);

        // 2. Set STAR MSR
        let star = (SYSRET_BASE << 48) | (KERNEL_CS << 32);
        wrmsr(MSR_STAR, star);

        // 3. Set LSTAR to our syscall entry point
        let entry_addr = syscall_entry_stub as *const () as u64;
        wrmsr(MSR_LSTAR, entry_addr);

        // 4. Set SFMASK: clear IF (bit 9) and TF (bit 8)
        wrmsr(MSR_SFMASK, 0x300);

        serial_println!("[syscall] SYSCALL/SYSRET configured.");
    }
}

/// Update the kernel RSP for the current CPU.
pub fn set_kernel_rsp(rsp: u64) {
    unsafe {
        let pcpu: *mut super::percpu::PerCpu;
        core::arch::asm!(
            "mov {}, gs:[0]", 
            out(reg) pcpu,
        );
        (*pcpu).kernel_rsp = rsp;
    }
}

/// Read a Model-Specific Register.
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
    );
    ((high as u64) << 32) | (low as u64)
}

/// Write a Model-Specific Register.
#[inline]
unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
    );
}

/// The SYSCALL entry point.
#[unsafe(naked)]
unsafe extern "C" fn syscall_entry_stub() {
    core::arch::naked_asm!(
        // Switch to kernel GS
        "swapgs",

        // Save user RSP to PerCpu (GS:[16] is user_rsp, GS:[24] is kernel_rsp)
        "mov gs:[16], rsp",
        "mov rsp, gs:[24]",

        // Push user return context
        "push rcx",         // user RIP
        "push r11",         // user RFLAGS

        // Push callee-saved registers
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // Push syscall args as SyscallFrame
        "push r9", "push r8", "push r10", "push rdx", "push rsi", "push rdi", "push rax",

        // Call Rust dispatcher
        "mov rdi, rsp",
        "call {dispatcher}",

        // Pop SyscallFrame
        "add rsp, 7 * 8",

        // Restore callee-saved registers
        "pop r15", "pop r14", "pop r13", "pop r12", "pop rbp", "pop rbx",

        // Restore user RFLAGS and RIP
        "pop r11", "pop rcx",

        // Restore user RSP
        "mov rsp, gs:[16]",

        // Switch back to user GS
        "swapgs",

        // Return
        "sysretq",

        dispatcher = sym syscall_dispatcher,
    );
}

/// Syscall frame passed to the Rust dispatcher.
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
extern "C" fn syscall_dispatcher(frame: *const SyscallFrame) -> u64 {
    let frame = unsafe { &*frame };

    let pid = crate::process::scheduler::current_pid().unwrap_or(0);
    let is_linux = if pid != 0 {
        crate::process::process::PROCESS_TABLE.lock().get(&pid).map(|p| p.is_linux).unwrap_or(false)
    } else {
        false
    };

    // Security anomaly detection: monitor every syscall from user processes
    {
        if pid != 0 && crate::security::monitor::on_syscall(pid, frame.nr as u8) {
            crate::security::monitor::freeze_process(pid);
            return u64::MAX; // Process frozen
        }
    }

    if is_linux {
        crate::ai::optimizer::OPTIMIZER.lock().record_syscall(pid, frame.nr);
        return crate::syscall::linux::linux_syscall_handler(
            frame.nr, frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9
        );
    }

    // Sandbox capability check for native Smart OS processes
    if pid != 0 && !crate::security::sandbox::check_syscall(pid, frame.nr as usize) {
        crate::security::audit::log_denied_syscall(pid, frame.nr, "sandbox");
        return u64::MAX; // EPERM
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
            let ms = frame.rdi;
            if ms == 0 {
                crate::process::scheduler::yield_now();
                return 0;
            }
            let current_tick = crate::drivers::timer::ticks();
            let wake_at = current_tick + (ms * 100) / 1000;
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
            if len > 0x10000 {
                return u64::MAX;
            }
            
            let mut safe_buf = alloc::vec![0u8; len];
            if crate::memory::paging::copy_from_user(&mut safe_buf, buf_ptr).is_err() {
                return u64::MAX;
            }

            if fd == 1 || fd == 2 {
                if let Ok(s) = core::str::from_utf8(&safe_buf) {
                    crate::serial_print!("{}", s);
                }
                len as u64
            } else {
                match crate::syscall::handlers::sys_write(fd, &safe_buf) {
                    Ok(n) => n as u64,
                    Err(_) => u64::MAX,
                }
            }
        }
        SYS_READ => {
            let fd = frame.rdi as usize;
            let buf_ptr = frame.rsi;
            let len = frame.rdx as usize;
            if len > 0x10000 {
                return u64::MAX;
            }

            let mut safe_buf = alloc::vec![0u8; len];
            match crate::syscall::handlers::sys_read(fd, &mut safe_buf) {
                Ok(n) => {
                    if crate::memory::paging::copy_to_user(buf_ptr, &safe_buf[..n]).is_ok() {
                        n as u64
                    } else {
                        u64::MAX
                    }
                }
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
                        let out_buf = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len) };
                        let mut pos = 0;
                        for name in &entries {
                            let bytes = name.as_bytes();
                            if pos + bytes.len() + 1 > out_len { break; }
                            out_buf[pos..pos + bytes.len()].copy_from_slice(bytes);
                            pos += bytes.len();
                            out_buf[pos] = 0;
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
        SYS_TLS_CONNECT => {
            let host_ptr = frame.rdi;
            let host_len = frame.rsi as usize;
            let port = frame.rdx as u16;
            if host_ptr >= 0x0000_8000_0000_0000 || host_len > 256 {
                return u64::MAX;
            }
            let host_bytes = unsafe { core::slice::from_raw_parts(host_ptr as *const u8, host_len) };
            if let Ok(host) = core::str::from_utf8(host_bytes) {
                match crate::syscall::handlers::sys_tls_connect(host, port) {
                    Ok(id) => id as u64,
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_TLS_SEND => {
            let id = frame.rdi as u32;
            let buf_ptr = frame.rsi;
            let len = frame.rdx as usize;
            if buf_ptr >= 0x0000_8000_0000_0000 || len > 0x10000 {
                return u64::MAX;
            }
            let mut safe_buf = alloc::vec![0u8; len];
            if crate::memory::paging::copy_from_user(&mut safe_buf, buf_ptr).is_err() {
                return u64::MAX;
            }
            match crate::syscall::handlers::sys_tls_send(id, &safe_buf) {
                Ok(()) => 0,
                Err(_) => u64::MAX,
            }
        }
        SYS_TLS_RECV => {
            let id = frame.rdi as u32;
            let buf_ptr = frame.rsi;
            let len = frame.rdx as usize;
            if buf_ptr >= 0x0000_8000_0000_0000 || len > 0x10000 {
                return u64::MAX;
            }
            let mut safe_buf = alloc::vec![0u8; len];
            match crate::syscall::handlers::sys_tls_recv(id, &mut safe_buf) {
                Ok(n) => {
                    if crate::memory::paging::copy_to_user(buf_ptr, &safe_buf[..n]).is_ok() {
                        n as u64
                    } else {
                        u64::MAX
                    }
                }
                Err(_) => u64::MAX,
            }
        }
        SYS_TLS_CLOSE => {
            let id = frame.rdi as u32;
            match crate::syscall::handlers::sys_tls_close(id) {
                Ok(()) => 0,
                Err(_) => u64::MAX,
            }
        }
        SYS_DISPLAY_CMD => {
            let pid = crate::process::scheduler::current_pid().unwrap_or(0);
            crate::gui::display_server::handle_cmd(pid, frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8)
        }
        SYS_SYSINFO => {
            let buf_ptr = frame.rdi;
            let buf_len = frame.rsi as usize;
            if buf_ptr >= 0x0000_8000_0000_0000 || buf_len > 1024 {
                return u64::MAX;
            }
            let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len) };
            match crate::syscall::handlers::sys_sysinfo(buf) {
                Ok(n) => n as u64,
                Err(_) => u64::MAX,
            }
        }
        SYS_DUP2 => {
            let old_fd = frame.rdi as usize;
            let new_fd = frame.rsi as usize;
            let pid = crate::process::scheduler::current_pid().unwrap_or(0);
            match crate::process::fd::with_fd_table(pid, |t| t.dup2(old_fd, new_fd)) {
                Ok(Ok(())) => 0,
                _ => u64::MAX,
            }
        }
        SYS_KMOD_LOAD => {
            let path_ptr = frame.rdi;
            let path_len = frame.rsi as usize;
            if path_ptr >= 0x0000_8000_0000_0000 || path_len > 256 { return u64::MAX; }
            let path_bytes = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, path_len) };
            if let Ok(path) = core::str::from_utf8(path_bytes) {
                match crate::syscall::handlers::sys_kmod_load(path) {
                    Ok(()) => 0,
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_AI_INFER => {
            let in_ptr = frame.rdi;
            let in_len = frame.rsi as usize;
            let out_ptr = frame.rdx;
            let out_len = frame.r10 as usize;
            if in_ptr >= 0x0000_8000_0000_0000 || in_len > 0x10000 { return u64::MAX; }
            let in_bytes = unsafe { core::slice::from_raw_parts(in_ptr as *const u8, in_len) };
            if let Ok(input) = core::str::from_utf8(in_bytes) {
                let out_buf = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len) };
                match crate::syscall::handlers::sys_ai_infer(input, out_buf) {
                    Ok(n) => n as u64,
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_AI_SEARCH => {
            let query_ptr = frame.rdi;
            let query_len = frame.rsi as usize;
            let out_ptr = frame.rdx;
            let out_len = frame.r10 as usize;
            if query_ptr >= 0x0000_8000_0000_0000 || query_len > 1024 { return u64::MAX; }
            let query_bytes = unsafe { core::slice::from_raw_parts(query_ptr as *const u8, query_len) };
            if let Ok(query) = core::str::from_utf8(query_bytes) {
                let out_buf = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len) };
                match crate::syscall::handlers::sys_ai_search(query, out_buf) {
                    Ok(n) => n as u64,
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_AUDIT_READ => {
            let out_ptr = frame.rdi;
            let out_len = frame.rsi as usize;
            if out_ptr >= 0x0000_8000_0000_0000 || out_len > 4096 { return u64::MAX; }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len) };
            match crate::syscall::handlers::sys_audit_read(out_buf) {
                Ok(n) => n as u64,
                Err(_) => u64::MAX,
            }
        }
        SYS_LICENSE_CHECK => {
            crate::syscall::handlers::sys_license_check()
        }
        SYS_TTS_SAY => {
            let text_ptr = frame.rdi;
            let text_len = frame.rsi as usize;
            if text_ptr >= 0x0000_8000_0000_0000 || text_len > 1024 { return u64::MAX; }
            let text_bytes = unsafe { core::slice::from_raw_parts(text_ptr as *const u8, text_len) };
            if let Ok(text) = core::str::from_utf8(text_bytes) {
                match crate::syscall::handlers::sys_tts_say(text) {
                    Ok(()) => 0,
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_UI_TRAVERSE => {
            let out_ptr = frame.rdi;
            let out_len = frame.rsi as usize;
            if out_ptr >= 0x0000_8000_0000_0000 || out_len > 4096 { return u64::MAX; }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len) };
            match crate::syscall::handlers::sys_ui_traverse(out_buf) {
                Ok(n) => n as u64,
                Err(_) => u64::MAX,
            }
        }
        SYS_GAMEPAD_STATE => {
            let out_ptr = frame.rdi;
            let out_len = frame.rsi as usize;
            if out_ptr >= 0x0000_8000_0000_0000 || out_len > 1024 { return u64::MAX; }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len) };
            match crate::syscall::handlers::sys_gamepad_state(out_buf) {
                Ok(n) => n as u64,
                Err(_) => u64::MAX,
            }
        }
        SYS_SET_GAME_MODE => {
            let active = frame.rdi != 0;
            match crate::syscall::handlers::sys_set_game_mode(active) {
                Ok(()) => 0,
                Err(_) => u64::MAX,
            }
        }
        SYS_FLEET_COMMAND => {
            let cmd_ptr = frame.rdi;
            let cmd_len = frame.rsi as usize;
            let par_ptr = frame.rdx;
            let par_len = frame.r10 as usize;
            if cmd_ptr >= 0x0000_8000_0000_0000 || par_ptr >= 0x0000_8000_0000_0000 { return u64::MAX; }
            let cmd_bytes = unsafe { core::slice::from_raw_parts(cmd_ptr as *const u8, cmd_len) };
            let par_bytes = unsafe { core::slice::from_raw_parts(par_ptr as *const u8, par_len) };
            if let (Ok(cmd), Ok(par)) = (core::str::from_utf8(cmd_bytes), core::str::from_utf8(par_bytes)) {
                match crate::syscall::handlers::sys_fleet_command(cmd, par) {
                    Ok(()) => 0,
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_TENSOR_CREATE => {
            // MVP mockup
            crate::serial_println!("[syscall] TENSOR_CREATE shape={:#X}", frame.rdi);
            42 // Mock handle
        }
        SYS_TENSOR_OP => {
            // MVP mockup
            crate::serial_println!("[syscall] TENSOR_OP handle={} op={}", frame.rdi, frame.rdx);
            if frame.rdx == 0 { // read
                let len = 4 * 4; // Mock size
                let buf_ptr = frame.r8;
                let mock_data = alloc::vec![1.0f32, 2.0, 3.0, 4.0];
                let mock_bytes = unsafe { core::slice::from_raw_parts(mock_data.as_ptr() as *const u8, len) };
                if crate::memory::paging::copy_to_user(buf_ptr, mock_bytes).is_ok() {
                    0
                } else {
                    u64::MAX
                }
            } else {
                43 // Mock new handle
            }
        }
        SYS_TENSOR_DESTROY => {
            crate::serial_println!("[syscall] TENSOR_DESTROY handle={}", frame.rdi);
            0
        }
        _ => {
            serial_println!("[syscall] Unknown syscall: {}", frame.nr);
            u64::MAX
        }
    }
}

/// Handle SYS_FORK: create a child process using CoW.
fn handle_fork() -> u64 {
    let pid = crate::process::scheduler::current_pid().unwrap_or(0);
    if pid == 0 { return u64::MAX; }
    let (pml4, is_linux) = {
        let table = crate::process::process::PROCESS_TABLE.lock();
        match table.get(&pid) {
            Some(proc) => (proc.page_table.expect("no page table"), proc.is_linux),
            None => return u64::MAX,
        }
    };
    let child_pml4 = crate::memory::cow::cow_fork(pml4).expect("cow_fork failed");
    let mut child_proc = crate::process::process::Process::new_user("forked", child_pml4, is_linux);
    let child_pid = child_proc.pid;
    child_proc.parent_pid = pid;
    crate::process::fd::clone_fd_table(pid, child_pid);
    let child_cr3 = child_pml4.start_address().as_u64();
    let child_tid = crate::process::scheduler::fork_current_thread(child_pid, child_cr3);
    child_proc.threads.push(child_tid);
    let mut table = crate::process::process::PROCESS_TABLE.lock();
    table.insert(child_pid, child_proc);
    if let Some(parent) = table.get_mut(&pid) { parent.children.push(child_pid); }
    child_pid
}

/// Handle SYS_EXEC: replace current process image.
fn handle_exec(frame: &SyscallFrame) -> u64 {
    use x86_64::structures::paging::PageTableFlags;
    let path_ptr = frame.rdi;
    let path_len = frame.rsi as usize;
    let path_bytes = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, path_len) };
    let path = core::str::from_utf8(path_bytes).unwrap();
    let fd = crate::vfs::open(path).expect("open failed");
    let mut elf_data = alloc::vec![0u8; 64 * 1024];
    let n = crate::vfs::read(fd, &mut elf_data).unwrap();
    crate::vfs::close(fd).ok();
    elf_data.truncate(n);
    let pid = crate::process::scheduler::current_pid().unwrap();
    let pml4 = {
        let table = crate::process::process::PROCESS_TABLE.lock();
        table.get(&pid).unwrap().page_table.unwrap()
    };
    crate::memory::paging::free_user_pages_only(pml4);
    let loaded = crate::process::elf::load_elf(&elf_data, pml4).unwrap();
    let user_stack_top = 0x7FFF_FFFF_0000u64;
    let stack_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::NO_EXECUTE;
    crate::memory::paging::map_range(pml4, user_stack_top - 64*1024, 16, stack_flags).unwrap();
    let cr3 = pml4.start_address().as_u64();
    crate::process::scheduler::exec_replace_context(loaded.entry_point, user_stack_top, cr3);
}

/// Handle SYS_WAITPID.
fn handle_waitpid(frame: &SyscallFrame) -> u64 {
    let target = frame.rdi;
    let pid = crate::process::scheduler::current_pid().unwrap_or(0);
    crate::process::scheduler::block_current_thread(crate::process::wait::WaitReason::WaitPid { target_pid: target });
    0 // placeholder
}

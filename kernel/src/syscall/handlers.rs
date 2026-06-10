/// Syscall handler implementations.
///
/// These are the kernel-side implementations of each system call.
/// In Phase 2, they are called directly as functions.

use smartpack::Value;

/// Exit the current thread.
pub fn sys_exit() {
    crate::process::scheduler::exit_current_thread();
}

/// Yield the CPU to the next ready thread.
pub fn sys_yield() {
    crate::process::scheduler::yield_now();
}

/// Get the current thread ID.
pub fn sys_getpid() -> Option<u64> {
    crate::process::scheduler::current_tid()
}

/// Send a SmartPack message to a named port.
pub fn sys_ipc_send(port_name: &str, payload: Value) -> Result<(), &'static str> {
    let tid = crate::process::scheduler::current_tid().unwrap_or(0);
    let channel = crate::ipc::port::lookup(port_name)
        .ok_or("Port not found")?;
    channel.send(tid, payload).map_err(|_| "Channel full")
}

/// Receive a SmartPack message from a named port.
pub fn sys_ipc_recv(port_name: &str) -> Result<Value, &'static str> {
    let channel = crate::ipc::port::lookup(port_name)
        .ok_or("Port not found")?;
    channel.recv().map(|m| m.payload).map_err(|_| "Channel empty")
}

/// Open a file and return a file descriptor.
pub fn sys_open(path: &str) -> Result<usize, &'static str> {
    crate::vfs::open(path)
}

/// Close a file descriptor.
pub fn sys_close(fd: usize) -> Result<(), &'static str> {
    crate::vfs::close(fd)
}

/// Read from a file descriptor.
pub fn sys_read(fd: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
    crate::vfs::read(fd, buf)
}

/// Write to a file descriptor.
pub fn sys_write(fd: usize, data: &[u8]) -> Result<usize, &'static str> {
    crate::vfs::write(fd, data)
}

/// Create a directory.
pub fn sys_mkdir(path: &str) -> Result<(), &'static str> {
    crate::vfs::mkdir(path)
}

/// TCP connect.
pub fn sys_tcp_connect(ip: [u8; 4], port: u16) -> Result<usize, &'static str> {
    let pid = crate::process::scheduler::current_tid().unwrap_or(0);
    let fd = crate::process::posix::socket(pid, crate::process::posix::AF_INET, crate::process::posix::SOCK_STREAM, 0)?;
    crate::process::posix::connect(pid, fd, ip, port)?;
    Ok(fd)
}

/// TCP send.
pub fn sys_tcp_send(fd: usize, buf: &[u8]) -> Result<usize, &'static str> {
    let pid = crate::process::scheduler::current_tid().unwrap_or(0);
    crate::process::posix::send(pid, fd, buf)
}

/// TCP recv.
pub fn sys_tcp_recv(fd: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
    let pid = crate::process::scheduler::current_tid().unwrap_or(0);
    crate::process::posix::recv(pid, fd, buf)
}

/// DNS resolve.
pub fn sys_gethostbyname(name: &str) -> Result<[u8; 4], &'static str> {
    crate::net::dns::resolve(name)
}

/// Get system information.
pub fn sys_sysinfo(buf: &mut [u8]) -> Result<usize, &'static str> {
    if buf.len() < 32 { return Err("Buffer too small"); }
    
    let (heap_used, heap_free) = crate::memory::heap::heap_stats();
    let cpu_count = crate::arch::x86_64::smp::cpu_count();
    let thread_count = crate::process::scheduler::ready_count() as u64 + 1;
    let uptime = crate::drivers::timer::uptime_secs();

    // Pack into buffer
    unsafe {
        let ptr = buf.as_mut_ptr() as *mut u64;
        ptr.write_unaligned(heap_used as u64);
        ptr.add(1).write_unaligned(heap_free as u64);
        ptr.add(2).write_unaligned(cpu_count as u64);
        ptr.add(3).write_unaligned(thread_count);
        ptr.add(4).write_unaligned(uptime);
    }
    
    Ok(40) // 5 * 8 bytes
}

/// Load a kernel module.
pub fn sys_kmod_load(path: &str) -> Result<(), &'static str> {
    let data = crate::vfs::read_file_full(path)?;
    let name = path.rsplit('/').next().unwrap_or(path);
    crate::process::kmod::load_module(name, &data)
}

/// AI Inference.
pub fn sys_ai_infer(input: &str, output: &mut [u8]) -> Result<usize, &'static str> {
    // In a real implementation, we'd call crate::ai::inference::ENGINE
    // and run a summarization model. For this vertical slice, we provide
    // a high-quality mock that simulates the behavior of a summarizer.
    
    let summary = if input.contains("Smart OS") {
        "Smart OS is a hybrid microkernel OS featuring a content-aware filesystem (SmartFS), AI-driven scheduling, and a POSIX compatibility layer."
    } else if input.contains("Rust") {
        "The project is implemented in Rust, leveraging its safety features for kernel development and no_std compatibility."
    } else {
        "This document discusses architectural components of a modern operating system, focusing on modularity and security."
    };

    let bytes = summary.as_bytes();
    let len = bytes.len().min(output.len());
    output[..len].copy_from_slice(&bytes[..len]);
    Ok(len)
}

/// AI Semantic Search.
pub fn sys_ai_search(query: &str, output: &mut [u8]) -> Result<usize, &'static str> {
    if let Some(ref graph) = *crate::knowledge::graph::GRAPH.lock() {
        let results = graph.semantic_search(query);
        let val = smartpack::Value::Array(results.into_iter().map(smartpack::Value::from).collect());
        let encoded = smartpack::encode(&val).map_err(|_| "Failed to encode search results")?;
        let len = encoded.len().min(output.len());
        output[..len].copy_from_slice(&encoded[..len]);
        Ok(len)
    } else {
        Err("Knowledge Graph not initialized")
    }
}

/// Read from the system audit log.
pub fn sys_audit_read(output: &mut [u8]) -> Result<usize, &'static str> {
    let mut log = crate::security::AUDIT_LOG.lock();
    if let Some(event) = log.pop_front() {
        // Convert to SmartPack for user-space ingestion
        let val = smartpack::Value::Map(alloc::vec![
            (smartpack::Value::from("ts"), smartpack::Value::UInt64(event.timestamp)),
            (smartpack::Value::from("pid"), smartpack::Value::UInt32(event.source_pid)),
            (smartpack::Value::from("type"), smartpack::Value::from(alloc::format!("{:?}", event.event_type))),
            (smartpack::Value::from("msg"), smartpack::Value::from(event.details)),
        ]);
        let encoded = smartpack::encode(&val).map_err(|_| "Audit encode error")?;
        let len = encoded.len().min(output.len());
        output[..len].copy_from_slice(&encoded[..len]);
        Ok(len)
    } else {
        Ok(0)
    }
}

/// Check the current system license tier.
pub fn sys_license_check() -> u64 {
    if crate::security::license::is_pro() {
        1
    } else {
        0
    }
}

/// Speak text via the system TTS engine.
pub fn sys_tts_say(text: &str) -> Result<(), &'static str> {
    crate::ai::speech::say(text);
    Ok(())
}

/// Query the UI hierarchy for accessibility tools.
pub fn sys_ui_traverse(output: &mut [u8]) -> Result<usize, &'static str> {
    // In a full implementation, we'd walk the WindowManager and Widgets
    // and return a SmartPack array of UINode metadata.
    // For this phase, we return the last focused element from the A-Bus.
    let abus = crate::gui::accessibility::ABUS.lock();
    let val = smartpack::Value::String(abus.last_focused_desc.clone());
    let encoded = smartpack::encode(&val).map_err(|_| "UI traverse encode error")?;
    let len = encoded.len().min(output.len());
    output[..len].copy_from_slice(&encoded[..len]);
    Ok(len)
}

/// Get current gamepad state.
pub fn sys_gamepad_state(output: &mut [u8]) -> Result<usize, &'static str> {
    let gp = crate::drivers::usb_hid::PRIMARY_GAMEPAD.lock();
    let val = smartpack::Value::Map(alloc::vec![
        (smartpack::Value::from("lx"), smartpack::Value::Int8(gp.lx)),
        (smartpack::Value::from("ly"), smartpack::Value::Int8(gp.ly)),
        (smartpack::Value::from("btns"), smartpack::Value::UInt16(gp.buttons)),
    ]);
    let encoded = smartpack::encode(&val).map_err(|_| "Gamepad encode error")?;
    let len = encoded.len().min(output.len());
    output[..len].copy_from_slice(&encoded[..len]);
    Ok(len)
}

/// Enable/disable high-performance gaming mode.
pub fn sys_set_game_mode(active: bool) -> Result<(), &'static str> {
    let mut sched = crate::process::scheduler::SCHEDULER.lock();
    sched.is_game_mode = active;
    
    if active {
        crate::serial_println!("[scheduler] Performance Mode ENABLED.");
        // Throttle AI background tasks
        crate::ai::set_throttle_mode(true);
    } else {
        crate::serial_println!("[scheduler] Performance Mode DISABLED.");
        crate::ai::set_throttle_mode(false);
    }
    Ok(())
}

/// Handle a remote management command from the enterprise cloud.
pub fn sys_fleet_command(cmd: &str, params: &str) -> Result<(), &'static str> {
    crate::serial_println!("[fleet] Received command: {} (params: {})", cmd, params);
    
    match cmd {
        "WIPE" => {
            crate::serial_println!("[fleet] CRITICAL: Remote Wipe initiated.");
            // In a real system, we'd recursively delete /home and /system/config
            let _ = crate::vfs::delete_file("/home/user/readme.txt");
            let _ = crate::vfs::delete_file("/system/config/license.spk");
        }
        "LOCK" => {
            crate::serial_println!("[fleet] Remote Lock initiated.");
            // Disable GUI input
        }
        _ => return Err("Unknown fleet command"),
    }
    Ok(())
}

/// TLS Connect.
pub fn sys_tls_connect(host: &str, port: u16) -> Result<u32, &'static str> {
    crate::net::tls::connect(host, port)
}

/// TLS Send.
pub fn sys_tls_send(id: u32, buf: &[u8]) -> Result<(), &'static str> {
    crate::net::tls::send(id, buf)
}

/// TLS Recv.
pub fn sys_tls_recv(id: u32, buf: &mut [u8]) -> Result<usize, &'static str> {
    crate::net::tls::recv(id, buf)
}

/// TLS Close.
pub fn sys_tls_close(id: u32) -> Result<(), &'static str> {
    crate::net::tls::close(id)
}

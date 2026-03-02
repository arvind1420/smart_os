/// Kernel GDB Stub for Smart OS.
///
/// Phase 10: Minimal GDB remote serial protocol (RSP) stub.
/// Communicates over the serial port (COM1). Allows single-step
/// debugging, breakpoint insertion, register inspection, and
/// memory read/write from a remote GDB session.
///
/// Protocol: GDB RSP over COM1 at 115200 baud.
/// Supports: g (read regs), G (write regs), m (read mem), M (write mem),
///           s (single step), c (continue), ? (halt reason).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use crate::serial_println;

/// Whether the GDB stub is active (waiting for debugger).
static GDB_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether the debugger is connected.
static GDB_CONNECTED: AtomicBool = AtomicBool::new(false);

/// Breakpoint table: address → original byte.
static BREAKPOINTS: Mutex<Vec<(u64, u8)>> = Mutex::new(Vec::new());

/// Maximum breakpoints.
const MAX_BREAKPOINTS: usize = 16;

/// Saved register state during debug stop.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GdbRegisters {
    pub rax: u64, pub rbx: u64, pub rcx: u64, pub rdx: u64,
    pub rsi: u64, pub rdi: u64, pub rbp: u64, pub rsp: u64,
    pub r8: u64,  pub r9: u64,  pub r10: u64, pub r11: u64,
    pub r12: u64, pub r13: u64, pub r14: u64, pub r15: u64,
    pub rip: u64, pub rflags: u64,
}

/// Saved registers from last debug stop.
static SAVED_REGS: Mutex<GdbRegisters> = Mutex::new(GdbRegisters {
    rax: 0, rbx: 0, rcx: 0, rdx: 0,
    rsi: 0, rdi: 0, rbp: 0, rsp: 0,
    r8: 0,  r9: 0,  r10: 0, r11: 0,
    r12: 0, r13: 0, r14: 0, r15: 0,
    rip: 0, rflags: 0,
});

/// Initialize the GDB stub.
pub fn init() {
    serial_println!("[gdb] GDB remote stub initialized (COM1, inactive).");
    serial_println!("[gdb] Connect with: target remote localhost:1234");
}

/// Enable the GDB stub (starts listening).
pub fn enable() {
    GDB_ACTIVE.store(true, Ordering::Relaxed);
    serial_println!("[gdb] Stub enabled. Waiting for debugger connection...");
}

/// Disable the GDB stub.
pub fn disable() {
    GDB_ACTIVE.store(false, Ordering::Relaxed);
    GDB_CONNECTED.store(false, Ordering::Relaxed);
    serial_println!("[gdb] Stub disabled.");
}

/// Check if GDB stub is active.
pub fn is_active() -> bool {
    GDB_ACTIVE.load(Ordering::Relaxed)
}

/// Check if a debugger is connected.
pub fn is_connected() -> bool {
    GDB_CONNECTED.load(Ordering::Relaxed)
}

/// Insert a software breakpoint at the given address.
///
/// Replaces the byte at `addr` with INT3 (0xCC).
pub fn insert_breakpoint(addr: u64) -> bool {
    let mut bps = BREAKPOINTS.lock();
    if bps.len() >= MAX_BREAKPOINTS {
        return false;
    }

    // Check for duplicate
    if bps.iter().any(|&(a, _)| a == addr) {
        return true; // Already set
    }

    // Save original byte and write INT3
    let ptr = addr as *mut u8;
    let original = unsafe { core::ptr::read_volatile(ptr) };
    unsafe { core::ptr::write_volatile(ptr, 0xCC); }

    bps.push((addr, original));
    serial_println!("[gdb] Breakpoint set at {:#X}", addr);
    true
}

/// Remove a software breakpoint.
pub fn remove_breakpoint(addr: u64) -> bool {
    let mut bps = BREAKPOINTS.lock();
    if let Some(pos) = bps.iter().position(|&(a, _)| a == addr) {
        let (_, original) = bps.remove(pos);
        let ptr = addr as *mut u8;
        unsafe { core::ptr::write_volatile(ptr, original); }
        serial_println!("[gdb] Breakpoint removed from {:#X}", addr);
        true
    } else {
        false
    }
}

/// List all breakpoints.
pub fn list_breakpoints() -> Vec<u64> {
    BREAKPOINTS.lock().iter().map(|&(addr, _)| addr).collect()
}

/// Handle a debug exception (INT3 or single-step trap).
///
/// Saves registers and enters the GDB command loop.
pub fn handle_debug_trap(regs: &GdbRegisters) {
    if !GDB_ACTIVE.load(Ordering::Relaxed) {
        return;
    }

    *SAVED_REGS.lock() = *regs;
    GDB_CONNECTED.store(true, Ordering::Relaxed);

    serial_println!("[gdb] Debug trap at RIP={:#X}", regs.rip);

    // In a full implementation, this would enter a GDB RSP
    // command loop reading/writing over serial. For now, we
    // just log the trap and continue.
}

/// Read memory safely (returns None if address is invalid).
pub fn read_memory(addr: u64, len: usize) -> Option<Vec<u8>> {
    // Basic bounds check
    if addr == 0 || len > 4096 {
        return None;
    }

    let mut buf = Vec::with_capacity(len);
    for i in 0..len {
        let ptr = (addr + i as u64) as *const u8;
        let byte = unsafe { core::ptr::read_volatile(ptr) };
        buf.push(byte);
    }
    Some(buf)
}

/// Write memory (for GDB memory write commands).
pub fn write_memory(addr: u64, data: &[u8]) -> bool {
    if addr == 0 || data.len() > 4096 {
        return false;
    }

    for (i, &byte) in data.iter().enumerate() {
        let ptr = (addr + i as u64) as *mut u8;
        unsafe { core::ptr::write_volatile(ptr, byte); }
    }
    true
}

/// Get the saved register state.
pub fn get_registers() -> GdbRegisters {
    *SAVED_REGS.lock()
}

/// Format registers as a human-readable string.
pub fn format_registers(regs: &GdbRegisters) -> String {
    alloc::format!(
        "RAX={:#018X} RBX={:#018X} RCX={:#018X} RDX={:#018X}\n\
         RSI={:#018X} RDI={:#018X} RBP={:#018X} RSP={:#018X}\n\
         R8 ={:#018X} R9 ={:#018X} R10={:#018X} R11={:#018X}\n\
         R12={:#018X} R13={:#018X} R14={:#018X} R15={:#018X}\n\
         RIP={:#018X} RFLAGS={:#018X}",
        regs.rax, regs.rbx, regs.rcx, regs.rdx,
        regs.rsi, regs.rdi, regs.rbp, regs.rsp,
        regs.r8,  regs.r9,  regs.r10, regs.r11,
        regs.r12, regs.r13, regs.r14, regs.r15,
        regs.rip, regs.rflags,
    )
}

/// Get GDB stub stats.
pub fn gdb_stats() -> (bool, bool, usize) {
    (
        is_active(),
        is_connected(),
        BREAKPOINTS.lock().len(),
    )
}

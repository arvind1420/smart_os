/// x86_64 machine code builder for Smart OS — Phase 12.
///
/// Provides functions to emit x86_64 instructions as byte sequences,
/// making it practical to build user-space ELF programs without
/// hand-assembling raw hex bytes.

use alloc::vec::Vec;

// ═══════════════════════════════════════════════════════════════
//  Register encoding
// ═══════════════════════════════════════════════════════════════

/// x86_64 general-purpose registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Reg {
    Rax = 0, Rcx = 1, Rdx = 2, Rbx = 3,
    Rsp = 4, Rbp = 5, Rsi = 6, Rdi = 7,
    R8 = 8, R9 = 9, R10 = 10,
}

impl Reg {
    /// Register number (0-10), bottom 3 bits for ModRM.
    fn num(self) -> u8 {
        self as u8
    }

    /// Whether this is an extended register (R8-R10) needing REX.B or REX.R.
    fn is_ext(self) -> bool {
        self.num() >= 8
    }

    /// Bottom 3 bits of register encoding.
    fn low3(self) -> u8 {
        self.num() & 0x07
    }
}

// ═══════════════════════════════════════════════════════════════
//  Core emit functions
// ═══════════════════════════════════════════════════════════════

/// Emit `mov reg, imm32` (sign-extended to 64-bit).
/// REX.W + C7 /0 + imm32. 7 bytes (or 8 if REX.B needed).
pub fn emit_mov_reg_imm32(code: &mut Vec<u8>, reg: Reg, val: u32) {
    let rex = 0x48 | if reg.is_ext() { 0x01 } else { 0 }; // REX.W + REX.B
    code.push(rex);
    code.push(0xC7);
    code.push(0xC0 | reg.low3()); // ModRM: mod=11, rm=reg
    code.extend_from_slice(&val.to_le_bytes());
}

/// Emit `mov reg, imm64` (full 64-bit immediate).
/// REX.W + B8+rd + imm64. 10 bytes.
#[allow(dead_code)]
pub fn emit_mov_reg_imm64(code: &mut Vec<u8>, reg: Reg, val: u64) {
    let rex = 0x48 | if reg.is_ext() { 0x01 } else { 0 };
    code.push(rex);
    code.push(0xB8 + reg.low3());
    code.extend_from_slice(&val.to_le_bytes());
}

/// Emit `mov dst, src` (64-bit register to register).
/// REX.W + 89 + ModRM.
pub fn emit_mov_reg_reg(code: &mut Vec<u8>, dst: Reg, src: Reg) {
    let mut rex = 0x48u8;
    if src.is_ext() { rex |= 0x04; } // REX.R
    if dst.is_ext() { rex |= 0x01; } // REX.B
    code.push(rex);
    code.push(0x89);
    code.push(0xC0 | (src.low3() << 3) | dst.low3());
}

/// Emit `xor dst, dst` (zero a register). 3 bytes.
pub fn emit_xor_reg_reg(code: &mut Vec<u8>, reg: Reg) {
    let mut rex = 0x48u8;
    if reg.is_ext() { rex |= 0x05; } // REX.R + REX.B
    code.push(rex);
    code.push(0x31);
    code.push(0xC0 | (reg.low3() << 3) | reg.low3());
}

/// Emit `syscall` instruction (0x0F 0x05).
pub fn emit_syscall(code: &mut Vec<u8>) {
    code.push(0x0F);
    code.push(0x05);
}

/// Emit `test rax, rax` (sets ZF if rax==0).
pub fn emit_test_rax_rax(code: &mut Vec<u8>) {
    code.push(0x48);
    code.push(0x85);
    code.push(0xC0);
}

/// Emit `jnz rel8` (jump if not zero, short).
pub fn emit_jnz_short(code: &mut Vec<u8>, offset: i8) {
    code.push(0x75);
    code.push(offset as u8);
}

/// Emit `je rel8` (jump if equal/zero, short).
#[allow(dead_code)]
pub fn emit_je_short(code: &mut Vec<u8>, offset: i8) {
    code.push(0x74);
    code.push(offset as u8);
}

/// Emit `jmp rel8` (unconditional short jump).
pub fn emit_jmp_short(code: &mut Vec<u8>, offset: i8) {
    code.push(0xEB);
    code.push(offset as u8);
}

/// Emit `jmp $` (infinite loop: `jmp` to self). 2 bytes.
pub fn emit_jmp_self(code: &mut Vec<u8>) {
    code.push(0xEB);
    code.push(0xFE); // jmp -2 (back to start of this jmp)
}

/// Emit `lea reg, [rip + offset]` (RIP-relative address load).
/// 7 bytes. Offset is from the END of this instruction.
pub fn emit_lea_rip_rel(code: &mut Vec<u8>, reg: Reg, offset: i32) {
    let mut rex = 0x48u8;
    if reg.is_ext() { rex |= 0x04; } // REX.R
    code.push(rex);
    code.push(0x8D);
    code.push(0x05 | (reg.low3() << 3)); // ModRM: mod=00, reg, rm=101 (RIP-relative)
    code.extend_from_slice(&offset.to_le_bytes());
}

/// Emit `push reg`.
#[allow(dead_code)]
pub fn emit_push(code: &mut Vec<u8>, reg: Reg) {
    if reg.is_ext() {
        code.push(0x41); // REX.B
    }
    code.push(0x50 + reg.low3());
}

/// Emit `pop reg`.
#[allow(dead_code)]
pub fn emit_pop(code: &mut Vec<u8>, reg: Reg) {
    if reg.is_ext() {
        code.push(0x41); // REX.B
    }
    code.push(0x58 + reg.low3());
}

/// Emit `ret`.
#[allow(dead_code)]
pub fn emit_ret(code: &mut Vec<u8>) {
    code.push(0xC3);
}

/// Emit `nop`.
#[allow(dead_code)]
pub fn emit_nop(code: &mut Vec<u8>) {
    code.push(0x90);
}

/// Emit `cmp rax, imm8` (sign-extended). 4 bytes.
#[allow(dead_code)]
pub fn emit_cmp_rax_imm8(code: &mut Vec<u8>, val: u8) {
    code.push(0x48);
    code.push(0x83);
    code.push(0xF8); // ModRM: mod=11, /7, rm=rax
    code.push(val);
}

/// Append raw data bytes.
pub fn emit_data(code: &mut Vec<u8>, data: &[u8]) {
    code.extend_from_slice(data);
}

/// Get current code offset (useful for computing relative jumps).
pub fn current_offset(code: &Vec<u8>) -> usize {
    code.len()
}

// ═══════════════════════════════════════════════════════════════
//  Convenience functions
// ═══════════════════════════════════════════════════════════════

/// Emit: `mov rax, nr; syscall`. Sets RAX to syscall number and calls.
pub fn emit_syscall_nr(code: &mut Vec<u8>, nr: u8) {
    emit_mov_reg_imm32(code, Reg::Rax, nr as u32);
    emit_syscall(code);
}

/// Emit: `mov rax, 0; mov rdi, exit_code; syscall; jmp $`
/// (SYS_EXIT with given code + infinite loop safety net).
pub fn emit_exit(code: &mut Vec<u8>, exit_code: u8) {
    emit_mov_reg_imm32(code, Reg::Rax, 0); // SYS_EXIT = 0
    emit_mov_reg_imm32(code, Reg::Rdi, exit_code as u32);
    emit_syscall(code);
    emit_jmp_self(code);
}

/// Emit: `mov rax, SYS_WRITE(23); mov rdi, 1(stdout); lea rsi, [rip+offset]; mov rdx, len; syscall`
/// Writes a string embedded in the code to stdout.
/// `data_offset` is the RIP-relative offset from the END of the LEA instruction to the data.
pub fn emit_write_stdout(code: &mut Vec<u8>, data_rip_offset: i32, len: u32) {
    emit_mov_reg_imm32(code, Reg::Rax, 23); // SYS_WRITE
    emit_mov_reg_imm32(code, Reg::Rdi, 1);  // stdout
    emit_lea_rip_rel(code, Reg::Rsi, data_rip_offset);
    emit_mov_reg_imm32(code, Reg::Rdx, len);
    emit_syscall(code);
}

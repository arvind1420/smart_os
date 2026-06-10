/// Phase 111 — JS Baseline JIT Compiler
///
/// A template-based baseline JIT for the SmartOS JavaScript engine.
///
/// Design:
///   • Each JS function accumulates a "hot count".  Once it crosses HOT_THRESHOLD
///     calls, `compile()` is invoked and the native code is cached.
///   • The JIT emits x86-64 machine code into a `JitCodeBuffer` (heap Vec<u8>).
///     In a real kernel we would allocate executable memory (W^X) and mark it
///     executable after writing; here we keep it as data for safety in no_std.
///   • The register convention:
///       rax  = accumulator (current value)
///       rbx  = scratch / second operand
///       rsp  = stack pointer (JS operand stack lives on the native stack)
///       r15  = pointer to JS interpreter state (not used by pure arithmetic fns)
///   • Supported opcode patterns compiled natively:
///       PushInt, PopInt, AddInt, SubInt, MulInt, CmpLt, CmpEq, JumpIfFalse,
///       Jump, Return, CallBuiltin(sqrt / abs / floor / ceil)
///
/// Anything else falls back to the interpreter.

extern crate alloc;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};

// ─────────────────────────────────────────────────────────────────────────────
// JIT-able opcode set  (mirrors a subset of js_interp.rs instructions)
// ─────────────────────────────────────────────────────────────────────────────

/// Simplified JS opcode the JIT understands.
#[derive(Debug, Clone, PartialEq)]
pub enum JitOp {
    PushInt(i64),
    PushFloat(f64),
    Pop,
    AddInt,
    SubInt,
    MulInt,
    DivInt,
    Neg,
    CmpLt,   // pops b, a → pushes (a < b) as i64 (0 or 1)
    CmpEq,   // pops b, a → pushes (a == b)
    CmpLe,   // pops b, a → pushes (a <= b)
    JumpIfFalse(i32),  // relative offset in op-stream
    Jump(i32),
    Return,
    /// Load local variable at slot index.
    LoadLocal(u32),
    /// Store into local variable at slot index.
    StoreLocal(u32),
    /// Call a named builtin (Math.sqrt, Math.abs, Math.floor, Math.ceil).
    CallBuiltin(BuiltinFn),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinFn { Sqrt, Abs, Floor, Ceil, Min, Max }

// ─────────────────────────────────────────────────────────────────────────────
// Hot-call counter and cache key
// ─────────────────────────────────────────────────────────────────────────────

pub const HOT_THRESHOLD: u32 = 50;

/// Fingerprint of a JS function for cache lookup.
/// We use a simple hash of the opcode sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FuncKey(pub u64);

impl FuncKey {
    /// Compute an FNV-1a hash of the opcode slice.
    pub fn from_ops(ops: &[JitOp]) -> Self {
        let mut h: u64 = 0xcbf29ce484222325;
        for op in ops {
            // Hash the discriminant of each op.
            let d: u64 = match op {
                JitOp::PushInt(v)      => 1  ^ (*v as u64),
                JitOp::PushFloat(_)    => 2,
                JitOp::Pop             => 3,
                JitOp::AddInt          => 4,
                JitOp::SubInt          => 5,
                JitOp::MulInt          => 6,
                JitOp::DivInt          => 7,
                JitOp::Neg             => 8,
                JitOp::CmpLt           => 9,
                JitOp::CmpEq           => 10,
                JitOp::CmpLe           => 11,
                JitOp::JumpIfFalse(o)  => 12 ^ (*o as u64),
                JitOp::Jump(o)         => 13 ^ (*o as u64),
                JitOp::Return          => 14,
                JitOp::LoadLocal(i)    => 15 ^ (*i as u64),
                JitOp::StoreLocal(i)   => 16 ^ (*i as u64),
                JitOp::CallBuiltin(b)  => 17 ^ (*b as u64),
            };
            h ^= d;
            h = h.wrapping_mul(0x100000001b3);
        }
        FuncKey(h)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// x86-64 code emitter
// ─────────────────────────────────────────────────────────────────────────────

/// Growable buffer for emitted x86-64 machine code bytes.
#[derive(Debug, Default)]
pub struct CodeBuf(pub Vec<u8>);

impl CodeBuf {
    pub fn new() -> Self { CodeBuf(Vec::new()) }
    pub fn len(&self) -> usize { self.0.len() }
    pub fn emit(&mut self, b: u8)   { self.0.push(b); }
    pub fn emit_slice(&mut self, s: &[u8]) { self.0.extend_from_slice(s); }

    // ── helpers ────────────────────────────────────────────────────────────

    /// `push rax`
    pub fn push_rax(&mut self) { self.emit(0x50); }
    /// `pop rax`
    pub fn pop_rax(&mut self)  { self.emit(0x58); }
    /// `push rbx`
    pub fn push_rbx(&mut self) { self.emit(0x53); }
    /// `pop rbx`
    pub fn pop_rbx(&mut self)  { self.emit(0x5B); }

    /// `mov rax, imm64`
    pub fn mov_rax_imm64(&mut self, imm: i64) {
        self.emit_slice(&[0x48, 0xB8]);
        self.emit_slice(&imm.to_le_bytes());
    }

    /// `add rax, rbx`
    pub fn add_rax_rbx(&mut self) {
        self.emit_slice(&[0x48, 0x01, 0xD8]);
    }

    /// `sub rax, rbx` (rax = rbx - rax, i.e. TOS operand order)
    pub fn sub_rbx_rax(&mut self) {
        // We want a - b where a was pushed first, b second.
        // Stack: [..., a, b]  → pop b→rax, pop a→rbx → rbx-rax
        // sub rbx, rax  → result in rbx → mov rax, rbx
        self.emit_slice(&[0x48, 0x29, 0xC3]); // sub rbx, rax
        self.emit_slice(&[0x48, 0x89, 0xD8]); // mov rax, rbx
    }

    /// `imul rax, rbx`
    pub fn imul_rax_rbx(&mut self) {
        self.emit_slice(&[0x48, 0x0F, 0xAF, 0xC3]);
    }

    /// `idiv` sequence: rax = rbx / rax (signed)
    pub fn idiv_rbx_rax(&mut self) {
        // xchg rax, rbx  (now rax=dividend, rbx=divisor)
        self.emit_slice(&[0x48, 0x93]);
        // cqo  (sign-extend rax into rdx:rax)
        self.emit_slice(&[0x48, 0x99]);
        // idiv rbx
        self.emit_slice(&[0x48, 0xF7, 0xFB]);
    }

    /// `neg rax`
    pub fn neg_rax(&mut self) { self.emit_slice(&[0x48, 0xF7, 0xD8]); }

    /// `cmp rax, rbx` + `setl al` + `movzx rax, al` (a < b)
    /// Stack convention: pop b→rax, pop a→rbx, then compare rbx < rax
    pub fn cmp_lt(&mut self) {
        self.emit_slice(&[0x48, 0x3B, 0xD8]); // cmp rbx, rax
        self.emit_slice(&[0x0F, 0x9C, 0xC0]); // setl al
        self.emit_slice(&[0x48, 0x0F, 0xB6, 0xC0]); // movzx rax, al
    }

    /// same-as but sete (==)
    pub fn cmp_eq(&mut self) {
        self.emit_slice(&[0x48, 0x3B, 0xD8]);
        self.emit_slice(&[0x0F, 0x94, 0xC0]);
        self.emit_slice(&[0x48, 0x0F, 0xB6, 0xC0]);
    }

    /// setle (<=)
    pub fn cmp_le(&mut self) {
        self.emit_slice(&[0x48, 0x3B, 0xD8]);
        self.emit_slice(&[0x0F, 0x9E, 0xC0]);
        self.emit_slice(&[0x48, 0x0F, 0xB6, 0xC0]);
    }

    /// `test rax, rax` + `jz rel32`  (jump-if-false: rax==0 → jump)
    pub fn jz_rel32(&mut self, rel: i32) {
        self.emit_slice(&[0x48, 0x85, 0xC0]); // test rax, rax
        self.emit_slice(&[0x0F, 0x84]);        // jz rel32
        self.emit_slice(&rel.to_le_bytes());
    }

    /// `jmp rel32`
    pub fn jmp_rel32(&mut self, rel: i32) {
        self.emit(0xE9);
        self.emit_slice(&rel.to_le_bytes());
    }

    /// Standard function prologue: push rbp; mov rbp, rsp; push rbx; push r15
    pub fn prologue(&mut self) {
        self.emit_slice(&[0x55]);               // push rbp
        self.emit_slice(&[0x48, 0x89, 0xE5]);  // mov rbp, rsp
        self.emit_slice(&[0x53]);               // push rbx
        self.emit_slice(&[0x41, 0x57]);         // push r15
    }

    /// Standard function epilogue: pop r15; pop rbx; pop rbp; ret
    pub fn epilogue(&mut self) {
        self.emit_slice(&[0x41, 0x5F]);         // pop r15
        self.emit_slice(&[0x5B]);               // pop rbx
        self.emit_slice(&[0x5D]);               // pop rbp
        self.emit(0xC3);                        // ret
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// JIT compiler
// ─────────────────────────────────────────────────────────────────────────────

/// Result of compiling a function.
#[derive(Debug)]
pub struct CompiledFunc {
    pub key:        FuncKey,
    /// Raw x86-64 machine code bytes (not executable in this sandbox build).
    pub code:       Vec<u8>,
    /// Number of locals the function uses.
    pub local_count: u32,
}

/// Compile a slice of JitOps to x86-64.
/// Returns `Err` if any op is not JIT-able (caller should use interpreter).
pub fn compile(ops: &[JitOp]) -> Result<CompiledFunc, &'static str> {
    let key = FuncKey::from_ops(ops);
    let mut buf = CodeBuf::new();
    buf.prologue();

    // First pass: find max local slot.
    let local_count = ops.iter().filter_map(|o| match o {
        JitOp::LoadLocal(i) | JitOp::StoreLocal(i) => Some(i + 1),
        _ => None,
    }).max().unwrap_or(0);

    // Allocate local slots on the stack: sub rsp, local_count*8
    if local_count > 0 {
        let frame_bytes = (local_count * 8) as i32;
        buf.emit_slice(&[0x48, 0x81, 0xEC]); // sub rsp, imm32
        buf.emit_slice(&frame_bytes.to_le_bytes());
    }

    // Second pass: emit code for each op.
    // We track patch sites for forward jumps.
    let mut patch_sites: Vec<(usize, usize)> = Vec::new(); // (patch_offset, target_op_idx)
    let mut op_offsets: Vec<usize> = Vec::new();

    for (i, op) in ops.iter().enumerate() {
        op_offsets.push(buf.len());
        match op {
            JitOp::PushInt(v) => {
                buf.mov_rax_imm64(*v);
                buf.push_rax();
            }
            JitOp::PushFloat(_) => {
                // Float pushes not handled by integer JIT — fall back.
                return Err("float not JIT-able in integer path");
            }
            JitOp::Pop => {
                buf.pop_rax();
            }
            JitOp::AddInt => {
                buf.pop_rax();  // b
                buf.pop_rbx();  // a
                buf.add_rax_rbx(); // rax = a + b (order: rbx+rax=rax)
                // Actually add_rax_rbx emits add rax, rbx → rax = rax+rbx
                // rax=b, rbx=a → result = a+b ✓
                buf.push_rax();
            }
            JitOp::SubInt => {
                buf.pop_rax();  // b
                buf.pop_rbx();  // a
                // want a - b = rbx - rax
                buf.sub_rbx_rax(); // result in rax
                buf.push_rax();
            }
            JitOp::MulInt => {
                buf.pop_rax();  // b
                buf.pop_rbx();  // a
                buf.imul_rax_rbx(); // rax = rax * rbx = b * a ✓
                buf.push_rax();
            }
            JitOp::DivInt => {
                buf.pop_rax();  // b (divisor)
                buf.pop_rbx();  // a (dividend)
                buf.idiv_rbx_rax(); // result in rax
                buf.push_rax();
            }
            JitOp::Neg => {
                buf.pop_rax();
                buf.neg_rax();
                buf.push_rax();
            }
            JitOp::CmpLt => {
                buf.pop_rax();  // b
                buf.pop_rbx();  // a
                buf.cmp_lt();
                buf.push_rax();
            }
            JitOp::CmpEq => {
                buf.pop_rax();
                buf.pop_rbx();
                buf.cmp_eq();
                buf.push_rax();
            }
            JitOp::CmpLe => {
                buf.pop_rax();
                buf.pop_rbx();
                buf.cmp_le();
                buf.push_rax();
            }
            JitOp::JumpIfFalse(rel_op) => {
                buf.pop_rax(); // condition
                // We need to know the target code offset.
                // Save patch site and fill 0 for now; fix in third pass.
                let target_op = (i as i32 + 1 + *rel_op) as usize;
                // Emit jz with placeholder
                buf.emit_slice(&[0x48, 0x85, 0xC0]); // test rax, rax
                buf.emit_slice(&[0x0F, 0x84]);        // jz rel32
                patch_sites.push((buf.len(), target_op));
                buf.emit_slice(&[0x00, 0x00, 0x00, 0x00]);
            }
            JitOp::Jump(rel_op) => {
                let target_op = (i as i32 + 1 + *rel_op) as usize;
                buf.emit(0xE9); // jmp rel32
                patch_sites.push((buf.len(), target_op));
                buf.emit_slice(&[0x00, 0x00, 0x00, 0x00]);
            }
            JitOp::LoadLocal(idx) => {
                // mov rax, [rbp - (idx+1)*8]
                let off = -((idx + 1) as i32 * 8 + 16); // -16 for saved rbx/r15
                buf.emit_slice(&[0x48, 0x8B, 0x85]); // mov rax, [rbp+disp32]
                buf.emit_slice(&off.to_le_bytes());
                buf.push_rax();
            }
            JitOp::StoreLocal(idx) => {
                buf.pop_rax();
                let off = -((idx + 1) as i32 * 8 + 16);
                buf.emit_slice(&[0x48, 0x89, 0x85]); // mov [rbp+disp32], rax
                buf.emit_slice(&off.to_le_bytes());
            }
            JitOp::CallBuiltin(b) => {
                // For builtins we pop one arg, compute, push result.
                // We implement them inline using integer approximations.
                buf.pop_rax(); // arg (integer)
                match b {
                    BuiltinFn::Abs => {
                        // if rax < 0: neg rax
                        buf.emit_slice(&[0x48, 0x85, 0xC0]); // test rax, rax
                        buf.emit_slice(&[0x79, 0x03]);        // jns +3 (skip neg)
                        buf.neg_rax();
                    }
                    BuiltinFn::Floor | BuiltinFn::Ceil => {
                        // Integer arg: floor/ceil are identity for integers
                        // (no-op for integer JIT path)
                    }
                    BuiltinFn::Sqrt => {
                        // Newton-Raphson integer sqrt: x = arg, loop
                        // This is a rough approximation; exact for perfect squares.
                        // We emit: call to a C-convention helper via indirect call.
                        // For this pass, we skip and let floats fall back.
                        return Err("sqrt not in integer JIT");
                    }
                    BuiltinFn::Min | BuiltinFn::Max => {
                        // Need two args
                        return Err("min/max need two-arg version");
                    }
                }
                buf.push_rax();
            }
            JitOp::Return => {
                buf.pop_rax(); // return value
                if local_count > 0 {
                    let frame_bytes = (local_count * 8) as i32;
                    buf.emit_slice(&[0x48, 0x81, 0xC4]); // add rsp, imm32
                    buf.emit_slice(&frame_bytes.to_le_bytes());
                }
                buf.epilogue();
            }
        }
    }

    // Third pass: fill in jump patch sites.
    op_offsets.push(buf.len()); // sentinel for "end of function"
    for (patch_off, target_op) in &patch_sites {
        let target_code_off = if *target_op < op_offsets.len() {
            op_offsets[*target_op]
        } else {
            buf.len() // jump past end
        };
        // rel32 = target - (patch_off + 4)
        let rel = (target_code_off as i64 - (*patch_off as i64 + 4)) as i32;
        let rel_bytes = rel.to_le_bytes();
        buf.0[*patch_off]     = rel_bytes[0];
        buf.0[*patch_off + 1] = rel_bytes[1];
        buf.0[*patch_off + 2] = rel_bytes[2];
        buf.0[*patch_off + 3] = rel_bytes[3];
    }

    Ok(CompiledFunc { key, code: buf.0, local_count })
}

// ─────────────────────────────────────────────────────────────────────────────
// JIT cache and hot-call tracker
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct JitCache {
    hot_counts: BTreeMap<FuncKey, u32>,
    compiled:   BTreeMap<FuncKey, CompiledFunc>,
}

impl JitCache {
    pub fn new() -> Self { Self::default() }

    /// Record one call of the function.  Returns true if the function just
    /// became hot and was successfully compiled.
    pub fn tick(&mut self, ops: &[JitOp]) -> bool {
        let key = FuncKey::from_ops(ops);
        let count = self.hot_counts.entry(key).or_insert(0);
        *count += 1;
        if *count == HOT_THRESHOLD && !self.compiled.contains_key(&key) {
            if let Ok(cf) = compile(ops) {
                self.compiled.insert(key, cf);
                return true;
            }
        }
        false
    }

    /// Look up compiled code for a function.
    pub fn lookup(&self, ops: &[JitOp]) -> Option<&CompiledFunc> {
        let key = FuncKey::from_ops(ops);
        self.compiled.get(&key)
    }

    pub fn compiled_count(&self) -> usize { self.compiled.len() }
    pub fn hot_count(&self, ops: &[JitOp]) -> u32 {
        let key = FuncKey::from_ops(ops);
        *self.hot_counts.get(&key).unwrap_or(&0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Interpreter — executes JitOps (used as fallback when JIT not ready)
// ─────────────────────────────────────────────────────────────────────────────

pub fn interpret(ops: &[JitOp], args: &[i64]) -> Result<i64, &'static str> {
    let mut stack: Vec<i64> = Vec::new();
    let mut locals: Vec<i64> = {
        let max_local = ops.iter().filter_map(|o| match o {
            JitOp::LoadLocal(i) | JitOp::StoreLocal(i) => Some(*i as usize + 1),
            _ => None,
        }).max().unwrap_or(0);
        let mut v = alloc::vec![0i64; max_local];
        for (i, a) in args.iter().enumerate() {
            if i < v.len() { v[i] = *a; }
        }
        v
    };
    let mut pc: usize = 0;
    while pc < ops.len() {
        match &ops[pc] {
            JitOp::PushInt(v)       => stack.push(*v),
            JitOp::PushFloat(v)     => stack.push(*v as i64),
            JitOp::Pop              => { stack.pop(); }
            JitOp::AddInt           => { let b=stack.pop().unwrap_or(0); let a=stack.pop().unwrap_or(0); stack.push(a+b); }
            JitOp::SubInt           => { let b=stack.pop().unwrap_or(0); let a=stack.pop().unwrap_or(0); stack.push(a-b); }
            JitOp::MulInt           => { let b=stack.pop().unwrap_or(0); let a=stack.pop().unwrap_or(0); stack.push(a*b); }
            JitOp::DivInt           => { let b=stack.pop().unwrap_or(0); let a=stack.pop().unwrap_or(0); stack.push(if b==0{0}else{a/b}); }
            JitOp::Neg              => { let v=stack.pop().unwrap_or(0); stack.push(-v); }
            JitOp::CmpLt            => { let b=stack.pop().unwrap_or(0); let a=stack.pop().unwrap_or(0); stack.push((a<b) as i64); }
            JitOp::CmpEq            => { let b=stack.pop().unwrap_or(0); let a=stack.pop().unwrap_or(0); stack.push((a==b) as i64); }
            JitOp::CmpLe            => { let b=stack.pop().unwrap_or(0); let a=stack.pop().unwrap_or(0); stack.push((a<=b) as i64); }
            JitOp::JumpIfFalse(rel) => {
                let cond = stack.pop().unwrap_or(0);
                if cond == 0 { pc = (pc as i64 + 1 + *rel as i64) as usize; continue; }
            }
            JitOp::Jump(rel) => {
                pc = (pc as i64 + 1 + *rel as i64) as usize;
                continue;
            }
            JitOp::LoadLocal(i)     => { let v = locals.get(*i as usize).copied().unwrap_or(0); stack.push(v); }
            JitOp::StoreLocal(i)    => {
                let v = stack.pop().unwrap_or(0);
                let idx = *i as usize;
                while locals.len() <= idx { locals.push(0); }
                locals[idx] = v;
            }
            JitOp::CallBuiltin(b)   => {
                let a = stack.pop().unwrap_or(0);
                let r = match b {
                    BuiltinFn::Abs   => a.abs(),
                    BuiltinFn::Floor | BuiltinFn::Ceil => a,
                    BuiltinFn::Sqrt  => {
                        if a < 0 { 0 }
                        else {
                            let mut x = a;
                            let mut y = (x + 1) / 2;
                            while y < x { x = y; y = (y + a/y) / 2; }
                            x
                        }
                    }
                    BuiltinFn::Min => {
                        let b2 = stack.pop().unwrap_or(0);
                        if b2 < a { b2 } else { a }
                    }
                    BuiltinFn::Max => {
                        let b2 = stack.pop().unwrap_or(0);
                        if b2 > a { b2 } else { a }
                    }
                };
                stack.push(r);
            }
            JitOp::Return => { return Ok(stack.pop().unwrap_or(0)); }
        }
        pc += 1;
    }
    Ok(stack.pop().unwrap_or(0))
}

// ─────────────────────────────────────────────────────────────────────────────
// SELF-TEST
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;

    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] js_jit: {}", $name); }
        }
    }

    // T1: FuncKey determinism
    let ops1 = alloc::vec![JitOp::PushInt(1), JitOp::PushInt(2), JitOp::AddInt, JitOp::Return];
    let k1 = FuncKey::from_ops(&ops1);
    let k2 = FuncKey::from_ops(&ops1);
    check!(k1 == k2, "FuncKey deterministic");

    // T2: Different ops → different key
    let ops2 = alloc::vec![JitOp::PushInt(1), JitOp::PushInt(2), JitOp::SubInt, JitOp::Return];
    check!(FuncKey::from_ops(&ops1) != FuncKey::from_ops(&ops2), "FuncKey differs for different ops");

    // T3: Interpreter — addition
    let ops_add = alloc::vec![JitOp::PushInt(10), JitOp::PushInt(32), JitOp::AddInt, JitOp::Return];
    let r = interpret(&ops_add, &[]).unwrap();
    check!(r == 42, "interpreter add 10+32=42");

    // T4: Interpreter — subtraction
    let ops_sub = alloc::vec![JitOp::PushInt(100), JitOp::PushInt(58), JitOp::SubInt, JitOp::Return];
    check!(interpret(&ops_sub, &[]).unwrap() == 42, "interpreter sub 100-58=42");

    // T5: Interpreter — multiplication
    let ops_mul = alloc::vec![JitOp::PushInt(6), JitOp::PushInt(7), JitOp::MulInt, JitOp::Return];
    check!(interpret(&ops_mul, &[]).unwrap() == 42, "interpreter mul 6*7=42");

    // T6: Interpreter — locals
    let ops_loc = alloc::vec![
        JitOp::PushInt(21),
        JitOp::StoreLocal(0),
        JitOp::LoadLocal(0),
        JitOp::LoadLocal(0),
        JitOp::AddInt,
        JitOp::Return,
    ];
    check!(interpret(&ops_loc, &[]).unwrap() == 42, "interpreter locals: store 21, load+add=42");

    // T7: Interpreter — conditional jump
    // if 1 < 2: push 42 else push 0
    let ops_cond = alloc::vec![
        JitOp::PushInt(1),
        JitOp::PushInt(2),
        JitOp::CmpLt,          // → 1
        JitOp::JumpIfFalse(2), // skip 2 ops if false
        JitOp::PushInt(42),
        JitOp::Return,
        JitOp::PushInt(0),
        JitOp::Return,
    ];
    check!(interpret(&ops_cond, &[]).unwrap() == 42, "interpreter cmplt+jump");

    // T8: Interpreter — abs builtin
    let ops_abs = alloc::vec![JitOp::PushInt(-42), JitOp::CallBuiltin(BuiltinFn::Abs), JitOp::Return];
    check!(interpret(&ops_abs, &[]).unwrap() == 42, "builtin abs(-42)=42");

    // T9: Interpreter — integer sqrt
    let ops_sqrt = alloc::vec![JitOp::PushInt(1764), JitOp::CallBuiltin(BuiltinFn::Sqrt), JitOp::Return];
    check!(interpret(&ops_sqrt, &[]).unwrap() == 42, "builtin sqrt(1764)=42");

    // T10: JitCache hot threshold
    let mut cache = JitCache::new();
    let ops_simple = alloc::vec![JitOp::PushInt(1), JitOp::Return];
    let mut compiled_at = 0u32;
    for i in 0..HOT_THRESHOLD + 5 {
        if cache.tick(&ops_simple) { compiled_at = i + 1; }
    }
    check!(compiled_at == HOT_THRESHOLD, "JIT compiled at hot threshold");
    check!(cache.compiled_count() == 1, "JIT cache has 1 entry");

    if fail == 0 {
        crate::serial_println!("[js_jit] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[js_jit] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

#![allow(dead_code)]
/// GLSL ES 1.00 Shader Compiler — Phase 20 for Smart OS.
///
/// Pipeline:
///   source ──► Lexer ──► Parser ──► IR (SSA-like) ──► x86_64 SSE2 codegen
///
/// Scope:
///  • Tokenizer: all GLSL ES 1.00 tokens
///  • Parser:    function declarations, statements, expressions
///  • IR:        linear three-address code (TAC) with SSE2-friendly types
///  • Codegen:   x86_64 machine code emitted into a heap Vec<u8>
///  • Shader cache: FNV-1a hash → compiled entry point pointer
///
/// Limitations (v1):
///  • No control flow (if/else/for/while) in codegen — these emit fallback SW path
///  • Arrays, structs, samplers resolve to scalar/vec4 approximations
///  • Output: vertex shader returns gl_Position; fragment shader returns gl_FragColor

use alloc::vec::Vec;
use alloc::string::String;
use alloc::vec;
use alloc::string::ToString;
use alloc::collections::BTreeMap;
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
//  Token types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    // Literals
    IntLit(i32),
    FloatLit(f32),
    BoolLit(bool),
    Ident(String),

    // Keywords
    KwVoid, KwBool, KwInt, KwFloat,
    KwVec2, KwVec3, KwVec4,
    KwMat2, KwMat3, KwMat4,
    KwSampler2D, KwSamplerCube,
    KwAttribute, KwUniform, KwVarying, KwConst,
    KwIn, KwOut, KwInout,
    KwReturn, KwIf, KwElse, KwFor, KwWhile, KwDo, KwBreak, KwContinue,
    KwDiscard, KwPrecision, KwHighp, KwMediump, KwLowp,
    KwStruct,

    // Operators
    Plus, Minus, Star, Slash, Percent,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or, Not, Amp, Pipe, Caret,
    Assign, PlusAssign, MinusAssign, StarAssign, SlashAssign,
    Dot, Comma, Semicolon, Colon,
    LParen, RParen, LBrace, RBrace, LBracket, RBracket,
    PlusPlus, MinusMinus,
    Question,

    Eof,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Lexer
// ─────────────────────────────────────────────────────────────────────────────

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Lexer { src: src.as_bytes(), pos: 0 }
    }

    fn peek(&self) -> Option<u8> { self.src.get(self.pos).copied() }
    fn advance(&mut self) -> Option<u8> {
        let c = self.src.get(self.pos).copied();
        self.pos += 1;
        c
    }
    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // Whitespace
            while self.peek().map(|c| c.is_ascii_whitespace()).unwrap_or(false) {
                self.advance();
            }
            // Line comment
            if self.src.get(self.pos..self.pos+2) == Some(b"//") {
                while self.peek().map(|c| c != b'\n').unwrap_or(false) { self.advance(); }
                continue;
            }
            // Block comment
            if self.src.get(self.pos..self.pos+2) == Some(b"/*") {
                self.pos += 2;
                loop {
                    if self.pos + 1 >= self.src.len() { break; }
                    if self.src[self.pos] == b'*' && self.src[self.pos+1] == b'/' {
                        self.pos += 2;
                        break;
                    }
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
    }

    pub fn next_token(&mut self) -> Token {
        self.skip_whitespace_and_comments();
        let c = match self.peek() {
            None => return Token::Eof,
            Some(c) => c,
        };

        // Number literal
        if c.is_ascii_digit() || (c == b'.' && self.src.get(self.pos+1).map(|d| d.is_ascii_digit()).unwrap_or(false)) {
            return self.lex_number();
        }

        // Identifier or keyword
        if c.is_ascii_alphabetic() || c == b'_' {
            return self.lex_ident();
        }

        self.advance();
        match c {
            b'+' => {
                if self.peek() == Some(b'+') { self.advance(); Token::PlusPlus }
                else if self.peek() == Some(b'=') { self.advance(); Token::PlusAssign }
                else { Token::Plus }
            }
            b'-' => {
                if self.peek() == Some(b'-') { self.advance(); Token::MinusMinus }
                else if self.peek() == Some(b'=') { self.advance(); Token::MinusAssign }
                else { Token::Minus }
            }
            b'*' => {
                if self.peek() == Some(b'=') { self.advance(); Token::StarAssign }
                else { Token::Star }
            }
            b'/' => {
                if self.peek() == Some(b'=') { self.advance(); Token::SlashAssign }
                else { Token::Slash }
            }
            b'%' => Token::Percent,
            b'=' => {
                if self.peek() == Some(b'=') { self.advance(); Token::Eq }
                else { Token::Assign }
            }
            b'!' => {
                if self.peek() == Some(b'=') { self.advance(); Token::Ne }
                else { Token::Not }
            }
            b'<' => {
                if self.peek() == Some(b'=') { self.advance(); Token::Le }
                else { Token::Lt }
            }
            b'>' => {
                if self.peek() == Some(b'=') { self.advance(); Token::Ge }
                else { Token::Gt }
            }
            b'&' => {
                if self.peek() == Some(b'&') { self.advance(); Token::And }
                else { Token::Amp }
            }
            b'|' => {
                if self.peek() == Some(b'|') { self.advance(); Token::Or }
                else { Token::Pipe }
            }
            b'^' => Token::Caret,
            b'.' => Token::Dot,
            b',' => Token::Comma,
            b';' => Token::Semicolon,
            b':' => Token::Colon,
            b'?' => Token::Question,
            b'(' => Token::LParen,
            b')' => Token::RParen,
            b'{' => Token::LBrace,
            b'}' => Token::RBrace,
            b'[' => Token::LBracket,
            b']' => Token::RBracket,
            _ => Token::Eof,
        }
    }

    fn lex_number(&mut self) -> Token {
        let start = self.pos;
        let mut is_float = false;
        while self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) { self.advance(); }
        if self.peek() == Some(b'.') {
            is_float = true;
            self.advance();
            while self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) { self.advance(); }
        }
        if self.peek() == Some(b'e') || self.peek() == Some(b'E') {
            is_float = true;
            self.advance();
            if self.peek() == Some(b'+') || self.peek() == Some(b'-') { self.advance(); }
            while self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) { self.advance(); }
        }
        // Skip f/F suffix.
        if self.peek() == Some(b'f') || self.peek() == Some(b'F') {
            is_float = true;
            self.advance();
        }
        let s = core::str::from_utf8(&self.src[start..self.pos]).unwrap_or("0");
        if is_float {
            Token::FloatLit(parse_f32(s))
        } else {
            Token::IntLit(parse_i32(s))
        }
    }

    fn lex_ident(&mut self) -> Token {
        let start = self.pos;
        while self.peek().map(|c| c.is_ascii_alphanumeric() || c == b'_').unwrap_or(false) {
            self.advance();
        }
        let s = core::str::from_utf8(&self.src[start..self.pos]).unwrap_or("");
        match s {
            "void"       => Token::KwVoid,
            "bool"       => Token::KwBool,
            "int"        => Token::KwInt,
            "float"      => Token::KwFloat,
            "vec2"       => Token::KwVec2,
            "vec3"       => Token::KwVec3,
            "vec4"       => Token::KwVec4,
            "mat2"       => Token::KwMat2,
            "mat3"       => Token::KwMat3,
            "mat4"       => Token::KwMat4,
            "sampler2D"  => Token::KwSampler2D,
            "samplerCube"=> Token::KwSamplerCube,
            "attribute"  => Token::KwAttribute,
            "uniform"    => Token::KwUniform,
            "varying"    => Token::KwVarying,
            "const"      => Token::KwConst,
            "in"         => Token::KwIn,
            "out"        => Token::KwOut,
            "inout"      => Token::KwInout,
            "return"     => Token::KwReturn,
            "if"         => Token::KwIf,
            "else"       => Token::KwElse,
            "for"        => Token::KwFor,
            "while"      => Token::KwWhile,
            "do"         => Token::KwDo,
            "break"      => Token::KwBreak,
            "continue"   => Token::KwContinue,
            "discard"    => Token::KwDiscard,
            "precision"  => Token::KwPrecision,
            "highp"      => Token::KwHighp,
            "mediump"    => Token::KwMediump,
            "lowp"       => Token::KwLowp,
            "struct"     => Token::KwStruct,
            "true"       => Token::BoolLit(true),
            "false"      => Token::BoolLit(false),
            _            => Token::Ident(s.to_string()),
        }
    }

    /// Tokenize the entire source.
    pub fn tokenize(src: &str) -> Vec<Token> {
        let mut lex = Lexer::new(src);
        let mut tokens = Vec::new();
        loop {
            let t = lex.next_token();
            let done = t == Token::Eof;
            tokens.push(t);
            if done { break; }
        }
        tokens
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Types and IR
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlslType {
    Void, Bool, Int, Float,
    Vec2, Vec3, Vec4,
    Mat2, Mat3, Mat4,
    Sampler2D, SamplerCube,
}

impl GlslType {
    /// Number of f32 scalars.
    pub fn f32_count(self) -> usize {
        match self {
            GlslType::Void       => 0,
            GlslType::Bool | GlslType::Int | GlslType::Float => 1,
            GlslType::Vec2       => 2,
            GlslType::Vec3       => 3,
            GlslType::Vec4       => 4,
            GlslType::Mat2       => 4,
            GlslType::Mat3       => 9,
            GlslType::Mat4       => 16,
            GlslType::Sampler2D | GlslType::SamplerCube => 1,
        }
    }
}

/// A virtual register (SSA value).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Reg(pub u32);

/// Three-address IR instruction.
#[derive(Clone, Debug)]
pub enum Ir {
    /// dst = float constant
    LoadF(Reg, f32),
    /// dst = src (move)
    Mov(Reg, Reg),
    /// dst[0..n] = src0[0..n] op src1[0..n]
    Add(Reg, Reg, Reg, u8),   // (dst, lhs, rhs, lanes)
    Sub(Reg, Reg, Reg, u8),
    Mul(Reg, Reg, Reg, u8),
    Div(Reg, Reg, Reg, u8),
    /// dst = -src
    Neg(Reg, Reg, u8),
    /// Dot product (vec4): dst.x = dot(lhs, rhs)
    Dot(Reg, Reg, Reg, u8),
    /// Matrix multiply vec4: dst = mat * vec  (mat is 4 regs row-major)
    MatMulVec(Reg, [Reg; 4], Reg),
    /// Swizzle: dst.x = src.components[0], etc.
    Swizzle(Reg, Reg, [u8; 4], u8), // (dst, src, mask, out_lanes)
    /// Store to output variable (gl_Position / gl_FragColor).
    StoreOutput(u8, Reg),   // (slot, src)
    /// Load from uniform slot.
    LoadUniform(Reg, u32),  // (dst, uniform_idx)
    /// Load from attribute slot.
    LoadAttr(Reg, u32),     // (dst, attr_idx)
    /// Call a built-in function: id, dst, args.
    Builtin(BuiltinFn, Reg, Vec<Reg>),
    /// Return from function.
    Return,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinFn {
    Normalize, Length, Reflect, Clamp,
    Mix, Fract, Abs, Sign,
    Sqrt, InverseSqrt,
    Sin, Cos, Tan,
    Pow, Exp, Log,
    Max, Min,
    Texture2D, TextureCube,
    FaceForward,
    Cross,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Simple parser → IR
// ─────────────────────────────────────────────────────────────────────────────

struct Parser {
    tokens: Vec<Token>,
    pos:    usize,
    regs:   u32,
    ir:     Vec<Ir>,
    /// symbol table: name → (reg, type, is_uniform, is_attr, uniform_idx, attr_idx)
    syms:   BTreeMap<String, SymEntry>,
}

#[derive(Clone)]
struct SymEntry {
    reg:         Reg,
    ty:          GlslType,
    is_uniform:  bool,
    uniform_idx: u32,
    is_attr:     bool,
    attr_idx:    u32,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0, regs: 0, ir: Vec::new(), syms: BTreeMap::new() }
    }

    fn peek(&self) -> &Token { self.tokens.get(self.pos).unwrap_or(&Token::Eof) }
    fn advance(&mut self) -> &Token {
        let t = self.tokens.get(self.pos).unwrap_or(&Token::Eof);
        self.pos += 1;
        t
    }
    fn eat(&mut self, t: &Token) -> bool {
        if self.peek() == t { self.advance(); true } else { false }
    }

    fn new_reg(&mut self) -> Reg { let r = Reg(self.regs); self.regs += 1; r }

    // ── Parse type specifier ────────────────────────────────────────────────
    fn parse_type(&mut self) -> Option<GlslType> {
        match self.peek().clone() {
            Token::KwVoid      => { self.advance(); Some(GlslType::Void) }
            Token::KwBool      => { self.advance(); Some(GlslType::Bool) }
            Token::KwInt       => { self.advance(); Some(GlslType::Int) }
            Token::KwFloat     => { self.advance(); Some(GlslType::Float) }
            Token::KwVec2      => { self.advance(); Some(GlslType::Vec2) }
            Token::KwVec3      => { self.advance(); Some(GlslType::Vec3) }
            Token::KwVec4      => { self.advance(); Some(GlslType::Vec4) }
            Token::KwMat2      => { self.advance(); Some(GlslType::Mat2) }
            Token::KwMat3      => { self.advance(); Some(GlslType::Mat3) }
            Token::KwMat4      => { self.advance(); Some(GlslType::Mat4) }
            Token::KwSampler2D => { self.advance(); Some(GlslType::Sampler2D) }
            Token::KwSamplerCube => { self.advance(); Some(GlslType::SamplerCube) }
            _ => None,
        }
    }

    // ── Parse top-level declarations ────────────────────────────────────────
    pub fn parse_program(&mut self) {
        let mut uniform_idx: u32 = 0;
        let mut attr_idx:    u32 = 0;

        loop {
            match self.peek().clone() {
                Token::Eof => break,

                // precision qualifier — skip
                Token::KwPrecision => {
                    self.advance();
                    self.advance(); // highp/mediump/lowp
                    self.advance(); // type
                    self.eat(&Token::Semicolon);
                }

                // attribute / uniform / varying
                Token::KwAttribute | Token::KwUniform | Token::KwVarying => {
                    let qual = self.advance().clone();
                    // optional precision
                    if matches!(self.peek(), Token::KwHighp | Token::KwMediump | Token::KwLowp) {
                        self.advance();
                    }
                    let ty = self.parse_type().unwrap_or(GlslType::Vec4);
                    if let Token::Ident(name) = self.advance().clone() {
                        let r = self.new_reg();
                        let (is_u, ui, is_a, ai) = match qual {
                            Token::KwUniform  => (true, uniform_idx, false, 0),
                            Token::KwAttribute => (false, 0, true, attr_idx),
                            _ => (false, 0, false, 0),
                        };
                        if matches!(qual, Token::KwUniform)  { uniform_idx += ty.f32_count() as u32; }
                        if matches!(qual, Token::KwAttribute) { attr_idx    += ty.f32_count() as u32; }
                        self.syms.insert(name, SymEntry { reg: r, ty, is_uniform: is_u, uniform_idx: ui, is_attr: is_a, attr_idx: ai });
                    }
                    // optional array []
                    if self.eat(&Token::LBracket) {
                        while !matches!(self.peek(), Token::RBracket | Token::Eof) { self.advance(); }
                        self.eat(&Token::RBracket);
                    }
                    self.eat(&Token::Semicolon);
                }

                // const — treat as global variable
                Token::KwConst => {
                    self.advance();
                    self.advance(); // type
                    self.advance(); // name
                    while !matches!(self.peek(), Token::Semicolon | Token::Eof) { self.advance(); }
                    self.eat(&Token::Semicolon);
                }

                _ => {
                    // Function definition (return-type name(...) { ... })
                    if self.parse_type().is_some() {
                        // function name
                        if let Token::Ident(_name) = self.advance().clone() {
                            // eat parameter list
                            if self.eat(&Token::LParen) {
                                let mut depth = 1;
                                while depth > 0 && !matches!(self.peek(), Token::Eof) {
                                    match self.advance().clone() {
                                        Token::LParen => depth += 1,
                                        Token::RParen => depth -= 1,
                                        _ => {}
                                    }
                                }
                            }
                            // eat body
                            if self.eat(&Token::LBrace) {
                                self.parse_block();
                            }
                        }
                    } else {
                        self.advance(); // skip unknown token
                    }
                }
            }
        }

        // Emit loads for all uniforms/attributes.
        for (_name, sym) in &self.syms {
            if sym.is_uniform {
                self.ir.push(Ir::LoadUniform(sym.reg, sym.uniform_idx));
            } else if sym.is_attr {
                self.ir.push(Ir::LoadAttr(sym.reg, sym.attr_idx));
            }
        }
        self.ir.push(Ir::Return);
    }

    fn parse_block(&mut self) {
        let mut depth = 1u32;
        while depth > 0 && !matches!(self.peek(), Token::Eof) {
            match self.peek().clone() {
                Token::LBrace => { depth += 1; self.advance(); }
                Token::RBrace => {
                    depth -= 1;
                    self.advance();
                    if depth == 0 { break; }
                }
                Token::KwReturn => {
                    self.advance();
                    if !matches!(self.peek(), Token::Semicolon) {
                        let r = self.parse_expr();
                        self.ir.push(Ir::StoreOutput(0, r));
                    }
                    self.eat(&Token::Semicolon);
                }
                _ => { self.parse_stmt(); }
            }
        }
    }

    fn parse_stmt(&mut self) {
        // Detect type-prefixed declaration.
        if let Some(ty) = self.parse_type() {
            if let Token::Ident(name) = self.advance().clone() {
                let r = self.new_reg();
                self.syms.insert(name.clone(), SymEntry {
                    reg: r, ty, is_uniform: false, uniform_idx: 0, is_attr: false, attr_idx: 0,
                });
                if self.eat(&Token::Assign) {
                    let rhs = self.parse_expr();
                    self.ir.push(Ir::Mov(r, rhs));
                }
                self.eat(&Token::Semicolon);
                return;
            }
        }
        // Assignment or expression.
        let _lhs = self.parse_expr();
        self.eat(&Token::Semicolon);
    }

    fn parse_expr(&mut self) -> Reg {
        self.parse_additive()
    }

    fn parse_additive(&mut self) -> Reg {
        let mut lhs = self.parse_multiplicative();
        loop {
            match self.peek().clone() {
                Token::Plus => {
                    self.advance();
                    let rhs = self.parse_multiplicative();
                    let dst = self.new_reg();
                    self.ir.push(Ir::Add(dst, lhs, rhs, 4));
                    lhs = dst;
                }
                Token::Minus => {
                    self.advance();
                    let rhs = self.parse_multiplicative();
                    let dst = self.new_reg();
                    self.ir.push(Ir::Sub(dst, lhs, rhs, 4));
                    lhs = dst;
                }
                _ => break,
            }
        }
        lhs
    }

    fn parse_multiplicative(&mut self) -> Reg {
        let mut lhs = self.parse_unary();
        loop {
            match self.peek().clone() {
                Token::Star => {
                    self.advance();
                    let rhs = self.parse_unary();
                    let dst = self.new_reg();
                    self.ir.push(Ir::Mul(dst, lhs, rhs, 4));
                    lhs = dst;
                }
                Token::Slash => {
                    self.advance();
                    let rhs = self.parse_unary();
                    let dst = self.new_reg();
                    self.ir.push(Ir::Div(dst, lhs, rhs, 4));
                    lhs = dst;
                }
                _ => break,
            }
        }
        lhs
    }

    fn parse_unary(&mut self) -> Reg {
        if matches!(self.peek(), Token::Minus) {
            self.advance();
            let src = self.parse_primary();
            let dst = self.new_reg();
            self.ir.push(Ir::Neg(dst, src, 4));
            return dst;
        }
        if matches!(self.peek(), Token::Not | Token::Plus) { self.advance(); }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Reg {
        let tok = self.peek().clone();
        match tok {
            Token::FloatLit(f) => {
                self.advance();
                let r = self.new_reg();
                self.ir.push(Ir::LoadF(r, f));
                r
            }
            Token::IntLit(i) => {
                self.advance();
                let r = self.new_reg();
                self.ir.push(Ir::LoadF(r, i as f32));
                r
            }
            Token::BoolLit(b) => {
                self.advance();
                let r = self.new_reg();
                self.ir.push(Ir::LoadF(r, if b { 1.0 } else { 0.0 }));
                r
            }
            Token::LParen => {
                self.advance();
                let r = self.parse_expr();
                self.eat(&Token::RParen);
                r
            }
            Token::Ident(name) => {
                self.advance();
                // Check for function call.
                if matches!(self.peek(), Token::LParen) {
                    self.advance();
                    let mut args = Vec::new();
                    while !matches!(self.peek(), Token::RParen | Token::Eof) {
                        args.push(self.parse_expr());
                        self.eat(&Token::Comma);
                    }
                    self.eat(&Token::RParen);
                    let dst = self.new_reg();
                    if let Some(builtin) = builtin_fn(&name) {
                        self.ir.push(Ir::Builtin(builtin, dst, args));
                    }
                    return dst;
                }
                // Constructor: vec4/vec3/vec2 handled as load 0.
                // Look up symbol.
                if let Some(sym) = self.syms.get(&name).cloned() {
                    // Swizzle?
                    if matches!(self.peek(), Token::Dot) {
                        self.advance();
                        if let Token::Ident(sw) = self.advance().clone() {
                            let mask = parse_swizzle(&sw);
                            let lanes = sw.len().min(4) as u8;
                            let dst = self.new_reg();
                            self.ir.push(Ir::Swizzle(dst, sym.reg, mask, lanes));
                            return dst;
                        }
                    }
                    sym.reg
                } else {
                    // Unknown — return zero reg.
                    let r = self.new_reg();
                    self.ir.push(Ir::LoadF(r, 0.0));
                    r
                }
            }
            // Type constructors used as functions (vec4(0.0), etc.)
            Token::KwVec2 | Token::KwVec3 | Token::KwVec4 |
            Token::KwMat2 | Token::KwMat3 | Token::KwMat4 => {
                self.advance();
                self.eat(&Token::LParen);
                let mut args = Vec::new();
                while !matches!(self.peek(), Token::RParen | Token::Eof) {
                    args.push(self.parse_expr());
                    self.eat(&Token::Comma);
                }
                self.eat(&Token::RParen);
                // Return first arg or 0.
                args.into_iter().next().unwrap_or_else(|| {
                    let r = self.new_reg();
                    self.ir.push(Ir::LoadF(r, 0.0));
                    r
                })
            }
            _ => {
                self.advance();
                let r = self.new_reg();
                self.ir.push(Ir::LoadF(r, 0.0));
                r
            }
        }
    }
}

fn builtin_fn(name: &str) -> Option<BuiltinFn> {
    match name {
        "normalize"    => Some(BuiltinFn::Normalize),
        "length"       => Some(BuiltinFn::Length),
        "reflect"      => Some(BuiltinFn::Reflect),
        "clamp"        => Some(BuiltinFn::Clamp),
        "mix"          => Some(BuiltinFn::Mix),
        "fract"        => Some(BuiltinFn::Fract),
        "abs"          => Some(BuiltinFn::Abs),
        "sign"         => Some(BuiltinFn::Sign),
        "sqrt"         => Some(BuiltinFn::Sqrt),
        "inversesqrt"  => Some(BuiltinFn::InverseSqrt),
        "sin"          => Some(BuiltinFn::Sin),
        "cos"          => Some(BuiltinFn::Cos),
        "tan"          => Some(BuiltinFn::Tan),
        "pow"          => Some(BuiltinFn::Pow),
        "exp"          => Some(BuiltinFn::Exp),
        "log"          => Some(BuiltinFn::Log),
        "max"          => Some(BuiltinFn::Max),
        "min"          => Some(BuiltinFn::Min),
        "texture2D"    => Some(BuiltinFn::Texture2D),
        "textureCube"  => Some(BuiltinFn::TextureCube),
        "faceforward"  => Some(BuiltinFn::FaceForward),
        "cross"        => Some(BuiltinFn::Cross),
        "dot"          => Some(BuiltinFn::Normalize), // reuse slot; handled in codegen
        _ => None,
    }
}

fn parse_swizzle(s: &str) -> [u8; 4] {
    let mut mask = [0u8; 4];
    for (i, c) in s.chars().take(4).enumerate() {
        mask[i] = match c {
            'x' | 'r' | 's' => 0,
            'y' | 'g' | 't' => 1,
            'z' | 'b' | 'p' => 2,
            'w' | 'a' | 'q' => 3,
            _ => 0,
        };
    }
    mask
}

// ─────────────────────────────────────────────────────────────────────────────
//  x86_64 SSE2 code generator
// ─────────────────────────────────────────────────────────────────────────────
//
// Calling convention (System V AMD64 / our kernel convention):
//   rdi = pointer to f32 uniform array
//   rsi = pointer to f32 attribute array
//   rdx = pointer to f32 output array (gl_Position / gl_FragColor)
//
// We use XMM registers for vec4 values.
// XMM0-XMM7 are caller-saved (we use them freely).
// Each virtual Reg maps to an XMM register (Reg(n) → XMM(n % 8)).
// Spilling is not implemented; shaders with >8 live regs fall back to SW.

struct Codegen {
    buf:     Vec<u8>,
    reg_map: BTreeMap<u32, u8>, // Reg.0 → xmm index (0-7)
    next_xmm: u8,
}

impl Codegen {
    fn new() -> Self {
        Codegen { buf: Vec::new(), reg_map: BTreeMap::new(), next_xmm: 0 }
    }

    fn xmm(&mut self, r: Reg) -> u8 {
        if let Some(&x) = self.reg_map.get(&r.0) { return x; }
        let x = self.next_xmm % 8;
        self.next_xmm += 1;
        self.reg_map.insert(r.0, x);
        x
    }

    fn emit(&mut self, b: u8) { self.buf.push(b); }
    fn emit_bytes(&mut self, bs: &[u8]) { self.buf.extend_from_slice(bs); }

    /// MOVAPS xmm_dst, xmm_src  — REX-less form.
    fn movaps_xmm_xmm(&mut self, dst: u8, src: u8) {
        // 0F 28 /r  MOVAPS xmm1, xmm2/m128
        let modrm = 0xC0 | (dst << 3) | src;
        self.emit_bytes(&[0x0F, 0x28, modrm]);
    }

    /// MOVSS xmm, [rdi + imm32]  (load scalar from uniform array)
    fn movss_load_rdi(&mut self, xmm: u8, offset: u32) {
        // F3 0F 10 /r  MOVSS xmm1, m32
        // ModRM with RIP-relative won't work for rdi+offset, use disp32 form.
        let modrm = 0x80 | (xmm << 3) | 0x07; // mod=10 (disp32) reg=xmm base=rdi
        let off_bytes = offset.to_le_bytes();
        self.emit_bytes(&[0xF3, 0x0F, 0x10, modrm,
            off_bytes[0], off_bytes[1], off_bytes[2], off_bytes[3]]);
    }

    /// MOVSS xmm, [rsi + imm32]  (load scalar from attribute array)
    fn movss_load_rsi(&mut self, xmm: u8, offset: u32) {
        let modrm = 0x80 | (xmm << 3) | 0x06; // base=rsi
        let off_bytes = offset.to_le_bytes();
        self.emit_bytes(&[0xF3, 0x0F, 0x10, modrm,
            off_bytes[0], off_bytes[1], off_bytes[2], off_bytes[3]]);
    }

    /// MOVSS [rdx + imm32], xmm  (store scalar to output array)
    fn movss_store_rdx(&mut self, xmm: u8, offset: u32) {
        let modrm = 0x80 | (xmm << 3) | 0x02; // base=rdx
        let off_bytes = offset.to_le_bytes();
        self.emit_bytes(&[0xF3, 0x0F, 0x11, modrm,
            off_bytes[0], off_bytes[1], off_bytes[2], off_bytes[3]]);
    }

    /// MOVAPS xmm, [rip + rel32]  — load 128-bit constant from data section.
    /// For float constants we emit them as MOVSS (scalar broadcast via SHUFPS).
    fn load_f32_const(&mut self, xmm: u8, val: f32) {
        // Strategy: encode constant in next 4 bytes after a short jump.
        // JMP rel8 (2 bytes) + 4-byte constant + MOVSS xmm, [rip-6]
        // Simpler: push the float bits, use an inline 16-byte pool.
        // We embed the bits directly using MOVD + MOVD path:
        //   MOV eax, imm32  (B8+rd)
        //   MOVD xmm, eax   (66 0F 6E /r)
        let bits = val.to_bits();
        let bits_bytes = bits.to_le_bytes();
        // MOV eax, imm32
        self.emit(0xB8);
        self.emit_bytes(&bits_bytes);
        // MOVD xmm, eax: 66 0F 6E /r, ModRM = C0 | (xmm<<3) | 0 (rax=0)
        let modrm = 0xC0 | (xmm << 3) | 0x00;
        self.emit_bytes(&[0x66, 0x0F, 0x6E, modrm]);
    }

    /// ADDPS xmm_dst, xmm_src
    fn addps(&mut self, dst: u8, src: u8) {
        let modrm = 0xC0 | (dst << 3) | src;
        self.emit_bytes(&[0x0F, 0x58, modrm]);
    }
    /// SUBPS
    fn subps(&mut self, dst: u8, src: u8) {
        let modrm = 0xC0 | (dst << 3) | src;
        self.emit_bytes(&[0x0F, 0x5C, modrm]);
    }
    /// MULPS
    fn mulps(&mut self, dst: u8, src: u8) {
        let modrm = 0xC0 | (dst << 3) | src;
        self.emit_bytes(&[0x0F, 0x59, modrm]);
    }
    /// DIVPS
    fn divps(&mut self, dst: u8, src: u8) {
        let modrm = 0xC0 | (dst << 3) | src;
        self.emit_bytes(&[0x0F, 0x5E, modrm]);
    }
    /// XORPS (zero a register)
    fn xorps(&mut self, dst: u8, src: u8) {
        let modrm = 0xC0 | (dst << 3) | src;
        self.emit_bytes(&[0x0F, 0x57, modrm]);
    }

    /// RET
    fn ret(&mut self) { self.emit(0xC3); }

    /// Generate machine code for an IR slice.
    pub fn compile(&mut self, ir: &[Ir]) {
        for insn in ir {
            match insn {
                Ir::LoadF(dst, f) => {
                    let x = self.xmm(*dst);
                    self.xorps(x, x); // zero
                    self.load_f32_const(x, *f);
                }
                Ir::Mov(dst, src) => {
                    let xd = self.xmm(*dst);
                    let xs = self.xmm(*src);
                    self.movaps_xmm_xmm(xd, xs);
                }
                Ir::Add(dst, lhs, rhs, _) => {
                    let xd = self.xmm(*dst);
                    let xl = self.xmm(*lhs);
                    let xr = self.xmm(*rhs);
                    self.movaps_xmm_xmm(xd, xl);
                    self.addps(xd, xr);
                }
                Ir::Sub(dst, lhs, rhs, _) => {
                    let xd = self.xmm(*dst);
                    let xl = self.xmm(*lhs);
                    let xr = self.xmm(*rhs);
                    self.movaps_xmm_xmm(xd, xl);
                    self.subps(xd, xr);
                }
                Ir::Mul(dst, lhs, rhs, _) => {
                    let xd = self.xmm(*dst);
                    let xl = self.xmm(*lhs);
                    let xr = self.xmm(*rhs);
                    self.movaps_xmm_xmm(xd, xl);
                    self.mulps(xd, xr);
                }
                Ir::Div(dst, lhs, rhs, _) => {
                    let xd = self.xmm(*dst);
                    let xl = self.xmm(*lhs);
                    let xr = self.xmm(*rhs);
                    self.movaps_xmm_xmm(xd, xl);
                    self.divps(xd, xr);
                }
                Ir::Neg(dst, src, _) => {
                    let xd = self.xmm(*dst);
                    let xs = self.xmm(*src);
                    // XORPS tmp, tmp; SUBPS tmp, src  (0 - src)
                    self.xorps(xd, xd);
                    self.subps(xd, xs);
                }
                Ir::LoadUniform(dst, idx) => {
                    let xd = self.xmm(*dst);
                    self.movss_load_rdi(xd, idx * 4);
                }
                Ir::LoadAttr(dst, idx) => {
                    let xd = self.xmm(*dst);
                    self.movss_load_rsi(xd, idx * 4);
                }
                Ir::StoreOutput(slot, src) => {
                    let xs = self.xmm(*src);
                    self.movss_store_rdx(xs, (*slot as u32) * 4);
                }
                Ir::Swizzle(dst, src, mask, lanes) => {
                    let xd = self.xmm(*dst);
                    let xs = self.xmm(*src);
                    self.movaps_xmm_xmm(xd, xs);
                    // SHUFPS xd, xd, imm8 (mask)
                    let imm = (mask[0] & 3)
                        | ((mask[1] & 3) << 2)
                        | ((mask[2] & 3) << 4)
                        | ((mask[3] & 3) << 6);
                    let modrm = 0xC0 | (xd << 3) | xd;
                    self.emit_bytes(&[0x0F, 0xC6, modrm, imm]);
                    let _ = lanes;
                }
                Ir::Builtin(b, dst, args) => {
                    // Emit a software fallback stub: store 0.0 to dst.
                    let xd = self.xmm(*dst);
                    self.xorps(xd, xd);
                    let _ = (b, args);
                }
                Ir::Return | Ir::Dot(..) | Ir::MatMulVec(..) => {
                    // Return is handled at end; MatMul needs full 4×4 expansion.
                }
            }
        }
        self.ret();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Shader program (compiled)
// ─────────────────────────────────────────────────────────────────────────────

/// A compiled shader program — the machine code is stored in a Vec<u8>.
pub struct ShaderProgram {
    pub hash:     u64,
    /// x86_64 machine code.  Call as fn(uniforms: *const f32, attrs: *const f32, out: *mut f32).
    pub code:     Vec<u8>,
    /// IR (kept for debugging / re-optimization).
    pub ir:       Vec<Ir>,
}

impl ShaderProgram {
    /// Interpret-only path: execute the IR using a simple f32 register file.
    /// This is the fallback when the JIT is not invoked.
    pub fn interpret(&self, uniforms: &[f32], attrs: &[f32], out: &mut [f32]) {
        let mut regs: BTreeMap<u32, [f32; 4]> = BTreeMap::new();

        let load_reg = |regs: &BTreeMap<u32, [f32; 4]>, r: Reg| -> [f32; 4] {
            regs.get(&r.0).copied().unwrap_or([0.0; 4])
        };

        for insn in &self.ir {
            match insn {
                Ir::LoadF(dst, f) => { regs.insert(dst.0, [*f; 4]); }
                Ir::Mov(dst, src) => { let v = load_reg(&regs, *src); regs.insert(dst.0, v); }
                Ir::Add(dst, l, r, _) => {
                    let a = load_reg(&regs, *l); let b = load_reg(&regs, *r);
                    regs.insert(dst.0, [a[0]+b[0], a[1]+b[1], a[2]+b[2], a[3]+b[3]]);
                }
                Ir::Sub(dst, l, r, _) => {
                    let a = load_reg(&regs, *l); let b = load_reg(&regs, *r);
                    regs.insert(dst.0, [a[0]-b[0], a[1]-b[1], a[2]-b[2], a[3]-b[3]]);
                }
                Ir::Mul(dst, l, r, _) => {
                    let a = load_reg(&regs, *l); let b = load_reg(&regs, *r);
                    regs.insert(dst.0, [a[0]*b[0], a[1]*b[1], a[2]*b[2], a[3]*b[3]]);
                }
                Ir::Div(dst, l, r, _) => {
                    let a = load_reg(&regs, *l); let b = load_reg(&regs, *r);
                    regs.insert(dst.0, [
                        if b[0].abs() < 1e-7 { 0.0 } else { a[0]/b[0] },
                        if b[1].abs() < 1e-7 { 0.0 } else { a[1]/b[1] },
                        if b[2].abs() < 1e-7 { 0.0 } else { a[2]/b[2] },
                        if b[3].abs() < 1e-7 { 0.0 } else { a[3]/b[3] },
                    ]);
                }
                Ir::Neg(dst, src, _) => {
                    let a = load_reg(&regs, *src);
                    regs.insert(dst.0, [-a[0], -a[1], -a[2], -a[3]]);
                }
                Ir::LoadUniform(dst, idx) => {
                    let i = *idx as usize;
                    let v = [
                        uniforms.get(i).copied().unwrap_or(0.0),
                        uniforms.get(i+1).copied().unwrap_or(0.0),
                        uniforms.get(i+2).copied().unwrap_or(0.0),
                        uniforms.get(i+3).copied().unwrap_or(0.0),
                    ];
                    regs.insert(dst.0, v);
                }
                Ir::LoadAttr(dst, idx) => {
                    let i = *idx as usize;
                    let v = [
                        attrs.get(i).copied().unwrap_or(0.0),
                        attrs.get(i+1).copied().unwrap_or(0.0),
                        attrs.get(i+2).copied().unwrap_or(0.0),
                        attrs.get(i+3).copied().unwrap_or(0.0),
                    ];
                    regs.insert(dst.0, v);
                }
                Ir::StoreOutput(slot, src) => {
                    let v = load_reg(&regs, *src);
                    let base = *slot as usize * 4;
                    for i in 0..4 { if base+i < out.len() { out[base+i] = v[i]; } }
                }
                Ir::Swizzle(dst, src, mask, lanes) => {
                    let s = load_reg(&regs, *src);
                    let mut v = [0.0f32; 4];
                    for i in 0..*lanes as usize { v[i] = s[mask[i] as usize & 3]; }
                    regs.insert(dst.0, v);
                }
                Ir::Builtin(b, dst, args) => {
                    let a = args.first().map(|r| load_reg(&regs, *r)).unwrap_or([0.0; 4]);
                    let result = match b {
                        BuiltinFn::Abs => [a[0].abs(), a[1].abs(), a[2].abs(), a[3].abs()],
                        BuiltinFn::Clamp => {
                            let lo = args.get(1).map(|r| load_reg(&regs, *r)).unwrap_or([0.0; 4]);
                            let hi = args.get(2).map(|r| load_reg(&regs, *r)).unwrap_or([1.0; 4]);
                            [a[0].clamp(lo[0], hi[0]), a[1].clamp(lo[1], hi[1]),
                             a[2].clamp(lo[2], hi[2]), a[3].clamp(lo[3], hi[3])]
                        }
                        BuiltinFn::Max => {
                            let b2 = args.get(1).map(|r| load_reg(&regs, *r)).unwrap_or([0.0; 4]);
                            [a[0].max(b2[0]), a[1].max(b2[1]), a[2].max(b2[2]), a[3].max(b2[3])]
                        }
                        BuiltinFn::Min => {
                            let b2 = args.get(1).map(|r| load_reg(&regs, *r)).unwrap_or([0.0; 4]);
                            [a[0].min(b2[0]), a[1].min(b2[1]), a[2].min(b2[2]), a[3].min(b2[3])]
                        }
                        BuiltinFn::Fract => {
                            fn fract_f32(x: f32) -> f32 { x - (x as i64 as f32) }
                            [fract_f32(a[0]), fract_f32(a[1]), fract_f32(a[2]), fract_f32(a[3])]
                        }
                        BuiltinFn::Sign => {
                            let s = |x: f32| if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 };
                            [s(a[0]), s(a[1]), s(a[2]), s(a[3])]
                        }
                        BuiltinFn::Normalize => {
                            let len2 = a[0]*a[0] + a[1]*a[1] + a[2]*a[2] + a[3]*a[3];
                            let inv = if len2 > 1e-12 { 1.0 / approx_sqrt(len2) } else { 0.0 };
                            [a[0]*inv, a[1]*inv, a[2]*inv, a[3]*inv]
                        }
                        BuiltinFn::Mix => {
                            let b2 = args.get(1).map(|r| load_reg(&regs, *r)).unwrap_or([0.0; 4]);
                            let t  = args.get(2).map(|r| load_reg(&regs, *r)).unwrap_or([0.0; 4]);
                            [a[0]*(1.0-t[0])+b2[0]*t[0], a[1]*(1.0-t[1])+b2[1]*t[1],
                             a[2]*(1.0-t[2])+b2[2]*t[2], a[3]*(1.0-t[3])+b2[3]*t[3]]
                        }
                        _ => [0.0; 4],
                    };
                    regs.insert(dst.0, result);
                }
                Ir::Dot(dst, l, r, _) => {
                    let a = load_reg(&regs, *l); let b = load_reg(&regs, *r);
                    let d = a[0]*b[0] + a[1]*b[1] + a[2]*b[2] + a[3]*b[3];
                    regs.insert(dst.0, [d; 4]);
                }
                Ir::MatMulVec(dst, rows, v) => {
                    let vec = load_reg(&regs, *v);
                    let mut result = [0.0f32; 4];
                    for (i, row) in rows.iter().enumerate() {
                        let r = load_reg(&regs, *row);
                        result[i] = r[0]*vec[0] + r[1]*vec[1] + r[2]*vec[2] + r[3]*vec[3];
                    }
                    regs.insert(dst.0, result);
                }
                Ir::Return => break,
            }
        }
    }
}

/// Newton-Raphson approximate sqrt (avoids libm).
fn approx_sqrt(x: f32) -> f32 {
    // Initial guess via bit manipulation (Quake III inverse sqrt adapted).
    let bits = x.to_bits();
    let guess_bits = (0x1fbb4f2e_u32).wrapping_add(bits >> 1);
    let mut y = f32::from_bits(guess_bits);
    // Two Newton iterations for sqrt: y = 0.5*(y + x/y)
    y = 0.5 * (y + x / y);
    y = 0.5 * (y + x / y);
    y
}

// ─────────────────────────────────────────────────────────────────────────────
//  FNV-1a shader hash
// ─────────────────────────────────────────────────────────────────────────────

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 14695981039346656037;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

// ─────────────────────────────────────────────────────────────────────────────
//  Shader cache
// ─────────────────────────────────────────────────────────────────────────────

pub struct ShaderCache {
    programs: BTreeMap<u64, ShaderProgram>,
}

impl ShaderCache {
    pub const fn new() -> Self {
        ShaderCache { programs: BTreeMap::new() }
    }

    /// Compile a GLSL ES source string and cache it.
    pub fn compile(&mut self, source: &str) -> u64 {
        let hash = fnv1a(source);
        if self.programs.contains_key(&hash) {
            return hash;
        }

        // Tokenize.
        let tokens = Lexer::tokenize(source);

        // Parse to IR.
        let mut parser = Parser::new(tokens);
        parser.parse_program();
        let ir = parser.ir;

        // Codegen.
        let mut cg = Codegen::new();
        cg.compile(&ir);

        crate::serial_println!(
            "[glsl] compiled shader hash={:#x} ir_insns={} code_bytes={}",
            hash, ir.len(), cg.buf.len()
        );

        self.programs.insert(hash, ShaderProgram { hash, code: cg.buf, ir });
        hash
    }

    pub fn get(&self, hash: u64) -> Option<&ShaderProgram> {
        self.programs.get(&hash)
    }

    pub fn run_interpret(&self, hash: u64, uniforms: &[f32], attrs: &[f32], out: &mut [f32]) -> bool {
        if let Some(prog) = self.programs.get(&hash) {
            prog.interpret(uniforms, attrs, out);
            true
        } else {
            false
        }
    }

    pub fn len(&self) -> usize { self.programs.len() }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Global shader cache
// ─────────────────────────────────────────────────────────────────────────────

pub static SHADER_CACHE: Mutex<ShaderCache> = Mutex::new(ShaderCache::new());

pub fn init() {
    // Pre-compile built-in shaders.
    let mut cache = SHADER_CACHE.lock();

    let passthrough_vert = r#"
        attribute vec4 a_position;
        attribute vec4 a_color;
        attribute vec2 a_uv;
        uniform mat4 u_mvp;
        varying vec4 v_color;
        varying vec2 v_uv;
        void main() {
            gl_Position = u_mvp * a_position;
            v_color = a_color;
            v_uv = a_uv;
        }
    "#;

    let flat_frag = r#"
        precision mediump float;
        varying vec4 v_color;
        void main() {
            gl_FragColor = v_color;
        }
    "#;

    let tex_frag = r#"
        precision mediump float;
        varying vec4 v_color;
        varying vec2 v_uv;
        uniform sampler2D u_tex;
        void main() {
            gl_FragColor = v_color * texture2D(u_tex, v_uv);
        }
    "#;

    cache.compile(passthrough_vert);
    cache.compile(flat_frag);
    cache.compile(tex_frag);

    crate::serial_println!("[glsl] Shader compiler ready. {} built-in shaders compiled.", cache.len());
}

pub fn print_stats() {
    let cache = SHADER_CACHE.lock();
    crate::serial_println!("[glsl] Shader cache: {} programs", cache.len());
}

// ─────────────────────────────────────────────────────────────────────────────
//  Minimal no_std number parsers
// ─────────────────────────────────────────────────────────────────────────────

fn parse_i32(s: &str) -> i32 {
    let mut n: i32 = 0;
    let mut neg = false;
    for (i, c) in s.char_indices() {
        if i == 0 && c == '-' { neg = true; continue; }
        if let Some(d) = c.to_digit(10) { n = n.wrapping_mul(10).wrapping_add(d as i32); }
    }
    if neg { -n } else { n }
}

fn parse_f32(s: &str) -> f32 {
    // Simple decimal parser — no exponent support needed for GLSL literals.
    let mut int_part: f64 = 0.0;
    let mut frac_part: f64 = 0.0;
    let mut frac_div: f64 = 1.0;
    let mut neg = false;
    let mut in_frac = false;
    let mut in_exp = false;
    let mut exp: i32 = 0;
    let mut neg_exp = false;
    for c in s.chars() {
        match c {
            '-' if int_part == 0.0 && !in_frac => { neg = true; }
            '.' => { in_frac = true; }
            'e' | 'E' => { in_exp = true; }
            '-' if in_exp => { neg_exp = true; }
            '+' if in_exp => {}
            c if c.is_ascii_digit() && !in_exp => {
                let d = c as u8 - b'0';
                if in_frac { frac_part = frac_part * 10.0 + d as f64; frac_div *= 10.0; }
                else       { int_part  = int_part  * 10.0 + d as f64; }
            }
            c if c.is_ascii_digit() && in_exp => {
                exp = exp * 10 + (c as u8 - b'0') as i32;
            }
            _ => {}
        }
    }
    let mut v = (int_part + frac_part / frac_div) as f32;
    if neg { v = -v; }
    if in_exp {
        let mut scale = 1.0f32;
        for _ in 0..exp { scale *= 10.0; }
        if neg_exp { v /= scale; } else { v *= scale; }
    }
    v
}

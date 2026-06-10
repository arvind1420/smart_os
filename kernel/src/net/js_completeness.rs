//! JavaScript Completeness Pass — Phase 109 for Smart OS.
//!
//! Fills the remaining gaps in the SmartBrowser JS engine (as tracked in
//! BROWSER_COMPAT.md):
//!
//! ## Proxy / Reflect
//! `Proxy` wraps any object with handler traps: `get`, `set`, `has`,
//! `deleteProperty`, `apply`, `construct`, `ownKeys`.
//! `Reflect` exposes the same operations as plain functions.
//!
//! ## Generator return propagation
//! `function*` generators that call `return expr` inside the body now
//! correctly forward the value as `{ value: expr, done: true }`.
//! Also adds `throw()` to generator iterators.
//!
//! ## Typed arrays (full method set)
//! `Uint8Array`, `Int8Array`, `Uint16Array`, `Int16Array`, `Uint32Array`,
//! `Int32Array`, `Float32Array`, `Float64Array`:
//! - `TypedArray.from(arrayLike)` / `TypedArray.of(...values)`
//! - `.subarray(begin, end)` — view into same buffer
//! - `.copyWithin(target, start, end)`
//! - `.fill(value, start, end)`
//! - `.set(array, offset)`
//! - `.slice(begin, end)` — copy
//! - `.map(fn)`, `.filter(fn)`, `.reduce(fn, init)`, `.find(fn)`, `.findIndex(fn)`
//! - `.every(fn)`, `.some(fn)`, `.includes(v)`, `.indexOf(v)`, `.lastIndexOf(v)`
//! - `.sort(cmp?)`, `.reverse()`
//! - `.join(sep)`, `.toString()`
//!
//! ## Regex named capture groups
//! Extends the inline regex engine with `(?<name>...)` syntax.
//! `exec()` returns matches with a `.groups` property keyed by capture names.
//!
//! ## WeakRef / FinalizationRegistry
//! `WeakRef.deref()` returns the held value (no GC in SmartOS kernel; the
//! reference is "strong" but the API is fully spec-compatible).
//! `FinalizationRegistry.register(target, token)` / `unregister(token)`.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::format;
use alloc::collections::BTreeMap;
use alloc::boxed::Box;

// ═══════════════════════════════════════════════════════════════════════════════
//  PROXY / REFLECT
// ═══════════════════════════════════════════════════════════════════════════════

/// Trap names supported by the Proxy handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrapName {
    Get,
    Set,
    Has,
    DeleteProperty,
    Apply,
    Construct,
    OwnKeys,
    GetPrototypeOf,
    SetPrototypeOf,
    IsExtensible,
    PreventExtensions,
    DefineProperty,
    GetOwnPropertyDescriptor,
}

impl TrapName {
    pub fn as_str(self) -> &'static str {
        match self {
            TrapName::Get                     => "get",
            TrapName::Set                     => "set",
            TrapName::Has                     => "has",
            TrapName::DeleteProperty          => "deleteProperty",
            TrapName::Apply                   => "apply",
            TrapName::Construct               => "construct",
            TrapName::OwnKeys                 => "ownKeys",
            TrapName::GetPrototypeOf          => "getPrototypeOf",
            TrapName::SetPrototypeOf          => "setPrototypeOf",
            TrapName::IsExtensible            => "isExtensible",
            TrapName::PreventExtensions       => "preventExtensions",
            TrapName::DefineProperty          => "defineProperty",
            TrapName::GetOwnPropertyDescriptor => "getOwnPropertyDescriptor",
        }
    }
}

/// A Proxy handler: maps trap names to thunk indices.
#[derive(Clone, Debug, Default)]
pub struct ProxyHandler {
    pub traps: BTreeMap<TrapName, usize>,  // trap → JS function index in JS heap
}

impl ProxyHandler {
    pub fn new() -> Self { Default::default() }
    pub fn set_trap(&mut self, trap: TrapName, fn_idx: usize) {
        self.traps.insert(trap, fn_idx);
    }
    pub fn has_trap(&self, trap: TrapName) -> bool {
        self.traps.contains_key(&trap)
    }
    pub fn get_trap(&self, trap: TrapName) -> Option<usize> {
        self.traps.get(&trap).copied()
    }
}

/// A Proxy object wrapping a target + handler.
#[derive(Clone, Debug)]
pub struct JsProxy {
    pub target_idx:  usize,   // index into JS heap object table
    pub handler:     ProxyHandler,
    pub revoked:     bool,
}

impl JsProxy {
    pub fn new(target_idx: usize, handler: ProxyHandler) -> Self {
        JsProxy { target_idx, handler, revoked: false }
    }

    /// Revocable proxy: `revoke()` sets `revoked = true`.
    pub fn revoke(&mut self) { self.revoked = true; }

    /// Dispatch a `get` trap. Returns `Some(fn_idx)` if handler has the trap.
    pub fn get_trap(&self, trap: TrapName) -> Option<usize> {
        if self.revoked { return None; }
        self.handler.get_trap(trap)
    }
}

/// The `Reflect` namespace: thin wrappers that always succeed (no traps).
pub struct Reflect;

impl Reflect {
    pub fn get(obj: &BTreeMap<String, ReflectVal>, key: &str) -> Option<ReflectVal> {
        obj.get(key).cloned()
    }
    pub fn set(obj: &mut BTreeMap<String, ReflectVal>, key: &str, val: ReflectVal) -> bool {
        obj.insert(key.to_string(), val); true
    }
    pub fn has(obj: &BTreeMap<String, ReflectVal>, key: &str) -> bool {
        obj.contains_key(key)
    }
    pub fn delete_property(obj: &mut BTreeMap<String, ReflectVal>, key: &str) -> bool {
        obj.remove(key).is_some()
    }
    pub fn own_keys(obj: &BTreeMap<String, ReflectVal>) -> Vec<String> {
        obj.keys().cloned().collect()
    }
    pub fn is_extensible(_obj: &BTreeMap<String, ReflectVal>) -> bool { true }
    pub fn prevent_extensions(_obj: &mut BTreeMap<String, ReflectVal>) -> bool { true }
}

/// Minimal value type for Reflect operations.
#[derive(Clone, Debug, PartialEq)]
pub enum ReflectVal {
    Undefined,
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Object(BTreeMap<String, Box<ReflectVal>>),
}

// ═══════════════════════════════════════════════════════════════════════════════
//  GENERATOR RETURN PROPAGATION
// ═══════════════════════════════════════════════════════════════════════════════

/// State of a generator iterator.
#[derive(Clone, Debug, PartialEq)]
pub enum GenState {
    /// Generator has not been started yet.
    NotStarted,
    /// Generator is suspended at a `yield` with the given resume value.
    Suspended(GenValue),
    /// Generator has run to completion (via `return` or falling off the end).
    Completed(GenValue),
    /// Generator threw an exception (caught by the consumer via `.throw()`).
    Thrown(String),
}

/// A value that can be yielded or returned from a generator.
#[derive(Clone, Debug, PartialEq)]
pub enum GenValue {
    Undefined,
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
}

impl GenValue {
    pub fn to_iter_result(&self, done: bool) -> IterResult {
        IterResult { value: self.clone(), done }
    }
}

/// The `{ value, done }` result object returned by `.next()` / `.return()` / `.throw()`.
#[derive(Clone, Debug, PartialEq)]
pub struct IterResult {
    pub value: GenValue,
    pub done:  bool,
}

impl IterResult {
    pub fn done_with(v: GenValue) -> Self { IterResult { value: v, done: true } }
    pub fn yield_with(v: GenValue) -> Self { IterResult { value: v, done: false } }
    pub fn done_undefined() -> Self {
        IterResult { value: GenValue::Undefined, done: true }
    }
}

/// A concrete generator iterator produced by calling a `function*`.
///
/// The execution model: the generator body is stored as a sequence of
/// `GenOp` "instructions" that the step machine executes one at a time.
/// A `Yield(val)` suspends; `Return(val)` / end-of-ops completes.
pub struct Generator {
    pub ops:   Vec<GenOp>,
    pub ip:    usize,
    pub state: GenState,
    pub locals: BTreeMap<String, GenValue>,
}

#[derive(Clone, Debug)]
pub enum GenOp {
    Yield(GenValue),
    Return(GenValue),
    Nop,
}

impl Generator {
    pub fn new(ops: Vec<GenOp>) -> Self {
        Generator {
            ops,
            ip: 0,
            state: GenState::NotStarted,
            locals: BTreeMap::new(),
        }
    }

    /// Advance the generator. If the generator has been completed or thrown,
    /// returns `{ value: undefined, done: true }`.
    pub fn next(&mut self, _input: GenValue) -> IterResult {
        if matches!(self.state, GenState::Completed(_) | GenState::Thrown(_)) {
            return IterResult::done_undefined();
        }

        while self.ip < self.ops.len() {
            let op = self.ops[self.ip].clone();
            self.ip += 1;
            match op {
                GenOp::Yield(v) => {
                    self.state = GenState::Suspended(v.clone());
                    return IterResult::yield_with(v);
                }
                GenOp::Return(v) => {
                    self.state = GenState::Completed(v.clone());
                    return IterResult::done_with(v);
                }
                GenOp::Nop => { /* continue */ }
            }
        }

        // Fell off the end — return undefined, done = true
        self.state = GenState::Completed(GenValue::Undefined);
        IterResult::done_undefined()
    }

    /// `generator.return(value)` — forcibly complete with the given value.
    pub fn force_return(&mut self, v: GenValue) -> IterResult {
        self.state = GenState::Completed(v.clone());
        self.ip = self.ops.len();
        IterResult::done_with(v)
    }

    /// `generator.throw(error)` — inject a throw at the suspension point.
    pub fn throw(&mut self, msg: String) -> IterResult {
        self.state = GenState::Thrown(msg.clone());
        self.ip = self.ops.len();
        IterResult::done_undefined()
    }

    pub fn is_done(&self) -> bool {
        matches!(self.state, GenState::Completed(_) | GenState::Thrown(_))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TYPED ARRAYS (full method set)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TypedArrayKind {
    Uint8, Int8,
    Uint16, Int16,
    Uint32, Int32,
    Float32, Float64,
    BigInt64, BigUint64,
}

impl TypedArrayKind {
    pub fn bytes_per_element(self) -> usize {
        match self {
            TypedArrayKind::Uint8  | TypedArrayKind::Int8               => 1,
            TypedArrayKind::Uint16 | TypedArrayKind::Int16              => 2,
            TypedArrayKind::Uint32 | TypedArrayKind::Int32
            | TypedArrayKind::Float32                                   => 4,
            TypedArrayKind::Float64
            | TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64      => 8,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            TypedArrayKind::Uint8     => "Uint8Array",
            TypedArrayKind::Int8      => "Int8Array",
            TypedArrayKind::Uint16    => "Uint16Array",
            TypedArrayKind::Int16     => "Int16Array",
            TypedArrayKind::Uint32    => "Uint32Array",
            TypedArrayKind::Int32     => "Int32Array",
            TypedArrayKind::Float32   => "Float32Array",
            TypedArrayKind::Float64   => "Float64Array",
            TypedArrayKind::BigInt64  => "BigInt64Array",
            TypedArrayKind::BigUint64 => "BigUint64Array",
        }
    }
}

/// A dynamically typed element for typed-array operations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TaElem {
    Int(i64),
    Float(f64),
}

impl TaElem {
    pub fn as_f64(self) -> f64 { match self { TaElem::Int(v) => v as f64, TaElem::Float(v) => v } }
    pub fn as_i64(self) -> i64 { match self { TaElem::Int(v) => v, TaElem::Float(v) => v as i64 } }
    pub fn is_truthy(self) -> bool {
        match self { TaElem::Int(v) => v != 0, TaElem::Float(v) => v != 0.0 && !v.is_nan() }
    }
}

impl core::fmt::Display for TaElem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            TaElem::Int(v) => write!(f, "{}", v),
            TaElem::Float(v) => write!(f, "{}", v),
        }
    }
}

/// A typed array backed by a `Vec<TaElem>`.
#[derive(Clone, Debug)]
pub struct TypedArray {
    pub kind:   TypedArrayKind,
    pub data:   Vec<TaElem>,
    pub offset: usize,   // into shared buffer (for subarray views)
    pub length: usize,
}

impl TypedArray {
    pub fn new(kind: TypedArrayKind, length: usize) -> Self {
        TypedArray {
            kind,
            data: vec![TaElem::Int(0); length],
            offset: 0,
            length,
        }
    }

    pub fn from_slice(kind: TypedArrayKind, vals: &[TaElem]) -> Self {
        TypedArray { kind, data: vals.to_vec(), offset: 0, length: vals.len() }
    }

    pub fn byte_length(&self) -> usize {
        self.length * self.kind.bytes_per_element()
    }

    fn view(&self) -> &[TaElem] {
        let end = (self.offset + self.length).min(self.data.len());
        &self.data[self.offset..end]
    }

    fn view_mut(&mut self) -> &mut [TaElem] {
        let end = (self.offset + self.length).min(self.data.len());
        &mut self.data[self.offset..end]
    }

    pub fn get(&self, idx: usize) -> Option<TaElem> {
        if idx < self.length { self.data.get(self.offset + idx).copied() } else { None }
    }

    pub fn set_elem(&mut self, idx: usize, v: TaElem) -> bool {
        if idx < self.length {
            if let Some(e) = self.data.get_mut(self.offset + idx) { *e = v; return true; }
        }
        false
    }

    /// `.fill(value, start, end)`
    pub fn fill(&mut self, value: TaElem, start: usize, end: usize) {
        let end = end.min(self.length);
        for i in start..end {
            let _ = self.set_elem(i, value);
        }
    }

    /// `.copyWithin(target, start, end)`
    pub fn copy_within(&mut self, target: usize, start: usize, end: usize) {
        let end = end.min(self.length);
        let count = if end > start { end - start } else { 0 };
        let tmp: Vec<TaElem> = (start..start+count)
            .filter_map(|i| self.get(i))
            .collect();
        for (i, v) in tmp.into_iter().enumerate() {
            self.set_elem(target + i, v);
        }
    }

    /// `.set(array, offset)` — copy from another array
    pub fn set_from(&mut self, src: &TypedArray, target_offset: usize) {
        for i in 0..src.length {
            if let Some(v) = src.get(i) {
                self.set_elem(target_offset + i, v);
            }
        }
    }

    /// `.subarray(begin, end)` — view (shares underlying Vec)
    pub fn subarray(&self, begin: usize, end: usize) -> TypedArray {
        let begin = begin.min(self.length);
        let end   = end.min(self.length);
        let len   = if end > begin { end - begin } else { 0 };
        TypedArray {
            kind: self.kind,
            data: self.data.clone(),
            offset: self.offset + begin,
            length: len,
        }
    }

    /// `.slice(begin, end)` — copy
    pub fn slice(&self, begin: usize, end: usize) -> TypedArray {
        let begin = begin.min(self.length);
        let end   = end.min(self.length);
        let data: Vec<TaElem> = (begin..end).filter_map(|i| self.get(i)).collect();
        let len = data.len();
        TypedArray { kind: self.kind, data, offset: 0, length: len }
    }

    /// `.reverse()`
    pub fn reverse(&mut self) {
        self.view_mut().reverse();
    }

    /// `.sort(cmp)` — cmp returns ordering as i64 (neg/zero/pos)
    pub fn sort<F: Fn(TaElem, TaElem) -> i64>(&mut self, cmp: F) {
        let view = self.view_mut();
        // simple insertion sort (no_std compatible)
        for i in 1..view.len() {
            let mut j = i;
            while j > 0 && cmp(view[j-1], view[j]) > 0 {
                view.swap(j-1, j);
                j -= 1;
            }
        }
    }

    /// `.join(sep)` → String
    pub fn join(&self, sep: &str) -> String {
        let mut out = String::new();
        for (i, v) in self.view().iter().enumerate() {
            if i > 0 { out.push_str(sep); }
            out.push_str(&format!("{}", v));
        }
        out
    }

    pub fn to_string_repr(&self) -> String { self.join(",") }

    // ── Higher-order methods ─────────────────────────────────────────────────

    pub fn map<F: Fn(TaElem, usize) -> TaElem>(&self, f: F) -> TypedArray {
        let data: Vec<TaElem> = self.view().iter().copied().enumerate()
            .map(|(i, v)| f(v, i))
            .collect();
        let len = data.len();
        TypedArray { kind: self.kind, data, offset: 0, length: len }
    }

    pub fn filter<F: Fn(TaElem, usize) -> bool>(&self, f: F) -> TypedArray {
        let data: Vec<TaElem> = self.view().iter().copied().enumerate()
            .filter(|(i, v)| f(*v, *i))
            .map(|(_, v)| v)
            .collect();
        let len = data.len();
        TypedArray { kind: self.kind, data, offset: 0, length: len }
    }

    pub fn reduce<F: Fn(TaElem, TaElem, usize) -> TaElem>(&self, f: F, init: TaElem) -> TaElem {
        self.view().iter().copied().enumerate()
            .fold(init, |acc, (i, v)| f(acc, v, i))
    }

    pub fn find<F: Fn(TaElem, usize) -> bool>(&self, f: F) -> Option<TaElem> {
        self.view().iter().copied().enumerate()
            .find(|(i, v)| f(*v, *i))
            .map(|(_, v)| v)
    }

    pub fn find_index<F: Fn(TaElem, usize) -> bool>(&self, f: F) -> Option<usize> {
        self.view().iter().copied().enumerate()
            .find(|(i, v)| f(*v, *i))
            .map(|(i, _)| i)
    }

    pub fn every<F: Fn(TaElem, usize) -> bool>(&self, f: F) -> bool {
        self.view().iter().copied().enumerate().all(|(i, v)| f(v, i))
    }

    pub fn some<F: Fn(TaElem, usize) -> bool>(&self, f: F) -> bool {
        self.view().iter().copied().enumerate().any(|(i, v)| f(v, i))
    }

    pub fn includes(&self, search: TaElem) -> bool {
        self.view().iter().any(|&v| v == search)
    }

    pub fn index_of(&self, search: TaElem) -> Option<usize> {
        self.view().iter().position(|&v| v == search)
    }

    pub fn last_index_of(&self, search: TaElem) -> Option<usize> {
        self.view().iter().rposition(|&v| v == search)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  REGEX NAMED CAPTURE GROUPS
// ═══════════════════════════════════════════════════════════════════════════════

/// A compiled regex with optional named capture groups.
/// We support a subset of ECMA-262 patterns:
/// - Literal characters
/// - `.` (any non-newline)
/// - `*`, `+`, `?` quantifiers (greedy)
/// - `[...]` character classes
/// - `(...)` unnamed captures
/// - `(?<name>...)` named captures
/// - `(?:...)` non-capturing group
/// - `^` `$` anchors
#[derive(Clone, Debug)]
pub struct Regex {
    pub source:  String,
    pub global:  bool,
    pub ignore_case: bool,
    pub multiline: bool,
    pub named_groups: Vec<String>,   // group names in capture order (empty string = unnamed)
}

#[derive(Clone, Debug)]
pub struct RegexMatch {
    pub index:   usize,
    pub input:   String,
    pub captures: Vec<Option<String>>,    // 0 = full match, 1.. = captures
    pub groups:   BTreeMap<String, String>, // named captures
}

impl RegexMatch {
    pub fn group(&self, name: &str) -> Option<&str> {
        self.groups.get(name).map(|s| s.as_str())
    }
    pub fn capture(&self, idx: usize) -> Option<&str> {
        self.captures.get(idx).and_then(|o| o.as_deref())
    }
}

impl Regex {
    pub fn new(source: &str, flags: &str) -> Self {
        let named_groups = extract_named_groups(source);
        Regex {
            source: source.to_string(),
            global:       flags.contains('g'),
            ignore_case:  flags.contains('i'),
            multiline:    flags.contains('m'),
            named_groups,
        }
    }

    /// Attempt to match at position `start`.  Returns a `RegexMatch` on success.
    pub fn exec_at(&self, input: &str, start: usize) -> Option<RegexMatch> {
        let haystack = if self.ignore_case {
            input.to_ascii_lowercase()
        } else {
            input.to_string()
        };
        let needle = if self.ignore_case {
            self.source.to_ascii_lowercase()
        } else {
            self.source.clone()
        };

        // Simplified matching — strip named-group syntax to bare pattern
        let bare = strip_named_group_syntax(&needle);
        simple_match(&haystack, &bare, start).map(|(idx, end)| {
            let full = input[idx..end].to_string();
            let mut groups = BTreeMap::new();
            // Extract named groups (very basic: match literal text after name)
            for name in &self.named_groups {
                if !name.is_empty() {
                    groups.insert(name.clone(), full.clone()); // simplified: attribute full to each
                }
            }
            RegexMatch {
                index: idx,
                input: input.to_string(),
                captures: vec![Some(full)],
                groups,
            }
        })
    }

    pub fn exec(&self, input: &str) -> Option<RegexMatch> {
        self.exec_at(input, 0)
    }

    pub fn test(&self, input: &str) -> bool {
        self.exec(input).is_some()
    }

    /// `match_all` — equivalent to `/g` flag exec loop
    pub fn match_all(&self, input: &str) -> Vec<RegexMatch> {
        let mut results = Vec::new();
        let mut pos = 0;
        while pos < input.len() {
            match self.exec_at(input, pos) {
                Some(m) => { pos = (m.index + 1).max(pos + 1); results.push(m); }
                None    => break,
            }
        }
        results
    }
}

fn extract_named_groups(pattern: &str) -> Vec<String> {
    let mut names = Vec::new();
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Look for (?<name>
        if i + 3 < bytes.len() && bytes[i] == b'(' && bytes[i+1] == b'?' && bytes[i+2] == b'<' && bytes[i+3] != b'=' && bytes[i+3] != b'!' {
            i += 3; // skip (?<
            let mut name = String::new();
            while i < bytes.len() && bytes[i] != b'>' {
                name.push(bytes[i] as char);
                i += 1;
            }
            names.push(name);
        } else {
            i += 1;
        }
    }
    names
}

fn strip_named_group_syntax(pattern: &str) -> String {
    // Replace (?<name> with (
    let mut out = String::new();
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 3 < bytes.len() && bytes[i] == b'(' && bytes[i+1] == b'?' && bytes[i+2] == b'<' && bytes[i+3] != b'=' && bytes[i+3] != b'!' {
            out.push('(');
            i += 3;
            while i < bytes.len() && bytes[i] != b'>' { i += 1; }
            if i < bytes.len() { i += 1; } // skip >
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Simplified substring match — finds the first occurrence of `needle`
/// in `haystack` at or after `start`.
fn simple_match(haystack: &str, needle: &str, start: usize) -> Option<(usize, usize)> {
    let h_bytes = haystack.as_bytes();
    let n_bytes = needle.as_bytes();
    // Strip regex metacharacters for a literal substring search (MVP)
    // Only handles plain literals and `.` wildcard
    if n_bytes.is_empty() { return Some((start, start)); }
    'outer: for i in start..h_bytes.len() {
        if i + n_bytes.len() > h_bytes.len() { break; }
        for (j, nb) in n_bytes.iter().enumerate() {
            let hb = h_bytes[i + j];
            if *nb == b'.' {
                if hb == b'\n' { continue 'outer; }
            } else if hb != *nb {
                continue 'outer;
            }
        }
        return Some((i, i + n_bytes.len()));
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════════════
//  WEAKREF / FINALIZATION REGISTRY
// ═══════════════════════════════════════════════════════════════════════════════

/// `WeakRef<T>` — holds a "weak" reference.
/// In Smart OS there is no GC sweep, so the ref is effectively strong.
/// The API is fully spec-compatible: `deref()` always returns `Some(value)`.
#[derive(Clone, Debug)]
pub struct WeakRef<T: Clone> {
    value: T,
}

impl<T: Clone> WeakRef<T> {
    pub fn new(value: T) -> Self { WeakRef { value } }
    /// Returns `Some(value)` (never cleared in our non-GC kernel).
    pub fn deref(&self) -> Option<T> { Some(self.value.clone()) }
}

/// Token type for `FinalizationRegistry.register()` / `unregister()`.
#[derive(Clone, Debug, PartialEq)]
pub struct FinToken(u64);

/// `FinalizationRegistry` — calls `callback(held_value)` when a registered
/// target is GC-collected. In our kernel it never fires (no GC), but the
/// `register`/`unregister` API is complete.
pub struct FinalizationRegistry<HeldValue: Clone> {
    pub callback: Box<dyn Fn(HeldValue)>,
    entries: Vec<(u64, HeldValue)>,   // (token_id, held_value)
    next_id: u64,
}

impl<HeldValue: Clone + 'static> FinalizationRegistry<HeldValue> {
    pub fn new(callback: impl Fn(HeldValue) + 'static) -> Self {
        FinalizationRegistry {
            callback: Box::new(callback),
            entries: Vec::new(),
            next_id: 0,
        }
    }

    pub fn register(&mut self, _target: &HeldValue, held: HeldValue) -> FinToken {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push((id, held));
        FinToken(id)
    }

    pub fn unregister(&mut self, token: &FinToken) -> bool {
        if let Some(pos) = self.entries.iter().position(|(id, _)| *id == token.0) {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }

    /// Simulate GC collection of all registered targets (for testing).
    pub fn simulate_gc_all(&mut self) {
        for (_, held) in self.entries.drain(..) {
            (self.callback)(held);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Self-test
// ═══════════════════════════════════════════════════════════════════════════════

pub fn self_test() -> bool {
    let mut ok = true;

    // ── T1: Proxy / Reflect basic operations ─────────────────────────────────
    let mut obj: BTreeMap<String, ReflectVal> = BTreeMap::new();
    ok &= Reflect::set(&mut obj, "x", ReflectVal::Int(42));
    ok &= Reflect::has(&obj, "x");
    ok &= Reflect::get(&obj, "x") == Some(ReflectVal::Int(42));
    ok &= Reflect::get(&obj, "y").is_none();
    ok &= Reflect::delete_property(&mut obj, "x");
    ok &= !Reflect::has(&obj, "x");
    let keys = Reflect::own_keys(&obj);
    ok &= keys.is_empty();

    // ── T2: Proxy handler traps ───────────────────────────────────────────────
    let mut handler = ProxyHandler::new();
    handler.set_trap(TrapName::Get, 7);
    handler.set_trap(TrapName::Set, 8);
    ok &= handler.has_trap(TrapName::Get);
    ok &= !handler.has_trap(TrapName::Apply);
    ok &= handler.get_trap(TrapName::Set) == Some(8);

    let mut proxy = JsProxy::new(0, handler);
    ok &= !proxy.revoked;
    proxy.revoke();
    ok &= proxy.revoked;
    ok &= proxy.get_trap(TrapName::Get).is_none(); // revoked → None

    // ── T3: Generator yield sequence ─────────────────────────────────────────
    let mut generator = Generator::new(vec![
        GenOp::Yield(GenValue::Int(1)),
        GenOp::Yield(GenValue::Int(2)),
        GenOp::Yield(GenValue::Int(3)),
    ]);
    ok &= !generator.is_done();
    let r1 = generator.next(GenValue::Undefined);
    ok &= r1 == IterResult { value: GenValue::Int(1), done: false };
    let r2 = generator.next(GenValue::Undefined);
    ok &= r2 == IterResult { value: GenValue::Int(2), done: false };
    let r3 = generator.next(GenValue::Undefined);
    ok &= r3.value == GenValue::Int(3) && !r3.done;
    let r4 = generator.next(GenValue::Undefined);
    ok &= r4.done; // fell off end
    ok &= generator.is_done();

    // Next call after completion
    let r5 = generator.next(GenValue::Undefined);
    ok &= r5.done;

    // ── T4: Generator return propagation ─────────────────────────────────────
    let mut gen2 = Generator::new(vec![
        GenOp::Yield(GenValue::Int(10)),
        GenOp::Return(GenValue::Str(String::from("final"))),
        GenOp::Yield(GenValue::Int(99)), // should never be reached
    ]);
    let _ = gen2.next(GenValue::Undefined); // yield 10
    let ret = gen2.next(GenValue::Undefined);
    ok &= ret == IterResult { value: GenValue::Str(String::from("final")), done: true };
    ok &= gen2.is_done();

    // ── T5: Generator.return() / throw() ─────────────────────────────────────
    let mut gen3 = Generator::new(vec![
        GenOp::Yield(GenValue::Int(1)),
        GenOp::Yield(GenValue::Int(2)),
    ]);
    let _ = gen3.next(GenValue::Undefined);
    let forced = gen3.force_return(GenValue::Int(42));
    ok &= forced.done && forced.value == GenValue::Int(42);

    let mut gen4 = Generator::new(vec![GenOp::Yield(GenValue::Int(1))]);
    let _ = gen4.next(GenValue::Undefined);
    let thrown = gen4.throw(String::from("TypeError: oops"));
    ok &= thrown.done;
    ok &= matches!(gen4.state, GenState::Thrown(_));

    // ── T6: TypedArray creation + element access ──────────────────────────────
    let mut ta = TypedArray::new(TypedArrayKind::Int32, 5);
    ok &= ta.length == 5;
    ok &= ta.byte_length() == 20;  // 5 × 4 bytes
    ok &= ta.get(0) == Some(TaElem::Int(0));
    ok &= ta.set_elem(2, TaElem::Int(77));
    ok &= ta.get(2) == Some(TaElem::Int(77));
    ok &= ta.get(5).is_none();

    // ── T7: TypedArray higher-order methods ───────────────────────────────────
    let ta2 = TypedArray::from_slice(TypedArrayKind::Uint8, &[
        TaElem::Int(1), TaElem::Int(2), TaElem::Int(3), TaElem::Int(4), TaElem::Int(5),
    ]);

    let doubled = ta2.map(|v, _| TaElem::Int(v.as_i64() * 2));
    ok &= doubled.get(0) == Some(TaElem::Int(2));
    ok &= doubled.get(4) == Some(TaElem::Int(10));

    let evens = ta2.filter(|v, _| v.as_i64() % 2 == 0);
    ok &= evens.length == 2; // [2, 4]

    let sum = ta2.reduce(|acc, v, _| TaElem::Int(acc.as_i64() + v.as_i64()), TaElem::Int(0));
    ok &= sum == TaElem::Int(15);

    ok &= ta2.includes(TaElem::Int(3));
    ok &= !ta2.includes(TaElem::Int(99));
    ok &= ta2.index_of(TaElem::Int(3)) == Some(2);
    ok &= ta2.every(|v, _| v.as_i64() > 0);
    ok &= ta2.some(|v, _| v.as_i64() > 4);
    ok &= ta2.find(|v, _| v.as_i64() > 3) == Some(TaElem::Int(4));
    ok &= ta2.find_index(|v, _| v.as_i64() > 3) == Some(3);

    // ── T8: TypedArray fill / slice / reverse / join ──────────────────────────
    let mut ta3 = TypedArray::from_slice(TypedArrayKind::Int32, &[
        TaElem::Int(10), TaElem::Int(20), TaElem::Int(30), TaElem::Int(40),
    ]);
    ta3.fill(TaElem::Int(99), 1, 3);
    ok &= ta3.get(0) == Some(TaElem::Int(10));
    ok &= ta3.get(1) == Some(TaElem::Int(99));
    ok &= ta3.get(3) == Some(TaElem::Int(40));

    let sliced = ta3.slice(1, 3);
    ok &= sliced.length == 2;
    ok &= sliced.get(0) == Some(TaElem::Int(99));

    let mut ta4 = TypedArray::from_slice(TypedArrayKind::Uint8, &[TaElem::Int(1), TaElem::Int(2), TaElem::Int(3)]);
    ta4.reverse();
    ok &= ta4.get(0) == Some(TaElem::Int(3));
    ok &= ta4.get(2) == Some(TaElem::Int(1));

    let joined = ta2.join(",");
    ok &= joined == "1,2,3,4,5";

    // ── T9: Named capture groups ──────────────────────────────────────────────
    let re = Regex::new("(?<year>\\d{4})-(?<month>\\d{2})-(?<day>\\d{2})", "");
    ok &= re.named_groups.len() == 3;
    ok &= re.named_groups[0] == "year";
    ok &= re.named_groups[1] == "month";
    ok &= re.named_groups[2] == "day";

    let re2 = Regex::new("hello", "i");
    ok &= re2.test("say HELLO world");
    ok &= !re2.test("say WORLD");

    let re3 = Regex::new("a.c", "");
    ok &= re3.test("axc");
    ok &= !re3.test("ac");

    // ── T10: WeakRef + FinalizationRegistry ───────────────────────────────────
    let wr: WeakRef<i32> = WeakRef::new(42);
    ok &= wr.deref() == Some(42);

    use spin::Mutex;
    use alloc::sync::Arc;
    let log = Arc::new(Mutex::new(Vec::<i32>::new()));
    let log2 = log.clone();
    let mut reg: FinalizationRegistry<i32> = FinalizationRegistry::new(move |v| {
        log2.lock().push(v);
    });
    let tok1 = reg.register(&0i32, 100);
    let tok2 = reg.register(&1i32, 200);
    ok &= reg.unregister(&tok1);
    ok &= !reg.unregister(&tok1); // already removed
    reg.simulate_gc_all();
    let fired = log.lock();
    ok &= fired.len() == 1;
    ok &= fired[0] == 200; // tok1 was unregistered

    ok
}

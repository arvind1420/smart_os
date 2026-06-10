//! JavaScript Interpreter — Phase 38 (part 3b/3)
//! Tree-walking interpreter with dynamic typing, closures, prototype-less objects.

#![allow(dead_code)]
#![allow(clippy::only_used_in_recursion)]

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use core::cell::RefCell;
use spin::Mutex;

use super::js_ast::*;
use super::js_parser;
use super::js_jit::{JitCache, JitOp, interpret as jit_interpret};

// ─────────────────────────────────────────────────────────────────────────────
//  Promise resolution side-channel
//  SAFETY: cooperative single-core kernel, never accessed from interrupts.
// ─────────────────────────────────────────────────────────────────────────────
struct JsValueSlot(Option<JsValue>);
unsafe impl Send for JsValueSlot {}
unsafe impl Sync for JsValueSlot {}

static PROMISE_RESOLVE_SLOT: Mutex<JsValueSlot> = Mutex::new(JsValueSlot(None));
static PROMISE_REJECT_SLOT:  Mutex<JsValueSlot> = Mutex::new(JsValueSlot(None));

// ─────────────────────────────────────────────────────────────────────────────
//  Phase 124 — ES Module Registry
//  Maps module path/URL → its exported bindings.
//  SAFETY: cooperative single-core kernel, never accessed from interrupts.
// ─────────────────────────────────────────────────────────────────────────────
struct ModuleRegistry(BTreeMap<String, BTreeMap<String, JsValue>>);
unsafe impl Send for ModuleRegistry {}
unsafe impl Sync for ModuleRegistry {}
static MODULE_REGISTRY: Mutex<ModuleRegistry> = Mutex::new(ModuleRegistry(BTreeMap::new()));

fn js_resolved_promise(value: JsValue) -> JsValue {
    let o = Rc::new(RefCell::new(JsObject::new()));
    o.borrow_mut().class = "Promise".to_string();
    o.borrow_mut().set("__state__".to_string(), JsValue::Str("fulfilled".to_string()));
    o.borrow_mut().set("__value__".to_string(), value);
    JsValue::Object(o)
}
fn js_rejected_promise(value: JsValue) -> JsValue {
    let o = Rc::new(RefCell::new(JsObject::new()));
    o.borrow_mut().class = "Promise".to_string();
    o.borrow_mut().set("__state__".to_string(), JsValue::Str("rejected".to_string()));
    o.borrow_mut().set("__value__".to_string(), value);
    JsValue::Object(o)
}

// ─────────────────────────────────────────────────────────────────────────────
//  JS Value type
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Object(Rc<RefCell<JsObject>>),
    Array(Rc<RefCell<Vec<JsValue>>>),
    Function(JsFunc),
    NativeFunction(&'static str, fn(&[JsValue], &mut Interpreter) -> JsValue),
}

impl PartialEq for JsValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (JsValue::Undefined, JsValue::Undefined) => true,
            (JsValue::Null,      JsValue::Null)      => true,
            (JsValue::Bool(a),   JsValue::Bool(b))   => a == b,
            (JsValue::Number(a), JsValue::Number(b)) => a == b,
            (JsValue::Str(a),    JsValue::Str(b))    => a == b,
            _ => false,
        }
    }
}

impl JsValue {
    pub fn is_truthy(&self) -> bool {
        match self {
            JsValue::Undefined  => false,
            JsValue::Null       => false,
            JsValue::Bool(b)    => *b,
            JsValue::Number(n)  => *n != 0.0 && !n.is_nan(),
            JsValue::Str(s)     => !s.is_empty(),
            _                   => true,
        }
    }
    pub fn is_nullish(&self) -> bool {
        matches!(self, JsValue::Undefined | JsValue::Null)
    }
    pub fn to_number(&self) -> f64 {
        match self {
            JsValue::Number(n)  => *n,
            JsValue::Bool(b)    => if *b { 1.0 } else { 0.0 },
            JsValue::Str(s)     => s.parse().unwrap_or(f64::NAN),
            JsValue::Null       => 0.0,
            _                   => f64::NAN,
        }
    }
    pub fn to_string_val(&self) -> String {
        match self {
            JsValue::Undefined   => "undefined".to_string(),
            JsValue::Null        => "null".to_string(),
            JsValue::Bool(b)     => if *b { "true".to_string() } else { "false".to_string() },
            JsValue::Number(n)   => fmt_number(*n),
            JsValue::Str(s)      => s.clone(),
            JsValue::Array(arr)  => {
                let v = arr.borrow();
                v.iter().map(|x| x.to_string_val()).collect::<Vec<_>>().join(",")
            }
            JsValue::Object(_)   => "[object Object]".to_string(),
            JsValue::Function(f) => format!("function {}() {{ [native code] }}", f.name),
            JsValue::NativeFunction(n, _) => format!("function {}() {{ [native code] }}", n),
        }
    }
    pub fn type_of(&self) -> &'static str {
        match self {
            JsValue::Undefined          => "undefined",
            JsValue::Null               => "object",
            JsValue::Bool(_)            => "boolean",
            JsValue::Number(_)          => "number",
            JsValue::Str(_)             => "string",
            JsValue::Object(_)          => "object",
            JsValue::Array(_)           => "object",
            JsValue::Function(_) | JsValue::NativeFunction(_, _) => "function",
        }
    }
}

fn fmt_number(n: f64) -> String {
    if n.is_nan()      { return "NaN".to_string(); }
    if n.is_infinite() { return if n > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() }; }
    if n == 0.0        { return "0".to_string(); }
    // Integer check
    if n == (n as i64) as f64 && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    format!("{}", n)
}

// ─────────────────────────────────────────────────────────────────────────────
//  JS Object (property map)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct JsObject {
    pub props: BTreeMap<String, JsValue>,
    pub class: String, // "Object", "Date", "Error", etc.
}

impl JsObject {
    pub fn new() -> Self { JsObject { props: BTreeMap::new(), class: "Object".to_string() } }
    pub fn get(&self, key: &str) -> JsValue {
        self.props.get(key).cloned().unwrap_or(JsValue::Undefined)
    }
    pub fn set(&mut self, key: String, val: JsValue) { self.props.insert(key, val); }
}

// ─────────────────────────────────────────────────────────────────────────────
//  JS Function (closure)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct JsFunc {
    pub name:     String,
    pub params:   Vec<String>,
    pub body:     Vec<Stmt>,
    pub closure:  Env,      // captured scope
    pub is_async: bool,     // Phase 104: wraps return in Promise
}

// ─────────────────────────────────────────────────────────────────────────────
//  Scope / Environment
// ─────────────────────────────────────────────────────────────────────────────

pub type EnvFrame = BTreeMap<String, JsValue>;

#[derive(Clone, Debug)]
pub struct Env(Vec<EnvFrame>);

impl Env {
    pub fn new() -> Self { Env(vec![EnvFrame::new()]) }
    pub fn push(&mut self) { self.0.push(EnvFrame::new()); }
    pub fn pop(&mut self)  { self.0.pop(); }

    pub fn get(&self, name: &str) -> JsValue {
        for frame in self.0.iter().rev() {
            if let Some(v) = frame.get(name) { return v.clone(); }
        }
        JsValue::Undefined
    }

    pub fn set_local(&mut self, name: String, val: JsValue) {
        if let Some(frame) = self.0.last_mut() { frame.insert(name, val); }
    }

    pub fn assign(&mut self, name: &str, val: JsValue) -> bool {
        for frame in self.0.iter_mut().rev() {
            if frame.contains_key(name) { frame.insert(name.to_string(), val); return true; }
        }
        false
    }

    pub fn define(&mut self, name: String, val: JsValue) {
        if let Some(frame) = self.0.last_mut() { frame.insert(name, val); }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Control flow signals
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
enum Signal {
    Return(JsValue),
    Throw(JsValue),
    Break,
    Continue,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Interpreter
// ─────────────────────────────────────────────────────────────────────────────

pub struct Interpreter {
    pub env:    Env,
    pub output: Vec<String>,         // console.log accumulator
    call_depth: usize,
    /// Phase 104: micro-task queue — drained at end of each top-level `run()`.
    pub microtask_queue: Vec<JsValue>,
    /// Phase 123: JIT cache for hot numeric functions.
    pub jit_cache: JitCache,
    /// Phase 124: exports accumulated while executing as a module.
    pub module_exports: BTreeMap<String, JsValue>,
}

impl Interpreter {
    pub fn new() -> Self {
        let mut interp = Interpreter {
            env:             Env::new(),
            output:          Vec::new(),
            call_depth:      0,
            microtask_queue: Vec::new(),
            jit_cache:       JitCache::new(),
            module_exports:  BTreeMap::new(),
        };
        interp.install_globals();
        interp
    }

    fn install_globals(&mut self) {
        // console
        let console = make_obj! {
            "log"   => JsValue::NativeFunction("log", native_console_log),
            "error" => JsValue::NativeFunction("error", native_console_log),
            "warn"  => JsValue::NativeFunction("warn", native_console_log)
        };
        self.env.define("console".to_string(), console);

        // Math
        let math = make_obj! {
            "PI"     => JsValue::Number(core::f64::consts::PI),
            "E"      => JsValue::Number(core::f64::consts::E),
            "floor"  => JsValue::NativeFunction("floor",  native_math_floor),
            "ceil"   => JsValue::NativeFunction("ceil",   native_math_ceil),
            "round"  => JsValue::NativeFunction("round",  native_math_round),
            "abs"    => JsValue::NativeFunction("abs",    native_math_abs),
            "sqrt"   => JsValue::NativeFunction("sqrt",   native_math_sqrt),
            "max"    => JsValue::NativeFunction("max",    native_math_max),
            "min"    => JsValue::NativeFunction("min",    native_math_min),
            "pow"    => JsValue::NativeFunction("pow",    native_math_pow),
            "log"    => JsValue::NativeFunction("log",    native_math_log),
            "sin"    => JsValue::NativeFunction("sin",    native_math_sin),
            "cos"    => JsValue::NativeFunction("cos",    native_math_cos),
            "tan"    => JsValue::NativeFunction("tan",    native_math_tan),
            "random" => JsValue::NativeFunction("random", native_math_random),
            "trunc"  => JsValue::NativeFunction("trunc",  native_math_trunc),
            "sign"   => JsValue::NativeFunction("sign",   native_math_sign),
            "log2"   => JsValue::NativeFunction("log2",   native_math_log2),
            "log10"  => JsValue::NativeFunction("log10",  native_math_log10),
            "atan2"  => JsValue::NativeFunction("atan2",  native_math_atan2),
            "hypot"  => JsValue::NativeFunction("hypot",  native_math_hypot),
            "cbrt"   => JsValue::NativeFunction("cbrt",   native_math_cbrt),
            "clz32"  => JsValue::NativeFunction("clz32",  native_math_clz32),
            "asin"   => JsValue::NativeFunction("asin",   native_math_asin),
            "acos"   => JsValue::NativeFunction("acos",   native_math_acos),
            "atan"   => JsValue::NativeFunction("atan",   native_math_atan),
            "sinh"   => JsValue::NativeFunction("sinh",   native_math_sinh),
            "cosh"   => JsValue::NativeFunction("cosh",   native_math_cosh),
            "tanh"   => JsValue::NativeFunction("tanh",   native_math_tanh),
            "expm1"  => JsValue::NativeFunction("expm1",  native_math_expm1),
            "log1p"  => JsValue::NativeFunction("log1p",  native_math_log1p),
            "fround" => JsValue::NativeFunction("fround", native_math_fround),
            "imul"   => JsValue::NativeFunction("imul",   native_math_imul)
        };
        self.env.define("Math".to_string(), math);

        // JSON
        let json = make_obj! {
            "stringify" => JsValue::NativeFunction("stringify", native_json_stringify),
            "parse"     => JsValue::NativeFunction("parse",     native_json_parse)
        };
        self.env.define("JSON".to_string(), json);

        // Global functions
        self.env.define("parseInt".to_string(),   JsValue::NativeFunction("parseInt",   native_parse_int));
        self.env.define("parseFloat".to_string(), JsValue::NativeFunction("parseFloat", native_parse_float));
        self.env.define("isNaN".to_string(),      JsValue::NativeFunction("isNaN",      native_is_nan));
        self.env.define("isFinite".to_string(),   JsValue::NativeFunction("isFinite",   native_is_finite));
        self.env.define("encodeURIComponent".to_string(), JsValue::NativeFunction("encodeURIComponent", native_encode_uri_component));
        self.env.define("decodeURIComponent".to_string(), JsValue::NativeFunction("decodeURIComponent", native_decode_uri_component));
        self.env.define("encodeURI".to_string(), JsValue::NativeFunction("encodeURI", native_encode_uri_component));
        self.env.define("decodeURI".to_string(), JsValue::NativeFunction("decodeURI", native_decode_uri_component));
        self.env.define("structuredClone".to_string(), JsValue::NativeFunction("structuredClone", native_structured_clone));
        // Async stubs (run synchronously in our cooperative kernel)
        self.env.define("setTimeout".to_string(),  JsValue::NativeFunction("setTimeout",  native_set_timeout));
        self.env.define("clearTimeout".to_string(), JsValue::NativeFunction("clearTimeout", native_clear_timeout));
        self.env.define("setInterval".to_string(), JsValue::NativeFunction("setInterval", native_set_timeout));
        self.env.define("clearInterval".to_string(), JsValue::NativeFunction("clearInterval", native_clear_timeout));
        self.env.define("queueMicrotask".to_string(), JsValue::NativeFunction("queueMicrotask", native_queue_microtask));
        self.env.define("requestAnimationFrame".to_string(), JsValue::NativeFunction("requestAnimationFrame", native_set_timeout));
        self.env.define("cancelAnimationFrame".to_string(), JsValue::NativeFunction("cancelAnimationFrame", native_clear_timeout));

        // Number constructor with static methods
        let number_obj = make_obj! {
            "isNaN"       => JsValue::NativeFunction("isNaN",       native_is_nan),
            "isFinite"    => JsValue::NativeFunction("isFinite",    native_is_finite),
            "isInteger"   => JsValue::NativeFunction("isInteger",   native_number_is_integer),
            "parseInt"    => JsValue::NativeFunction("parseInt",    native_parse_int),
            "parseFloat"  => JsValue::NativeFunction("parseFloat",  native_parse_float),
            "MAX_SAFE_INTEGER" => JsValue::Number(9007199254740991.0),
            "MIN_SAFE_INTEGER" => JsValue::Number(-9007199254740991.0),
            "MAX_VALUE"   => JsValue::Number(f64::MAX),
            "MIN_VALUE"   => JsValue::Number(f64::MIN_POSITIVE),
            "EPSILON"     => JsValue::Number(f64::EPSILON),
            "POSITIVE_INFINITY" => JsValue::Number(f64::INFINITY),
            "NEGATIVE_INFINITY" => JsValue::Number(f64::NEG_INFINITY),
            "NaN"         => JsValue::Number(f64::NAN),
            "__func__"    => JsValue::NativeFunction("Number", native_to_number)
        };
        self.env.define("Number".to_string(), number_obj);

        // Boolean
        self.env.define("Boolean".to_string(), JsValue::NativeFunction("Boolean", native_to_bool));

        // String constructor with static methods
        let string_obj = make_obj! {
            "fromCharCode"  => JsValue::NativeFunction("fromCharCode",  native_string_from_char_code),
            "fromCodePoint" => JsValue::NativeFunction("fromCodePoint", native_string_from_char_code),
            "raw"           => JsValue::NativeFunction("raw",           native_string_raw),
            "__func__"      => JsValue::NativeFunction("String", native_to_string)
        };
        self.env.define("String".to_string(), string_obj);

        // Array constructor with static methods
        let array_obj = make_obj! {
            "isArray"  => JsValue::NativeFunction("isArray",  native_array_is_array),
            "from"     => JsValue::NativeFunction("from",     native_array_from),
            "of"       => JsValue::NativeFunction("of",       native_array_of),
            "__func__" => JsValue::NativeFunction("Array", native_make_array)
        };
        self.env.define("Array".to_string(), array_obj);

        // Object constructor with static methods
        let object_obj = make_obj! {
            "keys"         => JsValue::NativeFunction("keys",         native_obj_keys),
            "values"       => JsValue::NativeFunction("values",       native_obj_values),
            "entries"      => JsValue::NativeFunction("entries",      native_obj_entries),
            "assign"       => JsValue::NativeFunction("assign",       native_obj_assign),
            "fromEntries"  => JsValue::NativeFunction("fromEntries",  native_obj_from_entries),
            "create"       => JsValue::NativeFunction("create",       native_obj_create),
            "freeze"       => JsValue::NativeFunction("freeze",       native_obj_freeze),
            "isFrozen"     => JsValue::NativeFunction("isFrozen",     native_obj_is_frozen),
            "hasOwn"       => JsValue::NativeFunction("hasOwn",       native_obj_has_own),
            "defineProperty" => JsValue::NativeFunction("defineProperty", native_obj_define_property),
            "getOwnPropertyNames" => JsValue::NativeFunction("getOwnPropertyNames", native_obj_keys),
            "__func__"     => JsValue::NativeFunction("Object", native_make_object)
        };
        self.env.define("Object".to_string(), object_obj);

        // Map constructor
        self.env.define("Map".to_string(), JsValue::NativeFunction("Map", native_map_new));

        // Set constructor
        self.env.define("Set".to_string(), JsValue::NativeFunction("Set", native_set_new));

        // Promise constructor (Phase 104: add withResolvers)
        let promise_obj = make_obj! {
            "resolve"       => JsValue::NativeFunction("resolve",       native_promise_resolve_static),
            "reject"        => JsValue::NativeFunction("reject",        native_promise_reject_static),
            "all"           => JsValue::NativeFunction("all",           native_promise_all),
            "allSettled"    => JsValue::NativeFunction("allSettled",    native_promise_all_settled),
            "race"          => JsValue::NativeFunction("race",          native_promise_race),
            "any"           => JsValue::NativeFunction("any",           native_promise_all),
            "withResolvers" => JsValue::NativeFunction("withResolvers", native_promise_with_resolvers),
            "__func__"      => JsValue::NativeFunction("Promise", native_promise_new)
        };
        self.env.define("Promise".to_string(), promise_obj);

        // Symbol (minimal stub)
        let symbol_obj = make_obj! {
            "iterator"     => JsValue::Str("Symbol(Symbol.iterator)".to_string()),
            "asyncIterator"=> JsValue::Str("Symbol(Symbol.asyncIterator)".to_string()),
            "toPrimitive"  => JsValue::Str("Symbol(Symbol.toPrimitive)".to_string()),
            "hasInstance"  => JsValue::Str("Symbol(Symbol.hasInstance)".to_string()),
            "__func__"     => JsValue::NativeFunction("Symbol", native_symbol)
        };
        self.env.define("Symbol".to_string(), symbol_obj);

        // Error constructors
        self.env.define("Error".to_string(),         JsValue::NativeFunction("Error",         native_make_error));
        self.env.define("TypeError".to_string(),     JsValue::NativeFunction("TypeError",     native_make_error));
        self.env.define("RangeError".to_string(),    JsValue::NativeFunction("RangeError",    native_make_error));
        self.env.define("ReferenceError".to_string(),JsValue::NativeFunction("ReferenceError",native_make_error));
        self.env.define("SyntaxError".to_string(),   JsValue::NativeFunction("SyntaxError",   native_make_error));

        // WeakMap / WeakRef / WeakSet stubs
        self.env.define("WeakMap".to_string(), JsValue::NativeFunction("WeakMap", native_map_new));
        self.env.define("WeakSet".to_string(), JsValue::NativeFunction("WeakSet", native_set_new));
        self.env.define("WeakRef".to_string(), JsValue::NativeFunction("WeakRef", native_weak_ref_new));

        // URL / URLSearchParams stubs
        self.env.define("URL".to_string(), JsValue::NativeFunction("URL", native_url_new));
        self.env.define("URLSearchParams".to_string(), JsValue::NativeFunction("URLSearchParams", native_url_search_params_new));

        // TextEncoder / TextDecoder stubs
        self.env.define("TextEncoder".to_string(), JsValue::NativeFunction("TextEncoder", native_text_encoder_new));
        self.env.define("TextDecoder".to_string(), JsValue::NativeFunction("TextDecoder", native_text_decoder_new));

        // AbortController stub
        self.env.define("AbortController".to_string(), JsValue::NativeFunction("AbortController", native_abort_controller_new));

        // globalThis / window / document stubs
        self.env.define("globalThis".to_string(), JsValue::Object(Rc::new(RefCell::new(JsObject::new()))));
        self.env.define("window".to_string(),     JsValue::Object(Rc::new(RefCell::new(JsObject::new()))));
        self.env.define("document".to_string(),   make_obj! {
            "createElement"    => JsValue::NativeFunction("createElement",    native_doc_create_element),
            "createTextNode"   => JsValue::NativeFunction("createTextNode",   native_doc_create_text_node),
            "getElementById"   => JsValue::NativeFunction("getElementById",   native_doc_get_element_by_id),
            "querySelector"    => JsValue::NativeFunction("querySelector",    native_doc_query_selector),
            "querySelectorAll" => JsValue::NativeFunction("querySelectorAll", native_doc_query_selector_all),
            "body"             => JsValue::Object(Rc::new(RefCell::new({ let mut o = JsObject::new(); o.class = "Element".to_string(); o }))),
            "head"             => JsValue::Object(Rc::new(RefCell::new({ let mut o = JsObject::new(); o.class = "Element".to_string(); o }))),
            "title"            => JsValue::Str(String::new()),
            "cookie"           => JsValue::Str(String::new()),
            "readyState"       => JsValue::Str("complete".to_string()),
            "location"         => make_obj! { "href" => JsValue::Str(String::new()), "hostname" => JsValue::Str(String::new()) },
            "addEventListener" => JsValue::NativeFunction("addEventListener", native_noop),
            "removeEventListener" => JsValue::NativeFunction("removeEventListener", native_noop)
        });

        // navigator stub (Phase 105: added mediaDevices)
        self.env.define("navigator".to_string(), make_obj! {
            "userAgent"    => JsValue::Str("SmartOS/0.61 Browser".to_string()),
            "language"     => JsValue::Str("en-US".to_string()),
            "languages"    => JsValue::Array(Rc::new(RefCell::new(vec![JsValue::Str("en-US".to_string())]))),
            "onLine"       => JsValue::Bool(true),
            "cookieEnabled"=> JsValue::Bool(false),
            "platform"     => JsValue::Str("SmartOS".to_string()),
            "mediaDevices" => make_obj! {
                "getUserMedia"            => JsValue::NativeFunction("getUserMedia",            native_get_user_media),
                "enumerateDevices"        => JsValue::NativeFunction("enumerateDevices",        native_enumerate_devices),
                "getSupportedConstraints" => JsValue::NativeFunction("getSupportedConstraints", native_get_supported_constraints),
                "addEventListener"        => JsValue::NativeFunction("addEventListener",        native_noop),
                "removeEventListener"     => JsValue::NativeFunction("removeEventListener",     native_noop)
            }
        });

        // location stub
        self.env.define("location".to_string(), make_obj! {
            "href"     => JsValue::Str(String::new()),
            "hostname" => JsValue::Str(String::new()),
            "pathname" => JsValue::Str("/".to_string()),
            "search"   => JsValue::Str(String::new()),
            "hash"     => JsValue::Str(String::new()),
            "protocol" => JsValue::Str("https:".to_string())
        });

        // performance stub
        self.env.define("performance".to_string(), make_obj! {
            "now"  => JsValue::NativeFunction("now", native_performance_now),
            "mark" => JsValue::NativeFunction("mark", native_noop),
            "measure" => JsValue::NativeFunction("measure", native_noop)
        });

        // fetch (synchronous stub in kernel)
        self.env.define("fetch".to_string(), JsValue::NativeFunction("fetch", native_fetch));

        // Proxy stub
        self.env.define("Proxy".to_string(), JsValue::NativeFunction("Proxy", native_proxy_new));

        // Reflect stub
        self.env.define("Reflect".to_string(), make_obj! {
            "apply"          => JsValue::NativeFunction("apply",          native_noop),
            "construct"      => JsValue::NativeFunction("construct",      native_noop),
            "defineProperty" => JsValue::NativeFunction("defineProperty", native_noop),
            "ownKeys"        => JsValue::NativeFunction("ownKeys",        native_obj_keys)
        });

        // Date constructor (class-like object with static .now())
        let date_obj = make_obj! {
            "now"   => JsValue::NativeFunction("now",   native_date_now),
            "parse" => JsValue::NativeFunction("parse", native_date_parse),
            "UTC"   => JsValue::NativeFunction("UTC",   native_date_utc),
            "__func__" => JsValue::NativeFunction("Date", native_date_new)
        };
        self.env.define("Date".to_string(), date_obj);

        // eval stub (refuse to run arbitrary strings for security)
        self.env.define("eval".to_string(), JsValue::NativeFunction("eval", native_eval_stub));

        // Intl stub
        let intl_obj = make_obj! {
            "DateTimeFormat" => JsValue::NativeFunction("DateTimeFormat", native_intl_format_new),
            "NumberFormat"   => JsValue::NativeFunction("NumberFormat",   native_intl_format_new),
            "Collator"       => JsValue::NativeFunction("Collator",       native_intl_format_new)
        };
        self.env.define("Intl".to_string(), intl_obj);

        // queueMicrotask already defined above; also add globalThis aliases
        self.env.define("self".to_string(), JsValue::Object(Rc::new(RefCell::new(JsObject::new()))));

        // crypto stub (Web Crypto mini-API)
        self.env.define("crypto".to_string(), make_obj! {
            "getRandomValues" => JsValue::NativeFunction("getRandomValues", native_noop),
            "randomUUID"      => JsValue::NativeFunction("randomUUID", native_crypto_random_uuid),
            "subtle"          => JsValue::Object(Rc::new(RefCell::new(JsObject::new())))
        });

        // Phase 105: WebRTC constructors
        self.env.define("RTCPeerConnection".to_string(),     JsValue::NativeFunction("RTCPeerConnection",     native_rtc_peer_connection_new));
        self.env.define("RTCSessionDescription".to_string(), JsValue::NativeFunction("RTCSessionDescription", native_rtc_session_description_new));
        self.env.define("RTCIceCandidate".to_string(),       JsValue::NativeFunction("RTCIceCandidate",       native_rtc_ice_candidate_new));

        // Constants
        self.env.define("NaN".to_string(),       JsValue::Number(f64::NAN));
        self.env.define("Infinity".to_string(),  JsValue::Number(f64::INFINITY));
        self.env.define("undefined".to_string(), JsValue::Undefined);
    }

    // ── Run ─────────────────────────────────────────────────────────────────

    pub fn run(&mut self, src: &str) -> JsValue {
        let stmts = js_parser::parse(src);
        let result = self.exec_block(&stmts);
        // Phase 104: drain micro-task queue (Promise callbacks, queueMicrotask)
        self.drain_microtasks();
        result
    }

    /// Phase 124: Run `src` as an ES module registered at `path`.
    /// Named exports (`export function/const/let/var`) are stored in the
    /// global MODULE_REGISTRY keyed by `path` so subsequent `import` statements
    /// can resolve them.
    pub fn run_module(&mut self, path: &str, src: &str) {
        self.module_exports = BTreeMap::new();
        self.run(src);
        let exports = core::mem::take(&mut self.module_exports);
        if let Some(mut reg) = MODULE_REGISTRY.try_lock() {
            reg.0.insert(path.to_string(), exports);
        }
    }

    /// Drain the micro-task queue — call each queued callback in FIFO order.
    /// Newly-queued microtasks during draining are also run (up to 1024 total).
    pub fn drain_microtasks(&mut self) {
        let mut iters = 0usize;
        while !self.microtask_queue.is_empty() && iters < 1024 {
            let tasks: Vec<JsValue> = core::mem::take(&mut self.microtask_queue);
            for cb in tasks {
                self.call_value(cb, JsValue::Undefined, &[]);
            }
            iters += 1;
        }
    }

    /// Schedule a microtask (Promise continuation or queueMicrotask callback).
    pub fn queue_microtask(&mut self, cb: JsValue) {
        self.microtask_queue.push(cb);
    }

    pub fn exec_block(&mut self, stmts: &[Stmt]) -> JsValue {
        let mut last = JsValue::Undefined;
        for stmt in stmts {
            match self.exec_stmt(stmt) {
                Ok(v)  => last = v,
                Err(Signal::Return(v)) => return v,
                Err(_) => {}
            }
        }
        last
    }

    fn exec_stmts(&mut self, stmts: &[Stmt]) -> Result<JsValue, Signal> {
        let mut last = JsValue::Undefined;
        for stmt in stmts {
            last = self.exec_stmt(stmt)?;
        }
        Ok(last)
    }

    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<JsValue, Signal> {
        match stmt {
            Stmt::Empty => Ok(JsValue::Undefined),

            Stmt::Expr(e) => Ok(self.eval(e)),

            Stmt::Block(stmts) => {
                self.env.push();
                let r = self.exec_stmts(stmts);
                self.env.pop();
                r
            }

            Stmt::VarDecl { name, init, .. } => {
                let v = init.as_ref().map(|e| self.eval(e)).unwrap_or(JsValue::Undefined);
                self.env.define(name.clone(), v);
                Ok(JsValue::Undefined)
            }

            Stmt::DestructDecl { pattern, init, .. } => {
                let val = self.eval(init);
                match pattern {
                    DestructPat::Object(keys) => {
                        for (key, alias) in keys {
                            let v = self.get_prop(&val, key);
                            let bind = alias.as_ref().unwrap_or(key);
                            self.env.define(bind.clone(), v);
                        }
                    }
                    DestructPat::Array(elems) => {
                        for (i, name) in elems.iter().enumerate() {
                            if let Some(n) = name {
                                let v = self.get_index(&val, i);
                                self.env.define(n.clone(), v);
                            }
                        }
                    }
                }
                Ok(JsValue::Undefined)
            }

            Stmt::FuncDecl { name, params, body, is_async } => {
                let f = JsValue::Function(JsFunc {
                    name:     name.clone(),
                    params:   params.clone(),
                    body:     body.clone(),
                    closure:  self.env.clone(),
                    is_async: *is_async,
                });
                self.env.define(name.clone(), f);
                Ok(JsValue::Undefined)
            }

            Stmt::ClassDecl { name, super_class, methods } => {
                // Resolve parent class (extends)
                let parent_class = super_class.as_ref().and_then(|sc| {
                    let v = self.env.get(sc);
                    if matches!(&v, JsValue::Object(_)) { Some(v) } else { None }
                });
                // Temporarily install __super__ so constructor closure captures it
                if let Some(ref parent) = parent_class {
                    self.env.define("__super__".to_string(), parent.clone());
                }
                // Build prototype — inherit parent prototype props first, then own
                let proto = {
                    let obj = Rc::new(RefCell::new(JsObject::new()));
                    if let Some(JsValue::Object(ref par_obj)) = parent_class {
                        if let JsValue::Object(par_proto) = par_obj.borrow().get("prototype") {
                            for (k, v) in par_proto.borrow().props.iter() {
                                obj.borrow_mut().set(k.clone(), v.clone());
                            }
                        }
                    }
                    for m in methods.iter().filter(|m| !m.is_static && !m.is_constructor) {
                        let f = JsValue::Function(JsFunc {
                            name:     m.name.clone(),
                            params:   m.params.clone(),
                            body:     m.body.clone(),
                            closure:  self.env.clone(),
                            is_async: m.is_async,
                        });
                        obj.borrow_mut().set(m.name.clone(), f);
                    }
                    JsValue::Object(obj)
                };
                // Find constructor
                let ctor_body = methods.iter().find(|m| m.is_constructor)
                    .map(|m| (m.params.clone(), m.body.clone()));
                let (ctor_params, ctor_body_stmts) = ctor_body.unwrap_or_else(|| (vec![], vec![]));
                let ctor = JsFunc {
                    name:     name.clone(),
                    params:   ctor_params,
                    body:     ctor_body_stmts,
                    closure:  self.env.clone(), // captures __super__
                    is_async: false,
                };
                let mut ctor_obj = JsObject::new();
                ctor_obj.set("prototype".to_string(), proto);
                ctor_obj.class = "Function".to_string();
                ctor_obj.set("__func__".to_string(), JsValue::Function(ctor));
                // Keep __super__ on class obj for instanceof support
                if let Some(ref parent) = parent_class {
                    ctor_obj.set("__super__".to_string(), parent.clone());
                }
                // Static methods go on the class object itself
                for m in methods.iter().filter(|m| m.is_static) {
                    let f = JsValue::Function(JsFunc {
                        name:     m.name.clone(),
                        params:   m.params.clone(),
                        body:     m.body.clone(),
                        closure:  self.env.clone(),
                        is_async: m.is_async,
                    });
                    ctor_obj.set(m.name.clone(), f);
                }
                let class_val = JsValue::Object(Rc::new(RefCell::new(ctor_obj)));
                self.env.define(name.clone(), class_val);
                Ok(JsValue::Undefined)
            }

            Stmt::Return(e) => {
                let v = e.as_ref().map(|ex| self.eval(ex)).unwrap_or(JsValue::Undefined);
                Err(Signal::Return(v))
            }

            Stmt::Throw(e) => Err(Signal::Throw(self.eval(e))),

            Stmt::If { cond, then, else_ } => {
                if self.eval(cond).is_truthy() {
                    self.exec_stmt(then)
                } else if let Some(el) = else_ {
                    self.exec_stmt(el)
                } else {
                    Ok(JsValue::Undefined)
                }
            }

            Stmt::While { cond, body } => {
                loop {
                    if !self.eval(cond).is_truthy() { break; }
                    match self.exec_stmt(body) {
                        Err(Signal::Break)    => break,
                        Err(Signal::Continue) => continue,
                        Err(e)                => return Err(e),
                        Ok(_)                 => {}
                    }
                }
                Ok(JsValue::Undefined)
            }

            Stmt::DoWhile { body, cond } => {
                loop {
                    match self.exec_stmt(body) {
                        Err(Signal::Break)    => break,
                        Err(Signal::Continue) => {}
                        Err(e)                => return Err(e),
                        Ok(_)                 => {}
                    }
                    if !self.eval(cond).is_truthy() { break; }
                }
                Ok(JsValue::Undefined)
            }

            Stmt::For { init, cond, update, body } => {
                self.env.push();
                if let Some(fi) = init {
                    match fi {
                        ForInit::Var(_, name, expr) => {
                            let v = expr.as_ref().map(|e| self.eval(e)).unwrap_or(JsValue::Undefined);
                            self.env.define(name.clone(), v);
                        }
                        ForInit::Expr(e) => { self.eval(e); }
                    }
                }
                loop {
                    if let Some(c) = cond { if !self.eval(c).is_truthy() { break; } }
                    match self.exec_stmt(body) {
                        Err(Signal::Break)    => break,
                        Err(Signal::Continue) => {}
                        Err(e)                => { self.env.pop(); return Err(e); }
                        Ok(_)                 => {}
                    }
                    if let Some(u) = update { self.eval(u); }
                }
                self.env.pop();
                Ok(JsValue::Undefined)
            }

            Stmt::ForIn { kind: _, name, obj, body } => {
                let val = self.eval(obj);
                let keys = self.object_keys(&val);
                for key in keys {
                    self.env.push();
                    self.env.define(name.clone(), JsValue::Str(key));
                    match self.exec_stmt(body) {
                        Err(Signal::Break)    => { self.env.pop(); break; }
                        Err(Signal::Continue) => { self.env.pop(); continue; }
                        Err(e)                => { self.env.pop(); return Err(e); }
                        Ok(_)                 => {}
                    }
                    self.env.pop();
                }
                Ok(JsValue::Undefined)
            }

            Stmt::ForOf { kind: _, name, iter, body } => {
                let val = self.eval(iter);
                let items = self.to_array_vals(&val);
                for item in items {
                    self.env.push();
                    self.env.define(name.clone(), item);
                    match self.exec_stmt(body) {
                        Err(Signal::Break)    => { self.env.pop(); break; }
                        Err(Signal::Continue) => { self.env.pop(); continue; }
                        Err(e)                => { self.env.pop(); return Err(e); }
                        Ok(_)                 => {}
                    }
                    self.env.pop();
                }
                Ok(JsValue::Undefined)
            }

            Stmt::TryCatch { body, param, catch, finally } => {
                let r = self.exec_stmts(body);
                let r2 = match r {
                    Err(Signal::Throw(thrown)) => {
                        if let Some(cb) = catch {
                            self.env.push();
                            if let Some(p) = param {
                                self.env.define(p.clone(), thrown);
                            }
                            let cr = self.exec_stmts(cb);
                            self.env.pop();
                            cr
                        } else { Err(Signal::Throw(thrown)) }
                    }
                    other => other,
                };
                if let Some(fb) = finally { self.exec_stmts(fb)?; }
                r2
            }

            Stmt::Break(_)    => Err(Signal::Break),
            Stmt::Continue(_) => Err(Signal::Continue),
            Stmt::Label(_, s) => self.exec_stmt(s),
            // ── Phase 124: ES Modules ─────────────────────────────────────────
            Stmt::Export(inner) => {
                // Execute the inner declaration so the name lands in scope.
                let res = self.exec_stmt(inner)?;
                // Register the name(s) in this module's export table.
                match inner.as_ref() {
                    Stmt::FuncDecl { name, .. } => {
                        let val = self.env.get(name);
                        self.module_exports.insert(name.clone(), val);
                    }
                    Stmt::VarDecl { name, .. } => {
                        let val = self.env.get(name);
                        self.module_exports.insert(name.clone(), val);
                    }
                    Stmt::ClassDecl { name, .. } => {
                        let val = self.env.get(name);
                        self.module_exports.insert(name.clone(), val);
                    }
                    // `export default <expr_stmt>` — register as "default"
                    Stmt::Expr(_) => {
                        self.module_exports.insert("default".to_string(), res.clone());
                    }
                    _ => {}
                }
                Ok(JsValue::Undefined)
            }
            Stmt::Import { what, from } => {
                // Fetch exported bindings for `from` from the registry.
                let exports: BTreeMap<String, JsValue> = {
                    if let Some(reg) = MODULE_REGISTRY.try_lock() {
                        reg.0.get(from.as_str()).cloned().unwrap_or_default()
                    } else {
                        BTreeMap::new()
                    }
                };
                match what {
                    ImportSpec::Side => {} // side-effect only import
                    ImportSpec::Default(alias) => {
                        let val = exports.get("default").cloned()
                            .unwrap_or(JsValue::Undefined);
                        self.env.define(alias.clone(), val);
                    }
                    ImportSpec::Named(pairs) => {
                        for (orig, alias) in pairs {
                            let val = exports.get(orig.as_str()).cloned()
                                .unwrap_or(JsValue::Undefined);
                            self.env.define(alias.clone(), val);
                        }
                    }
                    ImportSpec::Namespace(alias) => {
                        // Build a module namespace object with all exports.
                        let obj = Rc::new(RefCell::new(JsObject::new()));
                        obj.borrow_mut().class = "Module".to_string();
                        for (k, v) in &exports {
                            obj.borrow_mut().set(k.clone(), v.clone());
                        }
                        self.env.define(alias.clone(), JsValue::Object(obj));
                    }
                }
                Ok(JsValue::Undefined)
            }
        }
    }

    // ── Evaluate expression ──────────────────────────────────────────────────

    pub fn eval(&mut self, expr: &Expr) -> JsValue {
        match expr {
            Expr::Number(n)    => JsValue::Number(*n),
            Expr::Str(s)       => JsValue::Str(s.clone()),
            Expr::Template(s)  => self.eval_template(s),
            Expr::Bool(b)      => JsValue::Bool(*b),
            Expr::Null         => JsValue::Null,
            Expr::Undefined    => JsValue::Undefined,
            Expr::This         => self.env.get("this"),

            Expr::Ident(name) => self.env.get(name),

            Expr::Array(elems) => {
                let mut arr: Vec<JsValue> = Vec::new();
                for e in elems {
                    if let Expr::Spread(inner) = e {
                        let v = self.eval(inner);
                        arr.extend(self.to_array_vals(&v));
                    } else {
                        arr.push(self.eval(e));
                    }
                }
                JsValue::Array(Rc::new(RefCell::new(arr)))
            }

            Expr::Object(props) => {
                let obj = Rc::new(RefCell::new(JsObject::new()));
                for (key, val) in props {
                    let k = match key {
                        ObjectKey::Ident(s) | ObjectKey::Str(s) => s.clone(),
                        ObjectKey::Computed(e) => self.eval(e).to_string_val(),
                    };
                    if let Expr::Spread(inner) = val {
                        let v = self.eval(inner);
                        if let JsValue::Object(o) = &v {
                            for (pk, pv) in o.borrow().props.iter() {
                                obj.borrow_mut().set(pk.clone(), pv.clone());
                            }
                        }
                    } else {
                        let v = self.eval(val);
                        obj.borrow_mut().set(k, v);
                    }
                }
                JsValue::Object(obj)
            }

            Expr::FuncExpr { params, body, is_async } => JsValue::Function(JsFunc {
                name:     "<anonymous>".to_string(),
                params:   params.clone(),
                body:     body.clone(),
                closure:  self.env.clone(),
                is_async: *is_async,
            }),

            Expr::Arrow { params, body, is_async } => JsValue::Function(JsFunc {
                name:     "<arrow>".to_string(),
                params:   params.clone(),
                body:     match body {
                    ArrowBody::Block(stmts) => stmts.clone(),
                    ArrowBody::Expr(e)      => vec![Stmt::Return(Some(*e.clone()))],
                },
                closure:  self.env.clone(),
                is_async: *is_async,
            }),

            Expr::Typeof(e)  => JsValue::Str(self.eval(e).type_of().to_string()),
            Expr::Delete(e)  => { self.eval(e); JsValue::Bool(true) }
            Expr::Spread(e)  => self.eval(e),

            Expr::Unary { op, expr } => self.eval_unary(op, expr),
            Expr::Binary { op, left, right } => self.eval_binary(op, left, right),
            Expr::Logical { op, left, right } => self.eval_logical(op, left, right),
            Expr::Assign { op, target, value } => self.eval_assign(op, target, value),
            Expr::Ternary { cond, then, else_ } => {
                if self.eval(cond).is_truthy() { self.eval(then) } else { self.eval(else_) }
            }
            Expr::Member { obj, prop, computed } => {
                let o = self.eval(obj);
                let k = if *computed { self.eval(prop).to_string_val() }
                        else { match prop.as_ref() { Expr::Str(s) => s.clone(), _ => self.eval(prop).to_string_val() } };
                self.get_prop(&o, &k)
            }
            Expr::Call { callee, args } => self.eval_call(callee, args, None),
            Expr::New  { callee, args } => self.eval_new(callee, args),
            Expr::Sequence(exprs) => { let mut last = JsValue::Undefined; for e in exprs { last = self.eval(e); } last }
            // Phase 104: await — synchronously unwrap a Promise value.
            // In our cooperative kernel there is no event loop; all Promises
            // settle immediately when their executor runs, so unwrapping here
            // is semantically correct and safe.
            Expr::Await(e) => {
                let v = self.eval(e);
                // Unwrap Promise
                if let JsValue::Object(ref o) = v {
                    if o.borrow().class == "Promise" {
                        let state = o.borrow().get("__state__").to_string_val();
                        return match state.as_str() {
                            "fulfilled" => o.borrow().get("__value__"),
                            "rejected"  => {
                                // Re-throw so the surrounding try/catch can catch it
                                // We don't have Signal here so just return the error value
                                o.borrow().get("__value__")
                            }
                            _ => JsValue::Undefined,
                        };
                    }
                }
                v
            }
        }
    }

    // ── Operators ───────────────────────────────────────────────────────────

    fn eval_unary(&mut self, op: &UnaryOp, expr: &Expr) -> JsValue {
        match op {
            UnaryOp::Pos    => JsValue::Number(self.eval(expr).to_number()),
            UnaryOp::Neg    => JsValue::Number(-self.eval(expr).to_number()),
            UnaryOp::Not    => JsValue::Bool(!self.eval(expr).is_truthy()),
            UnaryOp::BitNot => JsValue::Number(!(self.eval(expr).to_number() as i32) as f64),
            UnaryOp::Void   => { self.eval(expr); JsValue::Undefined }
            UnaryOp::PreInc  => { let n = self.eval(expr).to_number() + 1.0; self.do_assign(expr, JsValue::Number(n)); JsValue::Number(n) }
            UnaryOp::PreDec  => { let n = self.eval(expr).to_number() - 1.0; self.do_assign(expr, JsValue::Number(n)); JsValue::Number(n) }
            UnaryOp::PostInc => { let n = self.eval(expr).to_number(); self.do_assign(expr, JsValue::Number(n + 1.0)); JsValue::Number(n) }
            UnaryOp::PostDec => { let n = self.eval(expr).to_number(); self.do_assign(expr, JsValue::Number(n - 1.0)); JsValue::Number(n) }
        }
    }

    fn eval_binary(&mut self, op: &BinaryOp, left: &Expr, right: &Expr) -> JsValue {
        // Short-circuit handled in logical; here eager
        let l = self.eval(left);
        let r = self.eval(right);
        match op {
            BinaryOp::Add => {
                if let (JsValue::Str(a), _) | (_, JsValue::Str(a)) = (&l, &r) {
                    // If either is string, concatenate
                    if matches!(&l, JsValue::Str(_)) {
                        JsValue::Str(format!("{}{}", l.to_string_val(), r.to_string_val()))
                    } else {
                        JsValue::Str(format!("{}{}", l.to_string_val(), r.to_string_val()))
                    }
                } else {
                    JsValue::Number(l.to_number() + r.to_number())
                }
            }
            BinaryOp::Sub => JsValue::Number(l.to_number() - r.to_number()),
            BinaryOp::Mul => JsValue::Number(l.to_number() * r.to_number()),
            BinaryOp::Div => JsValue::Number(l.to_number() / r.to_number()),
            BinaryOp::Rem => JsValue::Number(l.to_number() % r.to_number()),
            BinaryOp::Pow => JsValue::Number(js_pow(l.to_number(), r.to_number())),
            BinaryOp::BitAnd => JsValue::Number(((l.to_number() as i32) & (r.to_number() as i32)) as f64),
            BinaryOp::BitOr  => JsValue::Number(((l.to_number() as i32) | (r.to_number() as i32)) as f64),
            BinaryOp::BitXor => JsValue::Number(((l.to_number() as i32) ^ (r.to_number() as i32)) as f64),
            BinaryOp::Shl    => JsValue::Number(((l.to_number() as i32) << (r.to_number() as u32 & 31)) as f64),
            BinaryOp::Shr    => JsValue::Number(((l.to_number() as i32) >> (r.to_number() as u32 & 31)) as f64),
            BinaryOp::UShr   => JsValue::Number(((l.to_number() as u32) >> (r.to_number() as u32 & 31)) as f64),
            BinaryOp::Eq | BinaryOp::StrictEq => JsValue::Bool(js_eq(&l, &r)),
            BinaryOp::NotEq | BinaryOp::StrictNotEq => JsValue::Bool(!js_eq(&l, &r)),
            BinaryOp::Lt   => JsValue::Bool(js_lt(&l, &r)),
            BinaryOp::Gt   => JsValue::Bool(js_lt(&r, &l)),
            BinaryOp::LtEq => JsValue::Bool(!js_lt(&r, &l)),
            BinaryOp::GtEq => JsValue::Bool(!js_lt(&l, &r)),
            BinaryOp::In => {
                let key = l.to_string_val();
                JsValue::Bool(match &r {
                    JsValue::Object(o) => o.borrow().props.contains_key(&key),
                    _ => false,
                })
            }
            BinaryOp::Instanceof => {
                // Check if l was constructed by class r
                let class_name = match &r {
                    JsValue::Object(cls_obj) => {
                        match cls_obj.borrow().get("__func__") {
                            JsValue::Function(f) => f.name.clone(),
                            JsValue::NativeFunction(n, _) => n.to_string(),
                            _ => cls_obj.borrow().class.clone(),
                        }
                    }
                    JsValue::Function(f) => f.name.clone(),
                    JsValue::NativeFunction(n, _) => n.to_string(),
                    _ => String::new(),
                };
                let obj_class = match &l {
                    JsValue::Object(o) => o.borrow().class.clone(),
                    _ => String::new(),
                };
                JsValue::Bool(!class_name.is_empty() && obj_class == class_name)
            }
        }
    }

    fn eval_logical(&mut self, op: &LogicOp, left: &Expr, right: &Expr) -> JsValue {
        let l = self.eval(left);
        match op {
            LogicOp::And          => if l.is_truthy() { self.eval(right) } else { l }
            LogicOp::Or           => if l.is_truthy() { l } else { self.eval(right) }
            LogicOp::NullCoalesce => if l.is_nullish() { self.eval(right) } else { l }
        }
    }

    fn eval_assign(&mut self, op: &AssignOp, target: &Expr, value: &Expr) -> JsValue {
        let rhs = self.eval(value);
        let new_val = if *op == AssignOp::Plain {
            rhs
        } else {
            let lhs = self.eval(target);
            match op {
                AssignOp::Add => { if matches!(&lhs, JsValue::Str(_)) || matches!(&rhs, JsValue::Str(_)) { JsValue::Str(format!("{}{}", lhs.to_string_val(), rhs.to_string_val())) } else { JsValue::Number(lhs.to_number() + rhs.to_number()) } }
                AssignOp::Sub => JsValue::Number(lhs.to_number() - rhs.to_number()),
                AssignOp::Mul => JsValue::Number(lhs.to_number() * rhs.to_number()),
                AssignOp::Div => JsValue::Number(lhs.to_number() / rhs.to_number()),
                AssignOp::Rem => JsValue::Number(lhs.to_number() % rhs.to_number()),
                AssignOp::BitAnd => JsValue::Number(((lhs.to_number() as i32) & (rhs.to_number() as i32)) as f64),
                AssignOp::BitOr  => JsValue::Number(((lhs.to_number() as i32) | (rhs.to_number() as i32)) as f64),
                AssignOp::BitXor => JsValue::Number(((lhs.to_number() as i32) ^ (rhs.to_number() as i32)) as f64),
                _ => rhs,
            }
        };
        self.do_assign(target, new_val.clone());
        new_val
    }

    fn do_assign(&mut self, target: &Expr, val: JsValue) {
        match target {
            Expr::Ident(name) => {
                if !self.env.assign(name, val.clone()) {
                    self.env.define(name.clone(), val);
                }
            }
            Expr::Member { obj, prop, computed } => {
                let o = self.eval(obj);
                let key = if *computed { self.eval(prop).to_string_val() }
                          else { match prop.as_ref() { Expr::Str(s) => s.clone(), _ => self.eval(prop).to_string_val() } };
                self.set_prop(&o, &key, val);
            }
            _ => {}
        }
    }

    // ── Function call ────────────────────────────────────────────────────────

    fn eval_call(&mut self, callee: &Expr, args: &[Expr], this: Option<JsValue>) -> JsValue {
        // Collect args
        let mut arg_vals: Vec<JsValue> = Vec::new();
        for a in args {
            if let Expr::Spread(inner) = a {
                let v = self.eval(inner);
                arg_vals.extend(self.to_array_vals(&v));
            } else {
                arg_vals.push(self.eval(a));
            }
        }

        // Resolve callee + this
        let (func_val, this_val) = match callee {
            Expr::Member { obj, prop, computed } => {
                // Handle super.method() call
                if let Expr::Ident(super_name) = obj.as_ref() {
                    if super_name == "super" {
                        let key = if *computed { self.eval(prop).to_string_val() }
                                  else { match prop.as_ref() { Expr::Str(s) => s.clone(), _ => self.eval(prop).to_string_val() } };
                        let this_val2 = self.env.get("this");
                        let super_class = self.env.get("__super__");
                        let method = if let JsValue::Object(ref sc) = super_class {
                            if let JsValue::Object(proto) = sc.borrow().get("prototype") {
                                proto.borrow().get(&key)
                            } else { JsValue::Undefined }
                        } else { JsValue::Undefined };
                        return self.call_value(method, this_val2, &arg_vals);
                    }
                }
                let receiver = self.eval(obj);
                let key = if *computed { self.eval(prop).to_string_val() }
                          else { match prop.as_ref() { Expr::Str(s) => s.clone(), _ => self.eval(prop).to_string_val() } };
                let method = self.get_prop(&receiver, &key);
                // String methods — intercept here
                if let JsValue::Str(ref s) = receiver {
                    let sv = self.call_string_method(s.clone(), &key, &arg_vals);
                    if let Some(v) = sv { return v; }
                }
                // Number methods
                if let JsValue::Number(n) = receiver {
                    match key.as_str() {
                        "toFixed" => {
                            let digits = arg_vals.get(0).map(|v| v.to_number() as usize).unwrap_or(0).min(20);
                            return JsValue::Str(format_fixed(n, digits));
                        }
                        "toString" => {
                            let radix = arg_vals.get(0).map(|v| v.to_number() as u32).unwrap_or(10);
                            return if radix == 10 { JsValue::Str(fmt_number(n)) }
                                   else { JsValue::Str(format_radix(n as i64, radix)) };
                        }
                        "toPrecision" => {
                            let prec = arg_vals.get(0).map(|v| v.to_number() as usize).unwrap_or(6).max(1);
                            return JsValue::Str(format_precision(n, prec));
                        }
                        "valueOf" | "toLocaleString" => return JsValue::Number(n),
                        _ => {}
                    }
                }
                // Array methods
                if let JsValue::Array(_) = &receiver {
                    let av = self.call_array_method(receiver.clone(), &key, &arg_vals);
                    if let Some(v) = av { return v; }
                }
                // Map / Set / Promise method dispatch
                if let JsValue::Object(ref o) = receiver {
                    let class = o.borrow().class.clone();
                    match class.as_str() {
                        "Map" => {
                            if let Some(v) = self.call_map_method(receiver.clone(), &key, &arg_vals) {
                                return v;
                            }
                        }
                        "Set" => {
                            if let Some(v) = self.call_set_method(receiver.clone(), &key, &arg_vals) {
                                return v;
                            }
                        }
                        "Promise" => {
                            if let Some(v) = self.call_promise_method(receiver.clone(), &key, &arg_vals) {
                                return v;
                            }
                        }
                        "RTCPeerConnection" => {
                            if let Some(v) = self.call_rtc_pc_method(receiver.clone(), &key, &arg_vals) {
                                return v;
                            }
                        }
                        "MediaStream" | "MediaStreamTrack" => {
                            if let Some(v) = self.call_media_stream_method(receiver.clone(), &key, &arg_vals) {
                                return v;
                            }
                        }
                        _ => {}
                    }
                    // Common Object instance methods (hasOwnProperty, toString, valueOf)
                    match key.as_str() {
                        "hasOwnProperty" => {
                            let prop_name = arg_vals.get(0).map(|v| v.to_string_val()).unwrap_or_default();
                            return JsValue::Bool(o.borrow().props.contains_key(&prop_name));
                        }
                        "toString" => {
                            let cls = o.borrow().class.clone();
                            return JsValue::Str(format!("[object {}]", cls));
                        }
                        "valueOf" => return receiver.clone(),
                        "propertyIsEnumerable" => {
                            let prop_name = arg_vals.get(0).map(|v| v.to_string_val()).unwrap_or_default();
                            return JsValue::Bool(o.borrow().props.contains_key(&prop_name));
                        }
                        _ => {}
                    }
                }
                (method, this.unwrap_or(receiver))
            }
            _ => {
                // Handle super() call in constructor
                if let Expr::Ident(s) = callee {
                    if s == "super" {
                        let this_val2 = self.env.get("this");
                        let super_class = self.env.get("__super__");
                        if let JsValue::Object(ref sc) = super_class {
                            let ctor = sc.borrow().get("__func__");
                            if !matches!(ctor, JsValue::Undefined) {
                                self.call_value(ctor, this_val2, &arg_vals);
                            }
                        }
                        return self.env.get("this");
                    }
                }
                let f = self.eval(callee);
                (f, this.unwrap_or(JsValue::Undefined))
            }
        };

        self.call_value(func_val, this_val, &arg_vals)
    }

    pub fn call_value(&mut self, func: JsValue, this: JsValue, args: &[JsValue]) -> JsValue {
        if self.call_depth > 512 { return JsValue::Undefined; }
        self.call_depth += 1;
        let result = match func {
            JsValue::NativeFunction(_, f) => {
                // Expose `this` so native methods can access their receiver
                let prev_this = self.env.get("this");
                self.env.define("this".to_string(), this.clone());
                let r = f(args, self);
                self.env.define("this".to_string(), prev_this);
                r
            }
            JsValue::Function(jf) => {
                let is_async = jf.is_async;

                // ── Phase 123: JIT fast-path (pure integer arithmetic only) ──
                if !is_async && !jf.params.is_empty() {
                    let all_numeric = args.iter().all(|a| matches!(a, JsValue::Number(_)));
                    if all_numeric {
                        let args_i64: Vec<i64> = args.iter()
                            .map(|a| a.to_number() as i64)
                            .collect();
                        if let Some(ops) = lower_fn_to_jit(&jf.params, &jf.body) {
                            self.jit_cache.tick(&ops);
                            if let Ok(result) = jit_interpret(&ops, &args_i64) {
                                self.call_depth -= 1;
                                return JsValue::Number(result as f64);
                            }
                        }
                    }
                }
                // ─────────────────────────────────────────────────────────────

                // Save env, install closure
                let saved_env = core::mem::replace(&mut self.env, jf.closure.clone());
                self.env.push();
                self.env.define("this".to_string(), this);
                // Bind params
                for (i, p) in jf.params.iter().enumerate() {
                    let v = args.get(i).cloned().unwrap_or(JsValue::Undefined);
                    self.env.define(p.clone(), v);
                }
                // arguments object (array-like)
                let arguments = JsValue::Array(Rc::new(RefCell::new(args.to_vec())));
                self.env.define("arguments".to_string(), arguments);

                let exec_result = self.exec_stmts(&jf.body);
                self.env.pop();
                self.env = saved_env;

                // Phase 104: async function wraps return in Promise
                if is_async {
                    match exec_result {
                        Ok(v) | Err(Signal::Return(v)) => {
                            // If already a Promise, return as-is
                            if let JsValue::Object(ref o) = v {
                                if o.borrow().class == "Promise" {
                                    return v;
                                }
                            }
                            return js_resolved_promise(v);
                        }
                        Err(Signal::Throw(e)) => return js_rejected_promise(e),
                        Err(_) => return js_resolved_promise(JsValue::Undefined),
                    }
                }

                match exec_result {
                    Ok(v)                => v,
                    Err(Signal::Return(v)) => v,
                    Err(Signal::Throw(v)) => {
                        self.call_depth -= 1;
                        return v; // propagate — caller handles
                    }
                    Err(_) => JsValue::Undefined,
                }
            }
            JsValue::Object(ref o) => {
                // Class constructor call via __func__ (supports both Function and NativeFunction)
                let f = o.borrow().get("__func__");
                match &f {
                    JsValue::Function(_) | JsValue::NativeFunction(_, _) => self.call_value(f, this, args),
                    _ => JsValue::Undefined,
                }
            }
            _ => JsValue::Undefined,
        };
        self.call_depth -= 1;
        result
    }

    fn eval_new(&mut self, callee: &Expr, args: &[Expr]) -> JsValue {
        let class_val = self.eval(callee);
        let mut arg_vals: Vec<JsValue> = Vec::new();
        for a in args { arg_vals.push(self.eval(a)); }

        // Determine class name for instanceof tracking
        let class_name: String = match &class_val {
            JsValue::Object(cls) => {
                match cls.borrow().get("__func__") {
                    JsValue::Function(f) => f.name.clone(),
                    JsValue::NativeFunction(n, _) => n.to_string(),
                    _ => "Object".to_string(),
                }
            }
            JsValue::Function(f) => f.name.clone(),
            JsValue::NativeFunction(n, _) => n.to_string(),
            _ => "Object".to_string(),
        };

        // Create a new object with class name set
        let instance = Rc::new(RefCell::new(JsObject::new()));
        instance.borrow_mut().class = class_name;

        // If class_val is Object with __func__ (class), copy prototype props
        if let JsValue::Object(ref cls) = class_val {
            if let JsValue::Object(proto) = cls.borrow().get("prototype") {
                for (k, v) in proto.borrow().props.iter() {
                    instance.borrow_mut().set(k.clone(), v.clone());
                }
            }
            let ctor = cls.borrow().get("__func__");
            match &ctor {
                JsValue::Function(_) => {
                    self.call_value(ctor, JsValue::Object(instance.clone()), &arg_vals);
                }
                JsValue::NativeFunction(_, _) => {
                    // Native constructor: use its return value as the instance
                    let result = self.call_value(ctor, JsValue::Object(instance.clone()), &arg_vals);
                    if matches!(result, JsValue::Object(_)) { return result; }
                }
                _ => {}
            }
        } else if let JsValue::Function(_) = &class_val {
            self.call_value(class_val, JsValue::Object(instance.clone()), &arg_vals);
        } else if let JsValue::NativeFunction(_, _) = &class_val {
            let result = self.call_value(class_val, JsValue::Object(instance.clone()), &arg_vals);
            if matches!(result, JsValue::Object(_)) { return result; }
        }

        JsValue::Object(instance)
    }

    // ── Property access ──────────────────────────────────────────────────────

    pub fn get_prop(&self, obj: &JsValue, key: &str) -> JsValue {
        match obj {
            JsValue::Object(o) => {
                let class = o.borrow().class.clone();
                // Map / Set virtual `.size` property
                if (class == "Map" || class == "Set") && key == "size" {
                    let entries = o.borrow().get("__entries__");
                    if let JsValue::Array(a) = entries {
                        return JsValue::Number(a.borrow().len() as f64);
                    }
                }
                o.borrow().get(key)
            }
            JsValue::Array(a) => {
                if key == "length" {
                    return JsValue::Number(a.borrow().len() as f64);
                }
                if let Ok(idx) = key.parse::<usize>() {
                    return a.borrow().get(idx).cloned().unwrap_or(JsValue::Undefined);
                }
                JsValue::Undefined
            }
            JsValue::Str(s) => {
                if key == "length" { return JsValue::Number(s.len() as f64); }
                JsValue::Undefined
            }
            _ => JsValue::Undefined,
        }
    }

    fn set_prop(&self, obj: &JsValue, key: &str, val: JsValue) {
        match obj {
            JsValue::Object(o) => { o.borrow_mut().set(key.to_string(), val); }
            JsValue::Array(a)  => {
                if key == "length" {
                    let new_len = val.to_number() as usize;
                    a.borrow_mut().truncate(new_len);
                    return;
                }
                if let Ok(idx) = key.parse::<usize>() {
                    let mut arr = a.borrow_mut();
                    while arr.len() <= idx { arr.push(JsValue::Undefined); }
                    arr[idx] = val;
                }
            }
            _ => {}
        }
    }

    fn get_index(&self, obj: &JsValue, idx: usize) -> JsValue {
        match obj {
            JsValue::Array(a) => a.borrow().get(idx).cloned().unwrap_or(JsValue::Undefined),
            JsValue::Str(s) => {
                s.chars().nth(idx).map(|c| JsValue::Str(c.to_string())).unwrap_or(JsValue::Undefined)
            }
            JsValue::Object(o) => o.borrow().get(&format!("{}", idx)),
            _ => JsValue::Undefined,
        }
    }

    // ── String methods ───────────────────────────────────────────────────────

    fn call_string_method(&mut self, s: String, method: &str, args: &[JsValue]) -> Option<JsValue> {
        match method {
            "length"      => Some(JsValue::Number(s.len() as f64)),
            "charAt"      => { let i = args.get(0).map(|v| v.to_number() as usize).unwrap_or(0); Some(JsValue::Str(s.chars().nth(i).map(|c| c.to_string()).unwrap_or_default())) }
            "charCodeAt"  => { let i = args.get(0).map(|v| v.to_number() as usize).unwrap_or(0); Some(JsValue::Number(s.chars().nth(i).map(|c| c as u32 as f64).unwrap_or(f64::NAN))) }
            "indexOf"     => { let pat = args.get(0).map(|v| v.to_string_val()).unwrap_or_default(); Some(JsValue::Number(s.find(pat.as_str()).map(|i| i as f64).unwrap_or(-1.0))) }
            "includes"    => { let pat = args.get(0).map(|v| v.to_string_val()).unwrap_or_default(); Some(JsValue::Bool(s.contains(pat.as_str()))) }
            "startsWith"  => { let pat = args.get(0).map(|v| v.to_string_val()).unwrap_or_default(); Some(JsValue::Bool(s.starts_with(pat.as_str()))) }
            "endsWith"    => { let pat = args.get(0).map(|v| v.to_string_val()).unwrap_or_default(); Some(JsValue::Bool(s.ends_with(pat.as_str()))) }
            "slice"       => {
                let start = args.get(0).map(|v| v.to_number() as isize).unwrap_or(0);
                let end   = args.get(1).map(|v| v.to_number() as isize).unwrap_or(s.len() as isize);
                let len   = s.len() as isize;
                let s2    = to_abs_index(start, len);
                let e2    = to_abs_index(end, len);
                let chars: Vec<char> = s.chars().collect();
                let slice: String = chars.get(s2..e2.min(chars.len())).map(|c| c.iter().collect()).unwrap_or_default();
                Some(JsValue::Str(slice))
            }
            "substring"   => {
                let s2 = args.get(0).map(|v| v.to_number() as usize).unwrap_or(0).min(s.len());
                let e2 = args.get(1).map(|v| v.to_number() as usize).unwrap_or(s.len()).min(s.len());
                let chars: Vec<char> = s.chars().collect();
                let slice: String = chars[s2.min(e2)..s2.max(e2)].iter().collect();
                Some(JsValue::Str(slice))
            }
            "toLowerCase" => Some(JsValue::Str(s.to_ascii_lowercase())),
            "toUpperCase" => Some(JsValue::Str(s.to_ascii_uppercase())),
            "trim"        => Some(JsValue::Str(s.trim().to_string())),
            "trimStart" | "trimLeft"  => Some(JsValue::Str(s.trim_start().to_string())),
            "trimEnd"   | "trimRight" => Some(JsValue::Str(s.trim_end().to_string())),
            "split"       => {
                let sep = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
                let parts: Vec<JsValue> = if sep.is_empty() {
                    s.chars().map(|c| JsValue::Str(c.to_string())).collect()
                } else {
                    s.split(sep.as_str()).map(|p| JsValue::Str(p.to_string())).collect()
                };
                Some(JsValue::Array(Rc::new(RefCell::new(parts))))
            }
            "repeat"      => { let n = args.get(0).map(|v| v.to_number() as usize).unwrap_or(0); Some(JsValue::Str(s.repeat(n))) }
            "replace"     => {
                let pat = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
                let rep = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
                Some(JsValue::Str(s.replacen(pat.as_str(), &rep, 1)))
            }
            "replaceAll"  => {
                let pat = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
                let rep = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
                Some(JsValue::Str(s.replace(pat.as_str(), &rep)))
            }
            "padStart"    => {
                let target_len = args.get(0).map(|v| v.to_number() as usize).unwrap_or(0);
                let pad = args.get(1).map(|v| v.to_string_val()).unwrap_or_else(|| " ".to_string());
                if s.len() >= target_len { Some(JsValue::Str(s)) }
                else {
                    let needed = target_len - s.len();
                    let p: String = pad.chars().cycle().take(needed).collect();
                    Some(JsValue::Str(format!("{}{}", p, s)))
                }
            }
            "padEnd"      => {
                let target_len = args.get(0).map(|v| v.to_number() as usize).unwrap_or(0);
                let pad = args.get(1).map(|v| v.to_string_val()).unwrap_or_else(|| " ".to_string());
                if s.len() >= target_len { Some(JsValue::Str(s)) }
                else {
                    let needed = target_len - s.len();
                    let p: String = pad.chars().cycle().take(needed).collect();
                    Some(JsValue::Str(format!("{}{}", s, p)))
                }
            }
            "concat"      => {
                let mut out = s.clone();
                for a in args { out.push_str(&a.to_string_val()); }
                Some(JsValue::Str(out))
            }
            "at"          => {
                let i = args.get(0).map(|v| v.to_number() as isize).unwrap_or(0);
                let chars: Vec<char> = s.chars().collect();
                let idx = to_abs_index(i, chars.len() as isize);
                Some(JsValue::Str(chars.get(idx).map(|c| c.to_string()).unwrap_or_default()))
            }
            "toString" | "valueOf" => Some(JsValue::Str(s)),
            _ => None,
        }
    }

    // ── Array methods ────────────────────────────────────────────────────────

    fn call_array_method(&mut self, arr_val: JsValue, method: &str, args: &[JsValue]) -> Option<JsValue> {
        let arr_rc = match &arr_val { JsValue::Array(a) => a.clone(), _ => return None };
        match method {
            "length" => Some(JsValue::Number(arr_rc.borrow().len() as f64)),
            "push"   => {
                for a in args { arr_rc.borrow_mut().push(a.clone()); }
                Some(JsValue::Number(arr_rc.borrow().len() as f64))
            }
            "pop"    => Some(arr_rc.borrow_mut().pop().unwrap_or(JsValue::Undefined)),
            "shift"  => {
                let mut arr = arr_rc.borrow_mut();
                if arr.is_empty() { Some(JsValue::Undefined) } else { Some(arr.remove(0)) }
            }
            "unshift" => {
                let mut arr = arr_rc.borrow_mut();
                for (i, a) in args.iter().enumerate() { arr.insert(i, a.clone()); }
                Some(JsValue::Number(arr.len() as f64))
            }
            "reverse" => {
                arr_rc.borrow_mut().reverse();
                Some(arr_val.clone())
            }
            "slice"  => {
                let arr = arr_rc.borrow();
                let len = arr.len() as isize;
                let s   = to_abs_index(args.get(0).map(|v| v.to_number() as isize).unwrap_or(0), len);
                let e   = to_abs_index(args.get(1).map(|v| v.to_number() as isize).unwrap_or(len), len);
                let sl: Vec<JsValue> = arr.get(s..e.min(arr.len())).map(|x| x.to_vec()).unwrap_or_default();
                Some(JsValue::Array(Rc::new(RefCell::new(sl))))
            }
            "splice" => {
                let mut arr = arr_rc.borrow_mut();
                let arr_len = arr.len();
                let start = to_abs_index(args.get(0).map(|v| v.to_number() as isize).unwrap_or(0), arr_len as isize);
                let delete_count = args.get(1).map(|v| v.to_number() as usize).unwrap_or(arr_len - start);
                let end = (start + delete_count).min(arr_len);
                let removed: Vec<JsValue> = arr.drain(start..end).collect();
                for (i, a) in args.iter().skip(2).enumerate() { arr.insert(start + i, a.clone()); }
                Some(JsValue::Array(Rc::new(RefCell::new(removed))))
            }
            "indexOf" => {
                let target = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let arr = arr_rc.borrow();
                Some(JsValue::Number(arr.iter().position(|x| js_eq(x, &target)).map(|i| i as f64).unwrap_or(-1.0)))
            }
            "includes" => {
                let target = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let arr = arr_rc.borrow();
                Some(JsValue::Bool(arr.iter().any(|x| js_eq(x, &target))))
            }
            "join"   => {
                let sep = args.get(0).map(|v| v.to_string_val()).unwrap_or_else(|| ",".to_string());
                let arr = arr_rc.borrow();
                Some(JsValue::Str(arr.iter().map(|v| v.to_string_val()).collect::<Vec<_>>().join(&sep)))
            }
            "concat" => {
                let mut out = arr_rc.borrow().clone();
                for a in args {
                    match a {
                        JsValue::Array(b) => out.extend(b.borrow().iter().cloned()),
                        v => out.push(v.clone()),
                    }
                }
                Some(JsValue::Array(Rc::new(RefCell::new(out))))
            }
            "flat"   => {
                let depth = args.get(0).map(|v| v.to_number() as usize).unwrap_or(1);
                let arr = arr_rc.borrow().clone();
                Some(JsValue::Array(Rc::new(RefCell::new(flat_array(arr, depth)))))
            }
            "map" => {
                let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let items = arr_rc.borrow().clone();
                let mut out = Vec::new();
                for (i, v) in items.iter().enumerate() {
                    let args2 = [v.clone(), JsValue::Number(i as f64), arr_val.clone()];
                    out.push(self.call_value(cb.clone(), JsValue::Undefined, &args2));
                }
                Some(JsValue::Array(Rc::new(RefCell::new(out))))
            }
            "filter" => {
                let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let items = arr_rc.borrow().clone();
                let mut out = Vec::new();
                for (i, v) in items.iter().enumerate() {
                    let args2 = [v.clone(), JsValue::Number(i as f64), arr_val.clone()];
                    if self.call_value(cb.clone(), JsValue::Undefined, &args2).is_truthy() { out.push(v.clone()); }
                }
                Some(JsValue::Array(Rc::new(RefCell::new(out))))
            }
            "reduce" => {
                let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let items = arr_rc.borrow().clone();
                let mut acc = args.get(1).cloned().unwrap_or_else(|| items.first().cloned().unwrap_or(JsValue::Undefined));
                let start = if args.len() > 1 { 0 } else { 1 };
                for (i, v) in items.iter().enumerate().skip(start) {
                    let args2 = [acc, v.clone(), JsValue::Number(i as f64), arr_val.clone()];
                    acc = self.call_value(cb.clone(), JsValue::Undefined, &args2);
                }
                Some(acc)
            }
            "forEach" => {
                let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let items = arr_rc.borrow().clone();
                for (i, v) in items.iter().enumerate() {
                    let args2 = [v.clone(), JsValue::Number(i as f64), arr_val.clone()];
                    self.call_value(cb.clone(), JsValue::Undefined, &args2);
                }
                Some(JsValue::Undefined)
            }
            "find"   => {
                let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let items = arr_rc.borrow().clone();
                for (i, v) in items.iter().enumerate() {
                    let args2 = [v.clone(), JsValue::Number(i as f64), arr_val.clone()];
                    if self.call_value(cb.clone(), JsValue::Undefined, &args2).is_truthy() { return Some(v.clone()); }
                }
                Some(JsValue::Undefined)
            }
            "findIndex" => {
                let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let items = arr_rc.borrow().clone();
                for (i, v) in items.iter().enumerate() {
                    let args2 = [v.clone(), JsValue::Number(i as f64), arr_val.clone()];
                    if self.call_value(cb.clone(), JsValue::Undefined, &args2).is_truthy() { return Some(JsValue::Number(i as f64)); }
                }
                Some(JsValue::Number(-1.0))
            }
            "some"   => {
                let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let items = arr_rc.borrow().clone();
                for (i, v) in items.iter().enumerate() {
                    let args2 = [v.clone(), JsValue::Number(i as f64), arr_val.clone()];
                    if self.call_value(cb.clone(), JsValue::Undefined, &args2).is_truthy() { return Some(JsValue::Bool(true)); }
                }
                Some(JsValue::Bool(false))
            }
            "every"  => {
                let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let items = arr_rc.borrow().clone();
                for (i, v) in items.iter().enumerate() {
                    let args2 = [v.clone(), JsValue::Number(i as f64), arr_val.clone()];
                    if !self.call_value(cb.clone(), JsValue::Undefined, &args2).is_truthy() { return Some(JsValue::Bool(false)); }
                }
                Some(JsValue::Bool(true))
            }
            "flatMap" => {
                let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let items = arr_rc.borrow().clone();
                let mut out = Vec::new();
                for (i, v) in items.iter().enumerate() {
                    let args2 = [v.clone(), JsValue::Number(i as f64), arr_val.clone()];
                    let r = self.call_value(cb.clone(), JsValue::Undefined, &args2);
                    match r { JsValue::Array(a) => out.extend(a.borrow().iter().cloned()), v => out.push(v) }
                }
                Some(JsValue::Array(Rc::new(RefCell::new(out))))
            }
            "sort"   => {
                let cb = args.get(0).cloned();
                let mut items = arr_rc.borrow().clone();
                // Simple insertion sort (stable, works with callbacks)
                let n = items.len();
                for i in 1..n {
                    let mut j = i;
                    while j > 0 {
                        let cmp = if let Some(ref f) = cb {
                            let args2 = [items[j-1].clone(), items[j].clone()];
                            self.call_value(f.clone(), JsValue::Undefined, &args2).to_number() > 0.0
                        } else {
                            items[j-1].to_string_val() > items[j].to_string_val()
                        };
                        if cmp { items.swap(j-1, j); j -= 1; } else { break; }
                    }
                }
                *arr_rc.borrow_mut() = items;
                Some(arr_val.clone())
            }
            "fill"   => {
                let val = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let mut arr = arr_rc.borrow_mut();
                let len = arr.len() as isize;
                let s = to_abs_index(args.get(1).map(|v| v.to_number() as isize).unwrap_or(0), len);
                let e = to_abs_index(args.get(2).map(|v| v.to_number() as isize).unwrap_or(len), len);
                for i in s..e.min(arr.len()) { arr[i] = val.clone(); }
                Some(arr_val.clone())
            }
            "keys"   => {
                let len = arr_rc.borrow().len();
                let keys: Vec<JsValue> = (0..len).map(|i| JsValue::Number(i as f64)).collect();
                Some(JsValue::Array(Rc::new(RefCell::new(keys))))
            }
            "values" => Some(arr_val.clone()),
            "entries" => {
                let items = arr_rc.borrow().clone();
                let entries: Vec<JsValue> = items.iter().enumerate()
                    .map(|(i, v)| JsValue::Array(Rc::new(RefCell::new(vec![JsValue::Number(i as f64), v.clone()]))))
                    .collect();
                Some(JsValue::Array(Rc::new(RefCell::new(entries))))
            }
            "toString" => {
                let arr = arr_rc.borrow();
                Some(JsValue::Str(arr.iter().map(|v| v.to_string_val()).collect::<Vec<_>>().join(",")))
            }
            _ => None,
        }
    }

    // ── Template literal interpolation ──────────────────────────────────────

    fn eval_template(&mut self, s: &str) -> JsValue {
        let bytes = s.as_bytes();
        let mut result = String::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                i += 2; // skip `${`
                let start = i;
                let mut depth = 1i32;
                while i < bytes.len() {
                    match bytes[i] {
                        b'{' => { depth += 1; i += 1; }
                        b'}' => { depth -= 1; if depth == 0 { break; } i += 1; }
                        _    => { i += 1; }
                    }
                }
                // Extract expression string
                if let Ok(expr_str) = core::str::from_utf8(&bytes[start..i]) {
                    let val = self.run(expr_str.trim());
                    result.push_str(&val.to_string_val());
                }
                if i < bytes.len() { i += 1; } // skip closing `}`
            } else if bytes[i] == b'\\' && i + 1 < bytes.len() {
                // Handle common escape sequences in templates
                match bytes[i + 1] {
                    b'n'  => { result.push('\n'); i += 2; }
                    b'r'  => { result.push('\r'); i += 2; }
                    b't'  => { result.push('\t'); i += 2; }
                    b'\\' => { result.push('\\'); i += 2; }
                    b'`'  => { result.push('`');  i += 2; }
                    _     => { result.push(bytes[i] as char); i += 1; }
                }
            } else {
                result.push(bytes[i] as char);
                i += 1;
            }
        }
        JsValue::Str(result)
    }

    // ── Map methods ──────────────────────────────────────────────────────────

    fn call_map_method(&mut self, map: JsValue, method: &str, args: &[JsValue]) -> Option<JsValue> {
        let obj_rc = match &map { JsValue::Object(o) => o.clone(), _ => return None };
        let entries_val = obj_rc.borrow().get("__entries__");
        let entries_rc = match entries_val { JsValue::Array(a) => a, _ => return None };

        match method {
            "set" => {
                let k = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let v = args.get(1).cloned().unwrap_or(JsValue::Undefined);
                let mut ents = entries_rc.borrow_mut();
                // Update existing entry if key exists
                for entry in ents.iter_mut() {
                    if let JsValue::Array(pair) = entry {
                        if js_eq(&pair.borrow()[0], &k) {
                            if pair.borrow().len() > 1 { pair.borrow_mut()[1] = v.clone(); }
                            else { pair.borrow_mut().push(v.clone()); }
                            return Some(map.clone());
                        }
                    }
                }
                // Add new entry
                ents.push(JsValue::Array(Rc::new(RefCell::new(vec![k, v]))));
                Some(map.clone())
            }
            "get" => {
                let k = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let ents = entries_rc.borrow();
                for entry in ents.iter() {
                    if let JsValue::Array(pair) = entry {
                        if js_eq(&pair.borrow()[0], &k) {
                            return Some(pair.borrow().get(1).cloned().unwrap_or(JsValue::Undefined));
                        }
                    }
                }
                Some(JsValue::Undefined)
            }
            "has" => {
                let k = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let ents = entries_rc.borrow();
                Some(JsValue::Bool(ents.iter().any(|e| {
                    if let JsValue::Array(pair) = e { js_eq(&pair.borrow()[0], &k) } else { false }
                })))
            }
            "delete" => {
                let k = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let mut ents = entries_rc.borrow_mut();
                let before = ents.len();
                ents.retain(|e| {
                    if let JsValue::Array(pair) = e { !js_eq(&pair.borrow()[0], &k) } else { true }
                });
                Some(JsValue::Bool(ents.len() < before))
            }
            "clear" => { entries_rc.borrow_mut().clear(); Some(JsValue::Undefined) }
            "size"  => Some(JsValue::Number(entries_rc.borrow().len() as f64)),
            "keys"  => {
                let ents = entries_rc.borrow();
                let keys: Vec<JsValue> = ents.iter().filter_map(|e| {
                    if let JsValue::Array(p) = e { p.borrow().get(0).cloned() } else { None }
                }).collect();
                Some(JsValue::Array(Rc::new(RefCell::new(keys))))
            }
            "values" => {
                let ents = entries_rc.borrow();
                let vals: Vec<JsValue> = ents.iter().filter_map(|e| {
                    if let JsValue::Array(p) = e { p.borrow().get(1).cloned() } else { None }
                }).collect();
                Some(JsValue::Array(Rc::new(RefCell::new(vals))))
            }
            "entries" => Some(JsValue::Array(entries_rc.clone())),
            "forEach" => {
                let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let items = entries_rc.borrow().clone();
                for entry in &items {
                    if let JsValue::Array(pair) = entry {
                        let k = pair.borrow().get(0).cloned().unwrap_or(JsValue::Undefined);
                        let v = pair.borrow().get(1).cloned().unwrap_or(JsValue::Undefined);
                        self.call_value(cb.clone(), JsValue::Undefined, &[v, k, map.clone()]);
                    }
                }
                Some(JsValue::Undefined)
            }
            _ => None,
        }
    }

    // ── Set methods ──────────────────────────────────────────────────────────

    fn call_set_method(&mut self, set: JsValue, method: &str, args: &[JsValue]) -> Option<JsValue> {
        let obj_rc = match &set { JsValue::Object(o) => o.clone(), _ => return None };
        let entries_val = obj_rc.borrow().get("__entries__");
        let entries_rc = match entries_val { JsValue::Array(a) => a, _ => return None };

        match method {
            "add" => {
                let v = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let already = entries_rc.borrow().iter().any(|e| js_eq(e, &v));
                if !already { entries_rc.borrow_mut().push(v); }
                Some(set.clone())
            }
            "has"    => {
                let v = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                Some(JsValue::Bool(entries_rc.borrow().iter().any(|e| js_eq(e, &v))))
            }
            "delete" => {
                let v = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let before = entries_rc.borrow().len();
                entries_rc.borrow_mut().retain(|e| !js_eq(e, &v));
                Some(JsValue::Bool(entries_rc.borrow().len() < before))
            }
            "clear"  => { entries_rc.borrow_mut().clear(); Some(JsValue::Undefined) }
            "size"   => Some(JsValue::Number(entries_rc.borrow().len() as f64)),
            "values" => Some(JsValue::Array(entries_rc.clone())),
            "keys"   => Some(JsValue::Array(entries_rc.clone())), // same as values for Set
            "entries" => {
                let items = entries_rc.borrow().clone();
                let pairs: Vec<JsValue> = items.iter().map(|v| {
                    JsValue::Array(Rc::new(RefCell::new(vec![v.clone(), v.clone()])))
                }).collect();
                Some(JsValue::Array(Rc::new(RefCell::new(pairs))))
            }
            "forEach" => {
                let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let items = entries_rc.borrow().clone();
                for v in &items {
                    self.call_value(cb.clone(), JsValue::Undefined, &[v.clone(), v.clone(), set.clone()]);
                }
                Some(JsValue::Undefined)
            }
            _ => None,
        }
    }

    // ── Promise methods ──────────────────────────────────────────────────────

    fn call_promise_method(&mut self, promise: JsValue, method: &str, args: &[JsValue]) -> Option<JsValue> {
        let obj_rc = match &promise { JsValue::Object(o) => o.clone(), _ => return None };

        match method {
            "then" => {
                let state        = obj_rc.borrow().get("__state__").to_string_val();
                let value        = obj_rc.borrow().get("__value__");
                let on_fulfilled = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let on_rejected  = args.get(1).cloned().unwrap_or(JsValue::Undefined);

                let result = if state == "fulfilled" {
                    if on_fulfilled.type_of() == "function" {
                        self.call_value(on_fulfilled, JsValue::Undefined, &[value])
                    } else { value }
                } else if state == "rejected" {
                    if on_rejected.type_of() == "function" {
                        self.call_value(on_rejected, JsValue::Undefined, &[value])
                    } else {
                        return Some(promise.clone()); // propagate rejection
                    }
                } else {
                    // "pending" — in our sync kernel this shouldn't happen for
                    // standard Promises, but schedule the callback anyway
                    if on_fulfilled.type_of() == "function" {
                        self.microtask_queue.push(on_fulfilled);
                    }
                    return Some(js_resolved_promise(JsValue::Undefined));
                };
                // If result is itself a Promise, return it as-is
                if let JsValue::Object(ref o) = result {
                    if o.borrow().class == "Promise" { return Some(result); }
                }
                Some(js_resolved_promise(result))
            }
            "catch" => {
                let state       = obj_rc.borrow().get("__state__").to_string_val();
                let value       = obj_rc.borrow().get("__value__");
                let on_rejected = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                if state == "rejected" && on_rejected.type_of() == "function" {
                    let result = self.call_value(on_rejected, JsValue::Undefined, &[value]);
                    if let JsValue::Object(ref o) = result {
                        if o.borrow().class == "Promise" { return Some(result); }
                    }
                    Some(js_resolved_promise(result))
                } else {
                    Some(promise.clone())
                }
            }
            "finally" => {
                let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                if cb.type_of() == "function" {
                    self.call_value(cb, JsValue::Undefined, &[]);
                }
                Some(promise.clone())
            }
            _ => None,
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn object_keys(&self, val: &JsValue) -> Vec<String> {
        match val {
            JsValue::Object(o) => o.borrow().props.keys().cloned().collect(),
            JsValue::Array(a)  => (0..a.borrow().len()).map(|i| format!("{}", i)).collect(),
            _ => Vec::new(),
        }
    }

    pub fn to_array_vals(&self, val: &JsValue) -> Vec<JsValue> {
        match val {
            JsValue::Array(a) => a.borrow().clone(),
            JsValue::Str(s)   => s.chars().map(|c| JsValue::Str(c.to_string())).collect(),
            JsValue::Object(o) => {
                let class = o.borrow().class.clone();
                match class.as_str() {
                    // Set: iterate values
                    "Set" => {
                        if let JsValue::Array(entries) = o.borrow().get("__entries__") {
                            entries.borrow().clone()
                        } else { Vec::new() }
                    }
                    // Map: iterate [key, value] pairs
                    "Map" => {
                        if let JsValue::Array(entries) = o.borrow().get("__entries__") {
                            entries.borrow().clone()
                        } else { Vec::new() }
                    }
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    // ── Phase 105: RTCPeerConnection method dispatch ──────────────────────────

    fn call_rtc_pc_method(&mut self, pc: JsValue, method: &str, args: &[JsValue]) -> Option<JsValue> {
        let obj_rc = match &pc { JsValue::Object(o) => o.clone(), _ => return None };
        match method {
            "createOffer" => {
                Some(js_resolved_promise(rtc_make_desc_obj("offer")))
            }
            "createAnswer" => {
                Some(js_resolved_promise(rtc_make_desc_obj("answer")))
            }
            "setLocalDescription" => {
                let desc = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let sdp_type = match &desc {
                    JsValue::Object(o) => o.borrow().get("type").to_string_val(),
                    _ => "offer".to_string(),
                };
                let new_state = if sdp_type == "offer" { "have-local-offer" }
                                else { "stable" };
                obj_rc.borrow_mut().set("signalingState".to_string(),     JsValue::Str(new_state.to_string()));
                obj_rc.borrow_mut().set("iceGatheringState".to_string(),  JsValue::Str("complete".to_string()));
                obj_rc.borrow_mut().set("localDescription".to_string(),   desc);
                Some(js_resolved_promise(JsValue::Undefined))
            }
            "setRemoteDescription" => {
                let desc = args.get(0).cloned().unwrap_or(JsValue::Undefined);
                let sdp_type = match &desc {
                    JsValue::Object(o) => o.borrow().get("type").to_string_val(),
                    _ => "answer".to_string(),
                };
                let new_state = if sdp_type == "offer" { "have-remote-offer" }
                                else { "stable" };
                obj_rc.borrow_mut().set("signalingState".to_string(),      JsValue::Str(new_state.to_string()));
                if new_state == "stable" {
                    obj_rc.borrow_mut().set("iceConnectionState".to_string(), JsValue::Str("connected".to_string()));
                }
                obj_rc.borrow_mut().set("remoteDescription".to_string(),   desc);
                Some(js_resolved_promise(JsValue::Undefined))
            }
            "addIceCandidate" => Some(js_resolved_promise(JsValue::Undefined)),
            "close" => {
                obj_rc.borrow_mut().set("signalingState".to_string(),      JsValue::Str("closed".to_string()));
                obj_rc.borrow_mut().set("iceConnectionState".to_string(),  JsValue::Str("closed".to_string()));
                obj_rc.borrow_mut().set("connectionState".to_string(),     JsValue::Str("closed".to_string()));
                Some(JsValue::Undefined)
            }
            "addEventListener" | "removeEventListener" | "addTrack" | "removeTrack" => {
                Some(JsValue::Undefined)
            }
            "getReceivers" | "getSenders" | "getTransceivers" => {
                Some(JsValue::Array(Rc::new(RefCell::new(Vec::new()))))
            }
            "getStats" => Some(js_resolved_promise(JsValue::Undefined)),
            _ => None,
        }
    }

    // ── Phase 105: MediaStream method dispatch ────────────────────────────────

    fn call_media_stream_method(&mut self, stream: JsValue, method: &str, _args: &[JsValue]) -> Option<JsValue> {
        let obj_rc = match &stream { JsValue::Object(o) => o.clone(), _ => return None };
        let tracks_val = obj_rc.borrow().get("__tracks__");
        match method {
            "getTracks" => {
                match tracks_val {
                    JsValue::Array(a) => Some(JsValue::Array(a)),
                    _ => Some(JsValue::Array(Rc::new(RefCell::new(Vec::new())))),
                }
            }
            "getAudioTracks" => {
                let audio: Vec<JsValue> = match &tracks_val {
                    JsValue::Array(a) => a.borrow().iter().filter(|t| {
                        if let JsValue::Object(o) = t {
                            o.borrow().get("kind").to_string_val() == "audio"
                        } else { false }
                    }).cloned().collect(),
                    _ => Vec::new(),
                };
                Some(JsValue::Array(Rc::new(RefCell::new(audio))))
            }
            "getVideoTracks" => {
                let video: Vec<JsValue> = match &tracks_val {
                    JsValue::Array(a) => a.borrow().iter().filter(|t| {
                        if let JsValue::Object(o) = t {
                            o.borrow().get("kind").to_string_val() == "video"
                        } else { false }
                    }).cloned().collect(),
                    _ => Vec::new(),
                };
                Some(JsValue::Array(Rc::new(RefCell::new(video))))
            }
            "addTrack" | "removeTrack" | "addEventListener" | "removeEventListener" => {
                Some(JsValue::Undefined)
            }
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Comparison helpers
// ─────────────────────────────────────────────────────────────────────────────

fn js_eq(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Undefined, JsValue::Undefined) => true,
        (JsValue::Null,      JsValue::Null)      => true,
        (JsValue::Undefined, JsValue::Null) | (JsValue::Null, JsValue::Undefined) => true,
        (JsValue::Number(x), JsValue::Number(y)) => x == y,
        (JsValue::Bool(x), JsValue::Bool(y))     => x == y,
        (JsValue::Str(x),  JsValue::Str(y))      => x == y,
        (JsValue::Number(x), JsValue::Str(s)) | (JsValue::Str(s), JsValue::Number(x)) => {
            s.parse::<f64>().map(|y| *x == y).unwrap_or(false)
        }
        _ => false,
    }
}

fn js_lt(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Number(x), JsValue::Number(y)) => x < y,
        (JsValue::Str(x),    JsValue::Str(y))    => x < y,
        _ => a.to_number() < b.to_number(),
    }
}

fn js_pow(base: f64, exp: f64) -> f64 {
    if exp == 0.0 { return 1.0; }
    if exp < 0.0  { return 1.0 / js_pow(base, -exp); }
    let n = exp as u32;
    if n as f64 == exp {
        let mut r = 1.0;
        let mut b = base;
        let mut e = n;
        while e > 0 { if e & 1 == 1 { r *= b; } b *= b; e >>= 1; }
        r
    } else {
        // approximate: e^(exp * ln(base))  via identity — placeholder
        let ln_base = if base > 0.0 { js_ln(base) } else { f64::NAN };
        js_exp(exp * ln_base)
    }
}

fn js_ln(x: f64) -> f64 {
    // Natural log approximation (for non-integer pow)
    if x <= 0.0 { return f64::NAN; }
    if x == 1.0 { return 0.0; }
    // Reduce: x = m * 2^k, ln(x) = ln(m) + k*ln(2)
    let ln2 = 0.6931471805599453_f64;
    let mut m = x;
    let mut k = 0i32;
    while m < 0.5 { m *= 2.0; k -= 1; }
    while m > 1.0 { m /= 2.0; k += 1; }
    // Halley's method on [0.5, 1]
    let y = (m - 1.0) / (m + 1.0);
    let y2 = y * y;
    let series = y * (2.0 + y2 * (2.0/3.0 + y2 * (2.0/5.0 + y2 * 2.0/7.0)));
    series + k as f64 * ln2
}

fn js_exp(x: f64) -> f64 {
    if x == 0.0 { return 1.0; }
    if x < 0.0  { return 1.0 / js_exp(-x); }
    // Taylor: sum x^n / n!
    let mut r = 1.0; let mut term = 1.0;
    for n in 1..50u32 { term *= x / n as f64; r += term; if term < 1e-15 { break; } }
    r
}

fn to_abs_index(i: isize, len: isize) -> usize {
    if i < 0 { (len + i).max(0) as usize } else { i.min(len) as usize }
}

fn flat_array(arr: Vec<JsValue>, depth: usize) -> Vec<JsValue> {
    if depth == 0 { return arr; }
    let mut out = Vec::new();
    for v in arr {
        if let JsValue::Array(a) = v {
            out.extend(flat_array(a.borrow().clone(), depth - 1));
        } else { out.push(v); }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
//  Phase 123 — JIT lowering pass
//  Tries to translate a simple JS function body to JitOps.
//  Only pure-integer arithmetic is supported; anything else returns None so the
//  tree-walker can handle it as a fallback.
// ─────────────────────────────────────────────────────────────────────────────

fn lower_expr_to_jit(
    expr: &Expr,
    param_map: &BTreeMap<String, u32>,
    local_map:  &BTreeMap<String, u32>,
    ops:        &mut Vec<JitOp>,
) -> bool {
    match expr {
        Expr::Number(n) => {
            let i = *n as i64;
            if (i as f64) == *n {
                ops.push(JitOp::PushInt(i));
                true
            } else {
                false // floating-point constant — bail
            }
        }
        Expr::Ident(name) => {
            if let Some(&slot) = param_map.get(name) {
                ops.push(JitOp::LoadLocal(slot));
                true
            } else if let Some(&slot) = local_map.get(name) {
                ops.push(JitOp::LoadLocal(slot));
                true
            } else {
                false // free variable — bail
            }
        }
        Expr::Binary { op, left, right } => {
            let jop = match op {
                BinaryOp::Add              => JitOp::AddInt,
                BinaryOp::Sub              => JitOp::SubInt,
                BinaryOp::Mul              => JitOp::MulInt,
                BinaryOp::Div              => JitOp::DivInt,
                BinaryOp::Lt              => JitOp::CmpLt,
                BinaryOp::LtEq            => JitOp::CmpLe,
                BinaryOp::Eq | BinaryOp::StrictEq => JitOp::CmpEq,
                _ => return false,
            };
            if !lower_expr_to_jit(left,  param_map, local_map, ops) { return false; }
            if !lower_expr_to_jit(right, param_map, local_map, ops) { return false; }
            ops.push(jop);
            true
        }
        Expr::Unary { op: UnaryOp::Neg, expr } => {
            if !lower_expr_to_jit(expr, param_map, local_map, ops) { return false; }
            ops.push(JitOp::Neg);
            true
        }
        _ => false,
    }
}

/// Try to compile a JS function body to a flat JitOp slice.
/// - Params become locals[0..params.len()].
/// - Only handles `return <expr>`, `var x = <expr>`, and simple expression stmts.
/// - Returns None if any statement or sub-expression is not JIT-able.
fn lower_fn_to_jit(params: &[String], body: &[Stmt]) -> Option<Vec<JitOp>> {
    let mut param_map: BTreeMap<String, u32> = BTreeMap::new();
    for (i, p) in params.iter().enumerate() {
        param_map.insert(p.clone(), i as u32);
    }
    let mut local_map: BTreeMap<String, u32> = BTreeMap::new();
    let mut next_local = params.len() as u32;
    let mut ops: Vec<JitOp> = Vec::new();

    for stmt in body {
        match stmt {
            Stmt::Return(Some(expr)) => {
                if !lower_expr_to_jit(expr, &param_map, &local_map, &mut ops) {
                    return None;
                }
                ops.push(JitOp::Return);
            }
            Stmt::Return(None) => {
                ops.push(JitOp::PushInt(0));
                ops.push(JitOp::Return);
            }
            Stmt::VarDecl { name, init: Some(init_expr), .. } => {
                if !lower_expr_to_jit(init_expr, &param_map, &local_map, &mut ops) {
                    return None;
                }
                let slot = next_local;
                next_local += 1;
                local_map.insert(name.clone(), slot);
                ops.push(JitOp::StoreLocal(slot));
            }
            Stmt::Expr(e) => {
                if !lower_expr_to_jit(e, &param_map, &local_map, &mut ops) {
                    return None;
                }
                ops.push(JitOp::Pop);
            }
            _ => return None, // complex control flow — fall back to tree-walker
        }
    }

    if ops.is_empty() { return None; }
    Some(ops)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Native function implementations
// ─────────────────────────────────────────────────────────────────────────────

fn native_console_log(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let msg: String = args.iter().map(|v| v.to_string_val()).collect::<Vec<_>>().join(" ");
    crate::serial_println!("[js] {}", msg);
    interp.output.push(msg);
    JsValue::Undefined
}

fn native_math_floor(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let n = args.get(0).map(|v| v.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(js_floor(n))
}
fn native_math_ceil(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let n = args.get(0).map(|v| v.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(js_ceil(n))
}
fn native_math_round(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let n = args.get(0).map(|v| v.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(js_floor(n + 0.5))
}
fn native_math_abs(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let n = args.get(0).map(|v| v.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(if n < 0.0 { -n } else { n })
}
fn native_math_sqrt(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let n = args.get(0).map(|v| v.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(js_sqrt(n))
}
fn native_math_max(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let v = args.iter().map(|v| v.to_number()).fold(f64::NEG_INFINITY, f64::max);
    JsValue::Number(v)
}
fn native_math_min(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let v = args.iter().map(|v| v.to_number()).fold(f64::INFINITY, f64::min);
    JsValue::Number(v)
}
fn native_math_pow(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let b = args.get(0).map(|v| v.to_number()).unwrap_or(0.0);
    let e = args.get(1).map(|v| v.to_number()).unwrap_or(0.0);
    JsValue::Number(js_pow(b, e))
}
fn native_math_log(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Number(js_ln(args.get(0).map(|v| v.to_number()).unwrap_or(f64::NAN)))
}
fn native_math_sin(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Number(js_sin(args.get(0).map(|v| v.to_number()).unwrap_or(0.0)))
}
fn native_math_cos(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Number(js_cos(args.get(0).map(|v| v.to_number()).unwrap_or(0.0)))
}
fn native_math_tan(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let x = args.get(0).map(|v| v.to_number()).unwrap_or(0.0);
    JsValue::Number(js_sin(x) / js_cos(x))
}
fn native_math_random(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    // Pseudo-random using a simple LCG seeded from tick counter
    static SEED: spin::Mutex<u64> = spin::Mutex::new(12345678901234567);
    let mut s = SEED.lock();
    *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let r = ((*s >> 33) as f64) / (u32::MAX as f64);
    JsValue::Number(r)
}
fn native_math_trunc(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let n = args.get(0).map(|v| v.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(n as i64 as f64)
}
fn native_math_sign(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let n = args.get(0).map(|v| v.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(if n > 0.0 { 1.0 } else if n < 0.0 { -1.0 } else { 0.0 })
}

fn native_parse_int(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let s = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let radix = args.get(1).map(|v| v.to_number() as u32).unwrap_or(10);
    let s = s.trim();
    let (neg, rest) = if s.starts_with('-') { (true, &s[1..]) } else { (false, s) };
    let rest = if rest.starts_with("0x") || rest.starts_with("0X") { &rest[2..] } else { rest };
    let n = i64::from_str_radix(rest.split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap_or(""), radix as u32).unwrap_or(0);
    JsValue::Number(if neg { -(n as f64) } else { n as f64 })
}
fn native_parse_float(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let s = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    JsValue::Number(s.trim().parse().unwrap_or(f64::NAN))
}
fn native_is_nan(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Bool(args.get(0).map(|v| v.to_number().is_nan()).unwrap_or(true))
}
fn native_is_finite(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Bool(args.get(0).map(|v| v.to_number().is_finite()).unwrap_or(false))
}
fn native_to_string(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Str(args.get(0).map(|v| v.to_string_val()).unwrap_or_default())
}
fn native_to_number(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Number(args.get(0).map(|v| v.to_number()).unwrap_or(0.0))
}
fn native_to_bool(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Bool(args.get(0).map(|v| v.is_truthy()).unwrap_or(false))
}
fn native_make_array(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    if args.len() == 1 {
        if let JsValue::Number(n) = &args[0] {
            let len = *n as usize;
            return JsValue::Array(Rc::new(RefCell::new(vec![JsValue::Undefined; len])));
        }
    }
    JsValue::Array(Rc::new(RefCell::new(args.to_vec())))
}
fn native_make_object(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Object(Rc::new(RefCell::new(JsObject::new())))
}
fn native_json_stringify(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    fn to_json(v: &JsValue) -> String {
        match v {
            JsValue::Undefined | JsValue::NativeFunction(_, _) | JsValue::Function(_) => "undefined".to_string(),
            JsValue::Null    => "null".to_string(),
            JsValue::Bool(b) => if *b { "true".to_string() } else { "false".to_string() },
            JsValue::Number(n) => fmt_number(*n),
            JsValue::Str(s)  => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
            JsValue::Array(a) => {
                let items: Vec<String> = a.borrow().iter().map(to_json).collect();
                format!("[{}]", items.join(","))
            }
            JsValue::Object(o) => {
                let props: Vec<String> = o.borrow().props.iter()
                    .map(|(k, v)| format!("\"{}\":{}", k, to_json(v)))
                    .collect();
                format!("{{{}}}", props.join(","))
            }
        }
    }
    JsValue::Str(to_json(args.get(0).unwrap_or(&JsValue::Undefined)))
}
fn native_json_parse(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    // Minimal JSON parser (numbers, strings, arrays, objects, booleans, null)
    let s = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    parse_json(s.trim())
}

fn parse_json(s: &str) -> JsValue {
    let s = s.trim();
    if s == "null"  { return JsValue::Null; }
    if s == "true"  { return JsValue::Bool(true); }
    if s == "false" { return JsValue::Bool(false); }
    if s.starts_with('"') {
        return JsValue::Str(s[1..s.len()-1].to_string());
    }
    if let Ok(n) = s.parse::<f64>() { return JsValue::Number(n); }
    if s.starts_with('[') {
        let inner = &s[1..s.len()-1];
        let parts = split_json_array(inner);
        return JsValue::Array(Rc::new(RefCell::new(parts.iter().map(|p| parse_json(p)).collect())));
    }
    if s.starts_with('{') {
        let obj = Rc::new(RefCell::new(JsObject::new()));
        let inner = &s[1..s.len()-1];
        for part in split_json_array(inner) {
            if let Some(colon) = part.find(':') {
                let k = part[..colon].trim().trim_matches('"').to_string();
                let v = parse_json(part[colon+1..].trim());
                obj.borrow_mut().set(k, v);
            }
        }
        return JsValue::Object(obj);
    }
    JsValue::Undefined
}

fn split_json_array(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            if c == b'\\' { i += 1; }
            else if c == b'"' { in_str = false; }
        } else {
            match c {
                b'"'            => { in_str = true; }
                b'[' | b'{'     => { depth += 1; }
                b']' | b'}'     => { depth -= 1; }
                b',' if depth == 0 => {
                    parts.push(s[start..i].trim().to_string());
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    if start < s.len() {
        let last = s[start..].trim().to_string();
        if !last.is_empty() { parts.push(last); }
    }
    parts
}

// ─────────────────────────────────────────────────────────────────────────────
//  New native functions (Phase 38+)
// ─────────────────────────────────────────────────────────────────────────────

// ── Object static methods ────────────────────────────────────────────────────

fn native_obj_keys(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let keys: Vec<JsValue> = match args.get(0) {
        Some(JsValue::Object(o)) => o.borrow().props.keys().map(|k| JsValue::Str(k.clone())).collect(),
        Some(JsValue::Array(a))  => (0..a.borrow().len()).map(|i| JsValue::Str(format!("{}", i))).collect(),
        _ => Vec::new(),
    };
    JsValue::Array(Rc::new(RefCell::new(keys)))
}
fn native_obj_values(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let vals: Vec<JsValue> = match args.get(0) {
        Some(JsValue::Object(o)) => o.borrow().props.values().cloned().collect(),
        Some(JsValue::Array(a))  => a.borrow().clone(),
        _ => Vec::new(),
    };
    JsValue::Array(Rc::new(RefCell::new(vals)))
}
fn native_obj_entries(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let entries: Vec<JsValue> = match args.get(0) {
        Some(JsValue::Object(o)) => o.borrow().props.iter()
            .map(|(k, v)| JsValue::Array(Rc::new(RefCell::new(vec![JsValue::Str(k.clone()), v.clone()]))))
            .collect(),
        Some(JsValue::Array(a)) => a.borrow().iter().enumerate()
            .map(|(i, v)| JsValue::Array(Rc::new(RefCell::new(vec![JsValue::Str(format!("{}", i)), v.clone()]))))
            .collect(),
        _ => Vec::new(),
    };
    JsValue::Array(Rc::new(RefCell::new(entries)))
}
fn native_obj_assign(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let target = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    if let JsValue::Object(ref target_obj) = target {
        for src in args.iter().skip(1) {
            if let JsValue::Object(src_obj) = src {
                for (k, v) in src_obj.borrow().props.iter() {
                    target_obj.borrow_mut().set(k.clone(), v.clone());
                }
            }
        }
    }
    target
}
fn native_obj_from_entries(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    if let Some(JsValue::Array(arr)) = args.get(0) {
        for entry in arr.borrow().iter() {
            if let JsValue::Array(pair) = entry {
                let k = pair.borrow().get(0).map(|v| v.to_string_val()).unwrap_or_default();
                let v = pair.borrow().get(1).cloned().unwrap_or(JsValue::Undefined);
                obj.borrow_mut().set(k, v);
            }
        }
    }
    JsValue::Object(obj)
}
fn native_obj_create(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    // Object.create(proto) — create an object with proto's props as own props
    let obj = Rc::new(RefCell::new(JsObject::new()));
    if let Some(JsValue::Object(proto)) = args.get(0) {
        for (k, v) in proto.borrow().props.iter() {
            obj.borrow_mut().set(k.clone(), v.clone());
        }
    }
    JsValue::Object(obj)
}
fn native_obj_freeze(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    args.get(0).cloned().unwrap_or(JsValue::Undefined) // stub: return the object as-is
}
fn native_obj_is_frozen(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Bool(false) }
fn native_obj_has_own(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let has = match args.get(0) {
        Some(JsValue::Object(o)) => {
            let k = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
            o.borrow().props.contains_key(&k)
        }
        _ => false,
    };
    JsValue::Bool(has)
}
fn native_obj_define_property(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    // Object.defineProperty(obj, key, descriptor) — minimal: just set the value
    if let (Some(JsValue::Object(o)), Some(key_v), Some(JsValue::Object(desc))) =
        (args.get(0), args.get(1), args.get(2))
    {
        let key = key_v.to_string_val();
        if let JsValue::Undefined = desc.borrow().get("value") {
            // getter/setter — stub
        } else {
            let val = desc.borrow().get("value");
            o.borrow_mut().set(key, val);
        }
    }
    args.get(0).cloned().unwrap_or(JsValue::Undefined)
}

// ── Array static methods ─────────────────────────────────────────────────────

fn native_array_is_array(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Bool(matches!(args.get(0), Some(JsValue::Array(_))))
}
fn native_array_from(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let src = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let map_fn = args.get(1).cloned();
    let items: Vec<JsValue> = match &src {
        JsValue::Array(a)  => a.borrow().clone(),
        JsValue::Str(s)    => s.chars().map(|c| JsValue::Str(c.to_string())).collect(),
        JsValue::Object(o) => {
            // Array-like: has .length
            if let JsValue::Number(n) = o.borrow().get("length") {
                let len = n as usize;
                (0..len).map(|i| o.borrow().get(&format!("{}", i))).collect()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    };
    let out: Vec<JsValue> = if let Some(cb) = map_fn {
        items.iter().enumerate().map(|(i, v)| {
            interp.call_value(cb.clone(), JsValue::Undefined, &[v.clone(), JsValue::Number(i as f64)])
        }).collect()
    } else { items };
    JsValue::Array(Rc::new(RefCell::new(out)))
}
fn native_array_of(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Array(Rc::new(RefCell::new(args.to_vec())))
}

// ── String static methods ────────────────────────────────────────────────────

fn native_string_from_char_code(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let s: String = args.iter().map(|v| {
        let code = v.to_number() as u32;
        char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER)
    }).collect();
    JsValue::Str(s)
}
fn native_string_raw(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    // String.raw`...` — just return the raw string; args[0] is the strings array
    match args.get(0) {
        Some(JsValue::Object(o)) => {
            if let JsValue::Array(raw) = o.borrow().get("raw") {
                let parts: Vec<String> = raw.borrow().iter().map(|v| v.to_string_val()).collect();
                // Interleave with substitutions (args[1..])
                let mut result = String::new();
                for (i, part) in parts.iter().enumerate() {
                    result.push_str(part);
                    if i + 1 < args.len() { result.push_str(&args[i + 1].to_string_val()); }
                }
                return JsValue::Str(result);
            }
            JsValue::Str(String::new())
        }
        Some(v) => JsValue::Str(v.to_string_val()),
        None => JsValue::Str(String::new()),
    }
}

// ── Number static methods ────────────────────────────────────────────────────

fn native_number_is_integer(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Bool(match args.get(0) {
        Some(JsValue::Number(n)) => n.is_finite() && *n == (*n as i64) as f64,
        _ => false,
    })
}

// ── Promise constructors & statics ───────────────────────────────────────────

fn native_promise_new(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let executor = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    // Clear side-channels
    PROMISE_RESOLVE_SLOT.lock().0 = None;
    PROMISE_REJECT_SLOT.lock().0  = None;
    // Call executor(resolve, reject)
    let resolve_fn = JsValue::NativeFunction("resolve", native_promise_do_resolve);
    let reject_fn  = JsValue::NativeFunction("reject",  native_promise_do_reject);
    interp.call_value(executor, JsValue::Undefined, &[resolve_fn, reject_fn]);
    // Build promise from resolution
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "Promise".to_string();
    if let Some(v) = PROMISE_RESOLVE_SLOT.lock().0.take() {
        obj.borrow_mut().set("__state__".to_string(), JsValue::Str("fulfilled".to_string()));
        obj.borrow_mut().set("__value__".to_string(), v);
    } else if let Some(v) = PROMISE_REJECT_SLOT.lock().0.take() {
        obj.borrow_mut().set("__state__".to_string(), JsValue::Str("rejected".to_string()));
        obj.borrow_mut().set("__value__".to_string(), v);
    } else {
        // Executor didn't call resolve/reject — treat as resolved with undefined
        obj.borrow_mut().set("__state__".to_string(), JsValue::Str("fulfilled".to_string()));
        obj.borrow_mut().set("__value__".to_string(), JsValue::Undefined);
    }
    JsValue::Object(obj)
}
fn native_promise_do_resolve(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    PROMISE_RESOLVE_SLOT.lock().0 = Some(args.get(0).cloned().unwrap_or(JsValue::Undefined));
    JsValue::Undefined
}
fn native_promise_do_reject(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    PROMISE_REJECT_SLOT.lock().0 = Some(args.get(0).cloned().unwrap_or(JsValue::Undefined));
    JsValue::Undefined
}
fn native_promise_resolve_static(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let v = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    if let JsValue::Object(ref o) = v { if o.borrow().class == "Promise" { return v; } }
    js_resolved_promise(v)
}
fn native_promise_reject_static(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    js_rejected_promise(args.get(0).cloned().unwrap_or(JsValue::Undefined))
}
fn native_promise_all(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let items = match args.get(0) { Some(JsValue::Array(a)) => a.borrow().clone(), _ => Vec::new() };
    let mut results = Vec::new();
    for p in &items {
        let val = promise_unwrap(p);
        if let JsValue::Object(ref o) = val { if o.borrow().class == "Promise" && o.borrow().get("__state__").to_string_val() == "rejected" { return val.clone(); } }
        results.push(val);
    }
    js_resolved_promise(JsValue::Array(Rc::new(RefCell::new(results))))
}
fn native_promise_all_settled(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let items = match args.get(0) { Some(JsValue::Array(a)) => a.borrow().clone(), _ => Vec::new() };
    let results: Vec<JsValue> = items.iter().map(|p| {
        let (state, value) = match p {
            JsValue::Object(o) if o.borrow().class == "Promise" => {
                (o.borrow().get("__state__").to_string_val(), o.borrow().get("__value__"))
            }
            v => ("fulfilled".to_string(), v.clone()),
        };
        let obj = Rc::new(RefCell::new(JsObject::new()));
        obj.borrow_mut().set("status".to_string(), JsValue::Str(state.clone()));
        if state == "fulfilled" { obj.borrow_mut().set("value".to_string(), value); }
        else { obj.borrow_mut().set("reason".to_string(), value); }
        JsValue::Object(obj)
    }).collect();
    js_resolved_promise(JsValue::Array(Rc::new(RefCell::new(results))))
}
fn native_promise_race(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    // Return first settled promise
    if let Some(JsValue::Array(a)) = args.get(0) {
        if let Some(first) = a.borrow().first() {
            return promise_unwrap_wrap(first);
        }
    }
    js_resolved_promise(JsValue::Undefined)
}

/// Phase 104: `Promise.withResolvers()` — ES2024.
/// Returns `{ promise, resolve, reject }` without a constructor executor.
fn native_promise_with_resolvers(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    let promise = Rc::new(RefCell::new(JsObject::new()));
    promise.borrow_mut().class = "Promise".to_string();
    promise.borrow_mut().set("__state__".to_string(), JsValue::Str("pending".to_string()));
    promise.borrow_mut().set("__value__".to_string(), JsValue::Undefined);
    let promise_val = JsValue::Object(promise.clone());

    // resolve and reject close over the promise object via the side-channel
    // (in a real engine they'd close over the PromiseCapability record;
    //  here we use the global slot pattern)
    let result = Rc::new(RefCell::new(JsObject::new()));
    result.borrow_mut().set("promise".to_string(), promise_val);
    result.borrow_mut().set("resolve".to_string(), JsValue::NativeFunction("resolve", native_promise_do_resolve));
    result.borrow_mut().set("reject".to_string(),  JsValue::NativeFunction("reject",  native_promise_do_reject));
    JsValue::Object(result)
}
fn promise_unwrap(v: &JsValue) -> JsValue {
    if let JsValue::Object(o) = v { if o.borrow().class == "Promise" { return o.borrow().get("__value__"); } }
    v.clone()
}
fn promise_unwrap_wrap(v: &JsValue) -> JsValue {
    if let JsValue::Object(o) = v { if o.borrow().class == "Promise" { return v.clone(); } }
    js_resolved_promise(v.clone())
}

// ── Map / Set constructors ───────────────────────────────────────────────────

fn native_map_new(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "Map".to_string();
    let entries: Vec<JsValue> = if let Some(JsValue::Array(arr)) = args.get(0) {
        arr.borrow().iter().filter_map(|e| {
            if let JsValue::Array(pair) = e {
                let k = pair.borrow().get(0).cloned().unwrap_or(JsValue::Undefined);
                let v = pair.borrow().get(1).cloned().unwrap_or(JsValue::Undefined);
                Some(JsValue::Array(Rc::new(RefCell::new(vec![k, v]))))
            } else { None }
        }).collect()
    } else { Vec::new() };
    obj.borrow_mut().set("__entries__".to_string(), JsValue::Array(Rc::new(RefCell::new(entries))));
    JsValue::Object(obj)
}
fn native_set_new(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "Set".to_string();
    let mut entries: Vec<JsValue> = Vec::new();
    if let Some(JsValue::Array(arr)) = args.get(0) {
        for v in arr.borrow().iter() {
            if !entries.iter().any(|e| js_eq(e, v)) { entries.push(v.clone()); }
        }
    }
    obj.borrow_mut().set("__entries__".to_string(), JsValue::Array(Rc::new(RefCell::new(entries))));
    JsValue::Object(obj)
}

// ── Symbol ────────────────────────────────────────────────────────────────────

fn native_symbol(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let desc = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    JsValue::Str(format!("Symbol({})", desc))
}

// ── Error ─────────────────────────────────────────────────────────────────────

fn native_make_error(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let msg = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "Error".to_string();
    obj.borrow_mut().set("message".to_string(), JsValue::Str(msg.clone()));
    obj.borrow_mut().set("stack".to_string(),   JsValue::Str(format!("Error: {}", msg)));
    obj.borrow_mut().set("name".to_string(),    JsValue::Str("Error".to_string()));
    JsValue::Object(obj)
}

// ── WeakRef stub ──────────────────────────────────────────────────────────────

fn native_weak_ref_new(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let target = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "WeakRef".to_string();
    obj.borrow_mut().set("__target__".to_string(), target.clone());
    obj.borrow_mut().set("deref".to_string(), JsValue::NativeFunction("deref", |a, _| {
        // Can't easily close over target; return undefined as stub
        let _ = a;
        JsValue::Undefined
    }));
    JsValue::Object(obj)
}

// ── URL / URLSearchParams stubs ───────────────────────────────────────────────

fn native_url_new(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let href = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "URL".to_string();
    obj.borrow_mut().set("href".to_string(),     JsValue::Str(href.clone()));
    obj.borrow_mut().set("hostname".to_string(), JsValue::Str(String::new()));
    obj.borrow_mut().set("pathname".to_string(), JsValue::Str("/".to_string()));
    obj.borrow_mut().set("search".to_string(),   JsValue::Str(String::new()));
    obj.borrow_mut().set("hash".to_string(),     JsValue::Str(String::new()));
    obj.borrow_mut().set("protocol".to_string(), JsValue::Str("https:".to_string()));
    obj.borrow_mut().set("origin".to_string(),   JsValue::Str(String::new()));
    obj.borrow_mut().set("toString".to_string(), JsValue::NativeFunction("toString", |_, _| JsValue::Str(String::new())));
    JsValue::Object(obj)
}
fn native_url_search_params_new(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "URLSearchParams".to_string();
    obj.borrow_mut().set("__entries__".to_string(), JsValue::Array(Rc::new(RefCell::new(Vec::new()))));
    // Parse init string if provided
    if let Some(JsValue::Str(s)) = args.get(0) {
        let s = s.trim_start_matches('?');
        let entries: Vec<JsValue> = s.split('&').filter_map(|pair| {
            let mut it = pair.splitn(2, '=');
            let k = it.next()?.to_string();
            let v = it.next().unwrap_or("").to_string();
            Some(JsValue::Array(Rc::new(RefCell::new(vec![JsValue::Str(k), JsValue::Str(v)]))))
        }).collect();
        obj.borrow_mut().set("__entries__".to_string(), JsValue::Array(Rc::new(RefCell::new(entries))));
    }
    JsValue::Object(obj)
}

// ── TextEncoder / TextDecoder stubs ──────────────────────────────────────────

fn native_text_encoder_new(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "TextEncoder".to_string();
    obj.borrow_mut().set("encoding".to_string(), JsValue::Str("utf-8".to_string()));
    obj.borrow_mut().set("encode".to_string(), JsValue::NativeFunction("encode", |args, _| {
        let s = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
        let bytes: Vec<JsValue> = s.bytes().map(|b| JsValue::Number(b as f64)).collect();
        JsValue::Array(Rc::new(RefCell::new(bytes)))
    }));
    JsValue::Object(obj)
}
fn native_text_decoder_new(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "TextDecoder".to_string();
    obj.borrow_mut().set("decode".to_string(), JsValue::NativeFunction("decode", |args, _| {
        if let Some(JsValue::Array(a)) = args.get(0) {
            let s: String = a.borrow().iter().filter_map(|v| {
                if let JsValue::Number(n) = v { Some(*n as u8 as char) } else { None }
            }).collect();
            JsValue::Str(s)
        } else { JsValue::Str(String::new()) }
    }));
    JsValue::Object(obj)
}

// ── AbortController stub ──────────────────────────────────────────────────────

fn native_abort_controller_new(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    let signal = Rc::new(RefCell::new(JsObject::new()));
    signal.borrow_mut().set("aborted".to_string(), JsValue::Bool(false));
    signal.borrow_mut().set("addEventListener".to_string(), JsValue::NativeFunction("addEventListener", native_noop));
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "AbortController".to_string();
    obj.borrow_mut().set("signal".to_string(), JsValue::Object(signal));
    obj.borrow_mut().set("abort".to_string(), JsValue::NativeFunction("abort", native_noop));
    JsValue::Object(obj)
}

// ── DOM stubs ─────────────────────────────────────────────────────────────────

fn native_doc_create_element(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let tag = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "Element".to_string();
    obj.borrow_mut().set("tagName".to_string(), JsValue::Str(tag.to_ascii_uppercase()));
    obj.borrow_mut().set("innerHTML".to_string(), JsValue::Str(String::new()));
    obj.borrow_mut().set("textContent".to_string(), JsValue::Str(String::new()));
    obj.borrow_mut().set("style".to_string(), JsValue::Object(Rc::new(RefCell::new(JsObject::new()))));
    obj.borrow_mut().set("className".to_string(), JsValue::Str(String::new()));
    obj.borrow_mut().set("children".to_string(), JsValue::Array(Rc::new(RefCell::new(Vec::new()))));
    obj.borrow_mut().set("appendChild".to_string(), JsValue::NativeFunction("appendChild", native_noop));
    obj.borrow_mut().set("setAttribute".to_string(), JsValue::NativeFunction("setAttribute", native_noop));
    obj.borrow_mut().set("getAttribute".to_string(), JsValue::NativeFunction("getAttribute", |_, _| JsValue::Null));
    obj.borrow_mut().set("addEventListener".to_string(), JsValue::NativeFunction("addEventListener", native_noop));
    obj.borrow_mut().set("removeEventListener".to_string(), JsValue::NativeFunction("removeEventListener", native_noop));
    obj.borrow_mut().set("querySelector".to_string(), JsValue::NativeFunction("querySelector", |_, _| JsValue::Null));
    obj.borrow_mut().set("querySelectorAll".to_string(), JsValue::NativeFunction("querySelectorAll", |_, _| JsValue::Array(Rc::new(RefCell::new(Vec::new())))));
    JsValue::Object(obj)
}
fn native_doc_create_text_node(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let text = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "Text".to_string();
    obj.borrow_mut().set("textContent".to_string(), JsValue::Str(text));
    JsValue::Object(obj)
}
fn native_doc_get_element_by_id(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Null }
fn native_doc_query_selector(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Null }
fn native_doc_query_selector_all(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Array(Rc::new(RefCell::new(Vec::new())))
}

// ── fetch stub ────────────────────────────────────────────────────────────────
// Returns a synchronous resolved Promise. Real network call would be here.

fn native_fetch(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let _url = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    // Build a minimal response object
    let response = Rc::new(RefCell::new(JsObject::new()));
    response.borrow_mut().class = "Response".to_string();
    response.borrow_mut().set("ok".to_string(),     JsValue::Bool(false));
    response.borrow_mut().set("status".to_string(), JsValue::Number(0.0));
    response.borrow_mut().set("statusText".to_string(), JsValue::Str("Network Error".to_string()));
    response.borrow_mut().set("url".to_string(),    JsValue::Str(_url));
    // json() / text() / arrayBuffer() — all return resolved Promises
    response.borrow_mut().set("json".to_string(), JsValue::NativeFunction("json", |_, _| {
        js_resolved_promise(JsValue::Object(Rc::new(RefCell::new(JsObject::new()))))
    }));
    response.borrow_mut().set("text".to_string(), JsValue::NativeFunction("text", |_, _| {
        js_resolved_promise(JsValue::Str(String::new()))
    }));
    response.borrow_mut().set("arrayBuffer".to_string(), JsValue::NativeFunction("arrayBuffer", |_, _| {
        js_resolved_promise(JsValue::Array(Rc::new(RefCell::new(Vec::new()))))
    }));
    js_resolved_promise(JsValue::Object(response))
}

// ── Proxy stub ────────────────────────────────────────────────────────────────

fn native_proxy_new(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    // Return the target object as-is (no proxy trapping)
    args.get(0).cloned().unwrap_or(JsValue::Undefined)
}

// ── Async stubs ───────────────────────────────────────────────────────────────

fn native_set_timeout(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    // Call the callback immediately (cooperative kernel, no real timer)
    if let Some(cb) = args.get(0) {
        interp.call_value(cb.clone(), JsValue::Undefined, &[]);
    }
    JsValue::Number(0.0) // timer ID
}
fn native_clear_timeout(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Undefined }
fn native_queue_microtask(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    // Phase 104: push onto the micro-task queue instead of calling immediately
    if let Some(cb) = args.get(0) {
        if cb.type_of() == "function" {
            interp.microtask_queue.push(cb.clone());
        }
    }
    JsValue::Undefined
}

// ── URI encoding ──────────────────────────────────────────────────────────────

fn native_encode_uri_component(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let s = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                out.push(b as char);
            }
            _ => { out.push('%'); out.push(hex_digit(b >> 4)); out.push(hex_digit(b & 0xF)); }
        }
    }
    JsValue::Str(out)
}
fn native_decode_uri_component(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let s = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i+1]), from_hex(bytes[i+2])) {
                out.push((h << 4 | l) as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    JsValue::Str(out)
}
fn hex_digit(n: u8) -> char { b"0123456789ABCDEF"[n as usize] as char }
fn from_hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

// ── structuredClone ───────────────────────────────────────────────────────────

fn native_structured_clone(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    // Deep clone via Clone trait (Rc is Cloned, so sharing is maintained —
    // good enough for our synchronous kernel environment)
    args.get(0).cloned().unwrap_or(JsValue::Undefined)
}

// ── performance.now stub ──────────────────────────────────────────────────────

fn native_performance_now(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    // Return milliseconds since boot based on timer tick count
    let ticks = crate::drivers::timer::ticks();
    JsValue::Number(ticks as f64)
}

// ── No-op ─────────────────────────────────────────────────────────────────────

fn native_noop(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Undefined }

// ─────────────────────────────────────────────────────────────────────────────
//  Phase 105: WebRTC + MediaDevices native functions
// ─────────────────────────────────────────────────────────────────────────────

/// Build an RTCSessionDescription-like JsObject.
fn rtc_make_desc_obj(kind: &str) -> JsValue {
    let o = Rc::new(RefCell::new(JsObject::new()));
    o.borrow_mut().class = "RTCSessionDescription".to_string();
    o.borrow_mut().set("type".to_string(), JsValue::Str(kind.to_string()));
    o.borrow_mut().set("sdp".to_string(),  JsValue::Str(format!(
        "v=0\r\no=SmartOS 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\na={}:stub\r\n", kind
    )));
    JsValue::Object(o)
}

/// `new RTCPeerConnection([config])` — creates a stub RTCPeerConnection object.
fn native_rtc_peer_connection_new(_args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let o = Rc::new(RefCell::new(JsObject::new()));
    o.borrow_mut().class = "RTCPeerConnection".to_string();
    o.borrow_mut().set("signalingState".to_string(),          JsValue::Str("stable".to_string()));
    o.borrow_mut().set("iceConnectionState".to_string(),      JsValue::Str("new".to_string()));
    o.borrow_mut().set("iceGatheringState".to_string(),       JsValue::Str("new".to_string()));
    o.borrow_mut().set("connectionState".to_string(),         JsValue::Str("new".to_string()));
    o.borrow_mut().set("localDescription".to_string(),        JsValue::Null);
    o.borrow_mut().set("remoteDescription".to_string(),       JsValue::Null);
    o.borrow_mut().set("onicecandidate".to_string(),          JsValue::Null);
    o.borrow_mut().set("ontrack".to_string(),                 JsValue::Null);
    o.borrow_mut().set("onconnectionstatechange".to_string(), JsValue::Null);
    o.borrow_mut().set("onnegotiationneeded".to_string(),     JsValue::Null);
    o.borrow_mut().set("onicegatheringstatechange".to_string(),JsValue::Null);
    JsValue::Object(o)
}

/// `new RTCSessionDescription({type, sdp})` — wraps an SDP object.
fn native_rtc_session_description_new(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let (kind, sdp) = match args.get(0) {
        Some(JsValue::Object(init)) => {
            let t = init.borrow().get("type").to_string_val();
            let s = init.borrow().get("sdp").to_string_val();
            (t, s)
        }
        _ => ("offer".to_string(), String::new()),
    };
    let o = Rc::new(RefCell::new(JsObject::new()));
    o.borrow_mut().class = "RTCSessionDescription".to_string();
    o.borrow_mut().set("type".to_string(), JsValue::Str(kind));
    o.borrow_mut().set("sdp".to_string(),  JsValue::Str(sdp));
    JsValue::Object(o)
}

/// `new RTCIceCandidate({candidate, sdpMid, sdpMLineIndex})` — wraps an ICE candidate.
fn native_rtc_ice_candidate_new(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let (cand, mid, mline) = match args.get(0) {
        Some(JsValue::Object(init)) => {
            let c  = init.borrow().get("candidate").to_string_val();
            let m  = init.borrow().get("sdpMid").to_string_val();
            let ml = init.borrow().get("sdpMLineIndex").to_number() as u32;
            (c, m, ml)
        }
        _ => (
            "candidate:1 1 UDP 2122252543 127.0.0.1 9 typ host".to_string(),
            "0".to_string(), 0u32,
        ),
    };
    let o = Rc::new(RefCell::new(JsObject::new()));
    o.borrow_mut().class = "RTCIceCandidate".to_string();
    o.borrow_mut().set("candidate".to_string(),        JsValue::Str(cand));
    o.borrow_mut().set("sdpMid".to_string(),           JsValue::Str(mid));
    o.borrow_mut().set("sdpMLineIndex".to_string(),    JsValue::Number(mline as f64));
    o.borrow_mut().set("usernameFragment".to_string(), JsValue::Str("stub".to_string()));
    JsValue::Object(o)
}

/// `navigator.mediaDevices.getUserMedia(constraints)` → `Promise<MediaStream>`.
fn native_get_user_media(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let (want_audio, want_video) = match args.get(0) {
        Some(JsValue::Object(c)) => {
            let a = !matches!(c.borrow().get("audio"), JsValue::Bool(false) | JsValue::Undefined);
            let v = !matches!(c.borrow().get("video"), JsValue::Bool(false) | JsValue::Undefined);
            (a, v)
        }
        _ => (true, true),
    };
    let stream = Rc::new(RefCell::new(JsObject::new()));
    stream.borrow_mut().class = "MediaStream".to_string();
    stream.borrow_mut().set("id".to_string(),     JsValue::Str("stream-0".to_string()));
    stream.borrow_mut().set("active".to_string(), JsValue::Bool(true));

    let mut tracks: Vec<JsValue> = Vec::new();
    if want_audio {
        let t = Rc::new(RefCell::new(JsObject::new()));
        t.borrow_mut().class = "MediaStreamTrack".to_string();
        t.borrow_mut().set("kind".to_string(),       JsValue::Str("audio".to_string()));
        t.borrow_mut().set("id".to_string(),         JsValue::Str("audio-track-0".to_string()));
        t.borrow_mut().set("label".to_string(),      JsValue::Str("Microphone (stub)".to_string()));
        t.borrow_mut().set("enabled".to_string(),    JsValue::Bool(true));
        t.borrow_mut().set("muted".to_string(),      JsValue::Bool(false));
        t.borrow_mut().set("readyState".to_string(), JsValue::Str("live".to_string()));
        tracks.push(JsValue::Object(t));
    }
    if want_video {
        let t = Rc::new(RefCell::new(JsObject::new()));
        t.borrow_mut().class = "MediaStreamTrack".to_string();
        t.borrow_mut().set("kind".to_string(),       JsValue::Str("video".to_string()));
        t.borrow_mut().set("id".to_string(),         JsValue::Str("video-track-0".to_string()));
        t.borrow_mut().set("label".to_string(),      JsValue::Str("Camera (stub)".to_string()));
        t.borrow_mut().set("enabled".to_string(),    JsValue::Bool(true));
        t.borrow_mut().set("muted".to_string(),      JsValue::Bool(false));
        t.borrow_mut().set("readyState".to_string(), JsValue::Str("live".to_string()));
        tracks.push(JsValue::Object(t));
    }
    stream.borrow_mut().set("__tracks__".to_string(), JsValue::Array(Rc::new(RefCell::new(tracks))));
    js_resolved_promise(JsValue::Object(stream))
}

/// `navigator.mediaDevices.enumerateDevices()` → `Promise<DeviceInfo[]>`.
fn native_enumerate_devices(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    let mic = Rc::new(RefCell::new(JsObject::new()));
    mic.borrow_mut().set("kind".to_string(),     JsValue::Str("audioinput".to_string()));
    mic.borrow_mut().set("deviceId".to_string(), JsValue::Str("default".to_string()));
    mic.borrow_mut().set("label".to_string(),    JsValue::Str("Microphone (stub)".to_string()));
    mic.borrow_mut().set("groupId".to_string(),  JsValue::Str("".to_string()));
    let cam = Rc::new(RefCell::new(JsObject::new()));
    cam.borrow_mut().set("kind".to_string(),     JsValue::Str("videoinput".to_string()));
    cam.borrow_mut().set("deviceId".to_string(), JsValue::Str("default".to_string()));
    cam.borrow_mut().set("label".to_string(),    JsValue::Str("Camera (stub)".to_string()));
    cam.borrow_mut().set("groupId".to_string(),  JsValue::Str("".to_string()));
    js_resolved_promise(JsValue::Array(Rc::new(RefCell::new(vec![JsValue::Object(mic), JsValue::Object(cam)]))))
}

/// `navigator.mediaDevices.getSupportedConstraints()` → constraints object.
fn native_get_supported_constraints(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    let o = Rc::new(RefCell::new(JsObject::new()));
    for k in &["width", "height", "frameRate", "facingMode", "deviceId",
               "sampleRate", "sampleSize", "channelCount", "echoCancellation",
               "noiseSuppression", "autoGainControl"] {
        o.borrow_mut().set(k.to_string(), JsValue::Bool(true));
    }
    JsValue::Object(o)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Phase 98: Additional Math native functions
// ─────────────────────────────────────────────────────────────────────────────

fn native_math_log2(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let n = args.get(0).map(|v| v.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(js_ln(n) / 0.6931471805599453_f64)
}
fn native_math_log10(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let n = args.get(0).map(|v| v.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(js_ln(n) / 2.302585092994046_f64)
}
fn native_math_atan2(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let y = args.get(0).map(|v| v.to_number()).unwrap_or(0.0);
    let x = args.get(1).map(|v| v.to_number()).unwrap_or(0.0);
    JsValue::Number(js_atan2(y, x))
}
fn native_math_hypot(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let sum: f64 = args.iter().map(|v| { let n = v.to_number(); n * n }).sum();
    JsValue::Number(js_sqrt(sum))
}
fn native_math_cbrt(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let n = args.get(0).map(|v| v.to_number()).unwrap_or(f64::NAN);
    if n == 0.0 { return JsValue::Number(0.0); }
    let neg = n < 0.0;
    let n2 = if neg { -n } else { n };
    let r = js_exp(js_ln(n2) / 3.0);
    JsValue::Number(if neg { -r } else { r })
}
fn native_math_clz32(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let n = args.get(0).map(|v| v.to_number() as u32).unwrap_or(0);
    JsValue::Number(n.leading_zeros() as f64)
}
fn native_math_asin(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let x = args.get(0).map(|v| v.to_number()).unwrap_or(0.0);
    JsValue::Number(js_asin(x))
}
fn native_math_acos(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let x = args.get(0).map(|v| v.to_number()).unwrap_or(0.0);
    JsValue::Number(core::f64::consts::FRAC_PI_2 - js_asin(x))
}
fn native_math_atan(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let x = args.get(0).map(|v| v.to_number()).unwrap_or(0.0);
    JsValue::Number(js_atan(x))
}
fn native_math_sinh(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let x = args.get(0).map(|v| v.to_number()).unwrap_or(0.0);
    // sinh(x) = (e^x - e^-x) / 2
    JsValue::Number((js_exp(x) - js_exp(-x)) * 0.5)
}
fn native_math_cosh(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let x = args.get(0).map(|v| v.to_number()).unwrap_or(0.0);
    JsValue::Number((js_exp(x) + js_exp(-x)) * 0.5)
}
fn native_math_tanh(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let x = args.get(0).map(|v| v.to_number()).unwrap_or(0.0);
    let ex = js_exp(x); let emx = js_exp(-x);
    JsValue::Number((ex - emx) / (ex + emx))
}
fn native_math_expm1(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let x = args.get(0).map(|v| v.to_number()).unwrap_or(0.0);
    JsValue::Number(js_exp(x) - 1.0)
}
fn native_math_log1p(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let x = args.get(0).map(|v| v.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(js_ln(1.0 + x))
}
fn native_math_fround(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let n = args.get(0).map(|v| v.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(n as f32 as f64)
}
fn native_math_imul(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let a = args.get(0).map(|v| v.to_number() as i32).unwrap_or(0);
    let b = args.get(1).map(|v| v.to_number() as i32).unwrap_or(0);
    JsValue::Number(a.wrapping_mul(b) as f64)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Phase 98: Date constructor
// ─────────────────────────────────────────────────────────────────────────────

fn native_date_new(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "Date".to_string();
    obj.borrow_mut().set("getTime".to_string(),           JsValue::NativeFunction("getTime",           |_, _| JsValue::Number(0.0)));
    obj.borrow_mut().set("getFullYear".to_string(),       JsValue::NativeFunction("getFullYear",       |_, _| JsValue::Number(2026.0)));
    obj.borrow_mut().set("getUTCFullYear".to_string(),    JsValue::NativeFunction("getUTCFullYear",    |_, _| JsValue::Number(2026.0)));
    obj.borrow_mut().set("getMonth".to_string(),          JsValue::NativeFunction("getMonth",          |_, _| JsValue::Number(0.0)));
    obj.borrow_mut().set("getUTCMonth".to_string(),       JsValue::NativeFunction("getUTCMonth",       |_, _| JsValue::Number(0.0)));
    obj.borrow_mut().set("getDate".to_string(),           JsValue::NativeFunction("getDate",           |_, _| JsValue::Number(1.0)));
    obj.borrow_mut().set("getDay".to_string(),            JsValue::NativeFunction("getDay",            |_, _| JsValue::Number(0.0)));
    obj.borrow_mut().set("getHours".to_string(),          JsValue::NativeFunction("getHours",          |_, _| JsValue::Number(0.0)));
    obj.borrow_mut().set("getMinutes".to_string(),        JsValue::NativeFunction("getMinutes",        |_, _| JsValue::Number(0.0)));
    obj.borrow_mut().set("getSeconds".to_string(),        JsValue::NativeFunction("getSeconds",        |_, _| JsValue::Number(0.0)));
    obj.borrow_mut().set("getMilliseconds".to_string(),   JsValue::NativeFunction("getMilliseconds",   |_, _| JsValue::Number(0.0)));
    obj.borrow_mut().set("getTimezoneOffset".to_string(), JsValue::NativeFunction("getTimezoneOffset", |_, _| JsValue::Number(0.0)));
    obj.borrow_mut().set("setTime".to_string(),           JsValue::NativeFunction("setTime",           native_noop));
    obj.borrow_mut().set("setFullYear".to_string(),       JsValue::NativeFunction("setFullYear",       native_noop));
    obj.borrow_mut().set("setMonth".to_string(),          JsValue::NativeFunction("setMonth",          native_noop));
    obj.borrow_mut().set("setDate".to_string(),           JsValue::NativeFunction("setDate",           native_noop));
    obj.borrow_mut().set("setHours".to_string(),          JsValue::NativeFunction("setHours",          native_noop));
    obj.borrow_mut().set("setMinutes".to_string(),        JsValue::NativeFunction("setMinutes",        native_noop));
    obj.borrow_mut().set("setSeconds".to_string(),        JsValue::NativeFunction("setSeconds",        native_noop));
    obj.borrow_mut().set("toISOString".to_string(),       JsValue::NativeFunction("toISOString",       |_, _| JsValue::Str("2026-01-01T00:00:00.000Z".to_string())));
    obj.borrow_mut().set("toLocaleDateString".to_string(),JsValue::NativeFunction("toLocaleDateString",|_, _| JsValue::Str("1/1/2026".to_string())));
    obj.borrow_mut().set("toLocaleString".to_string(),    JsValue::NativeFunction("toLocaleString",    |_, _| JsValue::Str("1/1/2026, 12:00:00 AM".to_string())));
    obj.borrow_mut().set("toUTCString".to_string(),       JsValue::NativeFunction("toUTCString",       |_, _| JsValue::Str("Wed, 01 Jan 2026 00:00:00 GMT".to_string())));
    obj.borrow_mut().set("toString".to_string(),          JsValue::NativeFunction("toString",          |_, _| JsValue::Str("Wed Jan 01 2026 00:00:00 GMT+0000".to_string())));
    obj.borrow_mut().set("valueOf".to_string(),           JsValue::NativeFunction("valueOf",           |_, _| JsValue::Number(0.0)));
    JsValue::Object(obj)
}
fn native_date_now(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    // Approximate ms since epoch using RTC
    let dt = crate::drivers::rtc::now();
    let years_since_1970 = (dt.year as u64).saturating_sub(1970);
    let ms = (years_since_1970 * 365 * 24 * 3600
        + (dt.month as u64).saturating_sub(1) * 30 * 24 * 3600
        + (dt.day    as u64).saturating_sub(1) * 24 * 3600
        + dt.hour    as u64 * 3600
        + dt.minute  as u64 * 60
        + dt.second  as u64) * 1000;
    JsValue::Number(ms as f64)
}
fn native_date_parse(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let _s = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    JsValue::Number(0.0) // stub
}
fn native_date_utc(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let year  = args.get(0).map(|v| v.to_number() as u64).unwrap_or(2026);
    let month = args.get(1).map(|v| v.to_number() as u64).unwrap_or(0);
    let day   = args.get(2).map(|v| v.to_number() as u64).unwrap_or(1);
    let ms = ((year.saturating_sub(1970)) * 365 * 24 * 3600
        + month * 30 * 24 * 3600 + day * 24 * 3600) * 1000;
    JsValue::Number(ms as f64)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Phase 98: eval stub + Intl stub + crypto.randomUUID
// ─────────────────────────────────────────────────────────────────────────────

fn native_eval_stub(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    // Evaluate the string using the JS interpreter (same instance)
    if let Some(JsValue::Str(s)) = args.get(0) {
        return interp.run(s);
    }
    JsValue::Undefined
}

fn native_intl_format_new(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().class = "IntlFormat".to_string();
    obj.borrow_mut().set("format".to_string(), JsValue::NativeFunction("format", |args, _| {
        args.get(0).map(|v| JsValue::Str(v.to_string_val())).unwrap_or(JsValue::Str(String::new()))
    }));
    obj.borrow_mut().set("formatToParts".to_string(), JsValue::NativeFunction("formatToParts", |_, _| {
        JsValue::Array(Rc::new(RefCell::new(Vec::new())))
    }));
    JsValue::Object(obj)
}

fn native_crypto_random_uuid(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    // Deterministic "UUID" stub using LCG
    static UUID_SEED: spin::Mutex<u64> = spin::Mutex::new(0xdeadbeef_cafebabe);
    let mut s = UUID_SEED.lock();
    *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let hi = *s;
    *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let lo = *s;
    JsValue::Str(format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (hi >> 32) as u32,
        (hi >> 16) as u16,
        (hi & 0xFFF) as u16,
        (lo >> 48) as u16 | 0x8000,
        lo & 0xFFFF_FFFF_FFFF
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
//  Phase 98: Number formatting helpers
// ─────────────────────────────────────────────────────────────────────────────

fn format_fixed(n: f64, digits: usize) -> String {
    if n.is_nan()      { return "NaN".to_string(); }
    if n.is_infinite() { return if n > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() }; }
    if digits == 0     { return format!("{}", n as i64); }
    let neg = n < 0.0;
    let n2  = if neg { -n } else { n };
    let factor = js_pow(10.0, digits as f64);
    let scaled  = js_floor(n2 * factor + 0.5) as u64;
    let int_part  = scaled / factor as u64;
    let frac_part = scaled % factor as u64;
    // Pad frac with leading zeros to `digits` width
    let frac_str = format!("{}", frac_part);
    let pad: String = core::iter::repeat('0').take(digits.saturating_sub(frac_str.len())).collect();
    let padded = format!("{}{}", pad, frac_str);
    let padded = &padded[..padded.len().min(digits)]; // truncate to digits
    if neg { format!("-{}.{}", int_part, padded) }
    else   { format!("{}.{}", int_part, padded) }
}

fn format_radix(n: i64, radix: u32) -> String {
    if radix < 2 || radix > 36 { return "NaN".to_string(); }
    if n == 0 { return "0".to_string(); }
    let neg = n < 0;
    let mut n2 = if neg { (n as i128).unsigned_abs() } else { n as u128 };
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut result: Vec<char> = Vec::new();
    while n2 > 0 {
        result.push(digits[(n2 % radix as u128) as usize] as char);
        n2 /= radix as u128;
    }
    if neg { result.push('-'); }
    result.reverse();
    result.iter().collect()
}

fn format_precision(n: f64, prec: usize) -> String {
    if n.is_nan()      { return "NaN".to_string(); }
    if n.is_infinite() { return if n > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() }; }
    if n == 0.0        { return format!("0.{}", "0".repeat(prec.saturating_sub(1))); }
    let mag = js_floor(js_ln(n.abs()) / js_ln(10.0) + 1e-10) as i32;
    let dec_digits = (prec as i32 - mag - 1).max(0) as usize;
    format_fixed(n, dec_digits)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Phase 98: Trig helpers (atan, asin, atan2)
// ─────────────────────────────────────────────────────────────────────────────

fn js_atan(x: f64) -> f64 {
    let pi = core::f64::consts::PI;
    if x.abs() > 1.0 {
        let sign = if x > 0.0 { 1.0 } else { -1.0 };
        return sign * (pi / 2.0 - js_atan(1.0 / x.abs()));
    }
    // Taylor series: atan(x) = x - x³/3 + x⁵/5 - …
    let x2 = x * x;
    x * (1.0 - x2/3.0 * (1.0 - x2/5.0 * (1.0 - x2/7.0 * (1.0 - x2/9.0 * (1.0 - x2/11.0 * (1.0 - x2/13.0))))))
}

fn js_asin(x: f64) -> f64 {
    if x.abs() > 1.0 { return f64::NAN; }
    if x.abs() == 1.0 { return if x > 0.0 { core::f64::consts::FRAC_PI_2 } else { -core::f64::consts::FRAC_PI_2 }; }
    // asin(x) = atan(x / sqrt(1 - x²))
    js_atan(x / js_sqrt(1.0 - x * x))
}

fn js_atan2(y: f64, x: f64) -> f64 {
    let pi = core::f64::consts::PI;
    if x > 0.0 { js_atan(y / x) }
    else if x < 0.0 {
        if y >= 0.0 { js_atan(y / x) + pi } else { js_atan(y / x) - pi }
    } else {
        if y > 0.0 { pi / 2.0 } else if y < 0.0 { -pi / 2.0 } else { 0.0 }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  No-std math helpers
// ─────────────────────────────────────────────────────────────────────────────

fn js_floor(x: f64) -> f64 { let i = x as i64 as f64; if x < i { i - 1.0 } else { i } }
fn js_ceil(x: f64)  -> f64 { let i = x as i64 as f64; if x > i { i + 1.0 } else { i } }

fn js_sqrt(x: f64) -> f64 {
    if x < 0.0 { return f64::NAN; }
    if x == 0.0 { return 0.0; }
    let mut r = x / 2.0;
    for _ in 0..60 { r = (r + x / r) * 0.5; }
    r
}

fn js_sin(x: f64) -> f64 {
    // Reduce to [-PI, PI]
    let pi = core::f64::consts::PI;
    let two_pi = pi * 2.0;
    let mut x = x % two_pi;
    if x > pi { x -= two_pi; } else if x < -pi { x += two_pi; }
    // Taylor series
    let x2 = x * x;
    x * (1.0 - x2/6.0 * (1.0 - x2/20.0 * (1.0 - x2/42.0 * (1.0 - x2/72.0))))
}

fn js_cos(x: f64) -> f64 {
    js_sin(x + core::f64::consts::FRAC_PI_2)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Helper macro: build object literal
// ─────────────────────────────────────────────────────────────────────────────

macro_rules! make_obj {
    ($($k:expr => $v:expr),* $(,)?) => {{
        let obj = Rc::new(RefCell::new(JsObject::new()));
        $(obj.borrow_mut().set($k.to_string(), $v);)*
        JsValue::Object(obj)
    }};
}
use make_obj;

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[js] JavaScript engine v3 ready (Phase 105: WebRTC stubs, MediaDevices, getUserMedia).");
}

// ── Self-test ─────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;
    let mut interp = Interpreter::new();

    // T1: arithmetic
    if interp.run("2 + 3 * 4") != JsValue::Number(14.0) { ok = false; }

    // T2: string concat
    if interp.run("'hello' + ' ' + 'world'") != JsValue::Str("hello world".to_string()) { ok = false; }

    // T3: closure
    if interp.run("function adder(x){return function(y){return x+y;};} adder(5)(3)") != JsValue::Number(8.0) { ok = false; }

    // T4: array methods
    if interp.run("[1,2,3].map(x=>x*2).reduce((a,b)=>a+b,0)") != JsValue::Number(12.0) { ok = false; }

    // T5: object literal
    if interp.run("var o={x:10,y:20}; o.x+o.y") != JsValue::Number(30.0) { ok = false; }

    // T6: template literal
    if interp.run("var n=42; `val is ${n}`") != JsValue::Str("val is 42".to_string()) { ok = false; }

    // T7: class definition and instantiation
    interp.run("class Animal { constructor(n){this.name=n;} speak(){return this.name+' speaks';} }");
    if interp.run("new Animal('Dog').speak()") != JsValue::Str("Dog speaks".to_string()) { ok = false; }

    // T8: class extends + super()
    interp.run("class Dog extends Animal { constructor(n){super(n);this.type='dog';} bark(){return this.name+' barks';} }");
    if interp.run("new Dog('Rex').bark()") != JsValue::Str("Rex barks".to_string()) { ok = false; }
    // Inherited method
    if interp.run("new Dog('Rex').speak()") != JsValue::Str("Rex speaks".to_string()) { ok = false; }

    // T9: instanceof
    if interp.run("new Dog('x') instanceof Dog") != JsValue::Bool(true) { ok = false; }
    if interp.run("new Animal('x') instanceof Animal") != JsValue::Bool(true) { ok = false; }

    // T10: Number.toFixed
    if interp.run("(3.14159).toFixed(2)") != JsValue::Str("3.14".to_string()) { ok = false; }
    if interp.run("(1000).toFixed(0)") != JsValue::Str("1000".to_string()) { ok = false; }

    // T11: static class method
    interp.run("class Counter { static count=0; static inc(){Counter.count++;} }");
    // static properties not implemented — just test static method callable:
    let sv = interp.run("typeof Counter.inc");
    if sv != JsValue::Str("function".to_string()) { ok = false; }

    // T12: for...of with Set
    if interp.run("var s=new Set([1,2,3]); var t=0; for(var x of s){t+=x;} t") != JsValue::Number(6.0) { ok = false; }

    // T13: for...of with Map
    if interp.run("var m=new Map([[1,'a'],[2,'b']]); var k=0; for(var p of m){k+=p[0];} k") != JsValue::Number(3.0) { ok = false; }

    // T14: hasOwnProperty
    if interp.run("var obj={a:1}; obj.hasOwnProperty('a')") != JsValue::Bool(true) { ok = false; }
    if interp.run("var obj={a:1}; obj.hasOwnProperty('z')") != JsValue::Bool(false) { ok = false; }

    // T15: Math.log2 / Math.log10
    let v = interp.run("Math.round(Math.log2(8))");
    if v != JsValue::Number(3.0) { ok = false; }

    // T16: Date.now() is a number
    let dn = interp.run("typeof Date.now()");
    if dn != JsValue::Str("number".to_string()) { ok = false; }

    // T17: new Date() is object
    let dobj = interp.run("typeof new Date()");
    if dobj != JsValue::Str("object".to_string()) { ok = false; }

    // T18: format_radix (Number.toString(16))
    if interp.run("(255).toString(16)") != JsValue::Str("ff".to_string()) { ok = false; }

    // ── Phase 104: async/await + Promise ──────────────────────────────────────

    // T19: async function returns a Promise
    {
        interp.run("async function getVal() { return 42; }");
        let p = interp.run("getVal()");
        let is_promise = matches!(&p, JsValue::Object(o) if o.borrow().class == "Promise");
        if !is_promise { ok = false; }
    }

    // T20: await unwraps a resolved Promise
    {
        let v = interp.run("async function aw() { return await Promise.resolve(99); } aw()");
        // The async fn returns a Promise; unwrap its value
        let inner = match &v {
            JsValue::Object(o) if o.borrow().class == "Promise" => o.borrow().get("__value__"),
            other => other.clone(),
        };
        if inner != JsValue::Number(99.0) { ok = false; }
    }

    // T21: Promise .then() chains correctly
    {
        let v = interp.run("Promise.resolve(10).then(x => x * 3)");
        let inner = match &v {
            JsValue::Object(o) if o.borrow().class == "Promise" => o.borrow().get("__value__"),
            other => other.clone(),
        };
        if inner != JsValue::Number(30.0) { ok = false; }
    }

    // T22: Promise .catch() handles rejection
    {
        let v = interp.run("Promise.reject('err').catch(e => e + '!')");
        let inner = match &v {
            JsValue::Object(o) if o.borrow().class == "Promise" => o.borrow().get("__value__"),
            other => other.clone(),
        };
        if inner != JsValue::Str("err!".to_string()) { ok = false; }
    }

    // T23: Promise.all() resolves array
    {
        let v = interp.run(
            "Promise.all([Promise.resolve(1), Promise.resolve(2), Promise.resolve(3)])"
        );
        let arr_len = match &v {
            JsValue::Object(o) if o.borrow().class == "Promise" => {
                match o.borrow().get("__value__") {
                    JsValue::Array(a) => a.borrow().len(),
                    _ => 0,
                }
            }
            _ => 0,
        };
        if arr_len != 3 { ok = false; }
    }

    // T24: Promise.withResolvers() returns { promise, resolve, reject }
    {
        let v = interp.run("Promise.withResolvers()");
        let has_promise = match &v {
            JsValue::Object(o) => {
                !matches!(o.borrow().get("promise"), JsValue::Undefined)
                    && !matches!(o.borrow().get("resolve"), JsValue::Undefined)
                    && !matches!(o.borrow().get("reject"), JsValue::Undefined)
            }
            _ => false,
        };
        if !has_promise { ok = false; }
    }

    // ── Phase 105: WebRTC stubs + MediaDevices ────────────────────────────────

    // T25: RTCPeerConnection constructor creates object with correct class
    {
        let v = interp.run("typeof new RTCPeerConnection()");
        if v != JsValue::Str("object".to_string()) { ok = false; }
    }

    // T26: RTCPeerConnection initial signalingState = "stable"
    {
        let v = interp.run("new RTCPeerConnection().signalingState");
        if v != JsValue::Str("stable".to_string()) { ok = false; }
    }

    // T27: createOffer() returns a Promise whose value has type="offer"
    {
        interp.run("var _pc = new RTCPeerConnection();");
        let p = interp.run("_pc.createOffer()");
        let offer_type = match &p {
            JsValue::Object(o) if o.borrow().class == "Promise" => {
                match o.borrow().get("__value__") {
                    JsValue::Object(desc) => desc.borrow().get("type").to_string_val(),
                    _ => String::new(),
                }
            }
            _ => String::new(),
        };
        if offer_type != "offer" { ok = false; }
    }

    // T28: setLocalDescription() changes signalingState to have-local-offer
    {
        interp.run("var _pc2 = new RTCPeerConnection();");
        interp.run("_pc2.createOffer().then(function(d){ _pc2.setLocalDescription(d); });");
        let state = interp.run("_pc2.signalingState");
        if state != JsValue::Str("have-local-offer".to_string()) { ok = false; }
    }

    // T29: getUserMedia returns Promise<MediaStream>
    {
        let p = interp.run("navigator.mediaDevices.getUserMedia({audio:true,video:true})");
        let is_stream = match &p {
            JsValue::Object(o) if o.borrow().class == "Promise" => {
                match o.borrow().get("__value__") {
                    JsValue::Object(s) => s.borrow().class == "MediaStream",
                    _ => false,
                }
            }
            _ => false,
        };
        if !is_stream { ok = false; }
    }

    // T30: MediaStream.getTracks() returns 2-track array for audio+video
    {
        let p = interp.run("navigator.mediaDevices.getUserMedia({audio:true,video:true})");
        let track_count = match &p {
            JsValue::Object(o) if o.borrow().class == "Promise" => {
                match o.borrow().get("__value__") {
                    JsValue::Object(s) => {
                        match s.borrow().get("__tracks__") {
                            JsValue::Array(a) => a.borrow().len(),
                            _ => 0,
                        }
                    }
                    _ => 0,
                }
            }
            _ => 0,
        };
        if track_count != 2 { ok = false; }
    }

    ok
}

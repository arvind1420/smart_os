//! Browser JavaScript execution.
//!
//! Runs inline `<script>` blocks through the kernel's existing tree-walking
//! interpreter (`net::js_interp`), exposing a minimal `document` / `window` /
//! `console` shim so that real-world JS pages can set `document.title`,
//! `document.body.innerHTML`, etc.
//!
//! Outputs are surfaced back to the browser app via the `ScriptResult` struct.
//!
//! Safety:
//!   * Scripts > 256 KB are skipped (a runaway might exhaust the heap).
//!   * Obviously dangerous loops (`while(true)`, `for(;;)`, `while(1)`) are
//!     skipped textually — the interpreter has no time-slice limiter.
//!   * Any panic / `throw` is swallowed; we just take whatever document state
//!     we got up to that point.
//!
//! A literal-form scanner is kept as a fallback for the case where the script
//! is rejected by the heuristics above.

#![allow(dead_code)]

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::rc::Rc;
use core::cell::RefCell;

use crate::net::js_interp::{Interpreter, JsValue, JsObject};

/// Result of running every `<script>` in a page.
pub struct ScriptResult {
    pub title:    Option<String>,
    pub console:  Vec<String>,
    pub executed: usize,
    pub skipped:  usize,
}

/// Set per call to `run_scripts` so cookie-related native fns know which
/// host we're talking to when JS calls `document.cookie = ...`.
static CURRENT_HOST: spin::Mutex<Option<String>> = spin::Mutex::new(None);

fn set_current_host(host: &str) { *CURRENT_HOST.lock() = Some(host.to_string()); }
fn current_host() -> String { CURRENT_HOST.lock().clone().unwrap_or_default() }

impl ScriptResult {
    pub fn empty() -> Self {
        Self { title: None, console: Vec::new(), executed: 0, skipped: 0 }
    }
}

/// Top-level entry: parse and run every inline `<script>` in `html`.
///
/// `initial_title` should be the value of the document's `<title>` element
/// (empty if absent) so that JS that *reads* `document.title` sees the right
/// initial value.  `host` is the page hostname (used for `document.cookie`).
pub fn run_scripts(html: &str, initial_title: &str, host: &str) -> ScriptResult {
    let mut result = ScriptResult::empty();

    let bodies = iter_script_bodies(html);
    if bodies.is_empty() {
        return result;
    }

    set_current_host(host);

    // One interpreter for the page; sharing state lets later scripts see vars
    // set by earlier ones (HTML execution order).
    let mut interp = Interpreter::new();
    let document = install_document(&mut interp, initial_title);
    install_console(&mut interp);
    install_window(&mut interp);

    for body in &bodies {
        if !safe_to_run(body) {
            result.skipped += 1;
            // Fall back to the literal scanner for THIS block.
            if let Some(t) = scan_title_literal(body) {
                let mut d = document.borrow_mut();
                d.set("title".into(), JsValue::Str(t));
            }
            continue;
        }
        // Swallow panics: we can't catch them in no_std, but `interp.run` itself
        // is panic-free in normal operation (it returns Err Signal on bad input).
        let _ = interp.run(body);
        result.executed += 1;
    }

    // Read the final title back.
    {
        let d = document.borrow();
        if let JsValue::Str(t) = d.get("title") {
            if !t.is_empty() && t != initial_title {
                result.title = Some(t);
            } else if initial_title.is_empty() && !t.is_empty() {
                result.title = Some(t);
            }
        }
    }

    // Drain whatever console.log accumulated.
    result.console = drain_console_output();

    result
}

// ─────────────────────────────────────────────────────────────────────────────
//  Console capture (drains into a static so we can read it back here).
// ─────────────────────────────────────────────────────────────────────────────

use spin::Mutex;
static CONSOLE_BUFFER: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn drain_console_output() -> Vec<String> {
    let mut g = CONSOLE_BUFFER.lock();
    let out = core::mem::take(&mut *g);
    out
}

fn console_log_native(args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    let mut line = String::new();
    for (i, v) in args.iter().enumerate() {
        if i > 0 { line.push(' '); }
        line.push_str(&v.to_string_val());
    }
    CONSOLE_BUFFER.lock().push(line);
    JsValue::Undefined
}

fn install_console(interp: &mut Interpreter) {
    let console = Rc::new(RefCell::new(JsObject::new()));
    {
        let mut c = console.borrow_mut();
        c.set("log".into(),   JsValue::NativeFunction("log",   console_log_native));
        c.set("error".into(), JsValue::NativeFunction("error", console_log_native));
        c.set("warn".into(),  JsValue::NativeFunction("warn",  console_log_native));
        c.set("info".into(),  JsValue::NativeFunction("info",  console_log_native));
        c.set("debug".into(), JsValue::NativeFunction("debug", console_log_native));
    }
    interp.env.define("console".into(), JsValue::Object(console));
}

// ─────────────────────────────────────────────────────────────────────────────
//  Document / window shim
// ─────────────────────────────────────────────────────────────────────────────

fn install_document(interp: &mut Interpreter, initial_title: &str) -> Rc<RefCell<JsObject>> {
    let doc = Rc::new(RefCell::new(JsObject::new()));
    let host = current_host();
    let cookies_snapshot = crate::net::cookies::all_for(&host);
    {
        let mut d = doc.borrow_mut();
        d.class = "HTMLDocument".into();
        d.set("title".into(),           JsValue::Str(initial_title.to_string()));
        d.set("URL".into(),             JsValue::Str(format!("https://{}/", host)));
        d.set("documentURI".into(),     JsValue::Str(format!("https://{}/", host)));
        d.set("baseURI".into(),         JsValue::Str(format!("https://{}/", host)));
        d.set("domain".into(),          JsValue::Str(host.clone()));
        d.set("readyState".into(),      JsValue::Str("complete".into()));
        d.set("compatMode".into(),      JsValue::Str("CSS1Compat".into()));
        d.set("characterSet".into(),    JsValue::Str("UTF-8".into()));
        d.set("contentType".into(),     JsValue::Str("text/html".into()));
        d.set("location".into(),        location_object());
        d.set("body".into(),            element_object("body"));
        d.set("head".into(),            element_object("head"));
        d.set("documentElement".into(), element_object("html"));
        d.set("hidden".into(),          JsValue::Bool(false));
        d.set("visibilityState".into(), JsValue::Str("visible".into()));
        // `cookie` snapshot at page load — assignment to the property writes a
        // string but doesn't reach the jar (no setters in this JS engine).
        // `setCookie()` / `getCookie()` are the side-effecting versions.
        d.set("cookie".into(),          JsValue::Str(cookies_snapshot));
        d.set("setCookie".into(),       JsValue::NativeFunction("setCookie", native_set_cookie));
        d.set("getCookie".into(),       JsValue::NativeFunction("getCookie", native_get_cookie));
        // DOM methods — all return a stub element so chained calls don't blow up.
        d.set("getElementById".into(),     JsValue::NativeFunction("getElementById",     stub_get_element));
        d.set("getElementsByTagName".into(), JsValue::NativeFunction("getElementsByTagName", stub_get_collection));
        d.set("getElementsByClassName".into(), JsValue::NativeFunction("getElementsByClassName", stub_get_collection));
        d.set("getElementsByName".into(), JsValue::NativeFunction("getElementsByName", stub_get_collection));
        d.set("querySelector".into(),      JsValue::NativeFunction("querySelector",      stub_get_element));
        d.set("querySelectorAll".into(),   JsValue::NativeFunction("querySelectorAll",   stub_get_collection));
        d.set("createElement".into(),      JsValue::NativeFunction("createElement",      stub_create_element));
        d.set("createElementNS".into(),    JsValue::NativeFunction("createElementNS",    stub_create_element));
        d.set("createTextNode".into(),     JsValue::NativeFunction("createTextNode",     stub_create_element));
        d.set("createDocumentFragment".into(), JsValue::NativeFunction("createDocumentFragment", stub_create_fragment));
        d.set("addEventListener".into(),   JsValue::NativeFunction("addEventListener",   stub_void));
        d.set("removeEventListener".into(), JsValue::NativeFunction("removeEventListener", stub_void));
        d.set("dispatchEvent".into(),      JsValue::NativeFunction("dispatchEvent",      stub_true));
        d.set("write".into(),              JsValue::NativeFunction("write",              stub_void));
        d.set("writeln".into(),            JsValue::NativeFunction("writeln",            stub_void));
        d.set("open".into(),               JsValue::NativeFunction("open",               stub_void));
        d.set("close".into(),              JsValue::NativeFunction("close",              stub_void));
    }
    interp.env.define("document".into(), JsValue::Object(doc.clone()));
    doc
}

fn install_window(interp: &mut Interpreter) {
    let win = Rc::new(RefCell::new(JsObject::new()));
    {
        let mut w = win.borrow_mut();
        w.class = "Window".into();
        w.set("innerWidth".into(),  JsValue::Number(800.0));
        w.set("innerHeight".into(), JsValue::Number(600.0));
        w.set("outerWidth".into(),  JsValue::Number(800.0));
        w.set("outerHeight".into(), JsValue::Number(600.0));
        w.set("devicePixelRatio".into(), JsValue::Number(1.0));
        w.set("location".into(),    location_object());
        w.set("navigator".into(),   navigator_object());
        w.set("addEventListener".into(),    JsValue::NativeFunction("addEventListener",    stub_void));
        w.set("removeEventListener".into(), JsValue::NativeFunction("removeEventListener", stub_void));
        w.set("scrollTo".into(),     JsValue::NativeFunction("scrollTo",     stub_void));
        w.set("scrollBy".into(),     JsValue::NativeFunction("scrollBy",     stub_void));
        w.set("alert".into(),        JsValue::NativeFunction("alert",        stub_void));
        w.set("confirm".into(),      JsValue::NativeFunction("confirm",      stub_false));
        w.set("prompt".into(),       JsValue::NativeFunction("prompt",       stub_null));
        w.set("getComputedStyle".into(), JsValue::NativeFunction("getComputedStyle", stub_empty_object));
        w.set("matchMedia".into(),   JsValue::NativeFunction("matchMedia",   stub_media_query));
    }
    interp.env.define("window".into(), JsValue::Object(win));
    interp.env.define("self".into(),   interp.env.get("window"));
    interp.env.define("globalThis".into(), interp.env.get("window"));
}

fn element_object(tag: &str) -> JsValue {
    let o = Rc::new(RefCell::new(JsObject::new()));
    {
        let mut e = o.borrow_mut();
        e.class = format!("HTML{}Element", tag.chars().next().unwrap_or('X').to_uppercase().collect::<String>() + &tag[1..]).into();
        e.set("tagName".into(),     JsValue::Str(tag.to_ascii_uppercase()));
        e.set("nodeName".into(),    JsValue::Str(tag.to_ascii_uppercase()));
        e.set("nodeType".into(),    JsValue::Number(1.0));
        e.set("innerHTML".into(),   JsValue::Str(String::new()));
        e.set("outerHTML".into(),   JsValue::Str(String::new()));
        e.set("innerText".into(),   JsValue::Str(String::new()));
        e.set("textContent".into(), JsValue::Str(String::new()));
        e.set("className".into(),   JsValue::Str(String::new()));
        e.set("id".into(),          JsValue::Str(String::new()));
        e.set("style".into(),       JsValue::Object(Rc::new(RefCell::new(JsObject::new()))));
        e.set("dataset".into(),     JsValue::Object(Rc::new(RefCell::new(JsObject::new()))));
        e.set("classList".into(),   classlist_object());
        e.set("attributes".into(),  JsValue::Array(Rc::new(RefCell::new(Vec::new()))));
        e.set("children".into(),    JsValue::Array(Rc::new(RefCell::new(Vec::new()))));
        e.set("childNodes".into(),  JsValue::Array(Rc::new(RefCell::new(Vec::new()))));
        e.set("parentNode".into(),  JsValue::Null);
        e.set("parentElement".into(), JsValue::Null);
        e.set("firstChild".into(),  JsValue::Null);
        e.set("lastChild".into(),   JsValue::Null);
        e.set("offsetWidth".into(),  JsValue::Number(0.0));
        e.set("offsetHeight".into(), JsValue::Number(0.0));
        e.set("clientWidth".into(),  JsValue::Number(0.0));
        e.set("clientHeight".into(), JsValue::Number(0.0));
        e.set("scrollWidth".into(),  JsValue::Number(0.0));
        e.set("scrollHeight".into(), JsValue::Number(0.0));
        e.set("addEventListener".into(),    JsValue::NativeFunction("addEventListener",    stub_void));
        e.set("removeEventListener".into(), JsValue::NativeFunction("removeEventListener", stub_void));
        e.set("dispatchEvent".into(),       JsValue::NativeFunction("dispatchEvent",       stub_true));
        e.set("appendChild".into(),         JsValue::NativeFunction("appendChild",         identity_arg));
        e.set("removeChild".into(),         JsValue::NativeFunction("removeChild",         identity_arg));
        e.set("insertBefore".into(),        JsValue::NativeFunction("insertBefore",        identity_arg));
        e.set("replaceChild".into(),        JsValue::NativeFunction("replaceChild",        identity_arg));
        e.set("setAttribute".into(),        JsValue::NativeFunction("setAttribute",        stub_void));
        e.set("getAttribute".into(),        JsValue::NativeFunction("getAttribute",        stub_null));
        e.set("hasAttribute".into(),        JsValue::NativeFunction("hasAttribute",        stub_false));
        e.set("removeAttribute".into(),     JsValue::NativeFunction("removeAttribute",     stub_void));
        e.set("getBoundingClientRect".into(), JsValue::NativeFunction("getBoundingClientRect", stub_rect));
        e.set("focus".into(),               JsValue::NativeFunction("focus",  stub_void));
        e.set("blur".into(),                JsValue::NativeFunction("blur",   stub_void));
        e.set("click".into(),               JsValue::NativeFunction("click",  stub_void));
        e.set("querySelector".into(),       JsValue::NativeFunction("querySelector",    stub_null));
        e.set("querySelectorAll".into(),    JsValue::NativeFunction("querySelectorAll", stub_empty_array));
    }
    JsValue::Object(o)
}

fn classlist_object() -> JsValue {
    let o = Rc::new(RefCell::new(JsObject::new()));
    {
        let mut c = o.borrow_mut();
        c.set("add".into(),      JsValue::NativeFunction("add",      stub_void));
        c.set("remove".into(),   JsValue::NativeFunction("remove",   stub_void));
        c.set("toggle".into(),   JsValue::NativeFunction("toggle",   stub_false));
        c.set("contains".into(), JsValue::NativeFunction("contains", stub_false));
        c.set("replace".into(),  JsValue::NativeFunction("replace",  stub_void));
    }
    JsValue::Object(o)
}

fn location_object() -> JsValue {
    let o = Rc::new(RefCell::new(JsObject::new()));
    {
        let mut l = o.borrow_mut();
        l.set("href".into(),     JsValue::Str(String::new()));
        l.set("origin".into(),   JsValue::Str(String::new()));
        l.set("protocol".into(), JsValue::Str("https:".into()));
        l.set("host".into(),     JsValue::Str(String::new()));
        l.set("hostname".into(), JsValue::Str(String::new()));
        l.set("port".into(),     JsValue::Str(String::new()));
        l.set("pathname".into(), JsValue::Str("/".into()));
        l.set("search".into(),   JsValue::Str(String::new()));
        l.set("hash".into(),     JsValue::Str(String::new()));
        l.set("assign".into(),  JsValue::NativeFunction("assign",  stub_void));
        l.set("replace".into(), JsValue::NativeFunction("replace", stub_void));
        l.set("reload".into(),  JsValue::NativeFunction("reload",  stub_void));
    }
    JsValue::Object(o)
}

fn navigator_object() -> JsValue {
    let o = Rc::new(RefCell::new(JsObject::new()));
    {
        let mut n = o.borrow_mut();
        n.set("userAgent".into(),    JsValue::Str("SmartOS/0.17 SmartBrowser".into()));
        n.set("platform".into(),     JsValue::Str("SmartOS".into()));
        n.set("language".into(),     JsValue::Str("en-US".into()));
        n.set("languages".into(),    JsValue::Array(Rc::new(RefCell::new(alloc::vec![JsValue::Str("en-US".into())]))));
        n.set("onLine".into(),       JsValue::Bool(true));
        n.set("cookieEnabled".into(),JsValue::Bool(false));
        n.set("doNotTrack".into(),   JsValue::Str("1".into()));
        n.set("vendor".into(),       JsValue::Str("Smart OS".into()));
        n.set("appName".into(),      JsValue::Str("Netscape".into()));
        n.set("appVersion".into(),   JsValue::Str("5.0 (SmartOS)".into()));
    }
    JsValue::Object(o)
}

// ── Native function stubs ────────────────────────────────────────────────────

fn stub_void(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue { JsValue::Undefined }
fn stub_true(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue { JsValue::Bool(true) }
fn stub_false(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue { JsValue::Bool(false) }
fn stub_null(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue { JsValue::Null }
fn stub_empty_array(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    JsValue::Array(Rc::new(RefCell::new(Vec::new())))
}
fn stub_empty_object(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    JsValue::Object(Rc::new(RefCell::new(JsObject::new())))
}
fn stub_media_query(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    // Return an MQ list that's always false.
    let o = Rc::new(RefCell::new(JsObject::new()));
    {
        let mut m = o.borrow_mut();
        m.set("matches".into(), JsValue::Bool(false));
        m.set("media".into(),   JsValue::Str(String::new()));
        m.set("addEventListener".into(),    JsValue::NativeFunction("addEventListener",    stub_void));
        m.set("removeEventListener".into(), JsValue::NativeFunction("removeEventListener", stub_void));
        m.set("addListener".into(),         JsValue::NativeFunction("addListener",         stub_void));
        m.set("removeListener".into(),      JsValue::NativeFunction("removeListener",      stub_void));
    }
    JsValue::Object(o)
}
fn stub_rect(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    let o = Rc::new(RefCell::new(JsObject::new()));
    {
        let mut r = o.borrow_mut();
        for k in ["x","y","top","left","right","bottom","width","height"] {
            r.set(k.into(), JsValue::Number(0.0));
        }
    }
    JsValue::Object(o)
}
fn identity_arg(args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    args.first().cloned().unwrap_or(JsValue::Undefined)
}
fn stub_get_element(args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    // For getElementById/querySelector we still return a fresh element so chained calls work.
    let tag = if let Some(JsValue::Str(_)) = args.first() { "div" } else { "div" };
    element_object(tag)
}
fn stub_get_collection(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    // Return an empty list; pages that test `.length` get 0.
    JsValue::Array(Rc::new(RefCell::new(Vec::new())))
}
fn stub_create_element(args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    let tag = match args.first() {
        Some(JsValue::Str(s)) => s.as_str(),
        _ => "div",
    };
    element_object(tag)
}
fn stub_create_fragment(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    element_object("documentfragment")
}

fn native_set_cookie(args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    if let Some(JsValue::Str(s)) = args.first() {
        let host = current_host();
        crate::net::cookies::set_from_js(s, &host);
    }
    JsValue::Undefined
}

fn native_get_cookie(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    let host = current_host();
    JsValue::Str(crate::net::cookies::all_for(&host))
}

// ─────────────────────────────────────────────────────────────────────────────
//  Heuristic safety filter
// ─────────────────────────────────────────────────────────────────────────────

fn safe_to_run(src: &str) -> bool {
    // Real-world minified bundles can be megabytes; our tree-walking interpreter
    // can't handle anything close to that responsively, so cap aggressively.
    if src.len() > 16 * 1024 { return false; }
    let mut s = src.to_ascii_lowercase();
    s.retain(|c| !c.is_whitespace());
    if s.contains("while(true)") || s.contains("while(1)") || s.contains("for(;;)") {
        return false;
    }
    // Modern JS features our parser typically can't handle — bail early.
    // These tokens nearly always appear in minified production code.
    let unsupported = [
        "async function", "await ", "=>", "...",
        "import ", "export ", "class ", "extends ",
        "try{", "try {", "catch(", "catch (",
        "?.", "??", "`",
    ];
    let lc = src.to_ascii_lowercase();
    for needle in unsupported {
        if lc.contains(needle) { return false; }
    }
    true
}

// ─────────────────────────────────────────────────────────────────────────────
//  Public legacy entry points (used elsewhere; thin wrappers over run_scripts).
// ─────────────────────────────────────────────────────────────────────────────

/// Convenience: run scripts on raw HTML and return only the title (if any).
pub fn scan_title(html: &str) -> Option<String> {
    let r = run_scripts(html, "", "");
    r.title.or_else(|| {
        // Fallback to literal scan if the real interp didn't set it.
        for body in iter_script_bodies(html) {
            if let Some(t) = scan_title_literal(&body) { return Some(t); }
        }
        None
    })
}

pub fn scan_console_logs(html: &str) -> Vec<String> {
    run_scripts(html, "", "").console
}

// ─────────────────────────────────────────────────────────────────────────────
//  Script-body extraction & literal fallback
// ─────────────────────────────────────────────────────────────────────────────

fn iter_script_bodies(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = html.as_bytes();
    let lower: String = html.chars().map(|c| c.to_ascii_lowercase()).collect();
    let lb = lower.as_bytes();

    let mut pos = 0;
    while pos < lb.len() {
        let open = match find_subslice(&lb[pos..], b"<script") {
            Some(o) => pos + o,
            None => break,
        };
        let tag_end = match find_byte(&lb[open..], b'>') {
            Some(o) => open + o + 1,
            None => break,
        };
        let opening_tag = &lower[open..tag_end];
        let has_src  = opening_tag.contains(" src=");
        // Reject obviously non-JS scripts (type=application/ld+json etc.) — leave
        // the contents alone, the layout engine will skip them too.
        let bad_type = opening_tag.contains("type=\"application/json")
            || opening_tag.contains("type=application/json")
            || opening_tag.contains("type=\"application/ld+json")
            || opening_tag.contains("type=application/ld+json");
        let close = match find_subslice(&lb[tag_end..], b"</script>") {
            Some(o) => tag_end + o,
            None => break,
        };
        if !has_src && !bad_type {
            let body = &bytes[tag_end..close];
            if let Ok(s) = core::str::from_utf8(body) {
                out.push(s.into());
            }
        }
        pos = close + b"</script>".len();
    }
    out
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() { return None; }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn find_byte(haystack: &[u8], b: u8) -> Option<usize> {
    haystack.iter().position(|&x| x == b)
}

fn scan_title_literal(src: &str) -> Option<String> {
    for line in src.split(|c| c == ';' || c == '\n') {
        let l = line.trim();
        let l = l.trim_start_matches("var ").trim_start_matches("let ").trim_start_matches("const ");
        let l = l.trim_start_matches("window.");
        if l.starts_with("document.title") {
            if let Some(eq) = l.find('=') {
                let rhs = l[eq + 1..].trim().trim_end_matches(';').trim();
                if let Some(s) = extract_string_literal(rhs) {
                    return Some(s);
                }
            }
        }
    }
    None
}

fn extract_string_literal(rhs: &str) -> Option<String> {
    let bytes = rhs.as_bytes();
    if bytes.is_empty() { return None; }
    let quote = bytes[0];
    if quote != b'"' && quote != b'\'' && quote != b'`' { return None; }
    let mut out = String::new();
    let mut i = 1;
    while i < bytes.len() {
        let b = bytes[i];
        if b == quote { return Some(out); }
        if b == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'n'  => out.push('\n'),
                b't'  => out.push('\t'),
                b'r'  => out.push('\r'),
                b'\\' => out.push('\\'),
                b'\'' => out.push('\''),
                b'"'  => out.push('"'),
                b'0'  => out.push('\0'),
                other => out.push(other as char),
            }
            i += 2;
            continue;
        }
        out.push(b as char);
        i += 1;
    }
    Some(out)
}

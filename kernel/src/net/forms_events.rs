//! Phase 133 — FormData, EventSource (SSE), AbortController/Signal, CustomEvent
//!
//! • FormData: key/value store with append/set/get/getAll/has/delete/entries/keys/values
//! • EventSource: SSE client stub (fires 'open' immediately; dispatchEvent for testing)
//! • AbortController / AbortSignal: standard fetch-cancellation tokens
//! • CustomEvent: `new CustomEvent(type, {detail, bubbles, cancelable})`
//! • MessageChannel / MessagePort: postMessage(data), onmessage

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use core::cell::RefCell;

use super::js_interp::{Interpreter, JsValue, JsObject};

// ── FormData ──────────────────────────────────────────────────────────────────

fn get_fd_pairs(interp: &Interpreter) -> Vec<(String, String)> {
    let this = interp.env.get("this");
    if let JsValue::Object(o) = &this {
        let arr = o.borrow().get("__pairs__");
        if let JsValue::Array(a) = arr {
            return a.borrow().iter().filter_map(|item| {
                if let JsValue::Array(pair) = item {
                    let p = pair.borrow();
                    let k = p.get(0).map(|v| v.to_string_val()).unwrap_or_default();
                    let v = p.get(1).map(|v| v.to_string_val()).unwrap_or_default();
                    Some((k, v))
                } else { None }
            }).collect();
        }
    }
    Vec::new()
}

fn set_fd_pairs(pairs: Vec<(String, String)>, interp: &mut Interpreter) {
    let this = interp.env.get("this");
    if let JsValue::Object(o) = &this {
        let arr: Vec<JsValue> = pairs.iter().map(|(k, v)| {
            JsValue::Array(Rc::new(RefCell::new(vec![
                JsValue::Str(k.clone()), JsValue::Str(v.clone()),
            ])))
        }).collect();
        o.borrow_mut().set("__pairs__".to_string(), JsValue::Array(Rc::new(RefCell::new(arr))));
    }
}

fn native_fd_append(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let key = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let val = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
    let mut pairs = get_fd_pairs(interp);
    pairs.push((key, val));
    set_fd_pairs(pairs, interp);
    JsValue::Undefined
}

fn native_fd_set(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let key = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let val = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
    let mut pairs = get_fd_pairs(interp);
    pairs.retain(|(k, _)| k != &key);
    pairs.push((key, val));
    set_fd_pairs(pairs, interp);
    JsValue::Undefined
}

fn native_fd_get(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let key = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let pairs = get_fd_pairs(interp);
    pairs.into_iter().find(|(k,_)| k == &key)
        .map(|(_,v)| JsValue::Str(v))
        .unwrap_or(JsValue::Null)
}

fn native_fd_get_all(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let key = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let pairs = get_fd_pairs(interp);
    let arr: Vec<JsValue> = pairs.into_iter().filter(|(k,_)| k == &key)
        .map(|(_,v)| JsValue::Str(v)).collect();
    JsValue::Array(Rc::new(RefCell::new(arr)))
}

fn native_fd_has(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let key = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    JsValue::Bool(get_fd_pairs(interp).iter().any(|(k,_)| k == &key))
}

fn native_fd_delete(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let key = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let mut pairs = get_fd_pairs(interp);
    pairs.retain(|(k,_)| k != &key);
    set_fd_pairs(pairs, interp);
    JsValue::Undefined
}

fn native_fd_entries(_: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let arr: Vec<JsValue> = get_fd_pairs(interp).into_iter().map(|(k,v)| {
        JsValue::Array(Rc::new(RefCell::new(vec![JsValue::Str(k), JsValue::Str(v)])))
    }).collect();
    JsValue::Array(Rc::new(RefCell::new(arr)))
}

fn native_fd_keys(_: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let arr: Vec<JsValue> = get_fd_pairs(interp).into_iter().map(|(k,_)| JsValue::Str(k)).collect();
    JsValue::Array(Rc::new(RefCell::new(arr)))
}

fn native_fd_values(_: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let arr: Vec<JsValue> = get_fd_pairs(interp).into_iter().map(|(_,v)| JsValue::Str(v)).collect();
    JsValue::Array(Rc::new(RefCell::new(arr)))
}

fn native_fd_for_each(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let pairs = get_fd_pairs(interp);
    for (k, v) in pairs {
        interp.call_value(cb.clone(), JsValue::Undefined,
            &[JsValue::Str(v), JsValue::Str(k)]);
    }
    JsValue::Undefined
}

fn make_form_data_obj() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__pairs__".to_string(), JsValue::Array(Rc::new(RefCell::new(vec![]))));
    obj.borrow_mut().set("append".to_string(),   JsValue::NativeFunction("append",  native_fd_append));
    obj.borrow_mut().set("set".to_string(),      JsValue::NativeFunction("set",     native_fd_set));
    obj.borrow_mut().set("get".to_string(),      JsValue::NativeFunction("get",     native_fd_get));
    obj.borrow_mut().set("getAll".to_string(),   JsValue::NativeFunction("getAll",  native_fd_get_all));
    obj.borrow_mut().set("has".to_string(),      JsValue::NativeFunction("has",     native_fd_has));
    obj.borrow_mut().set("delete".to_string(),   JsValue::NativeFunction("delete",  native_fd_delete));
    obj.borrow_mut().set("entries".to_string(),  JsValue::NativeFunction("entries", native_fd_entries));
    obj.borrow_mut().set("keys".to_string(),     JsValue::NativeFunction("keys",    native_fd_keys));
    obj.borrow_mut().set("values".to_string(),   JsValue::NativeFunction("values",  native_fd_values));
    obj.borrow_mut().set("forEach".to_string(),  JsValue::NativeFunction("forEach", native_fd_for_each));
    JsValue::Object(obj)
}

fn native_new_form_data(args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    let obj = make_form_data_obj();
    // If a form element is passed, try to read its elements
    if let Some(JsValue::Object(form)) = args.get(0) {
        let elements = form.borrow().get("elements");
        if let JsValue::Array(elems) = elements {
            let fd_obj = obj.clone();
            for elem in elems.borrow().iter() {
                if let JsValue::Object(el) = elem {
                    let name  = el.borrow().get("name").to_string_val();
                    let value = el.borrow().get("value").to_string_val();
                    if !name.is_empty() {
                        if let JsValue::Object(fd) = &fd_obj {
                            let mut pairs = Vec::new();
                            let cur = fd.borrow().get("__pairs__");
                            if let JsValue::Array(a) = cur {
                                for item in a.borrow().iter() {
                                    if let JsValue::Array(pair) = item {
                                        let p = pair.borrow();
                                        pairs.push((
                                            p.get(0).map(|v| v.to_string_val()).unwrap_or_default(),
                                            p.get(1).map(|v| v.to_string_val()).unwrap_or_default(),
                                        ));
                                    }
                                }
                            }
                            pairs.push((name, value));
                            let arr: Vec<JsValue> = pairs.iter().map(|(k,v)| {
                                JsValue::Array(Rc::new(RefCell::new(vec![
                                    JsValue::Str(k.clone()), JsValue::Str(v.clone()),
                                ])))
                            }).collect();
                            fd.borrow_mut().set("__pairs__".to_string(),
                                JsValue::Array(Rc::new(RefCell::new(arr))));
                        }
                    }
                }
            }
        }
    }
    obj
}

// ── AbortController / AbortSignal ─────────────────────────────────────────────

fn native_abort_signal_add_el(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    // addEventListener('abort', cb) — store cb in __listeners__ array
    let event_type = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let cb = args.get(1).cloned().unwrap_or(JsValue::Undefined);
    if event_type != "abort" { return JsValue::Undefined; }
    let this = interp.env.get("this");
    if let JsValue::Object(o) = &this {
        let listeners = o.borrow().get("__listeners__");
        if let JsValue::Array(arr) = listeners {
            arr.borrow_mut().push(cb);
        }
    }
    JsValue::Undefined
}

pub fn make_abort_signal() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("aborted".to_string(),         JsValue::Bool(false));
    obj.borrow_mut().set("reason".to_string(),          JsValue::Undefined);
    obj.borrow_mut().set("__listeners__".to_string(),   JsValue::Array(Rc::new(RefCell::new(vec![]))));
    obj.borrow_mut().set("addEventListener".to_string(), JsValue::NativeFunction("addEventListener", native_abort_signal_add_el));
    obj.borrow_mut().set("throwIfAborted".to_string(),   JsValue::NativeFunction("throwIfAborted", |_,_| JsValue::Undefined));
    JsValue::Object(obj)
}

fn native_abort_ctrl_abort(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let reason = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let this = interp.env.get("this");
    if let JsValue::Object(ctrl) = &this {
        let signal = ctrl.borrow().get("signal");
        if let JsValue::Object(sig) = &signal {
            sig.borrow_mut().set("aborted".to_string(), JsValue::Bool(true));
            sig.borrow_mut().set("reason".to_string(),  reason.clone());
            // Fire abort listeners
            let listeners = sig.borrow().get("__listeners__");
            if let JsValue::Array(arr) = listeners {
                let cbs: Vec<JsValue> = arr.borrow().iter().cloned().collect();
                for cb in cbs {
                    let event = Rc::new(RefCell::new(JsObject::new()));
                    event.borrow_mut().set("type".to_string(),   JsValue::Str("abort".to_string()));
                    event.borrow_mut().set("target".to_string(), signal.clone());
                    interp.call_value(cb, signal.clone(), &[JsValue::Object(event)]);
                }
            }
        }
    }
    JsValue::Undefined
}

fn native_new_abort_controller(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    let signal = make_abort_signal();
    let ctrl = Rc::new(RefCell::new(JsObject::new()));
    ctrl.borrow_mut().set("signal".to_string(), signal);
    ctrl.borrow_mut().set("abort".to_string(),  JsValue::NativeFunction("abort", native_abort_ctrl_abort));
    JsValue::Object(ctrl)
}

// ── EventSource (SSE) ─────────────────────────────────────────────────────────

fn native_es_add_el(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let event_type = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let cb = args.get(1).cloned().unwrap_or(JsValue::Undefined);
    let this = interp.env.get("this");
    if let JsValue::Object(o) = &this {
        let key = format!("__on_{}__", event_type);
        o.borrow_mut().set(key, cb);
    }
    JsValue::Undefined
}

fn native_es_close(_: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let this = interp.env.get("this");
    if let JsValue::Object(o) = &this {
        o.borrow_mut().set("readyState".to_string(), JsValue::Number(2.0)); // CLOSED
    }
    JsValue::Undefined
}

fn native_new_event_source(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let url = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("url".to_string(),              JsValue::Str(url));
    obj.borrow_mut().set("readyState".to_string(),       JsValue::Number(1.0)); // OPEN
    obj.borrow_mut().set("CONNECTING".to_string(),       JsValue::Number(0.0));
    obj.borrow_mut().set("OPEN".to_string(),             JsValue::Number(1.0));
    obj.borrow_mut().set("CLOSED".to_string(),           JsValue::Number(2.0));
    obj.borrow_mut().set("onopen".to_string(),           JsValue::Null);
    obj.borrow_mut().set("onmessage".to_string(),        JsValue::Null);
    obj.borrow_mut().set("onerror".to_string(),          JsValue::Null);
    obj.borrow_mut().set("withCredentials".to_string(),  JsValue::Bool(false));
    obj.borrow_mut().set("close".to_string(),            JsValue::NativeFunction("close", native_es_close));
    obj.borrow_mut().set("addEventListener".to_string(), JsValue::NativeFunction("addEventListener", native_es_add_el));
    obj.borrow_mut().set("removeEventListener".to_string(), JsValue::NativeFunction("removeEventListener", |_,_| JsValue::Undefined));

    let es = JsValue::Object(obj.clone());

    // Fire 'open' asynchronously (synchronously here since no real async)
    let onopen = obj.borrow().get("onopen");
    if !matches!(onopen, JsValue::Null | JsValue::Undefined) {
        let event = Rc::new(RefCell::new(JsObject::new()));
        event.borrow_mut().set("type".to_string(),   JsValue::Str("open".to_string()));
        event.borrow_mut().set("target".to_string(), es.clone());
        interp.call_value(onopen, es.clone(), &[JsValue::Object(event)]);
    }
    es
}

// ── CustomEvent ───────────────────────────────────────────────────────────────

fn native_new_custom_event(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let event_type = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let options = args.get(1).cloned().unwrap_or(JsValue::Undefined);
    let (detail, bubbles, cancelable) = if let JsValue::Object(o) = &options {
        let d = o.borrow().get("detail");
        let b = o.borrow().get("bubbles").is_truthy();
        let c = o.borrow().get("cancelable").is_truthy();
        (d, b, c)
    } else { (JsValue::Null, false, false) };

    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("type".to_string(),       JsValue::Str(event_type));
    obj.borrow_mut().set("detail".to_string(),     detail);
    obj.borrow_mut().set("bubbles".to_string(),    JsValue::Bool(bubbles));
    obj.borrow_mut().set("cancelable".to_string(), JsValue::Bool(cancelable));
    obj.borrow_mut().set("defaultPrevented".to_string(), JsValue::Bool(false));
    obj.borrow_mut().set("target".to_string(),     JsValue::Null);
    obj.borrow_mut().set("currentTarget".to_string(), JsValue::Null);
    obj.borrow_mut().set("timeStamp".to_string(),  JsValue::Number(0.0));
    obj.borrow_mut().set("preventDefault".to_string(),
        JsValue::NativeFunction("preventDefault", |_, i| {
            let this = i.env.get("this");
            if let JsValue::Object(o) = &this {
                o.borrow_mut().set("defaultPrevented".to_string(), JsValue::Bool(true));
            }
            JsValue::Undefined
        }));
    obj.borrow_mut().set("stopPropagation".to_string(),
        JsValue::NativeFunction("stopPropagation", |_,_| JsValue::Undefined));
    obj.borrow_mut().set("stopImmediatePropagation".to_string(),
        JsValue::NativeFunction("stopImmediatePropagation", |_,_| JsValue::Undefined));
    JsValue::Object(obj)
}

// ── Event (base constructor) ──────────────────────────────────────────────────

fn native_new_event(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    // Delegate to CustomEvent with no detail
    native_new_custom_event(args, interp)
}

// ── MessageChannel / MessagePort ──────────────────────────────────────────────

fn native_mp_post_message(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let data = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    // Find the peer port and fire its onmessage
    let this = interp.env.get("this");
    if let JsValue::Object(port) = &this {
        let peer = port.borrow().get("__peer__");
        if let JsValue::Object(p) = &peer {
            let onmessage = p.borrow().get("onmessage");
            if !matches!(onmessage, JsValue::Null | JsValue::Undefined) {
                let ev = Rc::new(RefCell::new(JsObject::new()));
                ev.borrow_mut().set("type".to_string(),   JsValue::Str("message".to_string()));
                ev.borrow_mut().set("data".to_string(),   data);
                ev.borrow_mut().set("target".to_string(), peer.clone());
                interp.call_value(onmessage, peer.clone(), &[JsValue::Object(ev)]);
            }
        }
    }
    JsValue::Undefined
}

fn native_mp_start(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Undefined }
fn native_mp_close(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Undefined }

fn make_port() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("onmessage".to_string(),      JsValue::Null);
    obj.borrow_mut().set("onmessageerror".to_string(), JsValue::Null);
    obj.borrow_mut().set("postMessage".to_string(),    JsValue::NativeFunction("postMessage", native_mp_post_message));
    obj.borrow_mut().set("start".to_string(),          JsValue::NativeFunction("start",       native_mp_start));
    obj.borrow_mut().set("close".to_string(),          JsValue::NativeFunction("close",       native_mp_close));
    obj.borrow_mut().set("addEventListener".to_string(),
        JsValue::NativeFunction("addEventListener", |args, i| {
            let t = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
            let cb = args.get(1).cloned().unwrap_or(JsValue::Undefined);
            if t == "message" {
                let this = i.env.get("this");
                if let JsValue::Object(o) = &this {
                    o.borrow_mut().set("onmessage".to_string(), cb);
                }
            }
            JsValue::Undefined
        }));
    JsValue::Object(obj)
}

fn native_new_message_channel(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    let port1 = make_port();
    let port2 = make_port();
    // Cross-link peers
    if let (JsValue::Object(p1), JsValue::Object(p2)) = (&port1, &port2) {
        p1.borrow_mut().set("__peer__".to_string(), port2.clone());
        p2.borrow_mut().set("__peer__".to_string(), port1.clone());
    }
    let ch = Rc::new(RefCell::new(JsObject::new()));
    ch.borrow_mut().set("port1".to_string(), port1);
    ch.borrow_mut().set("port2".to_string(), port2);
    JsValue::Object(ch)
}

// ── Public Rust structs (for test access) ─────────────────────────────────────

/// Rust-side AbortSignal (mirrors the JS object state).
pub struct AbortSignal { pub aborted: bool }
/// Rust-side AbortController.
pub struct AbortController { pub signal: AbortSignal }
impl AbortController {
    pub fn new() -> Self { AbortController { signal: AbortSignal { aborted: false } } }
    pub fn abort(&mut self) { self.signal.aborted = true; }
}
/// Rust-side CustomEvent record.
pub struct CustomEventRecord {
    pub event_type: String,
    pub detail:     String,
    pub bubbles:    bool,
    pub cancelable: bool,
}
/// Rust-side Event record.
pub struct EventRecord {
    pub event_type:        String,
    pub bubbles:           bool,
    pub cancelable:        bool,
    pub default_prevented: bool,
}
/// Rust-side FormData pair.
pub struct FormDataPair { pub name: String, pub value: String }
/// EventSource ready-state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSourceState { Connecting = 0, Open = 1, Closed = 2 }
/// Rust-side EventSource record.
pub struct EventSourceRecord { pub url: String, pub ready_state: EventSourceState }
/// Rust-side MessagePort handle.
pub struct MessagePort { pub channel_id: u32, pub port_index: u8 }
/// Rust-side MessageChannel factory.
pub struct MessageChannel;
impl MessageChannel {
    /// Create a linked pair of ports.
    pub fn create() -> (MessagePort, MessagePort) {
        static CHAN_ID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);
        let id = CHAN_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        (MessagePort { channel_id: id, port_index: 0 },
         MessagePort { channel_id: id, port_index: 1 })
    }
}

// ── Install ───────────────────────────────────────────────────────────────────

pub fn install_forms_events_api(interp: &mut Interpreter) {
    interp.env.define("FormData".to_string(),
        JsValue::NativeFunction("FormData", native_new_form_data));
    interp.env.define("AbortController".to_string(),
        JsValue::NativeFunction("AbortController", native_new_abort_controller));
    interp.env.define("EventSource".to_string(),
        JsValue::NativeFunction("EventSource", native_new_event_source));
    interp.env.define("CustomEvent".to_string(),
        JsValue::NativeFunction("CustomEvent", native_new_custom_event));
    interp.env.define("Event".to_string(),
        JsValue::NativeFunction("Event", native_new_event));
    interp.env.define("MessageChannel".to_string(),
        JsValue::NativeFunction("MessageChannel", native_new_message_channel));
}

// ── Self-test ─────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] forms_events: {}", $name); }
        }
    }

    // T1: FormData append/get/has/delete
    {
        let mut interp = Interpreter::new();
        install_forms_events_api(&mut interp);
        interp.run(r#"
            var fd = new FormData();
            fd.append("name", "Alice");
            fd.append("tag",  "a");
            fd.append("tag",  "b");
            var n   = fd.get("name");
            var all = fd.getAll("tag");
            var has = fd.has("name");
            fd.delete("name");
            var gone = fd.has("name");
        "#);
        check!(interp.env.get("n").to_string_val() == "Alice", "FormData get");
        check!(interp.env.get("has").is_truthy(),               "FormData has");
        check!(!interp.env.get("gone").is_truthy(),             "FormData delete");
        if let JsValue::Array(a) = interp.env.get("all") {
            check!(a.borrow().len() == 2, "FormData getAll count");
        } else { fail += 1; }
    }

    // T2: FormData set (replaces all)
    {
        let mut interp = Interpreter::new();
        install_forms_events_api(&mut interp);
        interp.run(r#"
            var fd = new FormData();
            fd.append("x","1"); fd.append("x","2");
            fd.set("x","99");
            var all = fd.getAll("x");
        "#);
        if let JsValue::Array(a) = interp.env.get("all") {
            check!(a.borrow().len() == 1, "FormData set replaces all");
            check!(a.borrow()[0].to_string_val() == "99", "FormData set value");
        } else { fail += 1; }
    }

    // T3: AbortController
    {
        let mut interp = Interpreter::new();
        install_forms_events_api(&mut interp);
        interp.run(r#"
            var ac = new AbortController();
            var aborted = ac.signal.aborted;
            var fired = false;
            ac.signal.addEventListener("abort", function(){ fired = true; });
            ac.abort();
            var abortedAfter = ac.signal.aborted;
        "#);
        check!(!interp.env.get("aborted").is_truthy(),      "signal.aborted initially false");
        check!( interp.env.get("abortedAfter").is_truthy(), "signal.aborted after abort()");
        check!( interp.env.get("fired").is_truthy(),        "abort listener fired");
    }

    // T4: CustomEvent detail
    {
        let mut interp = Interpreter::new();
        install_forms_events_api(&mut interp);
        interp.run(r#"
            var ev = new CustomEvent("click", {detail: {x: 42}, bubbles: true});
            var t  = ev.type;
            var d  = ev.detail.x;
            var b  = ev.bubbles;
        "#);
        check!(interp.env.get("t").to_string_val() == "click", "CustomEvent type");
        check!(interp.env.get("d").to_number() == 42.0,        "CustomEvent detail.x");
        check!(interp.env.get("b").is_truthy(),                "CustomEvent bubbles");
    }

    // T5: MessageChannel postMessage
    {
        let mut interp = Interpreter::new();
        install_forms_events_api(&mut interp);
        interp.run(r#"
            var mc = new MessageChannel();
            var received = null;
            mc.port2.onmessage = function(e){ received = e.data; };
            mc.port1.postMessage("hello");
        "#);
        check!(interp.env.get("received").to_string_val() == "hello", "MessageChannel postMessage");
    }

    // T6: FormData forEach
    {
        let mut interp = Interpreter::new();
        install_forms_events_api(&mut interp);
        interp.run(r#"
            var fd = new FormData();
            fd.append("a","1"); fd.append("b","2");
            var sum = 0;
            fd.forEach(function(val, key){ sum += parseInt(val); });
        "#);
        check!(interp.env.get("sum").to_number() == 3.0, "FormData forEach sum");
    }

    if fail == 0 {
        crate::serial_println!("[forms_events] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[forms_events] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

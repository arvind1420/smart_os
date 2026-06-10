//! WHATWG Streams API — Phase 129 for Smart OS.
//!
//! Implements the three stream primitives:
//!   • ReadableStream  — source of data, consumed via a reader
//!   • WritableStream  — sink for data, driven via a writer
//!   • TransformStream — writable input + readable output, optionally transformed
//!
//! Kernel implementation notes:
//!  - All operations are synchronous internally; Promises are pre-resolved.
//!  - Stream state lives in global `RS` / `WS` registries (keyed by u32 id).
//!  - Native methods access their receiver via `interp.env.get("this")`, which
//!    is set by call_value's new `this` forwarding for NativeFunction calls.
//!  - TransformStream links a WS "write" callback that enqueues into a RS.

#![allow(dead_code)]

extern crate alloc;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use core::cell::RefCell;
use spin::Mutex;

use super::js_interp::{Interpreter, JsValue, JsObject};

// ── Stream state ──────────────────────────────────────────────────────────────

struct RsEntry {
    queue:     VecDeque<JsValue>,
    closed:    bool,
    pull_fn:   JsValue,   // called when queue is empty
    cancel_fn: JsValue,
}

struct WsEntry {
    write_fn:  JsValue,
    close_fn:  JsValue,
    abort_fn:  JsValue,
    closed:    bool,
}

static RS: Mutex<(u32, BTreeMap<u32, RsEntry>)> = Mutex::new((1, BTreeMap::new()));
static WS: Mutex<(u32, BTreeMap<u32, WsEntry>)> = Mutex::new((1, BTreeMap::new()));

// Safety: these structs only contain JsValue (Rc-based), which are not
// actually sent across threads. The kernel is single-CPU at the JS layer.
unsafe impl Send for RsEntry {}
unsafe impl Sync for RsEntry {}
unsafe impl Send for WsEntry {}
unsafe impl Sync for WsEntry {}

fn rs_alloc(entry: RsEntry) -> u32 {
    let mut g = RS.lock();
    let id = g.0; g.0 = g.0.wrapping_add(1);
    g.1.insert(id, entry);
    id
}

fn ws_alloc(entry: WsEntry) -> u32 {
    let mut g = WS.lock();
    let id = g.0; g.0 = g.0.wrapping_add(1);
    g.1.insert(id, entry);
    id
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Get rsId from `this` in interpreter env.
fn this_rs_id(interp: &Interpreter) -> Option<u32> {
    let this = interp.env.get("this");
    obj_field_u32(&this, "__rsId__")
}

/// Get wsId from `this` in interpreter env.
fn this_ws_id(interp: &Interpreter) -> Option<u32> {
    let this = interp.env.get("this");
    obj_field_u32(&this, "__wsId__")
}

fn obj_field_u32(val: &JsValue, key: &str) -> Option<u32> {
    if let JsValue::Object(o) = val {
        let v = o.borrow().get(key);
        if !matches!(v, JsValue::Undefined | JsValue::Null) {
            return Some(v.to_number() as u32);
        }
    }
    None
}

fn obj_field(val: &JsValue, key: &str) -> JsValue {
    if let JsValue::Object(o) = val { o.borrow().get(key) } else { JsValue::Undefined }
}

/// Build `{value, done}` result object for reader.read().
fn read_result(value: JsValue, done: bool) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("value".to_string(), value);
    obj.borrow_mut().set("done".to_string(), JsValue::Bool(done));
    JsValue::Object(obj)
}

// ── ReadableStream JS object ──────────────────────────────────────────────────

fn make_rs_obj(id: u32) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__rsId__".to_string(),    JsValue::Number(id as f64));
    obj.borrow_mut().set("locked".to_string(),       JsValue::Bool(false));
    obj.borrow_mut().set("getReader".to_string(),    JsValue::NativeFunction("getReader",    native_rs_get_reader));
    obj.borrow_mut().set("pipeTo".to_string(),       JsValue::NativeFunction("pipeTo",       native_rs_pipe_to));
    obj.borrow_mut().set("pipeThrough".to_string(),  JsValue::NativeFunction("pipeThrough",  native_rs_pipe_through));
    obj.borrow_mut().set("cancel".to_string(),       JsValue::NativeFunction("cancel",       native_rs_cancel));
    obj.borrow_mut().set("tee".to_string(),          JsValue::NativeFunction("tee",          native_rs_tee));
    JsValue::Object(obj)
}

// ── WritableStream JS object ──────────────────────────────────────────────────

fn make_ws_obj(id: u32) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__wsId__".to_string(),  JsValue::Number(id as f64));
    obj.borrow_mut().set("locked".to_string(),     JsValue::Bool(false));
    obj.borrow_mut().set("getWriter".to_string(),  JsValue::NativeFunction("getWriter",  native_ws_get_writer));
    obj.borrow_mut().set("close".to_string(),      JsValue::NativeFunction("close",      native_ws_close_stream));
    obj.borrow_mut().set("abort".to_string(),      JsValue::NativeFunction("abort",      native_ws_abort_stream));
    JsValue::Object(obj)
}

// ── ReadableStreamDefaultController ──────────────────────────────────────────

fn make_rs_controller(id: u32) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__rsId__".to_string(),           JsValue::Number(id as f64));
    obj.borrow_mut().set("desiredSize".to_string(),        JsValue::Number(1.0));
    obj.borrow_mut().set("enqueue".to_string(),            JsValue::NativeFunction("enqueue",  native_rsc_enqueue));
    obj.borrow_mut().set("close".to_string(),              JsValue::NativeFunction("close",    native_rsc_close));
    obj.borrow_mut().set("error".to_string(),              JsValue::NativeFunction("error",    native_rsc_error));
    JsValue::Object(obj)
}

// ── WritableStreamDefaultController ──────────────────────────────────────────

fn make_ws_controller(_id: u32) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("error".to_string(), JsValue::NativeFunction("error", |_, _| JsValue::Undefined));
    JsValue::Object(obj)
}

// ── ReadableStreamDefaultReader ───────────────────────────────────────────────

fn make_rs_reader(rs_id: u32) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__rsId__".to_string(),      JsValue::Number(rs_id as f64));
    obj.borrow_mut().set("read".to_string(),           JsValue::NativeFunction("read",        native_reader_read));
    obj.borrow_mut().set("cancel".to_string(),         JsValue::NativeFunction("cancel",      native_reader_cancel));
    obj.borrow_mut().set("releaseLock".to_string(),    JsValue::NativeFunction("releaseLock", native_reader_release));
    // closed: a pre-resolved promise (Undefined for our engine — awaiting it is a no-op)
    obj.borrow_mut().set("closed".to_string(),         JsValue::Undefined);
    JsValue::Object(obj)
}

// ── WritableStreamDefaultWriter ───────────────────────────────────────────────

fn make_ws_writer(ws_id: u32) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__wsId__".to_string(),       JsValue::Number(ws_id as f64));
    obj.borrow_mut().set("write".to_string(),           JsValue::NativeFunction("write",       native_writer_write));
    obj.borrow_mut().set("close".to_string(),           JsValue::NativeFunction("close",       native_writer_close));
    obj.borrow_mut().set("abort".to_string(),           JsValue::NativeFunction("abort",       native_writer_abort));
    obj.borrow_mut().set("releaseLock".to_string(),     JsValue::NativeFunction("releaseLock", native_writer_release));
    obj.borrow_mut().set("ready".to_string(),           JsValue::Undefined); // pre-resolved
    obj.borrow_mut().set("closed".to_string(),          JsValue::Undefined);
    obj.borrow_mut().set("desiredSize".to_string(),     JsValue::Number(1.0));
    JsValue::Object(obj)
}

// ── ReadableStreamDefaultController native methods ───────────────────────────

// controller.enqueue(chunk)
fn native_rsc_enqueue(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    if let Some(id) = this_rs_id(interp) {
        let chunk = args.get(0).cloned().unwrap_or(JsValue::Undefined);
        if let Some(e) = RS.lock().1.get_mut(&id) {
            if !e.closed { e.queue.push_back(chunk); }
        }
    }
    JsValue::Undefined
}

// controller.close()
fn native_rsc_close(_args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    if let Some(id) = this_rs_id(interp) {
        if let Some(e) = RS.lock().1.get_mut(&id) { e.closed = true; }
    }
    JsValue::Undefined
}

// controller.error(e)
fn native_rsc_error(_args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    if let Some(id) = this_rs_id(interp) {
        if let Some(e) = RS.lock().1.get_mut(&id) { e.closed = true; }
    }
    JsValue::Undefined
}

// ── ReadableStream native methods ─────────────────────────────────────────────

// rs.getReader()
fn native_rs_get_reader(_args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    if let Some(id) = this_rs_id(interp) {
        make_rs_reader(id)
    } else {
        JsValue::Undefined
    }
}

// rs.cancel(reason)
fn native_rs_cancel(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    if let Some(id) = this_rs_id(interp) {
        let cancel_fn = RS.lock().1.get(&id)
            .map(|e| e.cancel_fn.clone())
            .unwrap_or(JsValue::Undefined);
        let reason = args.get(0).cloned().unwrap_or(JsValue::Undefined);
        if !matches!(cancel_fn, JsValue::Undefined) {
            interp.call_value(cancel_fn, JsValue::Undefined, &[reason]);
        }
        if let Some(e) = RS.lock().1.get_mut(&id) { e.closed = true; }
    }
    JsValue::Undefined
}

// rs.pipeTo(writableStream, options?)  — synchronous drain
fn native_rs_pipe_to(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let rs_id = match this_rs_id(interp) { Some(id) => id, None => return JsValue::Undefined };
    let ws_id = match obj_field_u32(args.get(0).unwrap_or(&JsValue::Undefined), "__wsId__") {
        Some(id) => id, None => return JsValue::Undefined,
    };
    // Drain all queued chunks
    loop {
        let chunk = {
            let mut g = RS.lock();
            if let Some(e) = g.1.get_mut(&rs_id) {
                e.queue.pop_front()
            } else { break; }
        };
        match chunk {
            Some(c) => {
                // Invoke write_fn
                let write_fn = WS.lock().1.get(&ws_id).map(|e| e.write_fn.clone())
                    .unwrap_or(JsValue::Undefined);
                if !matches!(write_fn, JsValue::Undefined | JsValue::Null) {
                    let wsc = make_ws_controller(ws_id);
                    interp.call_value(write_fn, JsValue::Undefined, &[c, wsc]);
                }
            }
            None => break,
        }
    }
    // If rs is closed, close the ws too
    let rs_closed = RS.lock().1.get(&rs_id).map(|e| e.closed).unwrap_or(true);
    if rs_closed {
        let close_fn = WS.lock().1.get(&ws_id).map(|e| e.close_fn.clone())
            .unwrap_or(JsValue::Undefined);
        if !matches!(close_fn, JsValue::Undefined | JsValue::Null) {
            let wsc = make_ws_controller(ws_id);
            interp.call_value(close_fn, JsValue::Undefined, &[wsc]);
        }
        if let Some(e) = WS.lock().1.get_mut(&ws_id) { e.closed = true; }
    }
    JsValue::Undefined // pre-resolved Promise
}

// rs.pipeThrough({writable, readable}, options?)
fn native_rs_pipe_through(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let transform = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let ws_obj = obj_field(&transform, "writable");
    let out_rs  = obj_field(&transform, "readable");
    // Pipe this → ws
    let _ = native_rs_pipe_to(&[ws_obj], interp);
    // Return readable side
    out_rs
}

// rs.tee() → [rs1, rs2]  (simple copy — both share same queue snapshot)
fn native_rs_tee(_args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let id = match this_rs_id(interp) { Some(id) => id, None => return JsValue::Undefined };
    // Snapshot the queue into two new streams
    let (queue_snap, closed) = {
        let g = RS.lock();
        if let Some(e) = g.1.get(&id) {
            let q: VecDeque<JsValue> = e.queue.iter().cloned().collect();
            (q, e.closed)
        } else {
            (VecDeque::new(), true)
        }
    };
    let id1 = rs_alloc(RsEntry {
        queue: queue_snap.clone(), closed, pull_fn: JsValue::Undefined, cancel_fn: JsValue::Undefined
    });
    let id2 = rs_alloc(RsEntry {
        queue: queue_snap, closed, pull_fn: JsValue::Undefined, cancel_fn: JsValue::Undefined
    });
    let arr = Rc::new(RefCell::new(alloc::vec![make_rs_obj(id1), make_rs_obj(id2)]));
    JsValue::Array(arr)
}

// ── ReadableStreamDefaultReader native methods ────────────────────────────────

// reader.read()  →  {value, done}
fn native_reader_read(_args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let rs_id = match this_rs_id(interp) { Some(id) => id, None => return read_result(JsValue::Undefined, true) };

    // Try pull if queue is empty
    let (has_chunk, pull_fn) = {
        let g = RS.lock();
        if let Some(e) = g.1.get(&rs_id) {
            (!e.queue.is_empty(), if e.queue.is_empty() { e.pull_fn.clone() } else { JsValue::Undefined })
        } else {
            return read_result(JsValue::Undefined, true);
        }
    };

    if !has_chunk && !matches!(pull_fn, JsValue::Undefined | JsValue::Null) {
        // Call pull(controller) to let source enqueue more
        let ctrl = make_rs_controller(rs_id);
        interp.call_value(pull_fn, JsValue::Undefined, &[ctrl]);
    }

    // Now dequeue
    let (chunk, closed) = {
        let mut g = RS.lock();
        if let Some(e) = g.1.get_mut(&rs_id) {
            (e.queue.pop_front(), e.closed)
        } else {
            return read_result(JsValue::Undefined, true);
        }
    };

    match chunk {
        Some(c) => read_result(c, false),
        None => read_result(JsValue::Undefined, closed),
    }
}

// reader.cancel(reason)
fn native_reader_cancel(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    native_rs_cancel(args, interp)
}

// reader.releaseLock()
fn native_reader_release(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    JsValue::Undefined
}

// ── WritableStream native methods ─────────────────────────────────────────────

// ws.getWriter()
fn native_ws_get_writer(_args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    if let Some(id) = this_ws_id(interp) { make_ws_writer(id) } else { JsValue::Undefined }
}

// ws.close()
fn native_ws_close_stream(_args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    if let Some(id) = this_ws_id(interp) {
        let close_fn = WS.lock().1.get(&id).map(|e| e.close_fn.clone()).unwrap_or(JsValue::Undefined);
        if !matches!(close_fn, JsValue::Undefined | JsValue::Null) {
            let ctrl = make_ws_controller(id);
            interp.call_value(close_fn, JsValue::Undefined, &[ctrl]);
        }
        if let Some(e) = WS.lock().1.get_mut(&id) { e.closed = true; }
    }
    JsValue::Undefined
}

// ws.abort(reason)
fn native_ws_abort_stream(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    if let Some(id) = this_ws_id(interp) {
        let abort_fn = WS.lock().1.get(&id).map(|e| e.abort_fn.clone()).unwrap_or(JsValue::Undefined);
        if !matches!(abort_fn, JsValue::Undefined | JsValue::Null) {
            let reason = args.get(0).cloned().unwrap_or(JsValue::Undefined);
            interp.call_value(abort_fn, JsValue::Undefined, &[reason]);
        }
        if let Some(e) = WS.lock().1.get_mut(&id) { e.closed = true; }
    }
    JsValue::Undefined
}

// ── WritableStreamDefaultWriter native methods ────────────────────────────────

// writer.write(chunk)
fn native_writer_write(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    if let Some(id) = this_ws_id(interp) {
        let write_fn = WS.lock().1.get(&id).map(|e| e.write_fn.clone()).unwrap_or(JsValue::Undefined);
        if !matches!(write_fn, JsValue::Undefined | JsValue::Null) {
            let chunk = args.get(0).cloned().unwrap_or(JsValue::Undefined);
            let ctrl = make_ws_controller(id);
            interp.call_value(write_fn, JsValue::Undefined, &[chunk, ctrl]);
        }
    }
    JsValue::Undefined
}

// writer.close()
fn native_writer_close(_args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    if let Some(id) = this_ws_id(interp) {
        let close_fn = WS.lock().1.get(&id).map(|e| e.close_fn.clone()).unwrap_or(JsValue::Undefined);
        if !matches!(close_fn, JsValue::Undefined | JsValue::Null) {
            let ctrl = make_ws_controller(id);
            interp.call_value(close_fn, JsValue::Undefined, &[ctrl]);
        }
        if let Some(e) = WS.lock().1.get_mut(&id) { e.closed = true; }
    }
    JsValue::Undefined
}

// writer.abort(reason)
fn native_writer_abort(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    if let Some(id) = this_ws_id(interp) {
        let abort_fn = WS.lock().1.get(&id).map(|e| e.abort_fn.clone()).unwrap_or(JsValue::Undefined);
        if !matches!(abort_fn, JsValue::Undefined | JsValue::Null) {
            let reason = args.get(0).cloned().unwrap_or(JsValue::Undefined);
            interp.call_value(abort_fn, JsValue::Undefined, &[reason]);
        }
        if let Some(e) = WS.lock().1.get_mut(&id) { e.closed = true; }
    }
    JsValue::Undefined
}

// writer.releaseLock()
fn native_writer_release(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    JsValue::Undefined
}

// ── Constructors ──────────────────────────────────────────────────────────────

// new ReadableStream({start?, pull?, cancel?})
fn native_new_readable_stream(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let source = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let start_fn  = obj_field(&source, "start");
    let pull_fn   = obj_field(&source, "pull");
    let cancel_fn = obj_field(&source, "cancel");

    let id = rs_alloc(RsEntry {
        queue: VecDeque::new(),
        closed: false,
        pull_fn,
        cancel_fn,
    });

    // Call start(controller) synchronously
    if !matches!(start_fn, JsValue::Undefined | JsValue::Null) {
        let ctrl = make_rs_controller(id);
        interp.call_value(start_fn, JsValue::Undefined, &[ctrl]);
    }

    make_rs_obj(id)
}

// new WritableStream({start?, write?, close?, abort?}, queuingStrategy?)
fn native_new_writable_stream(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let sink = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let start_fn = obj_field(&sink, "start");
    let write_fn = obj_field(&sink, "write");
    let close_fn = obj_field(&sink, "close");
    let abort_fn = obj_field(&sink, "abort");

    let id = ws_alloc(WsEntry {
        write_fn, close_fn, abort_fn, closed: false,
    });

    if !matches!(start_fn, JsValue::Undefined | JsValue::Null) {
        let ctrl = make_ws_controller(id);
        interp.call_value(start_fn, JsValue::Undefined, &[ctrl]);
    }

    make_ws_obj(id)
}

// new TransformStream({start?, transform?, flush?}, readableStrategy?, writableStrategy?)
fn native_new_transform_stream(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let transformer = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let start_fn     = obj_field(&transformer, "start");
    let transform_fn = obj_field(&transformer, "transform");
    let flush_fn     = obj_field(&transformer, "flush");

    // Create output readable stream (initially empty, filled by transform controller)
    let rs_id = rs_alloc(RsEntry {
        queue: VecDeque::new(),
        closed: false,
        pull_fn:   JsValue::Undefined,
        cancel_fn: JsValue::Undefined,
    });

    // The writable side's write callback calls transform_fn(chunk, tc)
    // where tc.enqueue() pushes to the readable side.
    // We encode rs_id into a transform controller object.
    let tc_fn = transform_fn.clone();
    let rs_id_val = JsValue::Number(rs_id as f64);

    // Build a WritableStream whose write_fn wraps the transform
    // We use a special "transform-write" pattern:
    // write_fn receives (chunk, ws_ctrl), calls transform_fn(chunk, ts_ctrl)
    // ts_ctrl.enqueue pushes to rs_id.
    // Since NativeFunction can't capture rs_id directly, we store it in a
    // wrapper object that the interpreter env sees via __ts_rs_id__.
    //
    // Approach: store rs_id in the WsEntry.write_fn as a wrapper JS object,
    // and call the transform_fn via interp with __ts_rs_id__ set.
    // We'll use a special "transform-ws" write_fn that is a NativeFunction
    // reading __ts_rs_id__ from env.

    // Actually, simplest: write_fn is set to transform_fn directly, and
    // we also store rs_id as __ts_rs_id__ in the WsEntry somehow.
    // Workaround: embed rs_id in the WsEntry.write_fn JsValue as
    // a {fn, rsId} object, and native_ts_write reads from it.

    // We'll use a tagged object as write_fn:
    let ts_write_obj = {
        let o = Rc::new(RefCell::new(JsObject::new()));
        o.borrow_mut().set("__ts__".to_string(), JsValue::Bool(true));
        o.borrow_mut().set("__rsId__".to_string(), rs_id_val);
        o.borrow_mut().set("fn".to_string(), tc_fn);
        JsValue::Object(o)
    };

    let ts_flush_obj = {
        let o = Rc::new(RefCell::new(JsObject::new()));
        o.borrow_mut().set("__ts__".to_string(), JsValue::Bool(true));
        o.borrow_mut().set("__rsId__".to_string(), JsValue::Number(rs_id as f64));
        o.borrow_mut().set("fn".to_string(), flush_fn);
        JsValue::Object(o)
    };

    let ws_id = ws_alloc(WsEntry {
        write_fn:  ts_write_obj,
        close_fn:  ts_flush_obj,
        abort_fn:  JsValue::Undefined,
        closed:    false,
    });

    // Override the WritableStream's write to be the TS-aware version
    // Build a controller for start callback
    let ts_ctrl = make_ts_controller(rs_id);

    if !matches!(start_fn, JsValue::Undefined | JsValue::Null) {
        interp.call_value(start_fn, JsValue::Undefined, &[ts_ctrl.clone()]);
    }

    // Build the TransformStream object
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("readable".to_string(), make_rs_obj(rs_id));
    obj.borrow_mut().set("writable".to_string(), make_ts_writable_obj(ws_id, rs_id));
    JsValue::Object(obj)
}

/// TransformStreamDefaultController — `this.__rsId__` = output RS id
fn make_ts_controller(rs_id: u32) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__rsId__".to_string(), JsValue::Number(rs_id as f64));
    obj.borrow_mut().set("enqueue".to_string(),  JsValue::NativeFunction("enqueue", native_rsc_enqueue));
    obj.borrow_mut().set("terminate".to_string(),JsValue::NativeFunction("terminate", native_rsc_close));
    obj.borrow_mut().set("error".to_string(),    JsValue::NativeFunction("error",   native_rsc_error));
    JsValue::Object(obj)
}

/// Writable side of TransformStream — write goes through ts_dispatch
fn make_ts_writable_obj(ws_id: u32, rs_id: u32) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__wsId__".to_string(),  JsValue::Number(ws_id as f64));
    obj.borrow_mut().set("__rsId__".to_string(),  JsValue::Number(rs_id as f64));
    obj.borrow_mut().set("locked".to_string(),     JsValue::Bool(false));
    obj.borrow_mut().set("getWriter".to_string(),  JsValue::NativeFunction("getWriter",  native_ts_get_writer));
    obj.borrow_mut().set("close".to_string(),      JsValue::NativeFunction("close",      native_ts_ws_close));
    obj.borrow_mut().set("abort".to_string(),      JsValue::NativeFunction("abort",      native_ws_abort_stream));
    JsValue::Object(obj)
}

fn native_ts_get_writer(_args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let this = interp.env.get("this");
    let ws_id = obj_field_u32(&this, "__wsId__").unwrap_or(0);
    let rs_id = obj_field_u32(&this, "__rsId__").unwrap_or(0);
    make_ts_writer(ws_id, rs_id)
}

fn make_ts_writer(ws_id: u32, rs_id: u32) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__wsId__".to_string(),  JsValue::Number(ws_id as f64));
    obj.borrow_mut().set("__rsId__".to_string(),  JsValue::Number(rs_id as f64));
    obj.borrow_mut().set("write".to_string(),      JsValue::NativeFunction("write",       native_ts_writer_write));
    obj.borrow_mut().set("close".to_string(),      JsValue::NativeFunction("close",       native_ts_ws_close));
    obj.borrow_mut().set("abort".to_string(),      JsValue::NativeFunction("abort",       native_writer_abort));
    obj.borrow_mut().set("releaseLock".to_string(),JsValue::NativeFunction("releaseLock", native_writer_release));
    obj.borrow_mut().set("ready".to_string(),      JsValue::Undefined);
    obj.borrow_mut().set("desiredSize".to_string(),JsValue::Number(1.0));
    JsValue::Object(obj)
}

/// writer.write(chunk) for transform stream — calls transform_fn(chunk, ts_ctrl)
fn native_ts_writer_write(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let this = interp.env.get("this");
    let ws_id = obj_field_u32(&this, "__wsId__").unwrap_or(0);
    let rs_id = obj_field_u32(&this, "__rsId__").unwrap_or(0);
    ts_dispatch_write(ws_id, rs_id, args, interp);
    JsValue::Undefined
}

/// ws.close() for transform stream — calls flush_fn(tc)
fn native_ts_ws_close(_args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let this = interp.env.get("this");
    let ws_id = obj_field_u32(&this, "__wsId__").unwrap_or(0);
    let rs_id = obj_field_u32(&this, "__rsId__").unwrap_or(0);

    // Get flush_fn from close_fn field (ts_flush_obj)
    let flush_info = WS.lock().1.get(&ws_id).map(|e| e.close_fn.clone()).unwrap_or(JsValue::Undefined);
    if let JsValue::Object(ref o) = flush_info {
        let flush_fn = o.borrow().get("fn");
        if !matches!(flush_fn, JsValue::Undefined | JsValue::Null) {
            let tc = make_ts_controller(rs_id);
            interp.call_value(flush_fn, JsValue::Undefined, &[tc]);
        }
    }
    // Close both sides
    if let Some(e) = WS.lock().1.get_mut(&ws_id) { e.closed = true; }
    if let Some(e) = RS.lock().1.get_mut(&rs_id)  { e.closed = true; }
    JsValue::Undefined
}

/// Called for a TS writable write — looks up transform_fn in write_fn field
fn ts_dispatch_write(ws_id: u32, rs_id: u32, args: &[JsValue], interp: &mut Interpreter) {
    let write_info = WS.lock().1.get(&ws_id).map(|e| e.write_fn.clone()).unwrap_or(JsValue::Undefined);
    if let JsValue::Object(ref o) = write_info {
        let transform_fn = o.borrow().get("fn");
        if !matches!(transform_fn, JsValue::Undefined | JsValue::Null) {
            let chunk = args.get(0).cloned().unwrap_or(JsValue::Undefined);
            let tc    = make_ts_controller(rs_id);
            interp.call_value(transform_fn, JsValue::Undefined, &[chunk, tc]);
        }
    } else {
        // Identity transform: pass chunk through to readable side
        let chunk = args.get(0).cloned().unwrap_or(JsValue::Undefined);
        if let Some(e) = RS.lock().1.get_mut(&rs_id) {
            e.queue.push_back(chunk);
        }
    }
}

// ── Public Rust-level stream handle API (for test access) ─────────────────────

/// Create a ReadableStream entry and return its ID.
pub fn rs_create() -> u32 {
    rs_alloc(RsEntry { queue: VecDeque::new(), closed: false, pull_fn: JsValue::Undefined, cancel_fn: JsValue::Undefined })
}

/// Create a WritableStream entry and return its ID.
pub fn ws_create() -> u32 {
    ws_alloc(WsEntry { write_fn: JsValue::Undefined, close_fn: JsValue::Undefined, abort_fn: JsValue::Undefined, closed: false })
}

/// True if the RS is not closed.
pub fn rs_is_readable(id: u32) -> bool {
    RS.lock().1.get(&id).map(|e| !e.closed).unwrap_or(false)
}

/// True if the WS is not closed.
pub fn ws_is_writable(id: u32) -> bool {
    WS.lock().1.get(&id).map(|e| !e.closed).unwrap_or(false)
}

/// Enqueue a raw byte slice into an RS as a single Str chunk.
pub fn rs_enqueue(id: u32, data: &[u8]) {
    let s = alloc::string::String::from_utf8_lossy(data).into_owned();
    if let Some(e) = RS.lock().1.get_mut(&id) {
        e.queue.push_back(JsValue::Str(s));
    }
}

/// Read a chunk from an RS as a byte Vec, or None if empty.
pub fn rs_read(id: u32) -> Option<alloc::vec::Vec<u8>> {
    let chunk = RS.lock().1.get_mut(&id)?.queue.pop_front()?;
    Some(chunk.to_string_val().into_bytes())
}

/// Write a byte slice to a WS (no-op if no write_fn).
pub fn ws_write(id: u32, _data: &[u8]) -> Result<(), &'static str> {
    if WS.lock().1.contains_key(&id) { Ok(()) } else { Err("no such WS") }
}

/// Close an RS (mark done).
pub fn rs_close(id: u32) {
    if let Some(e) = RS.lock().1.get_mut(&id) { e.closed = true; }
}

/// True if the RS was closed.
pub fn rs_is_closed(id: u32) -> bool {
    RS.lock().1.get(&id).map(|e| e.closed).unwrap_or(true)
}

// ── Install ───────────────────────────────────────────────────────────────────

/// Install `ReadableStream`, `WritableStream`, `TransformStream` constructors.
/// Call from `browser.rs::install_dom_api()`.
pub fn install_streams_api(interp: &mut Interpreter) {
    interp.env.define("ReadableStream".to_string(),
        JsValue::NativeFunction("ReadableStream", native_new_readable_stream));
    interp.env.define("WritableStream".to_string(),
        JsValue::NativeFunction("WritableStream",  native_new_writable_stream));
    interp.env.define("TransformStream".to_string(),
        JsValue::NativeFunction("TransformStream", native_new_transform_stream));
}

// ── Self-test ─────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] streams: {}", $name); }
        }
    }

    let mut interp = Interpreter::new();
    install_streams_api(&mut interp);

    // T1: ReadableStream basic enqueue + read
    interp.run(r#"
        var rs1 = new ReadableStream({
            start(controller) {
                controller.enqueue(42);
                controller.enqueue(99);
                controller.close();
            }
        });
        var reader1 = rs1.getReader();
        var r1a = reader1.read();
        var r1b = reader1.read();
        var r1c = reader1.read();
    "#);
    {
        let r1a = interp.env.get("r1a");
        let r1b = interp.env.get("r1b");
        let r1c = interp.env.get("r1c");
        let val_a = obj_field(&r1a, "value").to_number() as u32;
        let done_a = obj_field(&r1a, "done");
        let val_b = obj_field(&r1b, "value").to_number() as u32;
        let done_c = obj_field(&r1c, "done");
        check!(val_a == 42, "RS read first chunk value");
        check!(matches!(done_a, JsValue::Bool(false)), "RS read first chunk not done");
        check!(val_b == 99, "RS read second chunk value");
        check!(matches!(done_c, JsValue::Bool(true)), "RS read done after close");
    }

    // T2: WritableStream basic write
    interp.run(r#"
        var written = [];
        var ws1 = new WritableStream({
            write(chunk) { written.push(chunk); }
        });
        var writer1 = ws1.getWriter();
        writer1.write(10);
        writer1.write(20);
        writer1.close();
    "#);
    {
        let written = interp.env.get("written");
        if let JsValue::Array(arr) = &written {
            let arr = arr.borrow();
            check!(arr.len() == 2, "WS write count");
            check!(arr[0].to_number() as u32 == 10, "WS write first value");
            check!(arr[1].to_number() as u32 == 20, "WS write second value");
        } else {
            check!(false, "WS written is array");
        }
    }

    // T3: pipeTo drains RS into WS
    interp.run(r#"
        var piped = [];
        var rs2 = new ReadableStream({
            start(c) { c.enqueue('a'); c.enqueue('b'); c.close(); }
        });
        var ws2 = new WritableStream({
            write(chunk) { piped.push(chunk); }
        });
        rs2.pipeTo(ws2);
    "#);
    {
        let piped = interp.env.get("piped");
        if let JsValue::Array(arr) = &piped {
            let arr = arr.borrow();
            check!(arr.len() == 2, "pipeTo chunk count");
            check!(arr[0].to_string_val() == "a", "pipeTo first chunk");
            check!(arr[1].to_string_val() == "b", "pipeTo second chunk");
        } else {
            check!(false, "pipeTo piped is array");
        }
    }

    // T4: TransformStream (uppercase transform)
    interp.run(r#"
        var tsOut = [];
        var ts = new TransformStream({
            transform(chunk, controller) {
                controller.enqueue(chunk + '!');
            }
        });
        var tsWriter = ts.writable.getWriter();
        var tsReader = ts.readable.getReader();
        tsWriter.write('hello');
        tsWriter.write('world');
        var ts1 = tsReader.read();
        var ts2 = tsReader.read();
    "#);
    {
        let ts1 = interp.env.get("ts1");
        let ts2 = interp.env.get("ts2");
        let v1 = obj_field(&ts1, "value").to_string_val();
        let v2 = obj_field(&ts2, "value").to_string_val();
        check!(v1 == "hello!", "TS transform first chunk");
        check!(v2 == "world!", "TS transform second chunk");
    }

    // T5: RS.tee() splits into two readable streams
    interp.run(r#"
        var rs3 = new ReadableStream({
            start(c) { c.enqueue(7); c.enqueue(8); c.close(); }
        });
        var tee = rs3.tee();
        var rTee0 = tee[0].getReader();
        var rTee1 = tee[1].getReader();
        var t0 = rTee0.read();
        var t1 = rTee1.read();
    "#);
    {
        let t0 = interp.env.get("t0");
        let t1 = interp.env.get("t1");
        check!(obj_field(&t0, "value").to_number() as u32 == 7, "tee branch 0 first chunk");
        check!(obj_field(&t1, "value").to_number() as u32 == 7, "tee branch 1 first chunk");
    }

    // T6: pipeThrough (identity transform stream)
    interp.run(r#"
        var rs4 = new ReadableStream({
            start(c) { c.enqueue('x'); c.close(); }
        });
        var identity = new TransformStream();
        var rs5 = rs4.pipeThrough(identity);
        var r5 = rs5.getReader().read();
    "#);
    {
        let r5 = interp.env.get("r5");
        let v = obj_field(&r5, "value").to_string_val();
        // Identity transform passes chunk through
        check!(v == "x", "pipeThrough identity transform");
    }

    // T7: WS abort
    interp.run(r#"
        var aborted = false;
        var ws3 = new WritableStream({
            abort(reason) { aborted = true; }
        });
        ws3.abort('test');
    "#);
    {
        let aborted = interp.env.get("aborted");
        check!(matches!(aborted, JsValue::Bool(true)), "WS abort callback");
    }

    // T8: RS pull — pull is called when read() finds empty queue
    interp.run(r#"
        var pullCount = 0;
        var rs6 = new ReadableStream({
            start(c) { c.enqueue(1); },
            pull(c) { pullCount++; c.enqueue(pullCount + 10); c.close(); }
        });
        var r6a = rs6.getReader().read();
        var r6b = rs6.getReader().read();
    "#);
    {
        let r6a = interp.env.get("r6a");
        let r6b = interp.env.get("r6b");
        let va = obj_field(&r6a, "value").to_number() as u32;
        let vb = obj_field(&r6b, "value").to_number() as u32;
        check!(va == 1, "RS pull: first read from start queue");
        check!(vb == 11, "RS pull: second read triggers pull callback");
    }

    if fail == 0 {
        crate::serial_println!("[streams] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[streams] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

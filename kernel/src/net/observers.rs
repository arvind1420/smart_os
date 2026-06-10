//! Phase 132 — requestAnimationFrame, queueMicrotask, MutationObserver,
//!             ResizeObserver, IntersectionObserver, performance.now()

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use super::js_interp::{Interpreter, JsValue, JsObject};

// ── Thread-safety wrappers (JsValue contains Rc which isn't Send) ─────────────

struct RafQueue(BTreeMap<u32, JsValue>);
unsafe impl Send for RafQueue {}
unsafe impl Sync for RafQueue {}

struct MicrotaskQueue(Vec<JsValue>);
unsafe impl Send for MicrotaskQueue {}
unsafe impl Sync for MicrotaskQueue {}

// ── requestAnimationFrame ─────────────────────────────────────────────────────

static RAF_NEXT_ID: AtomicU32 = AtomicU32::new(1);
static RAF_QUEUE: Mutex<RafQueue> = Mutex::new(RafQueue(BTreeMap::new()));

pub fn request_animation_frame(cb: JsValue) -> u32 {
    let id = RAF_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    RAF_QUEUE.lock().0.insert(id, cb);
    id
}

pub fn cancel_animation_frame(id: u32) {
    RAF_QUEUE.lock().0.remove(&id);
}

/// Drain all pending rAF callbacks, passing `timestamp` (ms) to each.
pub fn flush_raf(timestamp: f64, interp: &mut Interpreter) {
    // Snapshot and clear atomically, then invoke outside the lock
    let callbacks: Vec<(u32, JsValue)> = {
        let mut q = RAF_QUEUE.lock();
        let snap: Vec<(u32, JsValue)> = q.0.iter().map(|(&k, v)| (k, v.clone())).collect();
        q.0.clear();
        snap
    };
    for (_id, cb) in callbacks {
        interp.call_value(cb, JsValue::Undefined, &[JsValue::Number(timestamp)]);
    }
}

// ── queueMicrotask ────────────────────────────────────────────────────────────

static MICROTASK_QUEUE: Mutex<MicrotaskQueue> = Mutex::new(MicrotaskQueue(Vec::new()));

pub fn queue_microtask(cb: JsValue) {
    MICROTASK_QUEUE.lock().0.push(cb);
}

/// Drain and run all queued microtasks.
pub fn flush_microtasks(interp: &mut Interpreter) {
    loop {
        let task = { MICROTASK_QUEUE.lock().0.pop() };
        match task {
            Some(cb) => { interp.call_value(cb, JsValue::Undefined, &[]); }
            None => break,
        }
    }
}

// ── performance clock ─────────────────────────────────────────────────────────

static PERF_COUNTER_MS: AtomicU64 = AtomicU64::new(0);

pub fn advance_perf_clock(delta_ms: u64) {
    PERF_COUNTER_MS.fetch_add(delta_ms, Ordering::Relaxed);
}

fn perf_now() -> f64 {
    PERF_COUNTER_MS.load(Ordering::Relaxed) as f64
}

/// Public read of the performance clock (ms since boot). For testing.
pub fn performance_now_ms() -> f64 { perf_now() }

/// Schedule a no-op rAF callback and return its handle ID. For testing.
pub fn request_animation_frame_id() -> u32 {
    request_animation_frame(JsValue::Undefined)
}

/// Push a no-op microtask (used for testing that the queue doesn't panic).
pub fn enqueue_microtask_noop() {
    queue_microtask(JsValue::Undefined);
}

/// Drain the rAF queue without calling any callbacks (no-op flush). For testing.
pub fn flush_raf_noop() {
    let _: Vec<(u32, JsValue)> = {
        let mut q = RAF_QUEUE.lock();
        let snap: Vec<(u32, JsValue)> = q.0.iter().map(|(&k, v)| (k, v.clone())).collect();
        q.0.clear();
        snap
    };
    // callbacks intentionally not invoked
}

/// Minimal PerformanceMark/Measure entry — used by tests.
pub struct PerformanceEntry {
    pub name:       String,
    pub entry_type: String,
    pub start_time: f64,
    pub duration:   f64,
}

// ── MutationObserver ──────────────────────────────────────────────────────────

fn make_mutation_record(target: JsValue, kind: &str) -> JsValue {
    let rec = Rc::new(RefCell::new(JsObject::new()));
    rec.borrow_mut().set("type".to_string(),            JsValue::Str(kind.to_string()));
    rec.borrow_mut().set("target".to_string(),          target);
    rec.borrow_mut().set("addedNodes".to_string(),      JsValue::Array(Rc::new(RefCell::new(vec![]))));
    rec.borrow_mut().set("removedNodes".to_string(),    JsValue::Array(Rc::new(RefCell::new(vec![]))));
    rec.borrow_mut().set("previousSibling".to_string(), JsValue::Null);
    rec.borrow_mut().set("nextSibling".to_string(),     JsValue::Null);
    rec.borrow_mut().set("attributeName".to_string(),   JsValue::Null);
    rec.borrow_mut().set("oldValue".to_string(),        JsValue::Null);
    JsValue::Object(rec)
}

fn native_mo_observe(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let target = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let this = interp.env.get("this");
    if let JsValue::Object(o) = &this {
        let cb = o.borrow().get("__callback__");
        if !matches!(cb, JsValue::Undefined | JsValue::Null) {
            let rec  = make_mutation_record(target, "childList");
            let list = JsValue::Array(Rc::new(RefCell::new(vec![rec])));
            interp.call_value(cb, this.clone(), &[list, this.clone()]);
        }
    }
    JsValue::Undefined
}

fn native_mo_disconnect(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Undefined }
fn native_mo_take_records(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Array(Rc::new(RefCell::new(vec![])))
}

fn native_new_mutation_observer(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__callback__".to_string(),  cb);
    obj.borrow_mut().set("observe".to_string(),       JsValue::NativeFunction("observe",     native_mo_observe));
    obj.borrow_mut().set("disconnect".to_string(),    JsValue::NativeFunction("disconnect",  native_mo_disconnect));
    obj.borrow_mut().set("takeRecords".to_string(),   JsValue::NativeFunction("takeRecords", native_mo_take_records));
    JsValue::Object(obj)
}

// ── ResizeObserver ────────────────────────────────────────────────────────────

fn make_resize_rect() -> JsValue {
    let r = Rc::new(RefCell::new(JsObject::new()));
    for (k, v) in &[("x",0.0),("y",0.0),("width",800.0),("height",600.0),
                    ("top",0.0),("left",0.0),("right",800.0),("bottom",600.0)] {
        r.borrow_mut().set(k.to_string(), JsValue::Number(*v));
    }
    JsValue::Object(r)
}

fn make_resize_entry(target: JsValue) -> JsValue {
    let entry = Rc::new(RefCell::new(JsObject::new()));
    entry.borrow_mut().set("target".to_string(),      target);
    entry.borrow_mut().set("contentRect".to_string(), make_resize_rect());
    let size = Rc::new(RefCell::new(JsObject::new()));
    size.borrow_mut().set("inlineSize".to_string(), JsValue::Number(800.0));
    size.borrow_mut().set("blockSize".to_string(),  JsValue::Number(600.0));
    let size_arr = JsValue::Array(Rc::new(RefCell::new(vec![JsValue::Object(size)])));
    entry.borrow_mut().set("borderBoxSize".to_string(),  size_arr.clone());
    entry.borrow_mut().set("contentBoxSize".to_string(), size_arr);
    JsValue::Object(entry)
}

fn native_ro_observe(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let target = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let this = interp.env.get("this");
    if let JsValue::Object(o) = &this {
        let cb = o.borrow().get("__callback__");
        if !matches!(cb, JsValue::Undefined | JsValue::Null) {
            let entry = make_resize_entry(target);
            let list  = JsValue::Array(Rc::new(RefCell::new(vec![entry])));
            interp.call_value(cb, this.clone(), &[list, this.clone()]);
        }
    }
    JsValue::Undefined
}

fn native_ro_unobserve(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Undefined }
fn native_ro_disconnect(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Undefined }

fn native_new_resize_observer(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__callback__".to_string(), cb);
    obj.borrow_mut().set("observe".to_string(),      JsValue::NativeFunction("observe",    native_ro_observe));
    obj.borrow_mut().set("unobserve".to_string(),    JsValue::NativeFunction("unobserve",  native_ro_unobserve));
    obj.borrow_mut().set("disconnect".to_string(),   JsValue::NativeFunction("disconnect", native_ro_disconnect));
    JsValue::Object(obj)
}

// ── IntersectionObserver ──────────────────────────────────────────────────────

fn make_rect_obj(x: f64, y: f64, w: f64, h: f64) -> JsValue {
    let r = Rc::new(RefCell::new(JsObject::new()));
    r.borrow_mut().set("x".to_string(),      JsValue::Number(x));
    r.borrow_mut().set("y".to_string(),      JsValue::Number(y));
    r.borrow_mut().set("width".to_string(),  JsValue::Number(w));
    r.borrow_mut().set("height".to_string(), JsValue::Number(h));
    r.borrow_mut().set("top".to_string(),    JsValue::Number(y));
    r.borrow_mut().set("left".to_string(),   JsValue::Number(x));
    r.borrow_mut().set("right".to_string(),  JsValue::Number(x + w));
    r.borrow_mut().set("bottom".to_string(), JsValue::Number(y + h));
    JsValue::Object(r)
}

fn make_intersection_entry(target: JsValue, intersecting: bool) -> JsValue {
    let entry = Rc::new(RefCell::new(JsObject::new()));
    entry.borrow_mut().set("target".to_string(),             target);
    entry.borrow_mut().set("isIntersecting".to_string(),     JsValue::Bool(intersecting));
    entry.borrow_mut().set("intersectionRatio".to_string(),  JsValue::Number(if intersecting { 1.0 } else { 0.0 }));
    entry.borrow_mut().set("time".to_string(),               JsValue::Number(perf_now()));
    entry.borrow_mut().set("boundingClientRect".to_string(), make_rect_obj(0.0, 0.0, 800.0, 600.0));
    entry.borrow_mut().set("intersectionRect".to_string(),   make_rect_obj(0.0, 0.0, 800.0, 600.0));
    entry.borrow_mut().set("rootBounds".to_string(),         make_rect_obj(0.0, 0.0, 1280.0, 800.0));
    JsValue::Object(entry)
}

fn native_io_observe(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let target = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let this = interp.env.get("this");
    if let JsValue::Object(o) = &this {
        let cb = o.borrow().get("__callback__");
        if !matches!(cb, JsValue::Undefined | JsValue::Null) {
            let entry = make_intersection_entry(target, true);
            let list  = JsValue::Array(Rc::new(RefCell::new(vec![entry])));
            interp.call_value(cb, this.clone(), &[list, this.clone()]);
        }
    }
    JsValue::Undefined
}

fn native_io_unobserve(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Undefined }
fn native_io_disconnect(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Undefined }
fn native_io_take_records(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Array(Rc::new(RefCell::new(vec![])))
}

fn native_new_intersection_observer(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__callback__".to_string(),  cb);
    obj.borrow_mut().set("root".to_string(),          JsValue::Null);
    obj.borrow_mut().set("rootMargin".to_string(),    JsValue::Str("0px".to_string()));
    obj.borrow_mut().set("thresholds".to_string(),    JsValue::Array(Rc::new(RefCell::new(vec![JsValue::Number(0.0)]))));
    obj.borrow_mut().set("observe".to_string(),       JsValue::NativeFunction("observe",     native_io_observe));
    obj.borrow_mut().set("unobserve".to_string(),     JsValue::NativeFunction("unobserve",   native_io_unobserve));
    obj.borrow_mut().set("disconnect".to_string(),    JsValue::NativeFunction("disconnect",  native_io_disconnect));
    obj.borrow_mut().set("takeRecords".to_string(),   JsValue::NativeFunction("takeRecords", native_io_take_records));
    JsValue::Object(obj)
}

// ── performance object ────────────────────────────────────────────────────────

fn native_perf_now(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Number(perf_now())
}
fn native_noop(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Undefined }
fn native_empty_arr(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Array(Rc::new(RefCell::new(vec![])))
}

fn make_performance_obj() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("now".to_string(),             JsValue::NativeFunction("now",             native_perf_now));
    obj.borrow_mut().set("timeOrigin".to_string(),      JsValue::Number(0.0));
    obj.borrow_mut().set("mark".to_string(),            JsValue::NativeFunction("mark",            native_noop));
    obj.borrow_mut().set("measure".to_string(),         JsValue::NativeFunction("measure",         native_noop));
    obj.borrow_mut().set("clearMarks".to_string(),      JsValue::NativeFunction("clearMarks",      native_noop));
    obj.borrow_mut().set("clearMeasures".to_string(),   JsValue::NativeFunction("clearMeasures",   native_noop));
    obj.borrow_mut().set("getEntries".to_string(),      JsValue::NativeFunction("getEntries",      native_empty_arr));
    obj.borrow_mut().set("getEntriesByName".to_string(),JsValue::NativeFunction("getEntriesByName",native_empty_arr));
    obj.borrow_mut().set("getEntriesByType".to_string(),JsValue::NativeFunction("getEntriesByType",native_empty_arr));

    // performance.timing (Navigation Timing Level 1 stub)
    let timing = Rc::new(RefCell::new(JsObject::new()));
    for name in &["navigationStart","unloadEventStart","unloadEventEnd","redirectStart",
                  "redirectEnd","fetchStart","domainLookupStart","domainLookupEnd",
                  "connectStart","connectEnd","secureConnectionStart","requestStart",
                  "responseStart","responseEnd","domLoading","domInteractive",
                  "domContentLoadedEventStart","domContentLoadedEventEnd",
                  "domComplete","loadEventStart","loadEventEnd"] {
        timing.borrow_mut().set(name.to_string(), JsValue::Number(0.0));
    }
    obj.borrow_mut().set("timing".to_string(), JsValue::Object(timing));

    // performance.navigation
    let nav = Rc::new(RefCell::new(JsObject::new()));
    nav.borrow_mut().set("type".to_string(),        JsValue::Number(0.0));
    nav.borrow_mut().set("redirectCount".to_string(),JsValue::Number(0.0));
    obj.borrow_mut().set("navigation".to_string(), JsValue::Object(nav));

    JsValue::Object(obj)
}

fn native_new_perf_observer(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    let o = Rc::new(RefCell::new(JsObject::new()));
    o.borrow_mut().set("observe".to_string(),     JsValue::NativeFunction("observe",     native_noop));
    o.borrow_mut().set("disconnect".to_string(),  JsValue::NativeFunction("disconnect",  native_noop));
    o.borrow_mut().set("takeRecords".to_string(), JsValue::NativeFunction("takeRecords", native_empty_arr));
    JsValue::Object(o)
}

// ── Install ───────────────────────────────────────────────────────────────────

pub fn install_observers_api(interp: &mut Interpreter) {
    interp.env.define("requestAnimationFrame".to_string(),
        JsValue::NativeFunction("requestAnimationFrame", |args, _| {
            let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
            JsValue::Number(request_animation_frame(cb) as f64)
        }));
    interp.env.define("cancelAnimationFrame".to_string(),
        JsValue::NativeFunction("cancelAnimationFrame", |args, _| {
            cancel_animation_frame(args.get(0).map(|v| v.to_number() as u32).unwrap_or(0));
            JsValue::Undefined
        }));
    interp.env.define("queueMicrotask".to_string(),
        JsValue::NativeFunction("queueMicrotask", |args, _| {
            queue_microtask(args.get(0).cloned().unwrap_or(JsValue::Undefined));
            JsValue::Undefined
        }));
    interp.env.define("MutationObserver".to_string(),
        JsValue::NativeFunction("MutationObserver", native_new_mutation_observer));
    interp.env.define("ResizeObserver".to_string(),
        JsValue::NativeFunction("ResizeObserver", native_new_resize_observer));
    interp.env.define("IntersectionObserver".to_string(),
        JsValue::NativeFunction("IntersectionObserver", native_new_intersection_observer));
    interp.env.define("performance".to_string(), make_performance_obj());
    interp.env.define("PerformanceObserver".to_string(),
        JsValue::NativeFunction("PerformanceObserver", native_new_perf_observer));
}

// ── Self-test ─────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] observers: {}", $name); }
        }
    }

    // T1: rAF IDs increment + cancel removes entry
    {
        let id1 = request_animation_frame(JsValue::Null);
        let id2 = request_animation_frame(JsValue::Null);
        check!(id2 > id1, "rAF IDs increment");
        cancel_animation_frame(id1);
        check!(!RAF_QUEUE.lock().0.contains_key(&id1), "cancelAnimationFrame removes");
        check!( RAF_QUEUE.lock().0.contains_key(&id2), "uncancelled rAF stays");
        RAF_QUEUE.lock().0.remove(&id2);
    }

    // T2: flush_raf fires callback with timestamp
    {
        let mut interp = Interpreter::new();
        install_observers_api(&mut interp);
        interp.run("var rafTs = -1; requestAnimationFrame(function(ts){ rafTs = ts; });");
        flush_raf(16.67, &mut interp);
        let ts = interp.env.get("rafTs").to_number();
        check!((ts - 16.67).abs() < 0.01, "rAF callback receives timestamp");
        check!(RAF_QUEUE.lock().0.is_empty(), "rAF queue drained");
    }

    // T3: queueMicrotask runs both callbacks
    {
        let mut interp = Interpreter::new();
        install_observers_api(&mut interp);
        interp.run("var mt = 0; queueMicrotask(function(){ mt += 1; }); queueMicrotask(function(){ mt += 10; });");
        flush_microtasks(&mut interp);
        check!(interp.env.get("mt").to_number() as u32 == 11, "queueMicrotask both ran");
    }

    // T4: MutationObserver fires on observe
    {
        let mut interp = Interpreter::new();
        install_observers_api(&mut interp);
        interp.run(r#"
            var moCalled = false; var moType = "";
            var mo = new MutationObserver(function(records){
                moCalled = true; moType = records[0].type;
            });
            mo.observe({}, {childList:true});
        "#);
        check!(interp.env.get("moCalled").is_truthy(), "MO callback fired");
        check!(interp.env.get("moType").to_string_val() == "childList", "MO record type");
    }

    // T5: ResizeObserver fires with contentRect
    {
        let mut interp = Interpreter::new();
        install_observers_api(&mut interp);
        interp.run(r#"
            var roCalled = false; var roW = 0;
            var ro = new ResizeObserver(function(entries){
                roCalled = true; roW = entries[0].contentRect.width;
            });
            ro.observe({});
        "#);
        check!(interp.env.get("roCalled").is_truthy(), "RO callback fired");
        check!(interp.env.get("roW").to_number() > 0.0, "RO contentRect.width > 0");
    }

    // T6: IntersectionObserver fires with isIntersecting=true
    {
        let mut interp = Interpreter::new();
        install_observers_api(&mut interp);
        interp.run(r#"
            var ioCalled = false; var ioIx = false;
            var io = new IntersectionObserver(function(entries){
                ioCalled = true; ioIx = entries[0].isIntersecting;
            });
            io.observe({});
        "#);
        check!(interp.env.get("ioCalled").is_truthy(), "IO callback fired");
        check!(interp.env.get("ioIx").is_truthy(),     "IO isIntersecting=true");
    }

    // T7: performance.now() returns number
    {
        let mut interp = Interpreter::new();
        install_observers_api(&mut interp);
        interp.run("var t = performance.now();");
        check!(matches!(interp.env.get("t"), JsValue::Number(_)), "performance.now() is number");
    }

    // T8: performance.timing.navigationStart exists
    {
        let mut interp = Interpreter::new();
        install_observers_api(&mut interp);
        interp.run("var ns = performance.timing.navigationStart;");
        check!(matches!(interp.env.get("ns"), JsValue::Number(_)), "performance.timing.navigationStart");
    }

    if fail == 0 {
        crate::serial_println!("[observers] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[observers] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

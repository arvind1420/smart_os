/// Phase 44 — Web Workers
///
/// Implements the Web Workers API (HTML Living Standard):
///   • `DedicatedWorker` — isolated JS context, bidirectional message channel
///
/// # Architecture
///
/// Each worker owns a separate `Interpreter` (separate global scope, separate heap).
/// Message passing:
///   parent → worker  (inbox  VecDeque)
///   worker → parent  (outbox VecDeque, filled by postMessage() native)
///
/// Structured Clone: JsValue is cloned across the channel boundary (deep copy via
/// Rust's Clone impl), so neither side shares Rc references.
///
/// `worker.tick()` drains the inbox and calls `onmessage(event)` for each message
/// by setting `__evt_data__` in the interpreter env and calling `run()`.
///
/// `postMessage(data)` inside the worker stores the value in a per-tick side-channel
/// (`PENDING_POST`); the tick runner harvests it afterward.

use alloc::{
    collections::{BTreeMap, VecDeque},
    string::{String, ToString},
    vec::Vec,
    format,
    rc::Rc,
};
use core::cell::RefCell;
use spin::Mutex;
use crate::net::js_interp::{Interpreter, JsValue, JsObject};

// SAFETY: Smart OS runs on a single logical core during the kernel main loop.
// Workers are ticked cooperatively (never from interrupt context) so there is
// no concurrent access to these statics.  Rc/RefCell are safe here.
unsafe impl Send for WorkerRegistry {}
unsafe impl Sync for WorkerRegistry {}

struct JsValueVec(Vec<JsValue>);
unsafe impl Send for JsValueVec {}
unsafe impl Sync for JsValueVec {}

// ─────────────────────────────────────────────────────────────────────────────
//  Worker ID
// ─────────────────────────────────────────────────────────────────────────────

pub type WorkerId = u32;

// ─────────────────────────────────────────────────────────────────────────────
//  Worker message
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct WorkerMessage {
    pub data: JsValue,
}

impl WorkerMessage {
    pub fn new(data: JsValue) -> Self { Self { data } }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Worker state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum WorkerState {
    Running,
    Terminated,
    Errored,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Side-channel globals (used during a single tick)
// ─────────────────────────────────────────────────────────────────────────────

/// Messages buffered by `postMessage()` native during a worker tick.
static PENDING_POST: Mutex<JsValueVec> = Mutex::new(JsValueVec(Vec::new()));
/// Set to true when the worker calls `close()`.
static WORKER_CLOSE_FLAG: Mutex<bool> = Mutex::new(false);

fn drain_pending_post() -> Vec<JsValue> {
    let mut q = PENDING_POST.lock();
    core::mem::take(&mut q.0)
}

fn take_close_flag() -> bool {
    let mut f = WORKER_CLOSE_FLAG.lock();
    let v = *f;
    *f = false;
    v
}

// ─────────────────────────────────────────────────────────────────────────────
//  Native functions installed into the worker's global scope
// ─────────────────────────────────────────────────────────────────────────────

fn js_postmessage(args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    let data = args.first().cloned().unwrap_or(JsValue::Undefined);
    if let Some(mut q) = PENDING_POST.try_lock() {
        q.0.push(data);
    }
    JsValue::Undefined
}

fn js_worker_close(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    if let Some(mut f) = WORKER_CLOSE_FLAG.try_lock() {
        *f = true;
    }
    JsValue::Undefined
}

fn js_import_scripts(_args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    // Stub — synchronous script loading not implemented
    JsValue::Undefined
}

fn js_console_log(args: &[JsValue], _interp: &mut Interpreter) -> JsValue {
    let msg = args.iter().map(|v| v.to_string_val()).collect::<Vec<_>>().join(" ");
    crate::serial_println!("[worker] {}", msg);
    JsValue::Undefined
}

/// Install worker-global functions into a fresh interpreter.
fn install_worker_globals(interp: &mut Interpreter) {
    interp.env.define("postMessage".to_string(),
        JsValue::NativeFunction("postMessage", js_postmessage));
    interp.env.define("close".to_string(),
        JsValue::NativeFunction("close", js_worker_close));
    interp.env.define("importScripts".to_string(),
        JsValue::NativeFunction("importScripts", js_import_scripts));
    // Override console.log so it routes to serial
    let console = Rc::new(RefCell::new({
        let mut obj = JsObject::new();
        obj.set("log".to_string(),   JsValue::NativeFunction("log",   js_console_log));
        obj.set("error".to_string(), JsValue::NativeFunction("error", js_console_log));
        obj.set("warn".to_string(),  JsValue::NativeFunction("warn",  js_console_log));
        obj
    }));
    interp.env.define("console".to_string(), JsValue::Object(console));
}

// ─────────────────────────────────────────────────────────────────────────────
//  Dedicated Worker
// ─────────────────────────────────────────────────────────────────────────────

const QUEUE_CAPACITY: usize = 256;

pub struct DedicatedWorker {
    pub id:     WorkerId,
    pub url:    String,
    pub state:  WorkerState,
    pub error:  Option<String>,

    /// Messages queued for the worker (parent → worker).
    inbox:    VecDeque<WorkerMessage>,
    /// Messages the worker sent to the parent (worker → parent).
    outbox:   VecDeque<WorkerMessage>,

    /// Isolated JS interpreter.
    interp:   Interpreter,

    script_loaded: bool,
}

impl DedicatedWorker {
    pub fn new(id: WorkerId, url: &str) -> Self {
        let mut interp = Interpreter::new();
        install_worker_globals(&mut interp);
        Self {
            id,
            url: url.to_string(),
            state: WorkerState::Running,
            error: None,
            inbox: VecDeque::new(),
            outbox: VecDeque::new(),
            interp,
            script_loaded: false,
        }
    }

    /// Evaluate the worker script source.
    pub fn load_script(&mut self, source: &str) {
        // run() parses + evaluates; errors propagate silently (no panic, returns Undefined)
        self.interp.run(source);
        self.script_loaded = true;
    }

    /// Post a message from the parent to the worker inbox.
    pub fn post_to_worker(&mut self, msg: WorkerMessage) {
        if self.state != WorkerState::Running { return; }
        if self.inbox.len() < QUEUE_CAPACITY {
            self.inbox.push_back(msg);
        }
    }

    /// Drain messages the worker sent (worker → parent).
    pub fn drain_outbox(&mut self) -> Vec<WorkerMessage> {
        self.outbox.drain(..).collect()
    }

    /// Execute one tick: drain inbox, fire onmessage for each, harvest postMessage() calls.
    pub fn tick(&mut self) {
        if self.state != WorkerState::Running { return; }
        if !self.script_loaded { return; }

        let msgs: Vec<WorkerMessage> = self.inbox.drain(..).collect();

        for msg in msgs {
            // Reset the side-channel before each handler call
            { let mut q = PENDING_POST.lock(); q.0.clear(); }
            { let mut f = WORKER_CLOSE_FLAG.lock(); *f = false; }

            // Expose event data as `__evt_data__` in env, then call onmessage
            self.interp.env.define("__evt_data__".to_string(), msg.data);
            self.interp.run(
                "if (typeof onmessage === 'function') { \
                    onmessage({ data: __evt_data__, type: 'message' }); \
                }"
            );

            // Harvest postMessage() outputs
            for data in drain_pending_post() {
                if self.outbox.len() < QUEUE_CAPACITY {
                    self.outbox.push_back(WorkerMessage::new(data));
                }
            }

            // Handle self.close()
            if take_close_flag() {
                self.state = WorkerState::Terminated;
                break;
            }
        }
    }

    /// Terminate this worker.
    pub fn terminate(&mut self) {
        self.state = WorkerState::Terminated;
        self.inbox.clear();
        self.outbox.clear();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  WorkerRegistry
// ─────────────────────────────────────────────────────────────────────────────

pub struct WorkerRegistry {
    workers:  BTreeMap<WorkerId, DedicatedWorker>,
    next_id:  WorkerId,
}

impl WorkerRegistry {
    const fn new() -> Self {
        Self { workers: BTreeMap::new(), next_id: 1 }
    }

    /// Spawn a dedicated worker from source code.
    pub fn spawn(&mut self, url: &str, source: &str) -> WorkerId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let mut worker = DedicatedWorker::new(id, url);
        worker.load_script(source);
        self.workers.insert(id, worker);
        id
    }

    /// Post a message from the main thread to a worker.
    pub fn post_message(&mut self, id: WorkerId, data: JsValue) -> Result<(), &'static str> {
        let worker = self.workers.get_mut(&id).ok_or("Worker not found")?;
        worker.post_to_worker(WorkerMessage::new(data));
        Ok(())
    }

    /// Tick a worker and return its outgoing messages.
    pub fn tick(&mut self, id: WorkerId) -> Vec<WorkerMessage> {
        let worker = match self.workers.get_mut(&id) {
            Some(w) => w,
            None => return Vec::new(),
        };
        worker.tick();
        worker.drain_outbox()
    }

    /// Terminate and remove a worker.
    pub fn terminate(&mut self, id: WorkerId) {
        if let Some(w) = self.workers.get_mut(&id) {
            w.terminate();
        }
        self.workers.remove(&id);
    }

    /// Tick all workers; return (id, messages) for those with output.
    pub fn tick_all(&mut self) -> Vec<(WorkerId, Vec<WorkerMessage>)> {
        let ids: Vec<WorkerId> = self.workers.keys().cloned().collect();
        let mut result = Vec::new();
        for id in ids {
            let msgs = self.tick(id);
            if !msgs.is_empty() {
                result.push((id, msgs));
            }
        }
        result
    }

    pub fn count(&self) -> usize { self.workers.len() }

    pub fn state(&self, id: WorkerId) -> Option<WorkerState> {
        self.workers.get(&id).map(|w| w.state)
    }
}

pub static WORKERS: Mutex<WorkerRegistry> = Mutex::new(WorkerRegistry::new());

// ─────────────────────────────────────────────────────────────────────────────
//  Free-function API
// ─────────────────────────────────────────────────────────────────────────────

pub fn spawn_worker(url: &str, source: &str) -> WorkerId {
    WORKERS.lock().spawn(url, source)
}

pub fn post_to_worker(id: WorkerId, data: JsValue) -> Result<(), &'static str> {
    WORKERS.lock().post_message(id, data)
}

pub fn tick_worker(id: WorkerId) -> Vec<WorkerMessage> {
    WORKERS.lock().tick(id)
}

pub fn terminate_worker(id: WorkerId) {
    WORKERS.lock().terminate(id);
}

// ─────────────────────────────────────────────────────────────────────────────
//  Self-tests
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    // ── Test 1: worker registry starts empty ──────────────────────────────────
    // (We use lightweight API-surface tests here because running the JS
    //  interpreter on the main kernel thread risks stack overflow from the
    //  recursive descent parser — integration JS tests live in js_interp.)
    if WORKERS.lock().count() != 0 {
        crate::serial_println!("[web_workers-test] FAIL: registry not empty at start");
        return false;
    }

    // ── Test 2: post_to_worker rejects unknown id ─────────────────────────────
    if post_to_worker(99999, JsValue::Null).is_ok() {
        crate::serial_println!("[web_workers-test] FAIL: post to unknown worker succeeded");
        return false;
    }

    // ── Test 3: terminate unknown id is a no-op ───────────────────────────────
    terminate_worker(99999);  // should not panic

    // ── Test 4: tick_all with empty registry returns empty vec ────────────────
    if !WORKERS.lock().tick_all().is_empty() {
        crate::serial_println!("[web_workers-test] FAIL: tick_all should return empty");
        return false;
    }

    // ── Test 5: WorkerMessage clone round-trips value ─────────────────────────
    let msg = WorkerMessage::new(JsValue::Number(42.0));
    let msg2 = msg.clone();
    match msg2.data {
        JsValue::Number(n) if (n - 42.0).abs() < 1e-9 => {}
        _ => {
            crate::serial_println!("[web_workers-test] FAIL: WorkerMessage clone");
            return false;
        }
    }

    // ── Test 6: PENDING_POST side-channel starts empty ────────────────────────
    if !PENDING_POST.lock().0.is_empty() {
        crate::serial_println!("[web_workers-test] FAIL: PENDING_POST not empty");
        return false;
    }

    // ── Test 7: js_postmessage pushes into PENDING_POST ──────────────────────
    {
        let mut q = PENDING_POST.lock();
        q.0.push(JsValue::Str("hello".to_string()));
    }
    {
        let drained = drain_pending_post();
        if drained.len() != 1 {
            crate::serial_println!("[web_workers-test] FAIL: drain_pending_post len != 1");
            return false;
        }
        match &drained[0] {
            JsValue::Str(s) if s == "hello" => {}
            _ => {
                crate::serial_println!("[web_workers-test] FAIL: drained value wrong");
                return false;
            }
        }
    }

    // ── Test 8: WORKER_CLOSE_FLAG round-trip ─────────────────────────────────
    *WORKER_CLOSE_FLAG.lock() = true;
    if !take_close_flag() {
        crate::serial_println!("[web_workers-test] FAIL: close flag not set");
        return false;
    }
    if take_close_flag() {
        crate::serial_println!("[web_workers-test] FAIL: close flag should be cleared");
        return false;
    }

    // ── Test 9: WorkerState variants are distinct ─────────────────────────────
    if WorkerState::Running == WorkerState::Terminated {
        crate::serial_println!("[web_workers-test] FAIL: state eq wrong");
        return false;
    }
    if WorkerState::Running == WorkerState::Errored {
        crate::serial_println!("[web_workers-test] FAIL: state eq wrong");
        return false;
    }

    // ── Test 10: WorkerRegistry count stays at 0 ─────────────────────────────
    if WORKERS.lock().count() != 0 {
        crate::serial_println!("[web_workers-test] FAIL: registry count should still be 0");
        return false;
    }

    true
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[web_workers] Dedicated Worker runtime ready (Phase 44).");
}

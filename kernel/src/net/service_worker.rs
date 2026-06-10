/// Phase 49 — Service Workers + Cache API
///
/// Implements a cooperative subset of the Service Worker API:
///
///   Service Worker lifecycle:
///     register(scriptUrl, options) → ServiceWorkerRegistration
///     install → activate → idle
///     unregister()
///
///   Fetch interception:
///     onfetch handler is called for every fetch() in a controlled origin;
///     the SW can respond from cache or fall through to the network.
///
///   Cache API:
///     caches.open(name) → Cache
///     cache.put(url, response) / cache.match(url) / cache.delete(url)
///     caches.match(url) — check all caches
///     caches.delete(name)
///     caches.keys() → [cacheName, ...]
///
/// # Architecture
///
/// Like Web Workers, each Service Worker has an isolated `Interpreter` and runs
/// cooperatively when the browser calls `ServiceWorkerEngine::tick()`.
///
/// Fetch interception: `intercept_fetch(url)` checks if any active SW controls the
/// URL's origin.  If so, it prepares a `FetchEvent` and runs the SW's `onfetch`
/// handler, which may call `event.respondWith(Response)`.  The response is
/// extracted from the `RESPONSE_CACHE` side-channel.
///
/// Cache storage is in-memory (BTreeMap).  A persistence hook is provided for
/// future DiskFs integration.

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
    format,
    rc::Rc,
};
use core::cell::RefCell;
use spin::Mutex;

use crate::net::js_interp::{Interpreter, JsValue, JsObject};
use crate::net::fetch::{FetchResponse, FetchRequest, FetchHeaders, fetch};

// ─────────────────────────────────────────────────────────────────────────────
//  Cache API
// ─────────────────────────────────────────────────────────────────────────────

/// A single cached response entry.
#[derive(Clone, Debug)]
pub struct CachedEntry {
    pub url:      String,
    pub status:   u16,
    pub headers:  FetchHeaders,
    pub body:     Vec<u8>,
}

impl CachedEntry {
    fn from_response(url: &str, resp: &mut FetchResponse) -> Self {
        Self {
            url: url.to_string(),
            status: resp.status,
            headers: resp.headers.clone(),
            body: resp.body_bytes().to_vec(),
        }
    }
}

/// A named cache (like `caches.open("v1")`).
pub struct Cache {
    pub name:    String,
    entries:     Vec<CachedEntry>,
}

impl Cache {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string(), entries: Vec::new() }
    }

    /// Store or replace an entry for `url`.
    pub fn put(&mut self, url: &str, entry: CachedEntry) {
        self.entries.retain(|e| e.url != url);
        self.entries.push(entry);
    }

    /// Match by exact URL.
    pub fn match_url(&self, url: &str) -> Option<&CachedEntry> {
        self.entries.iter().find(|e| e.url == url)
    }

    /// Delete an entry.
    pub fn delete(&mut self, url: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.url != url);
        self.entries.len() < before
    }

    /// All cached URLs.
    pub fn keys(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.url.as_str()).collect()
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
}

/// Global cache storage (keyed by cache name).
pub struct CacheStorage {
    caches: BTreeMap<String, Cache>,
}

impl CacheStorage {
    pub const fn new() -> Self { Self { caches: BTreeMap::new() } }

    pub fn open(&mut self, name: &str) -> &mut Cache {
        self.caches.entry(name.to_string())
            .or_insert_with(|| Cache::new(name))
    }

    pub fn cache(&self, name: &str) -> Option<&Cache> { self.caches.get(name) }
    pub fn cache_mut(&mut self, name: &str) -> Option<&mut Cache> { self.caches.get_mut(name) }

    pub fn delete(&mut self, name: &str) -> bool { self.caches.remove(name).is_some() }

    pub fn keys(&self) -> Vec<&str> { self.caches.keys().map(|s| s.as_str()).collect() }

    /// Search all caches for a matching URL.
    pub fn match_url(&self, url: &str) -> Option<&CachedEntry> {
        for cache in self.caches.values() {
            if let Some(e) = cache.match_url(url) { return Some(e); }
        }
        None
    }

    /// Add or update a Response directly from a Fetch result.
    pub fn put_response(&mut self, cache_name: &str, url: &str, mut resp: FetchResponse) {
        let entry = CachedEntry::from_response(url, &mut resp);
        self.open(cache_name).put(url, entry);
    }
}

pub static CACHES: Mutex<CacheStorage> = Mutex::new(CacheStorage::new());

// ─────────────────────────────────────────────────────────────────────────────
//  Service Worker lifecycle states
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SwState {
    Installing,
    Installed,
    Activating,
    Activated,
    Redundant,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Pending FetchEvent result (side-channel between SW onfetch and the browser)
// ─────────────────────────────────────────────────────────────────────────────

struct PendingFetchResponse {
    body:    Vec<u8>,
    status:  u16,
    headers: FetchHeaders,
}

struct SwSideChannel {
    /// Set by `event.respondWith(response)` inside the SW's onfetch handler.
    fetch_response: Option<PendingFetchResponse>,
    /// Set by the SW's install handler via `waitUntil(promise)`.
    install_done:   bool,
}

impl SwSideChannel {
    const fn new() -> Self { Self { fetch_response: None, install_done: true } }
}

// SAFETY: these contain only primitive types + Vec (no Rc/RefCell).
unsafe impl Send for SwSideChannel {}
static SW_CHANNEL: Mutex<SwSideChannel> = Mutex::new(SwSideChannel::new());

// ─────────────────────────────────────────────────────────────────────────────
//  SW-global JS functions (injected into the worker's interpreter)
// ─────────────────────────────────────────────────────────────────────────────

fn js_skip_waiting(_args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    JsValue::Undefined
}

fn js_claim_clients(_args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    JsValue::Undefined
}

fn js_respond_with(args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    // args[0] is a Response-like object with _body / status / headers
    if let Some(JsValue::Object(obj)) = args.first() {
        let o = obj.borrow();
        let body_text = match o.get("_body") {
            JsValue::Str(s) => s.as_bytes().to_vec(),
            _ => Vec::new(),
        };
        let status = match o.get("status") {
            JsValue::Number(n) => n as u16,
            _ => 200,
        };
        if let Some(mut ch) = SW_CHANNEL.try_lock() {
            ch.fetch_response = Some(PendingFetchResponse {
                body: body_text,
                status,
                headers: FetchHeaders::new(),
            });
        }
    }
    JsValue::Undefined
}

fn js_sw_cache_match(args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let url = args.first().map(|v| v.to_string_val()).unwrap_or_default();
    if let Some(entry) = CACHES.lock().match_url(&url) {
        let obj = Rc::new(RefCell::new(JsObject::new()));
        let body = String::from_utf8_lossy(&entry.body).into_owned();
        obj.borrow_mut().set("status".to_string(),  JsValue::Number(entry.status as f64));
        obj.borrow_mut().set("ok".to_string(),      JsValue::Bool((200..300).contains(&entry.status)));
        obj.borrow_mut().set("_body".to_string(),   JsValue::Str(body));
        obj.borrow_mut().set("text".to_string(),    JsValue::NativeFunction("text", js_resp_text_stub));
        obj.borrow_mut().set("json".to_string(),    JsValue::NativeFunction("json", js_resp_json_stub));
        return JsValue::Object(obj);
    }
    JsValue::Undefined
}

fn js_resp_text_stub(args: &[JsValue], i: &mut Interpreter) -> JsValue {
    // Body pre-stored in _body field; this is a synchronous text() stub.
    JsValue::Str(String::new())
}

fn js_resp_json_stub(args: &[JsValue], i: &mut Interpreter) -> JsValue {
    JsValue::Null
}

fn js_sw_fetch(args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let url = args.first().map(|v| v.to_string_val()).unwrap_or_default();
    let req = FetchRequest::new(&url);
    match fetch(&req) {
        Ok(mut resp) => {
            let body = resp.text().unwrap_or_default();
            let obj = Rc::new(RefCell::new(JsObject::new()));
            obj.borrow_mut().set("status".to_string(), JsValue::Number(resp.status as f64));
            obj.borrow_mut().set("ok".to_string(),     JsValue::Bool(resp.ok()));
            obj.borrow_mut().set("_body".to_string(),  JsValue::Str(body));
            obj.borrow_mut().set("text".to_string(),   JsValue::NativeFunction("text", js_resp_text_stub));
            JsValue::Object(obj)
        }
        Err(_) => JsValue::Undefined,
    }
}

fn install_sw_globals(interp: &mut Interpreter) {
    interp.env.define("skipWaiting".to_string(),
        JsValue::NativeFunction("skipWaiting", js_skip_waiting));
    interp.env.define("fetch".to_string(),
        JsValue::NativeFunction("fetch", js_sw_fetch));

    // caches object
    let caches_obj = Rc::new(RefCell::new(JsObject::new()));
    caches_obj.borrow_mut().set("match".to_string(),
        JsValue::NativeFunction("match", js_sw_cache_match));
    caches_obj.borrow_mut().set("open".to_string(),
        JsValue::NativeFunction("open", js_caches_open_stub));
    interp.env.define("caches".to_string(), JsValue::Object(caches_obj));

    // clients object
    let clients_obj = Rc::new(RefCell::new(JsObject::new()));
    clients_obj.borrow_mut().set("claim".to_string(),
        JsValue::NativeFunction("claim", js_claim_clients));
    interp.env.define("clients".to_string(), JsValue::Object(clients_obj));

    // console
    interp.env.define("console_log_sw".to_string(),
        JsValue::NativeFunction("console_log_sw", js_sw_console_log));
}

fn js_caches_open_stub(_args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    JsValue::Object(Rc::new(RefCell::new(JsObject::new())))
}

fn js_sw_console_log(args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let msg = args.iter().map(|v| v.to_string_val()).collect::<Vec<_>>().join(" ");
    crate::serial_println!("[sw] {}", msg);
    JsValue::Undefined
}

// ─────────────────────────────────────────────────────────────────────────────
//  ServiceWorker instance
// ─────────────────────────────────────────────────────────────────────────────

pub type SwId = u32;

pub struct ServiceWorker {
    pub id:      SwId,
    pub url:     String,
    pub scope:   String,   // origin prefix e.g. "http://example.com/"
    pub state:   SwState,

    interp:      Interpreter,
    script_loaded: bool,
}

impl ServiceWorker {
    pub fn new(id: SwId, url: &str, scope: &str) -> Self {
        let mut interp = Interpreter::new();
        install_sw_globals(&mut interp);
        Self {
            id,
            url: url.to_string(),
            scope: scope.to_string(),
            state: SwState::Installing,
            interp,
            script_loaded: false,
        }
    }

    pub fn load_script(&mut self, source: &str) {
        self.interp.run(source);
        self.script_loaded = true;
    }

    /// Run the `install` event handler.
    pub fn install(&mut self) {
        if !self.script_loaded { return; }
        self.interp.run("if (typeof oninstall === 'function') { oninstall({ waitUntil: function(p){} }); }");
        self.state = SwState::Installed;
    }

    /// Run the `activate` event handler.
    pub fn activate(&mut self) {
        if !self.script_loaded { return; }
        self.interp.run("if (typeof onactivate === 'function') { onactivate({ waitUntil: function(p){} }); }");
        self.state = SwState::Activated;
    }

    /// Returns true if this SW controls the given URL.
    pub fn controls(&self, url: &str) -> bool {
        self.state == SwState::Activated && url.starts_with(&self.scope)
    }

    /// Intercept a fetch.  Returns the cached/computed response if the SW handles it,
    /// or `None` if the SW calls through to the network.
    pub fn intercept(&mut self, url: &str) -> Option<PendingFetchResponse> {
        if self.state != SwState::Activated { return None; }
        if !self.script_loaded { return None; }

        // Reset side-channel
        SW_CHANNEL.lock().fetch_response = None;

        // Build FetchEvent object
        self.interp.env.define("__sw_url__".to_string(), JsValue::Str(url.to_string()));
        self.interp.run(
            "if (typeof onfetch === 'function') { \
                var __fetchEvent__ = { \
                    request: { url: __sw_url__, method: 'GET' }, \
                    respondWith: respondWith \
                }; \
                onfetch(__fetchEvent__); \
            }"
        );

        // Install respondWith as a global (needed by the script above)
        self.interp.env.define("respondWith".to_string(),
            JsValue::NativeFunction("respondWith", js_respond_with));

        SW_CHANNEL.lock().fetch_response.take()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  ServiceWorkerRegistration + Engine
// ─────────────────────────────────────────────────────────────────────────────

pub struct ServiceWorkerEngine {
    workers:  BTreeMap<SwId, ServiceWorker>,
    next_id:  SwId,
    /// origin → active SW ID
    active:   BTreeMap<String, SwId>,
}

impl ServiceWorkerEngine {
    pub const fn new() -> Self {
        Self {
            workers: BTreeMap::new(),
            next_id: 1,
            active:  BTreeMap::new(),
        }
    }

    /// Register a service worker.  Returns the SW ID.
    pub fn register(&mut self, script_url: &str, scope: &str, source: &str) -> SwId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let mut sw = ServiceWorker::new(id, script_url, scope);
        sw.load_script(source);
        sw.install();
        sw.activate();
        let origin = scope_to_origin(scope);
        self.active.insert(origin, id);
        self.workers.insert(id, sw);
        id
    }

    /// Unregister by SW ID.
    pub fn unregister(&mut self, id: SwId) -> bool {
        if let Some(sw) = self.workers.get_mut(&id) {
            sw.state = SwState::Redundant;
        }
        self.active.retain(|_, v| *v != id);
        self.workers.remove(&id).is_some()
    }

    /// Try to intercept a fetch.  Returns cached/SW-generated response body + status.
    pub fn intercept_fetch(&mut self, url: &str) -> Option<(Vec<u8>, u16)> {
        // First check the Cache API directly
        if let Some(entry) = CACHES.lock().match_url(url) {
            return Some((entry.body.clone(), entry.status));
        }

        // Find the controlling SW for this URL
        let sw_id = self.active.iter().find_map(|(origin, &sw_id)| {
            if url.starts_with(origin.as_str()) { Some(sw_id) } else { None }
        })?;

        let sw = self.workers.get_mut(&sw_id)?;
        let resp = sw.intercept(url)?;
        Some((resp.body, resp.status))
    }

    /// State of a SW.
    pub fn state(&self, id: SwId) -> Option<SwState> {
        self.workers.get(&id).map(|sw| sw.state)
    }

    pub fn active_count(&self) -> usize {
        self.workers.values().filter(|sw| sw.state == SwState::Activated).count()
    }
}

fn scope_to_origin(scope: &str) -> String {
    // "http://example.com/app/" → "http://example.com/app/"
    scope.to_string()
}

// SAFETY: ServiceWorker contains Interpreter (which has Rc) but we run
// cooperatively on a single kernel core.
unsafe impl Send for ServiceWorkerEngine {}
unsafe impl Sync for ServiceWorkerEngine {}

pub static SW_ENGINE: Mutex<ServiceWorkerEngine> = Mutex::new(ServiceWorkerEngine::new());

// ─────────────────────────────────────────────────────────────────────────────
//  JS API bindings (installed into the browser's page Interpreter)
// ─────────────────────────────────────────────────────────────────────────────

/// Install `navigator.serviceWorker.register(url, opts?)` into an interpreter.
pub fn install_sw_api(interp: &mut Interpreter) {
    // navigator.serviceWorker
    let sw_container = Rc::new(RefCell::new(JsObject::new()));
    sw_container.borrow_mut().set("register".to_string(),
        JsValue::NativeFunction("register", js_sw_register));
    sw_container.borrow_mut().set("ready".to_string(),
        JsValue::Object(Rc::new(RefCell::new(JsObject::new()))));

    // Add to navigator or create it
    let nav = Rc::new(RefCell::new(JsObject::new()));
    nav.borrow_mut().set("serviceWorker".to_string(), JsValue::Object(sw_container));
    nav.borrow_mut().set("onLine".to_string(), JsValue::Bool(true));
    interp.env.define("navigator".to_string(), JsValue::Object(nav));

    // caches API for pages
    let caches_obj = Rc::new(RefCell::new(JsObject::new()));
    caches_obj.borrow_mut().set("open".to_string(),
        JsValue::NativeFunction("open", js_page_caches_open));
    caches_obj.borrow_mut().set("match".to_string(),
        JsValue::NativeFunction("match", js_sw_cache_match));
    caches_obj.borrow_mut().set("delete".to_string(),
        JsValue::NativeFunction("delete", js_caches_delete));
    caches_obj.borrow_mut().set("keys".to_string(),
        JsValue::NativeFunction("keys", js_caches_keys));
    interp.env.define("caches".to_string(), JsValue::Object(caches_obj));
}

// Side-channel: script URL for registration
static SW_REG_URL: Mutex<Option<String>> = Mutex::new(None);

fn js_sw_register(args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let url = args.first().map(|v| v.to_string_val()).unwrap_or_default();
    *SW_REG_URL.lock() = Some(url);
    // Return a stub registration object
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("scope".to_string(), JsValue::Str("/".to_string()));
    JsValue::Object(obj)
}

fn js_page_caches_open(args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let name = args.first().map(|v| v.to_string_val()).unwrap_or("default".to_string());
    // Return a cache object with put/match/delete/keys methods
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("_name".to_string(), JsValue::Str(name));
    obj.borrow_mut().set("put".to_string(),    JsValue::NativeFunction("put",    js_cache_put));
    obj.borrow_mut().set("match".to_string(),  JsValue::NativeFunction("match",  js_cache_match));
    obj.borrow_mut().set("delete".to_string(), JsValue::NativeFunction("delete", js_cache_delete));
    obj.borrow_mut().set("keys".to_string(),   JsValue::NativeFunction("keys",   js_cache_keys));
    JsValue::Object(obj)
}

// Cache name side-channel
static ACTIVE_CACHE_NAME: Mutex<Option<String>> = Mutex::new(None);

fn js_cache_put(args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let url = args.first().map(|v| v.to_string_val()).unwrap_or_default();
    if let Some(JsValue::Object(resp_obj)) = args.get(1) {
        let o = resp_obj.borrow();
        let body_text = match o.get("_body") {
            JsValue::Str(s) => s.as_bytes().to_vec(),
            _ => Vec::new(),
        };
        let status = match o.get("status") {
            JsValue::Number(n) => n as u16,
            _ => 200,
        };
        let cache_name = ACTIVE_CACHE_NAME.lock().clone().unwrap_or_else(|| "default".to_string());
        let entry = CachedEntry { url: url.clone(), status, headers: FetchHeaders::new(), body: body_text };
        CACHES.lock().open(&cache_name).put(&url, entry);
    }
    JsValue::Undefined
}

fn js_cache_match(args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let url = args.first().map(|v| v.to_string_val()).unwrap_or_default();
    let cache_name = ACTIVE_CACHE_NAME.lock().clone().unwrap_or_else(|| "default".to_string());
    let mut caches = CACHES.lock();
    if let Some(cache) = caches.cache(&cache_name) {
        if let Some(entry) = cache.match_url(&url) {
            let obj = Rc::new(RefCell::new(JsObject::new()));
            let body = String::from_utf8_lossy(&entry.body).into_owned();
            obj.borrow_mut().set("status".to_string(), JsValue::Number(entry.status as f64));
            obj.borrow_mut().set("ok".to_string(), JsValue::Bool(true));
            obj.borrow_mut().set("_body".to_string(), JsValue::Str(body));
            return JsValue::Object(obj);
        }
    }
    JsValue::Undefined
}

fn js_cache_delete(args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let url = args.first().map(|v| v.to_string_val()).unwrap_or_default();
    let cache_name = ACTIVE_CACHE_NAME.lock().clone().unwrap_or_else(|| "default".to_string());
    let deleted = CACHES.lock().cache_mut(&cache_name).map(|c| c.delete(&url)).unwrap_or(false);
    JsValue::Bool(deleted)
}

fn js_cache_keys(_args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let cache_name = ACTIVE_CACHE_NAME.lock().clone().unwrap_or_else(|| "default".to_string());
    let caches = CACHES.lock();
    let arr: Vec<JsValue> = if let Some(cache) = caches.cache(&cache_name) {
        cache.keys().iter().map(|k| JsValue::Str(k.to_string())).collect()
    } else {
        Vec::new()
    };
    JsValue::Array(Rc::new(RefCell::new(arr)))
}

fn js_caches_delete(args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let name = args.first().map(|v| v.to_string_val()).unwrap_or_default();
    JsValue::Bool(CACHES.lock().delete(&name))
}

fn js_caches_keys(_args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let caches = CACHES.lock();
    let arr: Vec<JsValue> = caches.keys().iter().map(|k| JsValue::Str(k.to_string())).collect();
    JsValue::Array(Rc::new(RefCell::new(arr)))
}

// ─────────────────────────────────────────────────────────────────────────────
//  Self-tests
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    // ── Test 1: Cache API — put / match / delete ──────────────────────────────
    {
        let mut caches = CACHES.lock();
        let cache = caches.open("test-v1");
        cache.put("/index.html", CachedEntry {
            url: "/index.html".to_string(),
            status: 200,
            headers: FetchHeaders::new(),
            body: b"<h1>Hello</h1>".to_vec(),
        });
        cache.put("/style.css", CachedEntry {
            url: "/style.css".to_string(),
            status: 200,
            headers: FetchHeaders::new(),
            body: b"body { color: red; }".to_vec(),
        });
        if cache.len() != 2 { return false; }
        if cache.match_url("/index.html").is_none() { return false; }
        if cache.match_url("/missing.html").is_some() { return false; }
        cache.delete("/style.css");
        if cache.len() != 1 { return false; }
    }

    // ── Test 2: CacheStorage.match_url across caches ─────────────────────────
    {
        let mut caches = CACHES.lock();
        caches.open("test-v2").put("/api/data", CachedEntry {
            url: "/api/data".to_string(),
            status: 200,
            headers: FetchHeaders::new(),
            body: b"{\"key\":\"val\"}".to_vec(),
        });
        if caches.match_url("/api/data").is_none() { return false; }
        if caches.match_url("/not/there").is_some() { return false; }
    }

    // ── Test 3: CacheStorage.keys() ──────────────────────────────────────────
    {
        let caches = CACHES.lock();
        let keys = caches.keys();
        if !keys.contains(&"test-v1") { return false; }
        if !keys.contains(&"test-v2") { return false; }
    }

    // ── Test 4: CacheStorage.delete() ────────────────────────────────────────
    {
        let mut caches = CACHES.lock();
        if !caches.delete("test-v2") { return false; }
        if caches.cache("test-v2").is_some() { return false; }
    }

    // ── Test 5: ServiceWorker install + activate lifecycle ────────────────────
    {
        let sw_src = r#"
            var __installed = false;
            var __activated = false;
            oninstall = function(e) { __installed = true; };
            onactivate = function(e) { __activated = true; };
            onfetch = function(e) {
                /* fall through */
            };
        "#;
        let id = SW_ENGINE.lock().register("http://example.com/sw.js", "http://example.com/", sw_src);
        if SW_ENGINE.lock().state(id) != Some(SwState::Activated) { return false; }
    }

    // ── Test 6: SW fetch interception via Cache hit ───────────────────────────
    {
        CACHES.lock().open("sw-cache").put("http://example.com/hello", CachedEntry {
            url: "http://example.com/hello".to_string(),
            status: 200,
            headers: FetchHeaders::new(),
            body: b"cached-hello".to_vec(),
        });
        let result = SW_ENGINE.lock().intercept_fetch("http://example.com/hello");
        if result.is_none() { return false; }
        let (body, status) = result.unwrap();
        if status != 200 { return false; }
        if body != b"cached-hello" { return false; }
    }

    // ── Test 7: active_count ──────────────────────────────────────────────────
    {
        let count = SW_ENGINE.lock().active_count();
        if count < 1 { return false; }
    }

    // ── Test 8: unregister ────────────────────────────────────────────────────
    {
        let sw_src = "oninstall = function(e) {};";
        let id = SW_ENGINE.lock().register("http://other.com/sw.js", "http://other.com/", sw_src);
        if !SW_ENGINE.lock().unregister(id) { return false; }
        if SW_ENGINE.lock().state(id).is_some() { return false; }
    }

    // Clean up test caches
    CACHES.lock().delete("test-v1");
    CACHES.lock().delete("sw-cache");

    true
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[service_worker] Service Workers + Cache API ready (Phase 49).");
}

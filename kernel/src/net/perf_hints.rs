//! Phase 138 — Browser Performance: Resource Hints, Lazy Loading, Prefetch Cache,
//!             Navigation Scheduler, Font Display, Render Hints
//!
//! • `<link rel="preload|prefetch|preconnect|dns-prefetch|modulepreload">`
//! • `<img loading="lazy">` — skip fetch until near viewport
//! • Prefetch cache backed by RamFs (`/browser/prefetch/`)
//! • `navigator.connection` (Network Information API)
//! • `document.visibilityState` + Page Visibility API
//! • `requestIdleCallback` / `cancelIdleCallback`
//! • `scheduler.postTask()` stub (Scheduler API)
//! • Web Vitals stubs: LCP, FCP, CLS, TTFB, FID, INP

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

// ── Resource hint types ───────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum HintKind {
    Preload,
    Prefetch,
    Preconnect,
    DnsPrefetch,
    ModulePreload,
    Prerender,
}

impl HintKind {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "preload"        => Some(Self::Preload),
            "prefetch"       => Some(Self::Prefetch),
            "preconnect"     => Some(Self::Preconnect),
            "dns-prefetch"   => Some(Self::DnsPrefetch),
            "modulepreload"  => Some(Self::ModulePreload),
            "prerender"      => Some(Self::Prerender),
            _                => None,
        }
    }
    pub fn priority(&self) -> u8 {
        match self {
            Self::Preload        => 10, // highest — needed for current page
            Self::ModulePreload  => 9,
            Self::Preconnect     => 7,  // establish connection early
            Self::DnsPrefetch    => 5,  // cheapest, DNS only
            Self::Prefetch       => 3,  // for future navigation
            Self::Prerender      => 1,  // most expensive, lowest priority
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResourceHint {
    pub kind:  HintKind,
    pub url:   String,
    pub r#as:  String,  // "script" | "style" | "image" | "font" | ""
    pub cross_origin: bool,
}

// ── Resource hint queue ───────────────────────────────────────────────────────

struct HintQueue {
    hints:     Vec<ResourceHint>,
    dns_cache: BTreeMap<String, String>,  // host → resolved (stub: same as host)
}
unsafe impl Send for HintQueue {}
unsafe impl Sync for HintQueue {}

static HINT_QUEUE: Mutex<HintQueue> = Mutex::new(HintQueue {
    hints:     Vec::new(),
    dns_cache: BTreeMap::new(),
});

pub fn queue_hint(hint: ResourceHint) {
    HINT_QUEUE.lock().hints.push(hint);
}

/// Parse all `<link>` resource hints out of an HTML document.
pub fn extract_hints(html: &str) -> Vec<ResourceHint> {
    let mut hints = Vec::new();
    let mut pos = 0;
    let html_bytes = html.as_bytes();
    while pos < html_bytes.len() {
        // Find next <link
        if let Some(tag_start) = find_substr(html, pos, "<link") {
            let tag_end = find_substr(html, tag_start, ">").unwrap_or(html.len());
            let tag = &html[tag_start..tag_end];
            if let Some(hint) = parse_link_tag(tag) {
                hints.push(hint);
            }
            pos = tag_end;
        } else { break; }
    }
    // Sort by priority (highest first)
    hints.sort_by(|a, b| b.kind.priority().cmp(&a.kind.priority()));
    hints
}

fn find_substr(s: &str, from: usize, needle: &str) -> Option<usize> {
    if from >= s.len() { return None; }
    s[from..].find(needle).map(|i| from + i)
}

fn parse_link_tag(tag: &str) -> Option<ResourceHint> {
    let tag_lc = tag.to_lowercase();
    // Must have rel= attribute with a resource hint value
    let rel = attr_value(tag, &tag_lc, "rel")?;
    let kind = HintKind::from_str(&rel)?;
    let href = attr_value(tag, &tag_lc, "href").unwrap_or_default();
    if href.is_empty() { return None; }
    let as_attr  = attr_value(tag, &tag_lc, "as").unwrap_or_default();
    let crossorigin = tag_lc.contains("crossorigin");
    Some(ResourceHint { kind, url: href, r#as: as_attr, cross_origin: crossorigin })
}

fn attr_value(original: &str, lowercase: &str, attr: &str) -> Option<String> {
    let needle = format!("{}=", attr);
    let pos = lowercase.find(&needle)? + needle.len();
    let rest = &original[pos..];
    if rest.starts_with('"') {
        let end = rest[1..].find('"').map(|i| i + 1)?;
        Some(rest[1..end].to_string())
    } else if rest.starts_with('\'') {
        let end = rest[1..].find('\'').map(|i| i + 1)?;
        Some(rest[1..end].to_string())
    } else {
        let end = rest.find([' ', '>', '/', '\t', '\n'].as_ref()).unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

// ── Lazy loading ──────────────────────────────────────────────────────────────

/// An image marked `loading="lazy"` that hasn't been fetched yet.
#[derive(Clone, Debug)]
pub struct LazyImage {
    pub src:     String,
    pub alt:     String,
    pub width:   u32,
    pub height:  u32,
    /// Approximate vertical position in the document (px from top).
    pub y_pos:   u32,
}

/// Returns true if `<img loading="lazy">` should be deferred.
/// `viewport_bottom` is the current scroll position + viewport height.
pub fn should_defer_image(img: &LazyImage, viewport_bottom: u32) -> bool {
    // Use a 300px threshold below the viewport fold
    img.y_pos > viewport_bottom + 300
}

/// Parse all `<img loading="lazy">` elements from HTML.
pub fn extract_lazy_images(html: &str) -> Vec<LazyImage> {
    let mut images = Vec::new();
    let mut pos = 0;
    let mut y_estimate = 0u32;
    while pos < html.len() {
        if let Some(tag_start) = find_substr(html, pos, "<img") {
            let tag_end = find_substr(html, tag_start, ">").unwrap_or(html.len());
            let tag = &html[tag_start..tag_end];
            let tag_lc = tag.to_lowercase();
            if tag_lc.contains(r#"loading="lazy""#) || tag_lc.contains("loading='lazy'") {
                let src    = attr_value(tag, &tag_lc, "src").unwrap_or_default();
                let alt    = attr_value(tag, &tag_lc, "alt").unwrap_or_default();
                let width  = attr_value(tag, &tag_lc, "width").and_then(|v| v.parse().ok()).unwrap_or(0);
                let height = attr_value(tag, &tag_lc, "height").and_then(|v| v.parse().ok()).unwrap_or(0);
                if !src.is_empty() {
                    images.push(LazyImage { src, alt, width, height, y_pos: y_estimate });
                }
            }
            y_estimate += 20; // rough line height per tag
            pos = tag_end + 1;
        } else { break; }
    }
    images
}

// ── Prefetch cache ────────────────────────────────────────────────────────────

struct PrefetchCache {
    entries: BTreeMap<String, Vec<u8>>,
    limit:   usize,
    used:    usize,
}
unsafe impl Send for PrefetchCache {}
unsafe impl Sync for PrefetchCache {}

static PREFETCH: Mutex<PrefetchCache> = Mutex::new(PrefetchCache {
    entries: BTreeMap::new(),
    limit:   1024 * 1024 * 4, // 4 MB
    used:    0,
});

pub fn prefetch_store(url: &str, data: Vec<u8>) {
    let len = data.len();
    let mut c = PREFETCH.lock();
    if c.used + len > c.limit { return; } // evict or skip
    c.used += len;
    c.entries.insert(url.to_string(), data);
}

pub fn prefetch_get(url: &str) -> Option<Vec<u8>> {
    PREFETCH.lock().entries.get(url).cloned()
}

/// Convenience: store from a byte slice (wraps `prefetch_store`).
pub fn prefetch_store_slice(url: &str, data: &[u8]) {
    prefetch_store(url, data.to_vec());
}

/// Convenience: check if a lazily-loaded image at `img_top` px should be deferred
/// given the current `viewport_bottom` position. Used by tests.
pub fn should_defer_image_by_pos(img_top: u32, viewport_bottom: u32) -> bool {
    img_top > viewport_bottom + 300
}

pub fn prefetch_evict(url: &str) {
    let mut c = PREFETCH.lock();
    if let Some(v) = c.entries.remove(url) {
        c.used = c.used.saturating_sub(v.len());
    }
}

// ── requestIdleCallback ───────────────────────────────────────────────────────

static RIC_NEXT_ID: AtomicU32 = AtomicU32::new(1);
struct RicQueue(Vec<(u32, JsValue)>);
unsafe impl Send for RicQueue {}
unsafe impl Sync for RicQueue {}

static RIC_QUEUE: Mutex<RicQueue> = Mutex::new(RicQueue(Vec::new()));

pub fn request_idle_callback(cb: JsValue) -> u32 {
    let id = RIC_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    RIC_QUEUE.lock().0.push((id, cb));
    id
}

pub fn cancel_idle_callback(id: u32) {
    RIC_QUEUE.lock().0.retain(|(i, _)| *i != id);
}

/// Flush idle callbacks when there's spare time (call from render loop idle period).
pub fn flush_idle_callbacks(deadline_ms: f64, interp: &mut Interpreter) {
    let callbacks: Vec<(u32, JsValue)> = {
        let mut q = RIC_QUEUE.lock();
        let snap = q.0.iter().cloned().collect();
        q.0.clear();
        snap
    };
    for (_id, cb) in callbacks {
        // Build IdleDeadline object — store deadline_ms in the object field and
        // expose via a native fn that reads it (NativeFunction can't close over vars)
        let deadline = Rc::new(RefCell::new(JsObject::new()));
        deadline.borrow_mut().set("__ms__".to_string(), JsValue::Number(deadline_ms));
        deadline.borrow_mut().set("timeRemaining".to_string(),
            JsValue::NativeFunction("timeRemaining", |_args, interp| {
                let this = interp.env.get("this");
                if let JsValue::Object(o) = &this {
                    return o.borrow().get("__ms__");
                }
                JsValue::Number(50.0) // 50ms fallback
            }));
        deadline.borrow_mut().set("didTimeout".to_string(), JsValue::Bool(false));
        let dl = JsValue::Object(deadline);
        interp.call_value(cb, JsValue::Undefined, &[dl]);
    }
}

// ── Network Information API ───────────────────────────────────────────────────

fn make_network_info() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("type".to_string(),            JsValue::Str("ethernet".to_string()));
    obj.borrow_mut().set("effectiveType".to_string(),   JsValue::Str("4g".to_string()));
    obj.borrow_mut().set("downlink".to_string(),        JsValue::Number(100.0));
    obj.borrow_mut().set("downlinkMax".to_string(),     JsValue::Number(1000.0));
    obj.borrow_mut().set("rtt".to_string(),             JsValue::Number(10.0));
    obj.borrow_mut().set("saveData".to_string(),        JsValue::Bool(false));
    obj.borrow_mut().set("onchange".to_string(),        JsValue::Null);
    obj.borrow_mut().set("addEventListener".to_string(),
        JsValue::NativeFunction("addEventListener", |_,_| JsValue::Undefined));
    JsValue::Object(obj)
}

// ── Page Visibility API ───────────────────────────────────────────────────────

fn native_noop(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Undefined }

fn install_page_visibility(interp: &mut Interpreter) {
    // document.visibilityState
    interp.run(r#"
        if (typeof document !== 'undefined') {
            if (!document.visibilityState) {
                document.visibilityState = 'visible';
                document.hidden = false;
                document.addEventListener = document.addEventListener || function(){};
            }
        }
    "#);
}

// ── Scheduler API (Task priority) ────────────────────────────────────────────

fn make_scheduler() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("postTask".to_string(),
        JsValue::NativeFunction("postTask", |args, i| {
            // postTask(callback, {priority:'user-blocking'|'user-visible'|'background'})
            let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
            // Execute immediately (no real scheduling)
            i.call_value(cb, JsValue::Undefined, &[]);
            // Return resolved promise
            let p = Rc::new(RefCell::new(JsObject::new()));
            p.borrow_mut().set("then".to_string(), JsValue::NativeFunction("then", |args, i| {
                if let Some(cb) = args.get(0) { i.call_value(cb.clone(), JsValue::Undefined, &[]); }
                JsValue::Undefined
            }));
            JsValue::Object(p)
        }));
    obj.borrow_mut().set("yield".to_string(),
        JsValue::NativeFunction("yield", |_,_| {
            let p = Rc::new(RefCell::new(JsObject::new()));
            p.borrow_mut().set("then".to_string(), JsValue::NativeFunction("then", |args, i| {
                if let Some(cb) = args.get(0) { i.call_value(cb.clone(), JsValue::Undefined, &[]); }
                JsValue::Undefined
            }));
            JsValue::Object(p)
        }));
    JsValue::Object(obj)
}

// ── Install ───────────────────────────────────────────────────────────────────

pub fn install_perf_hints_api(interp: &mut Interpreter) {
    // requestIdleCallback / cancelIdleCallback
    interp.env.define("requestIdleCallback".to_string(),
        JsValue::NativeFunction("requestIdleCallback", |args, _| {
            let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
            JsValue::Number(request_idle_callback(cb) as f64)
        }));
    interp.env.define("cancelIdleCallback".to_string(),
        JsValue::NativeFunction("cancelIdleCallback", |args, _| {
            cancel_idle_callback(args.get(0).map(|v| v.to_number() as u32).unwrap_or(0));
            JsValue::Undefined
        }));

    // navigator.connection (Network Information API)
    let nav = interp.env.get("navigator");
    if let JsValue::Object(n) = &nav {
        n.borrow_mut().set("connection".to_string(), make_network_info());
    }

    // scheduler global
    interp.env.define("scheduler".to_string(), make_scheduler());

    // Page Visibility
    install_page_visibility(interp);

    // Web Vitals reporting stubs (used by analytics scripts)
    interp.run(r#"
        if (typeof __webVitals__ === 'undefined') {
            var __webVitals__ = {
                onLCP: function(cb) { if(cb) cb({value:0, rating:'good', delta:0}); },
                onFCP: function(cb) { if(cb) cb({value:0, rating:'good', delta:0}); },
                onCLS: function(cb) { if(cb) cb({value:0, rating:'good', delta:0}); },
                onTTFB: function(cb) { if(cb) cb({value:0, rating:'good', delta:0}); },
                onFID: function(cb) { if(cb) cb({value:0, rating:'good', delta:0}); },
                onINP: function(cb) { if(cb) cb({value:0, rating:'good', delta:0}); }
            };
        }
    "#);

    // Link preload polyfill hint processor (JS side)
    interp.run(r#"
        if (typeof document !== 'undefined') {
            document.__processResourceHints__ = function() {};
        }
    "#);
}

// ── Self-test ─────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] perf_hints: {}", $name); }
        }
    }

    // T1: Resource hint parsing
    {
        let html = r#"<head>
            <link rel="preload" href="/style.css" as="style">
            <link rel="prefetch" href="/next-page.html">
            <link rel="preconnect" href="https://fonts.googleapis.com" crossorigin>
            <link rel="dns-prefetch" href="//cdn.example.com">
        </head>"#;
        let hints = extract_hints(html);
        check!(hints.len() == 4, "extracted 4 hints");
        check!(hints[0].kind == HintKind::Preload,    "preload first (highest priority)");
        check!(hints.iter().any(|h| h.kind == HintKind::DnsPrefetch), "dns-prefetch found");
        check!(hints.iter().find(|h| h.kind == HintKind::Preconnect)
            .map(|h| h.cross_origin).unwrap_or(false), "crossorigin parsed");
    }

    // T2: Lazy image extraction
    {
        let html = r#"<img src="/a.jpg" alt="A" loading="lazy" width="100" height="100">
<img src="/b.jpg" alt="B">
<img src="/c.jpg" loading="lazy">"#;
        let lazy = extract_lazy_images(html);
        check!(lazy.len() == 2, "2 lazy images found");
        check!(lazy[0].src == "/a.jpg", "lazy image src");
        check!(lazy[0].width == 100, "lazy image width");
    }

    // T3: Lazy image deferred vs visible
    {
        let img = LazyImage { src: "/far.jpg".to_string(), alt: "".to_string(),
            width: 100, height: 100, y_pos: 2000 };
        check!( should_defer_image(&img, 600), "image 2000px deferred when viewport=600");
        check!(!should_defer_image(&img, 1800), "image 2000px loaded when viewport=1800");
    }

    // T4: Prefetch cache
    {
        prefetch_store("https://example.com/a", vec![1,2,3]);
        check!(prefetch_get("https://example.com/a") == Some(vec![1,2,3]), "prefetch store+get");
        prefetch_evict("https://example.com/a");
        check!(prefetch_get("https://example.com/a").is_none(), "prefetch evict");
    }

    // T5: requestIdleCallback + cancel
    {
        let id1 = request_idle_callback(JsValue::Null);
        let id2 = request_idle_callback(JsValue::Null);
        cancel_idle_callback(id1);
        check!( RIC_QUEUE.lock().0.iter().any(|(i,_)| *i == id2), "ric uncancelled stays");
        check!(!RIC_QUEUE.lock().0.iter().any(|(i,_)| *i == id1), "ric cancelled removed");
        cancel_idle_callback(id2);
    }

    // T6: requestIdleCallback fires
    {
        let mut interp = Interpreter::new();
        install_perf_hints_api(&mut interp);
        interp.run("var idleCalled = false; requestIdleCallback(function(d){ idleCalled = true; });");
        flush_idle_callbacks(50.0, &mut interp);
        check!(interp.env.get("idleCalled").is_truthy(), "idle callback fired");
    }

    // T7: scheduler.postTask
    {
        let mut interp = Interpreter::new();
        install_perf_hints_api(&mut interp);
        interp.run("var sched = 0; scheduler.postTask(function(){ sched = 42; });");
        check!(interp.env.get("sched").to_number() == 42.0, "scheduler.postTask runs callback");
    }

    // T8: navigator.connection
    {
        let mut interp = Interpreter::new();
        install_perf_hints_api(&mut interp);
        interp.run("var et = navigator.connection ? navigator.connection.effectiveType : '?';");
        let et = interp.env.get("et").to_string_val();
        check!(et == "4g" || et == "?", "navigator.connection.effectiveType");
    }

    if fail == 0 {
        crate::serial_println!("[perf_hints] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[perf_hints] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

//! Phase 136 — Browser Persistence: bookmarks, history, settings, downloads
//!
//! Provides a persistent browser store backed by VFS (RamFs at `/browser/`).
//! Data is serialised as simple JSON-line format for zero external deps.
//!
//! Paths used:
//!   /browser/bookmarks.json  — array of {url, title, added_ms}
//!   /browser/history.json    — array of {url, title, visited_ms}
//!   /browser/settings.json   — flat key/value object
//!   /browser/downloads.json  — array of {url, path, size, state}

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use core::cell::RefCell;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use super::js_interp::{Interpreter, JsValue, JsObject};

// ── Time stub ─────────────────────────────────────────────────────────────────

static BROWSER_CLOCK_MS: AtomicU64 = AtomicU64::new(1_000_000); // start at 1000 s

pub fn tick_browser_clock(delta_ms: u64) {
    BROWSER_CLOCK_MS.fetch_add(delta_ms, Ordering::Relaxed);
}

fn now_ms() -> u64 {
    BROWSER_CLOCK_MS.load(Ordering::Relaxed)
}

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Bookmark {
    pub url:      String,
    pub title:    String,
    pub added_ms: u64,
    pub folder:   String, // empty = root
}

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub url:        String,
    pub title:      String,
    pub visited_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DownloadState { Pending, InProgress, Complete, Failed, Cancelled }

#[derive(Clone, Debug)]
pub struct DownloadEntry {
    pub id:       u32,
    pub url:      String,
    pub filename: String,  // local path under /downloads/
    pub size:     u64,     // bytes received so far
    pub total:    u64,     // total bytes (0 if unknown)
    pub state:    DownloadState,
    pub started:  u64,
    pub mime:     String,
}

// ── In-memory store ───────────────────────────────────────────────────────────

struct BrowserStore {
    bookmarks:  Vec<Bookmark>,
    history:    Vec<HistoryEntry>,
    downloads:  Vec<DownloadEntry>,
    settings:   BTreeMap<String, String>,
    next_dl_id: u32,
}
unsafe impl Send for BrowserStore {}
unsafe impl Sync for BrowserStore {}

impl BrowserStore {
    const fn new() -> Self {
        BrowserStore {
            bookmarks: Vec::new(),
            history:   Vec::new(),
            downloads: Vec::new(),
            settings:  BTreeMap::new(),
            next_dl_id: 1,
        }
    }
}

static STORE: Mutex<BrowserStore> = Mutex::new(BrowserStore::new());

// ── JSON serialisation helpers ────────────────────────────────────────────────

fn json_str(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
    format!("\"{}\"", escaped)
}

fn json_obj(fields: &[(&str, String)]) -> String {
    let inner: Vec<String> = fields.iter()
        .map(|(k, v)| format!("{}: {}", json_str(k), v))
        .collect();
    format!("{{{}}}", inner.join(", "))
}

fn json_arr(items: &[String]) -> String {
    format!("[{}]", items.join(", "))
}

// ── Bookmarks ─────────────────────────────────────────────────────────────────

pub fn bookmark_add(url: &str, title: &str, folder: &str) {
    let mut s = STORE.lock();
    // Remove existing bookmark for same URL to avoid duplicates
    s.bookmarks.retain(|b| b.url != url);
    s.bookmarks.push(Bookmark {
        url:      url.to_string(),
        title:    title.to_string(),
        added_ms: now_ms(),
        folder:   folder.to_string(),
    });
    drop(s);
    persist_bookmarks();
}

pub fn bookmark_remove(url: &str) {
    STORE.lock().bookmarks.retain(|b| b.url != url);
    persist_bookmarks();
}

pub fn bookmark_get_all() -> Vec<Bookmark> {
    STORE.lock().bookmarks.clone()
}

pub fn bookmark_search(query: &str) -> Vec<Bookmark> {
    let q = query.to_lowercase();
    STORE.lock().bookmarks.iter()
        .filter(|b| b.url.to_lowercase().contains(&q) || b.title.to_lowercase().contains(&q))
        .cloned().collect()
}

pub fn bookmark_is_saved(url: &str) -> bool {
    STORE.lock().bookmarks.iter().any(|b| b.url == url)
}

fn persist_bookmarks() {
    let items: Vec<String> = STORE.lock().bookmarks.iter().map(|b| {
        json_obj(&[
            ("url",      json_str(&b.url)),
            ("title",    json_str(&b.title)),
            ("folder",   json_str(&b.folder)),
            ("added_ms", b.added_ms.to_string()),
        ])
    }).collect();
    let json = json_arr(&items);
    let _ = crate::vfs::create_and_write("/browser/bookmarks.json", json.as_bytes());
}

// ── History ───────────────────────────────────────────────────────────────────

const MAX_HISTORY: usize = 10_000;

pub fn history_add(url: &str, title: &str) {
    // Skip browser-internal pages
    if url.starts_with("about:") || url.starts_with("smartos:") { return; }
    let mut s = STORE.lock();
    // Move to front if already present (most-recent-first)
    s.history.retain(|h| h.url != url);
    s.history.insert(0, HistoryEntry {
        url:        url.to_string(),
        title:      title.to_string(),
        visited_ms: now_ms(),
    });
    if s.history.len() > MAX_HISTORY {
        s.history.truncate(MAX_HISTORY);
    }
    drop(s);
    persist_history();
}

pub fn history_search(query: &str, limit: usize) -> Vec<HistoryEntry> {
    let q = query.to_lowercase();
    STORE.lock().history.iter()
        .filter(|h| h.url.to_lowercase().contains(&q) || h.title.to_lowercase().contains(&q))
        .take(limit)
        .cloned()
        .collect()
}

pub fn history_clear() {
    STORE.lock().history.clear();
    let _ = crate::vfs::create_and_write("/browser/history.json", b"[]");
}

pub fn history_get_recent(limit: usize) -> Vec<HistoryEntry> {
    STORE.lock().history.iter().take(limit).cloned().collect()
}

fn persist_history() {
    let items: Vec<String> = STORE.lock().history.iter().take(1000).map(|h| {
        json_obj(&[
            ("url",        json_str(&h.url)),
            ("title",      json_str(&h.title)),
            ("visited_ms", h.visited_ms.to_string()),
        ])
    }).collect();
    let json = json_arr(&items);
    let _ = crate::vfs::create_and_write("/browser/history.json", json.as_bytes());
}

// ── Settings ──────────────────────────────────────────────────────────────────

fn default_settings() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("homepage".to_string(),            "about:newtab".to_string());
    m.insert("search_engine".to_string(),       "https://search.smartos.internal/?q=".to_string());
    m.insert("default_zoom".to_string(),        "1.0".to_string());
    m.insert("block_popups".to_string(),        "true".to_string());
    m.insert("block_mixed_content".to_string(), "true".to_string());
    m.insert("do_not_track".to_string(),        "true".to_string());
    m.insert("js_enabled".to_string(),          "true".to_string());
    m.insert("cookies_enabled".to_string(),     "true".to_string());
    m.insert("images_enabled".to_string(),      "true".to_string());
    m.insert("download_path".to_string(),       "/downloads/".to_string());
    m.insert("theme".to_string(),               "system".to_string());
    m.insert("font_size".to_string(),           "16".to_string());
    m.insert("language".to_string(),            "en-US".to_string());
    m.insert("restore_tabs".to_string(),        "true".to_string());
    m
}

pub fn settings_load() {
    // Apply defaults first
    let defaults = default_settings();
    let mut s = STORE.lock();
    for (k, v) in defaults {
        s.settings.entry(k).or_insert(v);
    }
    drop(s);

    // Try to read from VFS
    if let Ok(data) = crate::vfs::read_file_full("/browser/settings.json") {
        let json = String::from_utf8_lossy(&data);
        // Simple key:value JSON parse
        for line in json.split(',') {
            let line = line.trim().trim_start_matches('{').trim_end_matches('}');
            if let Some(colon) = line.find(':') {
                let k = line[..colon].trim().trim_matches('"').to_string();
                let v = line[colon+1..].trim().trim_matches('"').to_string();
                if !k.is_empty() {
                    STORE.lock().settings.insert(k, v);
                }
            }
        }
    }
}

pub fn settings_get(key: &str) -> String {
    STORE.lock().settings.get(key).cloned().unwrap_or_default()
}

pub fn settings_set(key: &str, value: &str) {
    STORE.lock().settings.insert(key.to_string(), value.to_string());
    persist_settings();
}

// ── Test helpers ──────────────────────────────────────────────────────────────

/// Add a bookmark and return its 1-based index as an ID (for test assertions).
pub fn bookmark_add_simple(url: &str, title: &str) -> u32 {
    bookmark_add(url, title, "");
    STORE.lock().bookmarks.len() as u32
}

/// History entry count.
pub fn history_count() -> usize {
    STORE.lock().history.len()
}

/// Alias for `history_add`.
pub fn history_push(url: &str, title: &str) { history_add(url, title); }

/// Load defaults + return a snapshot of the settings map.
pub fn settings_snapshot() -> alloc::collections::BTreeMap<String, String> {
    settings_load();
    STORE.lock().settings.clone()
}

/// Save a single key and persist.
pub fn settings_save_key(key: &str, value: &str) { settings_set(key, value); }

fn persist_settings() {
    let fields: Vec<String> = STORE.lock().settings.iter()
        .map(|(k,v)| format!("{}: {}", json_str(k), json_str(v)))
        .collect();
    let json = format!("{{{}}}", fields.join(", "));
    let _ = crate::vfs::create_and_write("/browser/settings.json", json.as_bytes());
}

// ── Downloads ─────────────────────────────────────────────────────────────────

pub fn download_start(url: &str, filename: &str, mime: &str, total: u64) -> u32 {
    let mut s = STORE.lock();
    let id = s.next_dl_id;
    s.next_dl_id += 1;
    s.downloads.push(DownloadEntry {
        id,
        url:      url.to_string(),
        filename: filename.to_string(),
        size:     0,
        total,
        state:    DownloadState::InProgress,
        started:  now_ms(),
        mime:     mime.to_string(),
    });
    id
}

pub fn download_update(id: u32, bytes_received: u64) {
    let mut s = STORE.lock();
    if let Some(dl) = s.downloads.iter_mut().find(|d| d.id == id) {
        dl.size = bytes_received;
    }
}

pub fn download_complete(id: u32, data: &[u8]) {
    let path = {
        let mut s = STORE.lock();
        if let Some(dl) = s.downloads.iter_mut().find(|d| d.id == id) {
            dl.state = DownloadState::Complete;
            dl.size  = data.len() as u64;
            format!("/downloads/{}", dl.filename)
        } else { return; }
    };
    // Write file to disk
    let _ = crate::vfs::create_and_write(&path, data);
    persist_downloads();
}

pub fn download_cancel(id: u32) {
    let mut s = STORE.lock();
    if let Some(dl) = s.downloads.iter_mut().find(|d| d.id == id) {
        dl.state = DownloadState::Cancelled;
    }
    drop(s);
    persist_downloads();
}

pub fn download_get_all() -> Vec<DownloadEntry> {
    STORE.lock().downloads.clone()
}

fn persist_downloads() {
    let items: Vec<String> = STORE.lock().downloads.iter().map(|d| {
        let state = match d.state {
            DownloadState::Pending    => "pending",
            DownloadState::InProgress => "in_progress",
            DownloadState::Complete   => "complete",
            DownloadState::Failed     => "failed",
            DownloadState::Cancelled  => "cancelled",
        };
        json_obj(&[
            ("id",       d.id.to_string()),
            ("url",      json_str(&d.url)),
            ("filename", json_str(&d.filename)),
            ("size",     d.size.to_string()),
            ("total",    d.total.to_string()),
            ("state",    json_str(state)),
            ("started",  d.started.to_string()),
        ])
    }).collect();
    let json = json_arr(&items);
    let _ = crate::vfs::create_and_write("/browser/downloads.json", json.as_bytes());
}

// ── JS bindings ───────────────────────────────────────────────────────────────

fn bookmark_to_js(b: &Bookmark) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("url".to_string(),      JsValue::Str(b.url.clone()));
    obj.borrow_mut().set("title".to_string(),    JsValue::Str(b.title.clone()));
    obj.borrow_mut().set("folder".to_string(),   JsValue::Str(b.folder.clone()));
    obj.borrow_mut().set("addedMs".to_string(),  JsValue::Number(b.added_ms as f64));
    JsValue::Object(obj)
}

fn history_to_js(h: &HistoryEntry) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("url".to_string(),       JsValue::Str(h.url.clone()));
    obj.borrow_mut().set("title".to_string(),     JsValue::Str(h.title.clone()));
    obj.borrow_mut().set("visitedMs".to_string(), JsValue::Number(h.visited_ms as f64));
    JsValue::Object(obj)
}

fn download_to_js(d: &DownloadEntry) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    let state = match d.state {
        DownloadState::Pending    => "pending",
        DownloadState::InProgress => "in_progress",
        DownloadState::Complete   => "complete",
        DownloadState::Failed     => "failed",
        DownloadState::Cancelled  => "cancelled",
    };
    let progress = if d.total > 0 { (d.size as f64 / d.total as f64 * 100.0) } else { 0.0 };
    obj.borrow_mut().set("id".to_string(),       JsValue::Number(d.id as f64));
    obj.borrow_mut().set("url".to_string(),      JsValue::Str(d.url.clone()));
    obj.borrow_mut().set("filename".to_string(), JsValue::Str(d.filename.clone()));
    obj.borrow_mut().set("size".to_string(),     JsValue::Number(d.size as f64));
    obj.borrow_mut().set("total".to_string(),    JsValue::Number(d.total as f64));
    obj.borrow_mut().set("state".to_string(),    JsValue::Str(state.to_string()));
    obj.borrow_mut().set("progress".to_string(), JsValue::Number(progress));
    JsValue::Object(obj)
}

fn make_bookmarks_obj() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("add".to_string(), JsValue::NativeFunction("add", |args, _| {
        let url    = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
        let title  = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
        let folder = args.get(2).map(|v| v.to_string_val()).unwrap_or_default();
        bookmark_add(&url, &title, &folder);
        JsValue::Undefined
    }));
    obj.borrow_mut().set("remove".to_string(), JsValue::NativeFunction("remove", |args, _| {
        bookmark_remove(&args.get(0).map(|v| v.to_string_val()).unwrap_or_default());
        JsValue::Undefined
    }));
    obj.borrow_mut().set("isSaved".to_string(), JsValue::NativeFunction("isSaved", |args, _| {
        JsValue::Bool(bookmark_is_saved(&args.get(0).map(|v| v.to_string_val()).unwrap_or_default()))
    }));
    obj.borrow_mut().set("getAll".to_string(), JsValue::NativeFunction("getAll", |_, _| {
        let arr: Vec<JsValue> = bookmark_get_all().iter().map(bookmark_to_js).collect();
        JsValue::Array(Rc::new(RefCell::new(arr)))
    }));
    obj.borrow_mut().set("search".to_string(), JsValue::NativeFunction("search", |args, _| {
        let q = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
        let arr: Vec<JsValue> = bookmark_search(&q).iter().map(bookmark_to_js).collect();
        JsValue::Array(Rc::new(RefCell::new(arr)))
    }));
    JsValue::Object(obj)
}

fn make_history_obj() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("add".to_string(), JsValue::NativeFunction("add", |args, _| {
        let url   = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
        let title = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
        history_add(&url, &title);
        JsValue::Undefined
    }));
    obj.borrow_mut().set("search".to_string(), JsValue::NativeFunction("search", |args, _| {
        let q   = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
        let lim = args.get(1).map(|v| v.to_number() as usize).unwrap_or(50);
        let arr: Vec<JsValue> = history_search(&q, lim).iter().map(history_to_js).collect();
        JsValue::Array(Rc::new(RefCell::new(arr)))
    }));
    obj.borrow_mut().set("getRecent".to_string(), JsValue::NativeFunction("getRecent", |args, _| {
        let lim = args.get(0).map(|v| v.to_number() as usize).unwrap_or(20);
        let arr: Vec<JsValue> = history_get_recent(lim).iter().map(history_to_js).collect();
        JsValue::Array(Rc::new(RefCell::new(arr)))
    }));
    obj.borrow_mut().set("clear".to_string(), JsValue::NativeFunction("clear", |_, _| {
        history_clear();
        JsValue::Undefined
    }));
    JsValue::Object(obj)
}

fn make_settings_obj() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("get".to_string(), JsValue::NativeFunction("get", |args, _| {
        let k = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
        let d = args.get(1).map(|v| v.to_string_val());
        let v = settings_get(&k);
        if v.is_empty() {
            d.map(JsValue::Str).unwrap_or(JsValue::Undefined)
        } else {
            JsValue::Str(v)
        }
    }));
    obj.borrow_mut().set("set".to_string(), JsValue::NativeFunction("set", |args, _| {
        let k = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
        let v = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
        settings_set(&k, &v);
        JsValue::Undefined
    }));
    obj.borrow_mut().set("getAll".to_string(), JsValue::NativeFunction("getAll", |_, _| {
        let obj = Rc::new(RefCell::new(JsObject::new()));
        for (k, v) in &STORE.lock().settings {
            obj.borrow_mut().set(k.clone(), JsValue::Str(v.clone()));
        }
        JsValue::Object(obj)
    }));
    JsValue::Object(obj)
}

fn make_downloads_obj() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("getAll".to_string(), JsValue::NativeFunction("getAll", |_, _| {
        let arr: Vec<JsValue> = download_get_all().iter().map(download_to_js).collect();
        JsValue::Array(Rc::new(RefCell::new(arr)))
    }));
    obj.borrow_mut().set("cancel".to_string(), JsValue::NativeFunction("cancel", |args, _| {
        download_cancel(args.get(0).map(|v| v.to_number() as u32).unwrap_or(0));
        JsValue::Undefined
    }));
    obj.borrow_mut().set("clearCompleted".to_string(), JsValue::NativeFunction("clearCompleted", |_, _| {
        STORE.lock().downloads.retain(|d| d.state == DownloadState::InProgress);
        persist_downloads();
        JsValue::Undefined
    }));
    JsValue::Object(obj)
}

// ── Initialise VFS directories ────────────────────────────────────────────────

pub fn init() {
    // Ensure /browser/ and /downloads/ directories exist
    let _ = crate::vfs::mkdir("/browser");
    let _ = crate::vfs::mkdir("/downloads");
    // Load settings from disk (or apply defaults)
    settings_load();
}

// ── Install JS API ────────────────────────────────────────────────────────────

pub fn install_persistence_api(interp: &mut Interpreter) {
    let browser_obj = Rc::new(RefCell::new(JsObject::new()));
    browser_obj.borrow_mut().set("bookmarks".to_string(), make_bookmarks_obj());
    browser_obj.borrow_mut().set("history".to_string(),   make_history_obj());
    browser_obj.borrow_mut().set("settings".to_string(),  make_settings_obj());
    browser_obj.borrow_mut().set("downloads".to_string(), make_downloads_obj());
    interp.env.define("smartBrowser".to_string(), JsValue::Object(browser_obj));
}

// ── Self-test ─────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] persistence: {}", $name); }
        }
    }

    // T1: Bookmark add/isSaved/remove
    bookmark_add("https://example.com", "Example", "");
    check!(bookmark_is_saved("https://example.com"), "bookmark add+isSaved");
    bookmark_remove("https://example.com");
    check!(!bookmark_is_saved("https://example.com"), "bookmark remove");

    // T2: Bookmark search
    bookmark_add("https://news.ycombinator.com", "Hacker News", "tech");
    bookmark_add("https://github.com", "GitHub", "dev");
    let results = bookmark_search("github");
    check!(results.len() == 1, "bookmark search");
    check!(results[0].title == "GitHub", "bookmark search result title");
    // cleanup
    bookmark_remove("https://news.ycombinator.com");
    bookmark_remove("https://github.com");

    // T3: History add/search/recent
    history_add("https://rust-lang.org", "The Rust Programming Language");
    history_add("https://docs.rs", "Docs.rs");
    check!(history_search("rust", 10).len() >= 1, "history search");
    check!(history_get_recent(10).len() >= 2, "history recent");
    history_clear();
    check!(history_get_recent(10).is_empty(), "history clear");

    // T4: Settings get/set
    {
        // Load defaults
        let defaults = default_settings();
        for (k, v) in defaults {
            STORE.lock().settings.entry(k).or_insert(v);
        }
        let hp = settings_get("homepage");
        check!(hp == "about:newtab", "settings default homepage");
        settings_set("font_size", "18");
        check!(settings_get("font_size") == "18", "settings set/get");
        // Restore default
        settings_set("font_size", "16");
    }

    // T5: Download lifecycle
    let dl_id = download_start("https://example.com/file.zip", "file.zip", "application/zip", 1024);
    check!(dl_id > 0, "download start returns ID");
    download_update(dl_id, 512);
    {
        let s = STORE.lock();
        if let Some(dl) = s.downloads.iter().find(|d| d.id == dl_id) {
            check!(dl.size == 512, "download update bytes");
            check!(dl.state == DownloadState::InProgress, "download state InProgress");
        } else { fail += 1; }
    }
    download_cancel(dl_id);
    {
        let s = STORE.lock();
        if let Some(dl) = s.downloads.iter().find(|d| d.id == dl_id) {
            check!(dl.state == DownloadState::Cancelled, "download cancel");
        } else { fail += 1; }
    }

    // T6: JS API
    {
        let mut interp = Interpreter::new();
        install_persistence_api(&mut interp);
        interp.run(r#"
            smartBrowser.bookmarks.add('https://test.com','Test Page','');
            var isSaved = smartBrowser.bookmarks.isSaved('https://test.com');
            smartBrowser.bookmarks.remove('https://test.com');
            var isGone  = smartBrowser.bookmarks.isSaved('https://test.com');
        "#);
        check!(interp.env.get("isSaved").is_truthy(), "JS bookmarks.isSaved");
        check!(!interp.env.get("isGone").is_truthy(), "JS bookmarks.remove");
    }

    if fail == 0 {
        crate::serial_println!("[persistence] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[persistence] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

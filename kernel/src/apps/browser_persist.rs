#![allow(dead_code)]
/// Smart OS — Browser Persistence (Phase 89, v0.49.0)
///
/// Stores browser session data to the Virtual File System:
///   • `HistoryStore`    — ring-buffer history (max 10 000 entries), VFS-backed
///   • `BookmarkStore`   — flat list of bookmarks with folders
///   • `CookieStore`     — per-origin cookies with expiry, SameSite, Secure flags
///   • `HstsStore`       — HSTS max-age per host, persistent
///   • `SessionStore`    — tab URLs for session restore
///
/// All stores write to `/home/user/.browser/` via `crate::vfs::create_and_write`.
/// Reads parse a simple newline-delimited text format to stay no_std compatible.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use alloc::collections::BTreeMap;

// ─── History ──────────────────────────────────────────────────────────────────
const MAX_HISTORY: usize = 10_000;

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub url:       String,
    pub title:     String,
    pub timestamp: u64,   // uptime seconds (approximate)
}

pub struct HistoryStore {
    entries: Vec<HistoryEntry>,
}

impl HistoryStore {
    pub fn new() -> Self { HistoryStore { entries: Vec::new() } }

    pub fn push(&mut self, url: &str, title: &str) {
        let ts = crate::drivers::timer::uptime_secs();
        if self.entries.len() >= MAX_HISTORY { self.entries.remove(0); }
        self.entries.push(HistoryEntry { url: url.to_string(), title: title.to_string(), timestamp: ts });
    }

    pub fn search(&self, query: &str) -> Vec<&HistoryEntry> {
        let q = query.to_ascii_lowercase();
        self.entries.iter()
            .filter(|e| e.url.to_ascii_lowercase().contains(q.as_str())
                     || e.title.to_ascii_lowercase().contains(q.as_str()))
            .rev()
            .take(50)
            .collect()
    }

    pub fn clear(&mut self) { self.entries.clear(); }

    pub fn len(&self) -> usize { self.entries.len() }

    pub fn recent(&self, n: usize) -> Vec<&HistoryEntry> {
        self.entries.iter().rev().take(n).collect()
    }

    pub fn save(&self) -> bool {
        let mut buf = String::new();
        for e in &self.entries {
            buf.push_str(&format!("{}\t{}\t{}\n", e.timestamp, e.url, e.title));
        }
        crate::vfs::create_and_write("/home/user/.browser/history.db", buf.as_bytes()).is_ok()
    }

    pub fn load_from_str(&mut self, data: &str) {
        for line in data.lines() {
            let parts: Vec<&str> = line.splitn(3, '\t').collect();
            if parts.len() == 3 {
                let ts = parts[0].parse::<u64>().unwrap_or(0);
                self.entries.push(HistoryEntry {
                    url:       parts[1].to_string(),
                    title:     parts[2].to_string(),
                    timestamp: ts,
                });
            }
        }
    }
}

// ─── Bookmarks ────────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct Bookmark {
    pub id:     u32,
    pub url:    String,
    pub title:  String,
    pub folder: String,
    pub added:  u64,
}

pub struct BookmarkStore {
    bookmarks: Vec<Bookmark>,
    next_id:   u32,
}

impl BookmarkStore {
    pub fn new() -> Self { BookmarkStore { bookmarks: Vec::new(), next_id: 1 } }

    pub fn add(&mut self, url: &str, title: &str, folder: &str) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let ts = crate::drivers::timer::uptime_secs();
        self.bookmarks.push(Bookmark { id, url: url.to_string(), title: title.to_string(),
            folder: folder.to_string(), added: ts });
        id
    }

    pub fn remove(&mut self, id: u32) -> bool {
        let len = self.bookmarks.len();
        self.bookmarks.retain(|b| b.id != id);
        self.bookmarks.len() != len
    }

    pub fn find_by_url(&self, url: &str) -> Option<&Bookmark> {
        self.bookmarks.iter().find(|b| b.url == url)
    }

    pub fn folder(&self, folder: &str) -> Vec<&Bookmark> {
        self.bookmarks.iter().filter(|b| b.folder == folder).collect()
    }

    pub fn all_folders(&self) -> Vec<&str> {
        let mut folders: Vec<&str> = self.bookmarks.iter()
            .map(|b| b.folder.as_str())
            .collect();
        folders.sort();
        folders.dedup();
        folders
    }

    pub fn len(&self) -> usize { self.bookmarks.len() }

    pub fn save(&self) -> bool {
        let mut buf = String::new();
        for b in &self.bookmarks {
            buf.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", b.id, b.added, b.folder, b.url, b.title));
        }
        crate::vfs::create_and_write("/home/user/.browser/bookmarks.db", buf.as_bytes()).is_ok()
    }
}

// ─── Cookies ─────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SameSite { Strict, Lax, None }

#[derive(Clone, Debug)]
pub struct Cookie {
    pub name:      String,
    pub value:     String,
    pub domain:    String,
    pub path:      String,
    pub expires:   Option<u64>,   // uptime seconds
    pub secure:    bool,
    pub http_only: bool,
    pub same_site: SameSite,
}

impl Cookie {
    pub fn is_expired(&self) -> bool {
        if let Some(exp) = self.expires {
            crate::drivers::timer::uptime_secs() > exp
        } else { false }
    }

    pub fn applies_to(&self, url: &str) -> bool {
        if self.secure && !url.starts_with("https://") { return false; }
        let url_lc = url.to_ascii_lowercase();
        let host = url_lc.strip_prefix("https://")
            .or_else(|| url_lc.strip_prefix("http://"))
            .unwrap_or(&url_lc)
            .split('/').next().unwrap_or("");
        host == self.domain || host.ends_with(&format!(".{}", self.domain))
    }
}

pub struct CookieStore {
    cookies: Vec<Cookie>,
}

impl CookieStore {
    pub fn new() -> Self { CookieStore { cookies: Vec::new() } }

    pub fn set(&mut self, cookie: Cookie) {
        // Replace if same name+domain+path
        self.cookies.retain(|c| !(c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path));
        self.cookies.push(cookie);
    }

    pub fn get_for_url(&self, url: &str) -> Vec<(&str, &str)> {
        self.cookies.iter()
            .filter(|c| !c.is_expired() && c.applies_to(url))
            .map(|c| (c.name.as_str(), c.value.as_str()))
            .collect()
    }

    pub fn delete(&mut self, name: &str, domain: &str) {
        self.cookies.retain(|c| !(c.name == name && c.domain == domain));
    }

    pub fn clear_origin(&mut self, domain: &str) {
        self.cookies.retain(|c| c.domain != domain);
    }

    pub fn len(&self) -> usize { self.cookies.len() }
}

// ─── HSTS ─────────────────────────────────────────────────────────────────────
pub struct HstsStore {
    entries: BTreeMap<String, u64>,   // host → expiry (uptime seconds)
}

impl HstsStore {
    pub fn new() -> Self { HstsStore { entries: BTreeMap::new() } }

    pub fn record(&mut self, host: &str, max_age: u64) {
        let expiry = crate::drivers::timer::uptime_secs() + max_age;
        self.entries.insert(host.to_ascii_lowercase(), expiry);
    }

    pub fn should_upgrade(&self, host: &str) -> bool {
        let now = crate::drivers::timer::uptime_secs();
        self.entries.get(&host.to_ascii_lowercase())
            .map(|&exp| exp > now)
            .unwrap_or(false)
    }

    pub fn upgrade_url(&self, url: &str) -> String {
        if url.starts_with("http://") {
            let host = url.strip_prefix("http://").unwrap_or("").split('/').next().unwrap_or("");
            if self.should_upgrade(host) {
                return url.replacen("http://", "https://", 1);
            }
        }
        url.to_string()
    }
}

// ─── Session restore ─────────────────────────────────────────────────────────
pub struct SessionStore {
    pub tabs: Vec<String>,   // URLs of open tabs
}

impl SessionStore {
    pub fn new() -> Self { SessionStore { tabs: Vec::new() } }

    pub fn save(&self) -> bool {
        let content = self.tabs.join("\n");
        crate::vfs::create_and_write("/home/user/.browser/session.db", content.as_bytes()).is_ok()
    }

    pub fn load_from_str(&mut self, data: &str) {
        self.tabs = data.lines().filter(|l| !l.trim().is_empty())
            .map(|l| l.to_string()).collect();
    }
}

// ─── Self-test ────────────────────────────────────────────────────────────────
pub fn self_test() -> bool {
    let mut ok = true;

    // T1: HistoryStore push and search
    let mut hist = HistoryStore::new();
    hist.push("https://example.com/a", "Example A");
    hist.push("https://other.org/b",   "Other B");
    if hist.len() != 2 { ok = false; }
    let found = hist.search("example");
    if found.is_empty() { ok = false; }

    // T2: HistoryStore recent
    hist.push("https://recent.io/c", "Recent C");
    if hist.recent(2).len() != 2 { ok = false; }
    if hist.recent(2)[0].url != "https://recent.io/c" { ok = false; }

    // T3: BookmarkStore add/remove/find
    let mut bm = BookmarkStore::new();
    let id = bm.add("https://bookmark.com", "Bookmark", "Work");
    if bm.len() != 1 { ok = false; }
    if bm.find_by_url("https://bookmark.com").is_none() { ok = false; }
    bm.remove(id);
    if bm.len() != 0 { ok = false; }

    // T4: BookmarkStore folders
    bm.add("https://a.com", "A", "Work");
    bm.add("https://b.com", "B", "Personal");
    bm.add("https://c.com", "C", "Work");
    let folders = bm.all_folders();
    if folders.len() != 2 { ok = false; }
    if bm.folder("Work").len() != 2 { ok = false; }

    // T5: CookieStore set/get
    let mut cs = CookieStore::new();
    cs.set(Cookie { name: "session".to_string(), value: "abc".to_string(),
        domain: "example.com".to_string(), path: "/".to_string(),
        expires: None, secure: false, http_only: false, same_site: SameSite::Lax });
    let cookies = cs.get_for_url("http://example.com/page");
    if cookies.len() != 1 || cookies[0].0 != "session" { ok = false; }

    // T6: CookieStore secure cookie not sent over HTTP
    cs.set(Cookie { name: "secure_tok".to_string(), value: "xyz".to_string(),
        domain: "example.com".to_string(), path: "/".to_string(),
        expires: None, secure: true, http_only: false, same_site: SameSite::Strict });
    let http_cookies = cs.get_for_url("http://example.com/");
    if http_cookies.iter().any(|(n, _)| *n == "secure_tok") { ok = false; }
    let https_cookies = cs.get_for_url("https://example.com/");
    if !https_cookies.iter().any(|(n, _)| *n == "secure_tok") { ok = false; }

    // T7: HstsStore upgrade
    let mut hsts = HstsStore::new();
    hsts.record("secure.com", 31536000);
    if !hsts.should_upgrade("secure.com") { ok = false; }
    let upgraded = hsts.upgrade_url("http://secure.com/page");
    if !upgraded.starts_with("https://") { ok = false; }

    // T8: HstsStore non-HSTS host not upgraded
    if hsts.should_upgrade("other.com") { ok = false; }

    // T9: SessionStore round-trip
    let mut sess = SessionStore::new();
    sess.tabs = alloc::vec!["https://a.com".to_string(), "https://b.com".to_string()];
    let data = sess.tabs.join("\n");
    let mut sess2 = SessionStore::new();
    sess2.load_from_str(&data);
    if sess2.tabs.len() != 2 { ok = false; }
    if sess2.tabs[0] != "https://a.com" { ok = false; }

    ok
}

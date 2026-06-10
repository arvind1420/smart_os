//! Browser Performance Pass — Phase 95 for Smart OS.
//!
//! Provides five orthogonal optimisations that cut page-load cost on
//! modest hardware (no JIT, no hardware GPU):
//!
//! ## 1  CSS Rule Index
//! Instead of matching every CSS rule against every DOM element (O(R×N)),
//! we build an inverted index keyed by the *right-most simple selector*:
//!   - tag name  → rule ids   (e.g. `"p"` → [3, 7, 12])
//!   - class     → rule ids   (e.g. `"highlight"` → [5])
//!   - id        → rule ids   (e.g. `"header"` → [1])
//!   - universal → rule ids   (rules with `*` or complex combinators)
//!
//! At match time we only test the rules in the element's bucket plus the
//! universal bucket — typically < 10 rules per element.
//!
//! ## 2  Dirty-Subtree Layout
//! Each node carries a `dirty` flag.  Re-layout only propagates through
//! dirty subtrees; clean siblings are skipped entirely.
//!
//! ## 3  Paint Display-List Cache
//! The display list is re-generated only when the layout changes.
//! `DisplayListCache` stores the last-known list and a dirty flag.
//! The compositor compares the flag before re-running `build_display_list`.
//!
//! ## 4  HTTP Resource Cache
//! LRU cache (max 64 entries, 8 MiB total).  Each entry stores:
//!   - URL key
//!   - response bytes
//!   - ETag / Last-Modified for conditional GET revalidation
//!   - MIME type
//!   - age (monotonic ticks for TTL expiry)
//!
//! ## 5  JS / Image Decode Budgets
//! `JsParserCache` stores up to 32 previously-seen script texts by URL,
//! avoiding repeated tokenisation on navigation back.
//! `ImageDecodeQueue` rate-limits image decoding to at most
//! `MAX_DECODE_BYTES_PER_FRAME` bytes per render frame so the main thread
//! is never blocked longer than ~2 ms.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};

// ─── CSS Rule Index ──────────────────────────────────────────────────────────

/// Opaque rule identifier matching the index into your `Vec<Rule>`.
pub type RuleId = u32;

/// Index entry for a bucket of rules that share a right-most key.
#[derive(Clone, Debug, Default)]
struct RuleBucket {
    ids: Vec<RuleId>,
}

/// The inverted CSS rule index.
///
/// Build it once after parsing the stylesheet; query it per element.
#[derive(Debug, Default)]
pub struct CssRuleIndex {
    by_tag:       BTreeMap<String, RuleBucket>,
    by_class:     BTreeMap<String, RuleBucket>,
    by_id:        BTreeMap<String, RuleBucket>,
    /// Rules that cannot be bucketed (universal, multi-level combinators, etc.)
    universal:    RuleBucket,
    total_rules:  u32,
}

/// Hint returned by the rule index: the caller should test only these rule ids.
#[derive(Debug, Clone)]
pub struct RuleHint {
    pub ids: Vec<RuleId>,
}

impl CssRuleIndex {
    pub fn new() -> Self { Self::default() }

    /// Register rule `id` with the given right-most simple-selector components.
    ///
    /// * `tag`   — e.g. `Some("p")`.  Pass `None` for `*`.
    /// * `class` — e.g. `Some("highlight")`.
    /// * `id`    — e.g. `Some("header")`.
    ///
    /// If all three are `None` the rule goes into the universal bucket.
    pub fn insert(&mut self, id: RuleId, tag: Option<&str>,
                  class: Option<&str>, id_sel: Option<&str>)
    {
        self.total_rules += 1;
        let mut bucketed = false;
        if let Some(t) = tag {
            self.by_tag.entry(t.to_string()).or_default().ids.push(id);
            bucketed = true;
        }
        if let Some(c) = class {
            self.by_class.entry(c.to_string()).or_default().ids.push(id);
            bucketed = true;
        }
        if let Some(i) = id_sel {
            self.by_id.entry(i.to_string()).or_default().ids.push(id);
            bucketed = true;
        }
        if !bucketed {
            self.universal.ids.push(id);
        }
    }

    /// Return the candidate rule ids for an element with `tag`, `classes`, `id`.
    ///
    /// The caller must still run full selector matching on each returned id;
    /// this just reduces the search space.
    pub fn candidates(&self, tag: &str, classes: &[&str], id: Option<&str>) -> RuleHint {
        let mut seen: Vec<RuleId> = Vec::new();

        // tag bucket
        if let Some(b) = self.by_tag.get(tag) {
            for &r in &b.ids {
                if !seen.contains(&r) { seen.push(r); }
            }
        }
        // class buckets
        for &cls in classes {
            if let Some(b) = self.by_class.get(cls) {
                for &r in &b.ids {
                    if !seen.contains(&r) { seen.push(r); }
                }
            }
        }
        // id bucket
        if let Some(eid) = id {
            if let Some(b) = self.by_id.get(eid) {
                for &r in &b.ids {
                    if !seen.contains(&r) { seen.push(r); }
                }
            }
        }
        // universal bucket always included
        for &r in &self.universal.ids {
            if !seen.contains(&r) { seen.push(r); }
        }

        RuleHint { ids: seen }
    }

    pub fn total_rules(&self) -> u32 { self.total_rules }
}

// ─── Dirty-Node Tracking ─────────────────────────────────────────────────────

/// Tracks which DOM nodes need re-layout.
///
/// Set `mark_dirty(id)` whenever a style or attribute changes.
/// Call `take_dirty()` to drain the set before running layout.
#[derive(Debug, Default)]
pub struct DirtySet {
    nodes:    Vec<u64>,   // NodeId values
    subtrees: Vec<u64>,   // full subtree roots (children also dirty)
}

impl DirtySet {
    pub fn new() -> Self { Self::default() }

    /// Mark a single node as needing re-layout.
    pub fn mark_dirty(&mut self, node_id: u64) {
        if !self.nodes.contains(&node_id) {
            self.nodes.push(node_id);
        }
    }

    /// Mark a node and all its descendants.
    pub fn mark_subtree(&mut self, root: u64) {
        if !self.subtrees.contains(&root) {
            self.subtrees.push(root);
        }
    }

    /// Mark all nodes dirty (full re-layout).
    pub fn mark_all(&mut self) {
        self.subtrees.push(u64::MAX); // sentinel: full document
    }

    /// Returns true if full document re-layout is required.
    pub fn needs_full_layout(&self) -> bool {
        self.subtrees.contains(&u64::MAX)
    }

    /// Returns true if `node_id` or one of its ancestor subtrees is dirty.
    pub fn is_dirty(&self, node_id: u64) -> bool {
        self.needs_full_layout()
            || self.nodes.contains(&node_id)
            || self.subtrees.iter().any(|&root| root == node_id)
    }

    /// Drain and return all explicitly-dirty node ids.
    pub fn take_dirty(&mut self) -> Vec<u64> {
        let mut out = self.nodes.clone();
        for &r in &self.subtrees {
            if r != u64::MAX && !out.contains(&r) { out.push(r); }
        }
        self.nodes.clear();
        self.subtrees.clear();
        out
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.subtrees.is_empty()
    }
}

// ─── Paint Display-List Cache ─────────────────────────────────────────────────

/// Represents a single cached display-list item, mirroring `paint::PaintCmd`
/// in a serialisable, clone-friendly form.
#[derive(Clone, Debug)]
pub enum CachedPaintItem {
    FillRect   { x: f32, y: f32, w: f32, h: f32, r: u8, g: u8, b: u8, a: u8 },
    DrawBorder { x: f32, y: f32, w: f32, h: f32, top: f32, right: f32,
                 bottom: f32, left: f32, r: u8, g: u8, b: u8, a: u8 },
    DrawText   { x: f32, y: f32, text: String, font_px: f32,
                 r: u8, g: u8, b: u8, a: u8 },
    DrawImage  { x: f32, y: f32, w: f32, h: f32,
                 data: Vec<u8>, img_w: u32, img_h: u32 },
    PushClip   { x: f32, y: f32, w: f32, h: f32 },
    PopClip,
    PushLayer  { opacity: u8 },
    PopLayer,
}

/// Cache for the most recently-generated display list.
#[derive(Default)]
pub struct DisplayListCache {
    pub items:        Vec<CachedPaintItem>,
    pub dirty:        bool,
    pub layout_gen:   u64,  // increments with each re-layout
}

impl DisplayListCache {
    pub fn new() -> Self {
        DisplayListCache { items: Vec::new(), dirty: true, layout_gen: 0 }
    }

    /// Mark the cached list stale; next `is_dirty()` will return true.
    pub fn invalidate(&mut self) {
        self.dirty = true;
        self.layout_gen += 1;
    }

    pub fn is_dirty(&self) -> bool { self.dirty }

    /// Store a freshly-built display list and clear the dirty flag.
    pub fn store(&mut self, items: Vec<CachedPaintItem>) {
        self.items = items;
        self.dirty = false;
    }
}

// ─── HTTP Resource Cache ──────────────────────────────────────────────────────

const HTTP_CACHE_MAX_ENTRIES: usize = 64;
const HTTP_CACHE_MAX_BYTES:   usize = 8 * 1024 * 1024; // 8 MiB total

/// A single cached HTTP response.
#[derive(Clone, Debug)]
pub struct CacheEntry {
    pub url:           String,
    pub data:          Vec<u8>,
    pub mime:          String,
    pub etag:          Option<String>,
    pub last_modified: Option<String>,
    /// Maximum age in seconds (from Cache-Control: max-age=N).
    pub max_age_secs:  u64,
    /// Timestamp when the entry was inserted (monotonic tick counter).
    pub inserted_at:   u64,
    /// LRU counter — incremented on each access.
    pub last_used:     u64,
}

/// Global monotonic tick counter for cache age tracking.
static CACHE_TICK: AtomicU64 = AtomicU64::new(0);

pub fn cache_tick() -> u64 { CACHE_TICK.load(Ordering::Relaxed) }
pub fn advance_cache_tick() { CACHE_TICK.fetch_add(1, Ordering::Relaxed); }

/// LRU HTTP resource cache.
///
/// Key = URL string.  Evicts the least-recently-used entry when full.
#[derive(Default)]
pub struct HttpCache {
    entries:    Vec<CacheEntry>,
    total_bytes: usize,
}

impl HttpCache {
    pub fn new() -> Self { Self::default() }

    /// Look up a URL.  Returns a reference if found and not stale.
    pub fn get(&mut self, url: &str) -> Option<&CacheEntry> {
        let tick = cache_tick();
        // check staleness first
        let pos = self.entries.iter().position(|e| {
            e.url == url && (e.max_age_secs == 0 || tick.saturating_sub(e.inserted_at) < e.max_age_secs)
        })?;
        self.entries[pos].last_used = tick;
        Some(&self.entries[pos])
    }

    /// Check if we have a stale (expired) entry for `url` to send
    /// conditional GET headers with.
    pub fn get_stale_headers(&self, url: &str) -> Option<(Option<String>, Option<String>)> {
        let tick = cache_tick();
        self.entries.iter().find(|e| e.url == url && tick.saturating_sub(e.inserted_at) >= e.max_age_secs)
            .map(|e| (e.etag.clone(), e.last_modified.clone()))
    }

    /// Insert or replace an entry.  Evicts LRU if over capacity.
    pub fn insert(&mut self, entry: CacheEntry) {
        // Remove existing entry for same URL
        if let Some(pos) = self.entries.iter().position(|e| e.url == entry.url) {
            self.total_bytes -= self.entries[pos].data.len();
            self.entries.remove(pos);
        }

        let bytes = entry.data.len();

        // Evict if needed
        while (self.entries.len() >= HTTP_CACHE_MAX_ENTRIES
            || self.total_bytes + bytes > HTTP_CACHE_MAX_BYTES)
            && !self.entries.is_empty()
        {
            // find LRU
            let lru_pos = self.entries.iter().enumerate()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.total_bytes -= self.entries[lru_pos].data.len();
            self.entries.remove(lru_pos);
        }

        self.total_bytes += bytes;
        self.entries.push(entry);
    }

    /// Invalidate a specific URL (e.g., after POST).
    pub fn invalidate(&mut self, url: &str) {
        if let Some(pos) = self.entries.iter().position(|e| e.url == url) {
            self.total_bytes -= self.entries[pos].data.len();
            self.entries.remove(pos);
        }
    }

    /// Purge all entries (Ctrl+Shift+R / hard reload).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
    }

    pub fn entry_count(&self) -> usize { self.entries.len() }
    pub fn total_bytes(&self) -> usize { self.total_bytes }

    /// Build conditional GET headers for `url` if we have a stale entry.
    /// Returns `(If-None-Match, If-Modified-Since)` header strings.
    pub fn conditional_headers(&self, url: &str) -> (Option<String>, Option<String>) {
        if let Some(e) = self.entries.iter().find(|e| e.url == url) {
            return (
                e.etag.as_ref().map(|t| format!("If-None-Match: {}\r\n", t)),
                e.last_modified.as_ref().map(|m| format!("If-Modified-Since: {}\r\n", m)),
            );
        }
        (None, None)
    }

    /// Parse `Cache-Control` header value and return max-age in seconds (0 = no-cache).
    pub fn parse_max_age(cc: &str) -> u64 {
        for part in cc.split(',') {
            let p = part.trim();
            if p.starts_with("max-age=") {
                if let Ok(v) = p[8..].trim().parse::<u64>() {
                    return v;
                }
            }
            if p == "no-cache" || p == "no-store" { return 0; }
        }
        3600 // default: 1 hour
    }
}

// ─── JS Parser Cache ─────────────────────────────────────────────────────────

const JS_CACHE_MAX_ENTRIES: usize = 32;
const JS_CACHE_MAX_SCRIPT_BYTES: usize = 2 * 1024 * 1024; // 2 MiB per script

/// Cached parsed-script record.
///
/// We store the raw source text; in the future this would hold a
/// pre-tokenised bytecode blob.  For now it avoids re-reading the resource.
#[derive(Clone, Debug)]
pub struct JsCacheEntry {
    pub url:    String,
    pub source: String,
    pub hits:   u32,
}

#[derive(Default)]
pub struct JsParserCache {
    entries: Vec<JsCacheEntry>,
}

impl JsParserCache {
    pub fn new() -> Self { Self::default() }

    pub fn get(&mut self, url: &str) -> Option<&str> {
        if let Some(e) = self.entries.iter_mut().find(|e| e.url == url) {
            e.hits += 1;
            // return a reference to the source; safe because we borrow self mutably
            // but we need to re-borrow immutably:
        }
        self.entries.iter().find(|e| e.url == url).map(|e| e.source.as_str())
    }

    pub fn insert(&mut self, url: String, source: String) {
        if source.len() > JS_CACHE_MAX_SCRIPT_BYTES { return; }
        if self.entries.iter().any(|e| e.url == url) { return; }
        if self.entries.len() >= JS_CACHE_MAX_ENTRIES {
            // evict least-hit
            if let Some(pos) = self.entries.iter().enumerate().min_by_key(|(_, e)| e.hits).map(|(i, _)| i) {
                self.entries.remove(pos);
            }
        }
        self.entries.push(JsCacheEntry { url, source, hits: 0 });
    }

    pub fn invalidate(&mut self, url: &str) {
        self.entries.retain(|e| e.url != url);
    }

    pub fn clear(&mut self) { self.entries.clear(); }
    pub fn entry_count(&self) -> usize { self.entries.len() }
}

// ─── Image Decode Budget ─────────────────────────────────────────────────────

/// Maximum number of image bytes decoded per render frame (~2 ms budget).
const MAX_DECODE_BYTES_PER_FRAME: usize = 512 * 1024; // 512 KiB

/// Controls how much image data is decoded each render frame.
///
/// The compositor calls `begin_frame()` at the start of each frame and
/// `charge(bytes)` before decoding a resource.  If the budget is exhausted,
/// decoding is deferred to the next frame.
#[derive(Debug, Default)]
pub struct ImageDecodeQueue {
    budget_remaining: usize,
    /// Number of images decoded so far this frame.
    decoded_this_frame: u32,
    /// Total images ever decoded.
    total_decoded: u64,
}

impl ImageDecodeQueue {
    pub fn new() -> Self {
        ImageDecodeQueue {
            budget_remaining: MAX_DECODE_BYTES_PER_FRAME,
            decoded_this_frame: 0,
            total_decoded: 0,
        }
    }

    /// Reset budget at the start of each render frame.
    pub fn begin_frame(&mut self) {
        self.budget_remaining   = MAX_DECODE_BYTES_PER_FRAME;
        self.decoded_this_frame = 0;
    }

    /// Try to charge `bytes` against this frame's budget.
    /// Returns `true` if the decode may proceed; `false` if it must wait.
    pub fn charge(&mut self, bytes: usize) -> bool {
        if bytes > self.budget_remaining { return false; }
        self.budget_remaining   -= bytes;
        self.decoded_this_frame += 1;
        self.total_decoded      += 1;
        true
    }

    pub fn budget_remaining(&self) -> usize { self.budget_remaining }
    pub fn decoded_this_frame(&self) -> u32  { self.decoded_this_frame }
    pub fn total_decoded(&self) -> u64        { self.total_decoded }
}

// ─── Speculative Pre-connect hints ──────────────────────────────────────────

/// DNS + TCP pre-connect targets found in `<link rel="preconnect">` or
/// `<link rel="dns-prefetch">` tags.
#[derive(Debug, Clone)]
pub struct PreconnectHint {
    pub host:  String,
    pub port:  u16,
    pub tls:   bool,
}

#[derive(Debug, Default)]
pub struct PreconnectQueue {
    hints:   Vec<PreconnectHint>,
    resolved: Vec<String>,  // hosts already resolved / connected
}

impl PreconnectQueue {
    pub fn new() -> Self { Self::default() }

    pub fn add(&mut self, host: String, port: u16, tls: bool) {
        if !self.hints.iter().any(|h| h.host == host && h.port == port) {
            self.hints.push(PreconnectHint { host, port, tls });
        }
    }

    pub fn mark_done(&mut self, host: &str) {
        self.hints.retain(|h| h.host != host);
        if !self.resolved.contains(&host.to_string()) {
            self.resolved.push(host.to_string());
        }
    }

    pub fn pending(&self) -> &[PreconnectHint] { &self.hints }
    pub fn is_resolved(&self, host: &str) -> bool { self.resolved.contains(&host.to_string()) }
}

// ─── Perf Stats ──────────────────────────────────────────────────────────────

/// Accumulated performance counters for one page load.
#[derive(Debug, Default, Clone)]
pub struct PerfStats {
    pub css_rules_total:       u32,
    pub css_rules_tested:      u64,  // without index
    pub css_rules_tested_idx:  u64,  // with index (should be much lower)
    pub layout_passes:         u32,
    pub layout_skipped_nodes:  u64,
    pub paint_list_hits:       u32,  // display list reuse
    pub paint_list_misses:     u32,
    pub http_cache_hits:       u32,
    pub http_cache_misses:     u32,
    pub js_cache_hits:         u32,
    pub js_cache_misses:       u32,
    pub images_decoded:        u32,
    pub images_deferred:       u32,  // budget exhausted → next frame
}

impl PerfStats {
    pub fn new() -> Self { Self::default() }
    pub fn reset(&mut self) { *self = Self::default(); }

    pub fn css_speedup(&self) -> f64 {
        if self.css_rules_tested == 0 { return 1.0; }
        self.css_rules_tested as f64 / self.css_rules_tested_idx.max(1) as f64
    }

    pub fn cache_hit_rate(&self) -> f64 {
        let total = (self.http_cache_hits + self.http_cache_misses) as f64;
        if total == 0.0 { return 0.0; }
        self.http_cache_hits as f64 / total
    }

    pub fn summary(&self) -> alloc::string::String {
        format!(
            "CSS: {}/{} rules tested ({:.1}× speedup) | layout passes: {} skipped {} | \
             cache hit: {:.0}% | display list reuse: {}/{} | img decoded: {} deferred: {}",
            self.css_rules_tested_idx, self.css_rules_tested,
            self.css_speedup(),
            self.layout_passes, self.layout_skipped_nodes,
            self.cache_hit_rate() * 100.0,
            self.paint_list_hits, self.paint_list_hits + self.paint_list_misses,
            self.images_decoded, self.images_deferred,
        )
    }
}

// ─── Page Performance Context ─────────────────────────────────────────────────

/// All performance machinery bundled for one browser tab.
pub struct TabPerfContext {
    pub rule_index:   CssRuleIndex,
    pub dirty:        DirtySet,
    pub dl_cache:     DisplayListCache,
    pub http_cache:   HttpCache,
    pub js_cache:     JsParserCache,
    pub img_budget:   ImageDecodeQueue,
    pub preconnect:   PreconnectQueue,
    pub stats:        PerfStats,
}

impl TabPerfContext {
    pub fn new() -> Self {
        TabPerfContext {
            rule_index: CssRuleIndex::new(),
            dirty:      DirtySet::new(),
            dl_cache:   DisplayListCache::new(),
            http_cache: HttpCache::new(),
            js_cache:   JsParserCache::new(),
            img_budget: ImageDecodeQueue::new(),
            preconnect: PreconnectQueue::new(),
            stats:      PerfStats::new(),
        }
    }

    /// Call at the start of each navigation to reset transient state.
    pub fn on_navigate(&mut self) {
        self.dirty.mark_all();
        self.dl_cache.invalidate();
        self.js_cache.clear();
        self.preconnect = PreconnectQueue::new();
        self.stats.reset();
    }

    /// Call at the start of each render frame.
    pub fn begin_frame(&mut self) {
        self.img_budget.begin_frame();
        advance_cache_tick();
    }
}

impl Default for TabPerfContext {
    fn default() -> Self { Self::new() }
}

// ─── Self-test ───────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // ── T1: CSS Rule Index ────────────────────────────────────────────────────
    let mut idx = CssRuleIndex::new();
    // Rule 0: p { color: red }
    idx.insert(0, Some("p"), None, None);
    // Rule 1: .highlight { background: yellow }
    idx.insert(1, None, Some("highlight"), None);
    // Rule 2: #header { font-size: 2em }
    idx.insert(2, None, None, Some("header"));
    // Rule 3: * { box-sizing: border-box }
    idx.insert(3, None, None, None);

    let h = idx.candidates("p", &["highlight"], Some("header"));
    // Should contain all four rules
    ok &= h.ids.contains(&0);
    ok &= h.ids.contains(&1);
    ok &= h.ids.contains(&2);
    ok &= h.ids.contains(&3);

    let h2 = idx.candidates("div", &[], None);
    // Only the universal rule
    ok &= !h2.ids.contains(&0);  // tag bucket "p" not queried
    ok &= !h2.ids.contains(&1);  // class bucket not queried
    ok &= h2.ids.contains(&3);   // universal always present

    // ── T2: Dirty set ─────────────────────────────────────────────────────────
    let mut dirty = DirtySet::new();
    ok &= dirty.is_empty();
    dirty.mark_dirty(42);
    ok &= dirty.is_dirty(42);
    ok &= !dirty.is_dirty(99);
    dirty.mark_subtree(10);
    ok &= dirty.is_dirty(10);
    let drained = dirty.take_dirty();
    ok &= drained.contains(&42);
    ok &= dirty.is_empty();

    dirty.mark_all();
    ok &= dirty.needs_full_layout();
    ok &= dirty.is_dirty(999_999); // everything is dirty
    dirty.take_dirty();

    // ── T3: Display-list cache ────────────────────────────────────────────────
    let mut dlc = DisplayListCache::new();
    ok &= dlc.is_dirty(); // starts dirty
    dlc.store(vec![CachedPaintItem::PopClip]);
    ok &= !dlc.is_dirty();
    dlc.invalidate();
    ok &= dlc.is_dirty();
    ok &= dlc.layout_gen == 1;

    // ── T4: HTTP cache ────────────────────────────────────────────────────────
    let mut cache = HttpCache::new();
    let entry = CacheEntry {
        url: "https://example.com/style.css".to_string(),
        data: vec![0u8; 256],
        mime: "text/css".to_string(),
        etag: Some("\"abc123\"".to_string()),
        last_modified: None,
        max_age_secs: 3600,
        inserted_at: cache_tick(),
        last_used: cache_tick(),
    };
    cache.insert(entry);
    ok &= cache.entry_count() == 1;
    ok &= cache.total_bytes() == 256;
    ok &= cache.get("https://example.com/style.css").is_some();
    ok &= cache.get("https://other.com/foo.js").is_none();

    // eviction: fill to capacity, verify old entries dropped
    for i in 0..HTTP_CACHE_MAX_ENTRIES + 2 {
        let e = CacheEntry {
            url: format!("https://cdn.test/{}", i),
            data: vec![0u8; 1024],
            mime: "text/plain".to_string(),
            etag: None,
            last_modified: None,
            max_age_secs: 0,
            inserted_at: 0,
            last_used: 0,
        };
        cache.insert(e);
    }
    ok &= cache.entry_count() <= HTTP_CACHE_MAX_ENTRIES;

    // clear
    cache.clear();
    ok &= cache.entry_count() == 0;

    // parse max-age
    ok &= HttpCache::parse_max_age("max-age=300") == 300;
    ok &= HttpCache::parse_max_age("no-cache") == 0;
    ok &= HttpCache::parse_max_age("public, max-age=86400") == 86400;

    // conditional headers
    let e2 = CacheEntry {
        url: "https://ex.com/img.png".to_string(),
        data: vec![1, 2, 3],
        mime: "image/png".to_string(),
        etag: Some("\"etag42\"".to_string()),
        last_modified: Some("Mon, 01 Jan 2024 00:00:00 GMT".to_string()),
        max_age_secs: 3600,
        inserted_at: 0,
        last_used: 0,
    };
    cache.insert(e2);
    let (inm, ims) = cache.conditional_headers("https://ex.com/img.png");
    ok &= inm.is_some();
    ok &= ims.is_some();

    // ── T5: JS cache ─────────────────────────────────────────────────────────
    let mut jsc = JsParserCache::new();
    jsc.insert("https://cdn.test/app.js".to_string(), "console.log(1)".to_string());
    ok &= jsc.entry_count() == 1;
    ok &= jsc.get("https://cdn.test/app.js").is_some();
    ok &= jsc.get("https://other.com/x.js").is_none();
    jsc.invalidate("https://cdn.test/app.js");
    ok &= jsc.entry_count() == 0;

    // eviction at capacity
    for i in 0..JS_CACHE_MAX_ENTRIES + 2 {
        jsc.insert(format!("https://cdn.test/{}.js", i), format!("var x={};", i));
    }
    ok &= jsc.entry_count() <= JS_CACHE_MAX_ENTRIES;

    // ── T6: Image decode budget ───────────────────────────────────────────────
    let mut budget = ImageDecodeQueue::new();
    ok &= budget.charge(100_000);  // within budget
    ok &= !budget.charge(MAX_DECODE_BYTES_PER_FRAME); // exceeds remaining
    budget.begin_frame();
    ok &= budget.charge(MAX_DECODE_BYTES_PER_FRAME); // reset → full budget
    ok &= !budget.charge(1); // exhausted
    ok &= budget.decoded_this_frame() == 1;

    // ── T7: Pre-connect queue ─────────────────────────────────────────────────
    let mut pc = PreconnectQueue::new();
    pc.add("cdn.example.com".to_string(), 443, true);
    pc.add("cdn.example.com".to_string(), 443, true); // deduplicated
    ok &= pc.pending().len() == 1;
    pc.mark_done("cdn.example.com");
    ok &= pc.pending().is_empty();
    ok &= pc.is_resolved("cdn.example.com");

    // ── T8: PerfStats ─────────────────────────────────────────────────────────
    let mut ps = PerfStats::new();
    ps.css_rules_total = 100;
    ps.css_rules_tested = 10_000;
    ps.css_rules_tested_idx = 500;
    let speedup = ps.css_speedup();
    ok &= speedup > 15.0 && speedup < 25.0; // 10000/500 = 20×

    ps.http_cache_hits   = 80;
    ps.http_cache_misses = 20;
    let hit_rate = ps.cache_hit_rate();
    ok &= hit_rate > 0.79 && hit_rate < 0.81;

    let summary = ps.summary();
    ok &= !summary.is_empty();

    // ── T9: TabPerfContext ────────────────────────────────────────────────────
    let mut ctx = TabPerfContext::new();
    ctx.begin_frame();
    ctx.on_navigate();
    ok &= ctx.dirty.needs_full_layout();

    if ok {
        crate::serial_println!("[perf] Phase 95: all 9 performance-pass tests PASSED");
    } else {
        crate::serial_println!("[perf] Phase 95: FAILED");
    }
    ok
}

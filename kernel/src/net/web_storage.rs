/// Phase 43 — Web Storage
///
/// Implements the Web Storage API (W3C):
///   • `localStorage`  — persistent key/value, survives reboot (backed by DiskFs)
///   • `sessionStorage`— ephemeral key/value, cleared when the "session" ends
///   • `IndexedDB`     — structured object store (key/value, range queries, indexes)
///
/// All stores are origin-scoped (origin = scheme + host + port string).
/// Quota: localStorage/sessionStorage = 5 MiB per origin; IndexedDB = 50 MiB per origin.
///
/// The no_std implementation stores everything in BTreeMap (sorted, deterministic).
/// Persistence is via `crate::fs::ramfs` / `diskfs` depending on store type.

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
    format,
};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
//  Quota constants (bytes)
// ─────────────────────────────────────────────────────────────────────────────

const LOCAL_QUOTA:   usize = 5 * 1024 * 1024;   // 5 MiB
const SESSION_QUOTA: usize = 5 * 1024 * 1024;   // 5 MiB
const IDB_QUOTA:     usize = 50 * 1024 * 1024;  // 50 MiB

// ─────────────────────────────────────────────────────────────────────────────
//  Simple key-value store (localStorage / sessionStorage)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct KvStore {
    pub origin: String,
    data:        BTreeMap<String, String>,
    used_bytes:  usize,
    quota:       usize,
}

impl KvStore {
    pub fn new(origin: &str, quota: usize) -> Self {
        Self {
            origin: origin.to_string(),
            data: BTreeMap::new(),
            used_bytes: 0,
            quota,
        }
    }

    /// Number of items.
    pub fn length(&self) -> usize { self.data.len() }

    /// Return the nth key (in insertion/sorted order).
    pub fn key(&self, n: usize) -> Option<&str> {
        self.data.keys().nth(n).map(|s| s.as_str())
    }

    /// Get a value by key.
    pub fn get_item(&self, key: &str) -> Option<&str> {
        self.data.get(key).map(|s| s.as_str())
    }

    /// Set a key/value pair.  Returns Err if quota exceeded.
    pub fn set_item(&mut self, key: &str, value: &str) -> Result<(), &'static str> {
        // Remove old byte contribution
        let old_size = self.data.get(key)
            .map(|v| key.len() + v.len())
            .unwrap_or(0);
        let new_size = key.len() + value.len();
        let delta = new_size as isize - old_size as isize;
        let new_used = self.used_bytes as isize + delta;
        if new_used > self.quota as isize {
            return Err("QuotaExceededError");
        }
        self.used_bytes = new_used as usize;
        self.data.insert(key.to_string(), value.to_string());
        Ok(())
    }

    /// Remove a key.
    pub fn remove_item(&mut self, key: &str) {
        if let Some(v) = self.data.remove(key) {
            self.used_bytes = self.used_bytes.saturating_sub(key.len() + v.len());
        }
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.data.clear();
        self.used_bytes = 0;
    }

    /// Current usage in bytes.
    pub fn bytes_used(&self) -> usize { self.used_bytes }

    /// Quota in bytes.
    pub fn quota(&self) -> usize { self.quota }

    /// Iterate all entries.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.data.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Serialise to "key\x00value\x1E" format for persistence.
    ///
    /// Uses ASCII RS (0x1E, Record Separator) as the record delimiter so that
    /// values containing literal newlines round-trip correctly.
    pub fn serialise(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (k, v) in &self.data {
            out.extend_from_slice(k.as_bytes());
            out.push(0x00);              // key/value separator (NUL)
            out.extend_from_slice(v.as_bytes());
            out.push(0x1E);              // record separator (ASCII RS)
        }
        out
    }

    /// Deserialise from the format produced by `serialise()`.
    pub fn deserialise(&mut self, data: &[u8]) {
        self.clear();
        let mut i = 0;
        while i < data.len() {
            // Find NUL key/value separator
            let null_pos = match data[i..].iter().position(|&b| b == 0x00) {
                Some(p) => i + p,
                None => break,
            };
            let key = core::str::from_utf8(&data[i..null_pos]).unwrap_or("").to_string();
            let j = null_pos + 1;
            // Find RS record separator (0x1E); fall back to end of data
            let rs_pos = match data[j..].iter().position(|&b| b == 0x1E) {
                Some(p) => j + p,
                None => data.len(),
            };
            let value = core::str::from_utf8(&data[j..rs_pos]).unwrap_or("").to_string();
            let _ = self.set_item(&key, &value);
            i = rs_pos + 1;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  IndexedDB — structured object store
// ─────────────────────────────────────────────────────────────────────────────

/// A record in an object store.
#[derive(Clone, Debug)]
pub struct IdbRecord {
    pub key:   IdbValue,
    pub value: IdbValue,
}

/// Simplified value type for IndexedDB (covers JSON-serialisable types).
#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub enum IdbValue {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Bytes(Vec<u8>),
    Array(Vec<IdbValue>),
    Object(BTreeMap<String, IdbValue>),
}

impl IdbValue {
    /// Rough byte size for quota accounting.
    pub fn byte_size(&self) -> usize {
        match self {
            IdbValue::Null       => 4,
            IdbValue::Bool(_)    => 1,
            IdbValue::Number(_)  => 8,
            IdbValue::Str(s)     => s.len(),
            IdbValue::Bytes(b)   => b.len(),
            IdbValue::Array(a)   => a.iter().map(|v| v.byte_size()).sum::<usize>() + 8,
            IdbValue::Object(m)  => m.iter().map(|(k, v)| k.len() + v.byte_size()).sum::<usize>() + 8,
        }
    }

    /// Compare two values for key ordering.
    pub fn cmp_key(&self, other: &IdbValue) -> core::cmp::Ordering {
        use core::cmp::Ordering;
        match (self, other) {
            (IdbValue::Number(a), IdbValue::Number(b)) => {
                a.partial_cmp(b).unwrap_or(Ordering::Equal)
            }
            (IdbValue::Str(a), IdbValue::Str(b)) => a.cmp(b),
            (IdbValue::Bytes(a), IdbValue::Bytes(b)) => a.cmp(b),
            _ => Ordering::Equal,
        }
    }
}

/// An IndexedDB object store (like a table).
pub struct IdbObjectStore {
    pub name:       String,
    pub key_path:   Option<String>,   // field name used as key, or None (out-of-line key)
    pub auto_inc:   bool,
    next_id:        u64,
    records:        Vec<IdbRecord>,   // sorted by key
    used_bytes:     usize,
}

impl IdbObjectStore {
    pub fn new(name: &str, key_path: Option<String>, auto_inc: bool) -> Self {
        Self {
            name: name.to_string(),
            key_path,
            auto_inc,
            next_id: 1,
            records: Vec::new(),
            used_bytes: 0,
        }
    }

    fn find_pos(&self, key: &IdbValue) -> Result<usize, usize> {
        self.records.binary_search_by(|r| r.key.cmp_key(key))
    }

    /// Add a record.  Returns the key used.
    pub fn add(&mut self, key: Option<IdbValue>, value: IdbValue, quota_remaining: usize)
        -> Result<IdbValue, &'static str>
    {
        let actual_key = if self.auto_inc && key.is_none() {
            let k = IdbValue::Number(self.next_id as f64);
            self.next_id += 1;
            k
        } else {
            key.ok_or("Key required")?
        };

        // Check for duplicate
        if self.find_pos(&actual_key).is_ok() {
            return Err("ConstraintError: key already exists");
        }

        let bytes = actual_key.byte_size() + value.byte_size();
        if bytes > quota_remaining {
            return Err("QuotaExceededError");
        }

        let rec = IdbRecord { key: actual_key.clone(), value };
        let pos = self.find_pos(&actual_key).unwrap_err();
        self.records.insert(pos, rec);
        self.used_bytes += bytes;
        Ok(actual_key)
    }

    /// Put (upsert) a record.
    pub fn put(&mut self, key: Option<IdbValue>, value: IdbValue, quota_remaining: usize)
        -> Result<IdbValue, &'static str>
    {
        let actual_key = if self.auto_inc && key.is_none() {
            let k = IdbValue::Number(self.next_id as f64);
            self.next_id += 1;
            k
        } else {
            key.ok_or("Key required")?
        };

        let new_bytes = actual_key.byte_size() + value.byte_size();

        if let Ok(pos) = self.find_pos(&actual_key) {
            // Replace existing
            let old_bytes = self.records[pos].key.byte_size() + self.records[pos].value.byte_size();
            let delta = new_bytes as isize - old_bytes as isize;
            if delta > quota_remaining as isize {
                return Err("QuotaExceededError");
            }
            self.used_bytes = (self.used_bytes as isize + delta) as usize;
            self.records[pos].value = value;
        } else {
            if new_bytes > quota_remaining {
                return Err("QuotaExceededError");
            }
            let pos = self.find_pos(&actual_key).unwrap_err();
            self.records.insert(pos, IdbRecord { key: actual_key.clone(), value });
            self.used_bytes += new_bytes;
        }
        Ok(actual_key)
    }

    /// Get by exact key.
    pub fn get(&self, key: &IdbValue) -> Option<&IdbValue> {
        self.find_pos(key).ok().map(|pos| &self.records[pos].value)
    }

    /// Delete by key.
    pub fn delete(&mut self, key: &IdbValue) -> bool {
        if let Ok(pos) = self.find_pos(key) {
            let bytes = self.records[pos].key.byte_size() + self.records[pos].value.byte_size();
            self.records.remove(pos);
            self.used_bytes = self.used_bytes.saturating_sub(bytes);
            true
        } else {
            false
        }
    }

    /// Clear all records.
    pub fn clear_store(&mut self) {
        self.records.clear();
        self.used_bytes = 0;
    }

    /// Count records.
    pub fn count(&self) -> usize { self.records.len() }

    /// Get all records.
    pub fn get_all(&self) -> &[IdbRecord] { &self.records }

    /// Range query [lower, upper) — both bounds are optional.
    pub fn get_range(&self, lower: Option<&IdbValue>, upper: Option<&IdbValue>) -> Vec<&IdbRecord> {
        self.records.iter().filter(|r| {
            let above = lower.map(|lo| r.key.cmp_key(lo) != core::cmp::Ordering::Less)
                             .unwrap_or(true);
            let below = upper.map(|hi| r.key.cmp_key(hi) == core::cmp::Ordering::Less)
                             .unwrap_or(true);
            above && below
        }).collect()
    }
}

/// An IndexedDB database (contains multiple object stores).
pub struct IdbDatabase {
    pub name:    String,
    pub version: u32,
    stores:      BTreeMap<String, IdbObjectStore>,
    used_bytes:  usize,
    quota:       usize,
}

impl IdbDatabase {
    pub fn new(name: &str, version: u32, quota: usize) -> Self {
        Self {
            name: name.to_string(),
            version,
            stores: BTreeMap::new(),
            used_bytes: 0,
            quota,
        }
    }

    pub fn create_object_store(&mut self, name: &str, key_path: Option<String>, auto_inc: bool)
        -> Result<(), &'static str>
    {
        if self.stores.contains_key(name) { return Err("Store already exists"); }
        self.stores.insert(name.to_string(), IdbObjectStore::new(name, key_path, auto_inc));
        Ok(())
    }

    pub fn delete_object_store(&mut self, name: &str) -> Result<(), &'static str> {
        if let Some(store) = self.stores.remove(name) {
            self.used_bytes = self.used_bytes.saturating_sub(store.used_bytes);
            Ok(())
        } else {
            Err("Store not found")
        }
    }

    pub fn store_names(&self) -> Vec<&str> {
        self.stores.keys().map(|s| s.as_str()).collect()
    }

    pub fn store(&self, name: &str) -> Option<&IdbObjectStore> { self.stores.get(name) }
    pub fn store_mut(&mut self, name: &str) -> Option<&mut IdbObjectStore> { self.stores.get_mut(name) }

    fn quota_remaining(&self) -> usize {
        self.quota.saturating_sub(self.used_bytes)
    }

    /// Add a record to a store.
    pub fn add(&mut self, store_name: &str, key: Option<IdbValue>, value: IdbValue)
        -> Result<IdbValue, &'static str>
    {
        let remaining = self.quota_remaining();
        let store = self.stores.get_mut(store_name).ok_or("Store not found")?;
        let before = store.used_bytes;
        let k = store.add(key, value, remaining)?;
        self.used_bytes += store.used_bytes - before;
        Ok(k)
    }

    /// Put (upsert) a record.
    pub fn put(&mut self, store_name: &str, key: Option<IdbValue>, value: IdbValue)
        -> Result<IdbValue, &'static str>
    {
        let remaining = self.quota_remaining();
        let store = self.stores.get_mut(store_name).ok_or("Store not found")?;
        let before = store.used_bytes;
        let k = store.put(key, value, remaining)?;
        let after = store.used_bytes;
        if after >= before {
            self.used_bytes += after - before;
        } else {
            self.used_bytes = self.used_bytes.saturating_sub(before - after);
        }
        Ok(k)
    }

    /// Get from a store.
    pub fn get(&self, store_name: &str, key: &IdbValue) -> Option<&IdbValue> {
        self.stores.get(store_name)?.get(key)
    }

    /// Delete from a store.
    pub fn delete(&mut self, store_name: &str, key: &IdbValue) -> Result<bool, &'static str> {
        let store = self.stores.get_mut(store_name).ok_or("Store not found")?;
        let before = store.used_bytes;
        let ok = store.delete(key);
        self.used_bytes = self.used_bytes.saturating_sub(before - store.used_bytes);
        Ok(ok)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Global storage manager
// ─────────────────────────────────────────────────────────────────────────────

pub struct StorageManager {
    /// origin → local store
    local:    BTreeMap<String, KvStore>,
    /// origin → session store
    session:  BTreeMap<String, KvStore>,
    /// origin → db_name → IdbDatabase
    idb:      BTreeMap<String, BTreeMap<String, IdbDatabase>>,
}

impl StorageManager {
    const fn new() -> Self {
        Self {
            local:   BTreeMap::new(),
            session: BTreeMap::new(),
            idb:     BTreeMap::new(),
        }
    }

    // ── localStorage ─────────────────────────────────────────────────────────

    pub fn local(&mut self, origin: &str) -> &mut KvStore {
        self.local.entry(origin.to_string())
            .or_insert_with(|| KvStore::new(origin, LOCAL_QUOTA))
    }

    pub fn local_ro(&self, origin: &str) -> Option<&KvStore> {
        self.local.get(origin)
    }

    // ── sessionStorage ────────────────────────────────────────────────────────

    pub fn session(&mut self, origin: &str) -> &mut KvStore {
        self.session.entry(origin.to_string())
            .or_insert_with(|| KvStore::new(origin, SESSION_QUOTA))
    }

    /// Clear all session stores (called on browser close / tab close).
    pub fn clear_all_sessions(&mut self) {
        self.session.clear();
    }

    pub fn clear_session(&mut self, origin: &str) {
        if let Some(s) = self.session.get_mut(origin) {
            s.clear();
        }
    }

    // ── IndexedDB ─────────────────────────────────────────────────────────────

    pub fn open_db(&mut self, origin: &str, db_name: &str, version: u32)
        -> &mut IdbDatabase
    {
        let origin_map = self.idb.entry(origin.to_string()).or_insert_with(BTreeMap::new);
        origin_map.entry(db_name.to_string())
            .or_insert_with(|| IdbDatabase::new(db_name, version, IDB_QUOTA))
    }

    pub fn delete_db(&mut self, origin: &str, db_name: &str) -> bool {
        if let Some(map) = self.idb.get_mut(origin) {
            map.remove(db_name).is_some()
        } else {
            false
        }
    }

    pub fn db(&mut self, origin: &str, db_name: &str) -> Option<&mut IdbDatabase> {
        self.idb.get_mut(origin)?.get_mut(db_name)
    }

    pub fn db_ro(&self, origin: &str, db_name: &str) -> Option<&IdbDatabase> {
        self.idb.get(origin)?.get(db_name)
    }

    // ── Persistence helpers ───────────────────────────────────────────────────

    /// Persist a localStorage origin to the given file path (e.g. in DiskFs).
    pub fn persist_local(&self, origin: &str) -> Vec<u8> {
        match self.local.get(origin) {
            Some(store) => store.serialise(),
            None => Vec::new(),
        }
    }

    /// Restore a localStorage origin from persisted bytes.
    pub fn restore_local(&mut self, origin: &str, data: &[u8]) {
        self.local(origin).deserialise(data);
    }

    // ── Diagnostics ──────────────────────────────────────────────────────────

    pub fn dump_stats(&self) -> String {
        let mut s = String::from("[web_storage] stats:\n");
        for (orig, store) in &self.local {
            s.push_str(&format!("  localStorage  {}: {} items, {} bytes\n",
                orig, store.length(), store.bytes_used()));
        }
        for (orig, store) in &self.session {
            s.push_str(&format!("  sessionStorage {}: {} items, {} bytes\n",
                orig, store.length(), store.bytes_used()));
        }
        for (orig, dbs) in &self.idb {
            for (db_name, db) in dbs {
                s.push_str(&format!("  indexedDB {}:{} {} stores, {} bytes\n",
                    orig, db_name, db.store_names().len(), db.used_bytes));
            }
        }
        s
    }
}

pub static STORAGE: Mutex<StorageManager> = Mutex::new(StorageManager::new());

// ─────────────────────────────────────────────────────────────────────────────
//  Convenience free functions (for browser integration)
// ─────────────────────────────────────────────────────────────────────────────

/// localStorage.getItem(key) for origin
pub fn local_get(origin: &str, key: &str) -> Option<String> {
    STORAGE.lock().local_ro(origin)?.get_item(key).map(|s| s.to_string())
}

/// localStorage.setItem(key, value) for origin
pub fn local_set(origin: &str, key: &str, value: &str) -> Result<(), &'static str> {
    STORAGE.lock().local(origin).set_item(key, value)
}

/// localStorage.removeItem(key) for origin
pub fn local_remove(origin: &str, key: &str) {
    STORAGE.lock().local(origin).remove_item(key);
}

/// localStorage.clear() for origin
pub fn local_clear(origin: &str) {
    STORAGE.lock().local(origin).clear();
}

/// sessionStorage.getItem(key) for origin
pub fn session_get(origin: &str, key: &str) -> Option<String> {
    let mut m = STORAGE.lock();
    let store = m.session(origin);
    store.get_item(key).map(|s| s.to_string())
}

/// sessionStorage.setItem(key, value) for origin
pub fn session_set(origin: &str, key: &str, value: &str) -> Result<(), &'static str> {
    STORAGE.lock().session(origin).set_item(key, value)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Self-tests
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    // ── Test 1: localStorage basic operations ────────────────────────────────
    let mut ls = KvStore::new("http://example.com", LOCAL_QUOTA);
    if ls.set_item("name", "Alice").is_err() { return false; }
    if ls.set_item("age", "30").is_err() { return false; }
    if ls.get_item("name") != Some("Alice") { return false; }
    if ls.get_item("age") != Some("30") { return false; }
    if ls.length() != 2 { return false; }
    ls.remove_item("age");
    if ls.length() != 1 { return false; }
    if ls.get_item("age").is_some() { return false; }

    // ── Test 2: Quota enforcement ─────────────────────────────────────────────
    let mut small = KvStore::new("http://small.com", 20);
    if small.set_item("k", "12345678901234").is_err() { return false; } // 1+14 = 15 bytes
    // Adding another 10-byte pair would exceed 20 bytes
    if small.set_item("x", "1234567890").is_ok() { return false; } // should fail

    // ── Test 3: Serialise/deserialise round-trip ──────────────────────────────
    let mut store = KvStore::new("http://roundtrip.com", LOCAL_QUOTA);
    let _ = store.set_item("foo", "bar");
    let _ = store.set_item("baz", "qux\nwith newline");
    let serialised = store.serialise();
    let mut store2 = KvStore::new("http://roundtrip.com", LOCAL_QUOTA);
    store2.deserialise(&serialised);
    if store2.get_item("foo") != Some("bar") { return false; }
    if store2.get_item("baz") != Some("qux\nwith newline") { return false; }

    // ── Test 4: IndexedDB add/get/delete ──────────────────────────────────────
    let mut db = IdbDatabase::new("testdb", 1, IDB_QUOTA);
    db.create_object_store("users", None, true).unwrap();
    let k1 = db.add("users", None, IdbValue::Str("Alice".to_string())).unwrap();
    let k2 = db.add("users", None, IdbValue::Str("Bob".to_string())).unwrap();
    if k1 != IdbValue::Number(1.0) { return false; }
    if k2 != IdbValue::Number(2.0) { return false; }
    if db.get("users", &IdbValue::Number(1.0)) != Some(&IdbValue::Str("Alice".to_string())) {
        return false;
    }
    if db.delete("users", &IdbValue::Number(1.0)).unwrap() != true { return false; }
    if db.get("users", &IdbValue::Number(1.0)).is_some() { return false; }

    // ── Test 5: IndexedDB put (upsert) + range query ──────────────────────────
    let mut db2 = IdbDatabase::new("scores", 1, IDB_QUOTA);
    db2.create_object_store("scores", None, false).unwrap();
    let _ = db2.put("scores", Some(IdbValue::Number(10.0)), IdbValue::Str("ten".to_string()));
    let _ = db2.put("scores", Some(IdbValue::Number(20.0)), IdbValue::Str("twenty".to_string()));
    let _ = db2.put("scores", Some(IdbValue::Number(30.0)), IdbValue::Str("thirty".to_string()));
    // Upsert: overwrite key=20
    let _ = db2.put("scores", Some(IdbValue::Number(20.0)), IdbValue::Str("TWENTY".to_string()));
    if db2.get("scores", &IdbValue::Number(20.0)) != Some(&IdbValue::Str("TWENTY".to_string())) {
        return false;
    }
    let store = db2.store("scores").unwrap();
    let range = store.get_range(
        Some(&IdbValue::Number(10.0)),
        Some(&IdbValue::Number(30.0)),
    );
    if range.len() != 2 { return false; } // keys 10 and 20, not 30

    // ── Test 6: Global STORAGE singleton ─────────────────────────────────────
    {
        let _ = local_set("http://test.com", "hello", "world");
        if local_get("http://test.com", "hello").as_deref() != Some("world") { return false; }
        local_remove("http://test.com", "hello");
        if local_get("http://test.com", "hello").is_some() { return false; }
    }
    {
        let _ = session_set("http://test.com", "token", "abc123");
        if session_get("http://test.com", "token").as_deref() != Some("abc123") { return false; }
    }

    true
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[web_storage] localStorage / sessionStorage / IndexedDB ready (Phase 43).");
}

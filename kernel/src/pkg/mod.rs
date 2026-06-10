/// Smart OS Package Manager — Phase 33.
///
/// `pkg install <name>` downloads a SmartPack-wrapped ELF from the package
/// registry, extracts it to /bin (or /usr/bin for system packages), and
/// registers it in the local package database at /var/pkg/installed.
///
/// Package format (SmartPack envelope):
///   { "name": str, "version": str, "arch": "x86_64",
///     "description": str, "files": [{ "path": str, "data": bin }] }
///
/// The package index lives at /var/pkg/index and is fetched via HTTPS
/// from the built-in registry URL on first use.

pub mod index;
pub mod install;
pub mod spk;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;

// ── Package metadata ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct PackageMeta {
    pub name:        String,
    pub version:     String,
    pub description: String,
    pub size_kb:     u32,
    pub installed:   bool,
}

// ── Installed package database ────────────────────────────────────────────────

static INSTALLED: Mutex<Vec<PackageMeta>> = Mutex::new(Vec::new());

/// Register a package as installed (called by install::run after extraction).
pub fn register_installed(meta: PackageMeta) {
    let mut db = INSTALLED.lock();
    // Replace if already present
    if let Some(existing) = db.iter_mut().find(|p| p.name == meta.name) {
        *existing = meta;
    } else {
        db.push(meta);
    }
    // Persist to VFS
    drop(db);
    persist_db();
}

/// Remove a package from the installed database.
pub fn unregister(name: &str) {
    INSTALLED.lock().retain(|p| p.name != name);
    persist_db();
}

/// List all installed packages.
pub fn list_installed() -> Vec<PackageMeta> {
    INSTALLED.lock().clone()
}

/// Check if a package is installed.
pub fn is_installed(name: &str) -> bool {
    INSTALLED.lock().iter().any(|p| p.name == name)
}

fn persist_db() {
    let db = INSTALLED.lock();
    let mut out = String::new();
    for pkg in db.iter() {
        out.push_str(&format!("{}={} # {}\n", pkg.name, pkg.version, pkg.description));
    }
    drop(db);
    let _ = crate::vfs::create_and_write("/var/pkg/installed", out.as_bytes());
}

fn load_db() {
    let data = match crate::vfs::read_file_full("/var/pkg/installed") {
        Ok(d)  => d,
        Err(_) => return,
    };
    let text = core::str::from_utf8(&data).unwrap_or("");
    let mut db = INSTALLED.lock();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        // Format: name=version # description
        let (kv, desc) = if let Some(idx) = line.find('#') {
            (&line[..idx], line[idx+1..].trim())
        } else {
            (line, "")
        };
        let (name, ver) = if let Some(idx) = kv.find('=') {
            (&kv[..idx], &kv[idx+1..])
        } else {
            (kv, "")
        };
        db.push(PackageMeta {
            name:        name.trim().to_string(),
            version:     ver.trim().to_string(),
            description: desc.to_string(),
            size_kb:     0,
            installed:   true,
        });
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Install a package by name. Returns Ok(()) on success.
pub fn pkg_install(name: &str) -> Result<(), &'static str> {
    if is_installed(name) {
        return Err("already installed");
    }
    // Look up in index
    let entry = index::find(name).ok_or("package not found in index")?;
    // Download + extract
    install::run(&entry)?;
    // Record in DB
    register_installed(PackageMeta {
        name:        name.to_string(),
        version:     entry.version.clone(),
        description: entry.description.clone(),
        size_kb:     entry.size_kb,
        installed:   true,
    });
    crate::serial_println!("[pkg] Installed {} v{}.", name, entry.version);
    Ok(())
}

/// Remove an installed package (deletes its files).
pub fn pkg_remove(name: &str) -> Result<(), &'static str> {
    if !is_installed(name) {
        return Err("not installed");
    }
    let entry = index::find(name).ok_or("package not in index")?;
    for file in &entry.files {
        crate::vfs::unlink(&file.install_path).ok();
    }
    unregister(name);
    crate::serial_println!("[pkg] Removed {}.", name);
    Ok(())
}

/// Search the package index for packages matching a query string.
pub fn pkg_search(query: &str) -> Vec<index::IndexEntry> {
    index::search(query)
}

/// Update the package index from the network.
pub fn pkg_update() -> Result<(), &'static str> {
    index::refresh()
}

/// Initialize the package manager: create dirs, load installed DB, load index.
pub fn init() {
    crate::vfs::mkdir("/var").ok();
    crate::vfs::mkdir("/var/pkg").ok();
    crate::vfs::mkdir("/var/pkg/cache").ok();
    crate::vfs::mkdir("/var/pkg/tmp").ok();
    crate::vfs::mkdir("/usr/bin").ok();
    crate::vfs::mkdir("/usr/share").ok();
    crate::vfs::mkdir("/usr/share/doc").ok();

    load_db();
    index::init();

    crate::serial_println!(
        "[pkg] Package manager ready ({} installed, {} in index).",
        INSTALLED.lock().len(),
        index::count(),
    );
}

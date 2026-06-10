/// ELF Dynamic Linker for Smart OS (ld.so equivalent).
///
/// Provides library resolution, GOT/PLT fixup stubs, and LD_LIBRARY_PATH
/// support so unmodified Linux ELF binaries can load shared libraries
/// from the VFS. In practice Smart OS runs static ELFs; this module
/// handles the metadata/cache layer that programs query at startup.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;

/// Standard library search path order (mirrors GNU ld.so).
const SEARCH_PATHS: &[&str] = &[
    "/lib",
    "/lib64",
    "/usr/lib",
    "/usr/lib64",
    "/usr/local/lib",
    "/usr/local/lib64",
];

/// Well-known library stubs provided by Smart OS (musl-compatible shims).
const BUILTIN_LIBS: &[(&str, &[u8])] = &[
    ("libc.so.6",          b"# Smart OS musl libc shim v0.12\n"),
    ("libc.so",            b"# Smart OS musl libc shim v0.12\n"),
    ("libm.so.6",          b"# Smart OS libm shim\n"),
    ("libm.so",            b"# Smart OS libm shim\n"),
    ("libdl.so.2",         b"# Smart OS libdl shim\n"),
    ("libdl.so",           b"# Smart OS libdl shim\n"),
    ("libpthread.so.0",    b"# Smart OS libpthread shim\n"),
    ("libpthread.so",      b"# Smart OS libpthread shim\n"),
    ("librt.so.1",         b"# Smart OS librt shim\n"),
    ("libgcc_s.so.1",      b"# Smart OS libgcc_s shim\n"),
    ("libstdc++.so.6",     b"# Smart OS libstdc++ shim\n"),
    ("ld-linux-x86-64.so.2", b"# Smart OS dynamic linker stub\n"),
    ("ld-musl-x86_64.so.1",  b"# Smart OS musl dynamic linker\n"),
];

// ── ld.so.cache format (glibc-compatible binary blob) ────────────────────────
//
// Programs like ldd and dlopen check /etc/ld.so.cache for fast library lookup.
// We write a simplified version: a small header + name→path pairs as text.
// Real glibc uses a binary format; most programs only check the path entries.

/// Build the /etc/ld.so.cache content (simplified text format).
fn build_ld_so_cache() -> Vec<u8> {
    let mut out = Vec::new();
    // Magic header that satisfies a quick check
    out.extend_from_slice(b"ld.so-1.7.0\0");
    // Pad to 16 bytes
    while out.len() < 16 { out.push(0); }

    for (name, _) in BUILTIN_LIBS {
        // Each entry: libname TAB path NEWLINE
        for base in SEARCH_PATHS {
            let line = format!("{}\t{}/{}\n", name, base, name);
            out.extend_from_slice(line.as_bytes());
        }
    }
    out
}

/// Populate /lib, /lib64, /usr/lib with all library stubs.
fn install_library_stubs() {
    for (name, content) in BUILTIN_LIBS {
        for base in SEARCH_PATHS {
            let path = format!("{}/{}", base, name);
            let _ = crate::vfs::create_and_write(&path, content);
        }
    }
    // Also place the linker in the canonical location
    let _ = crate::vfs::create_and_write(
        "/lib64/ld-linux-x86-64.so.2",
        b"# Smart OS dynamic linker stub\n",
    );
    let _ = crate::vfs::create_and_write(
        "/lib/ld-linux-x86-64.so.2",
        b"# Smart OS dynamic linker stub\n",
    );
}

/// Install /usr/bin/ldd and related linker utilities.
fn install_linker_tools() {
    // ldd: prints shared library dependencies (we always say "statically linked")
    let ldd_script = b"#!/bin/sh\necho 'statically linked'\n";
    let _ = crate::vfs::create_and_write("/usr/bin/ldd", ldd_script);

    // ldconfig: rebuilds ld.so.cache (we're a no-op since cache is static)
    let ldconfig_script = b"#!/bin/sh\necho 'ldconfig: Smart OS ld.so.cache is pre-built'\n";
    let _ = crate::vfs::create_and_write("/sbin/ldconfig", ldconfig_script);
    let _ = crate::vfs::create_and_write("/usr/sbin/ldconfig", ldconfig_script);
}

// ── GOT/PLT stub table ────────────────────────────────────────────────────────
//
// For statically-linked ELFs there is no GOT/PLT patching to do at runtime.
// For dynamic ELFs (rare in our environment) we keep a stub table so that
// dlopen/dlsym queries return sensible values rather than panicking.

/// A resolved symbol entry: name → kernel address or 0 if unimplemented.
pub struct SymbolEntry {
    pub name: &'static str,
    pub addr: u64,   // 0 = "exists but not callable from user space"
}

/// Minimal symbol table for the most common libc symbols.
/// Programs that call dlsym("RTLD_DEFAULT", "malloc") etc. get these back.
static SYMBOL_TABLE: &[SymbolEntry] = &[
    SymbolEntry { name: "malloc",        addr: 0 },
    SymbolEntry { name: "free",          addr: 0 },
    SymbolEntry { name: "realloc",       addr: 0 },
    SymbolEntry { name: "calloc",        addr: 0 },
    SymbolEntry { name: "memcpy",        addr: 0 },
    SymbolEntry { name: "memset",        addr: 0 },
    SymbolEntry { name: "memmove",       addr: 0 },
    SymbolEntry { name: "strlen",        addr: 0 },
    SymbolEntry { name: "strcpy",        addr: 0 },
    SymbolEntry { name: "strncpy",       addr: 0 },
    SymbolEntry { name: "strcmp",        addr: 0 },
    SymbolEntry { name: "strncmp",       addr: 0 },
    SymbolEntry { name: "printf",        addr: 0 },
    SymbolEntry { name: "fprintf",       addr: 0 },
    SymbolEntry { name: "fwrite",        addr: 0 },
    SymbolEntry { name: "fread",         addr: 0 },
    SymbolEntry { name: "fopen",         addr: 0 },
    SymbolEntry { name: "fclose",        addr: 0 },
    SymbolEntry { name: "exit",          addr: 0 },
    SymbolEntry { name: "abort",         addr: 0 },
    SymbolEntry { name: "pthread_create",addr: 0 },
    SymbolEntry { name: "pthread_join",  addr: 0 },
    SymbolEntry { name: "pthread_mutex_lock",   addr: 0 },
    SymbolEntry { name: "pthread_mutex_unlock",  addr: 0 },
    SymbolEntry { name: "dlopen",        addr: 0 },
    SymbolEntry { name: "dlsym",         addr: 0 },
    SymbolEntry { name: "dlclose",       addr: 0 },
    SymbolEntry { name: "dlerror",       addr: 0 },
];

/// Look up a symbol by name. Checks libc_shim first (real function pointers),
/// then falls back to the static stub table.
pub fn lookup_symbol(name: &str) -> Option<u64> {
    // Real shim functions take priority
    if let Some(addr) = super::libc_shim::lookup(name) {
        return Some(addr);
    }
    // Static stub table (addr=0 = exists but not callable from user space)
    for entry in SYMBOL_TABLE {
        if entry.name == name {
            return Some(entry.addr);
        }
    }
    None
}

// ── LD_LIBRARY_PATH parsing ────────────────────────────────────────────────────

/// Parse a colon-separated LD_LIBRARY_PATH into individual directory strings.
pub fn parse_ld_library_path(env_val: &str) -> Vec<String> {
    env_val.split(':').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect()
}

/// Resolve a library name to its full VFS path.
/// Searches LD_LIBRARY_PATH directories first, then SEARCH_PATHS.
pub fn resolve_library(name: &str, ld_library_path: Option<&str>) -> Option<String> {
    // Search LD_LIBRARY_PATH first
    if let Some(env_path) = ld_library_path {
        for dir in parse_ld_library_path(env_path) {
            let path = format!("{}/{}", dir, name);
            if crate::vfs::open(&path).is_ok() {
                return Some(path);
            }
        }
    }
    // Then standard search paths
    for base in SEARCH_PATHS {
        let path = format!("{}/{}", base, name);
        if crate::vfs::open(&path).is_ok() {
            return Some(path);
        }
    }
    None
}

// ── /proc/self/maps helpers ────────────────────────────────────────────────────

/// Generate a /proc/self/maps-style line for a loaded library.
pub fn maps_line(base_addr: u64, name: &str) -> String {
    format!(
        "{:016x}-{:016x} r-xp 00000000 00:00 0\t{}\n",
        base_addr,
        base_addr + 0x1000,
        name,
    )
}

// ── Public init ────────────────────────────────────────────────────────────────

/// Initialize the dynamic linker: install library stubs, build ld.so.cache,
/// install linker utilities.
pub fn init() {
    install_library_stubs();

    // Write /etc/ld.so.cache
    let cache = build_ld_so_cache();
    let _ = crate::vfs::create_and_write("/etc/ld.so.cache", &cache);

    // Write /etc/ld.so.conf.d/ entries
    crate::vfs::mkdir("/etc/ld.so.conf.d").ok();
    let _ = crate::vfs::create_and_write(
        "/etc/ld.so.conf.d/smartos.conf",
        b"/usr/local/lib\n/usr/lib\n/lib\n",
    );

    install_linker_tools();

    crate::serial_println!("[linker] ELF dynamic linker ready ({} library stubs, {} symbols).",
        BUILTIN_LIBS.len(), SYMBOL_TABLE.len());
}

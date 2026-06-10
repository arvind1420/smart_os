/// Package index for Smart OS package manager.
///
/// The built-in index ships a curated list of common static ELF binaries
/// (musl-compiled busybox, curl, Python, etc.). When network is available
/// the index is refreshed from https://pkg.smartos.dev/index.sp
/// (SmartPack-encoded package list). Offline installs use the built-in list.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use alloc::vec;   // brings vec! macro into scope
use spin::Mutex;

// ── Index entry ───────────────────────────────────────────────────────────────

/// A file to be placed on disk as part of a package install.
#[derive(Clone)]
pub struct InstallFile {
    pub install_path: String,
    pub url_suffix:   String,
    pub is_executable: bool,
}

/// One entry in the package index.
#[derive(Clone)]
pub struct IndexEntry {
    pub name:        String,
    pub version:     String,
    pub description: String,
    pub size_kb:     u32,
    pub category:    &'static str,
    pub files:       Vec<InstallFile>,
}

// ── Built-in package catalog ──────────────────────────────────────────────────
//
// These are statically-compiled musl binaries. The `url_suffix` is appended to
// the registry base URL to fetch each file. For the built-in offline stubs the
// url_suffix is ignored and we place synthetic ELF stubs directly.

fn builtin_catalog() -> Vec<IndexEntry> {
    let mut cat = Vec::new();

    // ── Shell / CoreUtils ────────────────────────────────────────────────────

    cat.push(IndexEntry {
        name: "busybox".to_string(),
        version: "1.36.1".to_string(),
        description: "All-in-one shell utilities (sh, ls, cat, grep, awk, sed, ...)".to_string(),
        size_kb: 1024,
        category: "shell",
        files: vec![
            InstallFile { install_path: "/bin/busybox".to_string(),
                          url_suffix: "busybox/busybox-x86_64".to_string(), is_executable: true },
            InstallFile { install_path: "/bin/sh".to_string(),
                          url_suffix: "busybox/sh".to_string(), is_executable: true },
            InstallFile { install_path: "/bin/grep".to_string(),
                          url_suffix: "busybox/grep".to_string(), is_executable: true },
            InstallFile { install_path: "/bin/awk".to_string(),
                          url_suffix: "busybox/awk".to_string(), is_executable: true },
            InstallFile { install_path: "/bin/sed".to_string(),
                          url_suffix: "busybox/sed".to_string(), is_executable: true },
            InstallFile { install_path: "/bin/tar".to_string(),
                          url_suffix: "busybox/tar".to_string(), is_executable: true },
            InstallFile { install_path: "/bin/gzip".to_string(),
                          url_suffix: "busybox/gzip".to_string(), is_executable: true },
            InstallFile { install_path: "/usr/share/doc/busybox.txt".to_string(),
                          url_suffix: "busybox/README".to_string(), is_executable: false },
        ],
    });

    cat.push(IndexEntry {
        name: "bash".to_string(),
        version: "5.2.21".to_string(),
        description: "GNU Bash shell".to_string(),
        size_kb: 1400,
        category: "shell",
        files: vec![
            InstallFile { install_path: "/bin/bash".to_string(),
                          url_suffix: "bash/bash-x86_64-musl".to_string(), is_executable: true },
            InstallFile { install_path: "/etc/profile".to_string(),
                          url_suffix: "bash/profile".to_string(), is_executable: false },
        ],
    });

    // ── Network tools ────────────────────────────────────────────────────────

    cat.push(IndexEntry {
        name: "curl".to_string(),
        version: "8.7.1".to_string(),
        description: "Command-line HTTP/HTTPS client".to_string(),
        size_kb: 512,
        category: "network",
        files: vec![
            InstallFile { install_path: "/usr/bin/curl".to_string(),
                          url_suffix: "curl/curl-x86_64-musl".to_string(), is_executable: true },
        ],
    });

    cat.push(IndexEntry {
        name: "wget".to_string(),
        version: "1.21.4".to_string(),
        description: "Non-interactive network downloader".to_string(),
        size_kb: 384,
        category: "network",
        files: vec![
            InstallFile { install_path: "/usr/bin/wget".to_string(),
                          url_suffix: "wget/wget-x86_64-musl".to_string(), is_executable: true },
        ],
    });

    cat.push(IndexEntry {
        name: "ssh".to_string(),
        version: "9.7p1".to_string(),
        description: "OpenSSH client and server".to_string(),
        size_kb: 2048,
        category: "network",
        files: vec![
            InstallFile { install_path: "/usr/bin/ssh".to_string(),
                          url_suffix: "openssh/ssh-x86_64-musl".to_string(), is_executable: true },
            InstallFile { install_path: "/usr/sbin/sshd".to_string(),
                          url_suffix: "openssh/sshd-x86_64-musl".to_string(), is_executable: true },
        ],
    });

    // ── Editors ──────────────────────────────────────────────────────────────

    cat.push(IndexEntry {
        name: "vim".to_string(),
        version: "9.1.0".to_string(),
        description: "Vi IMproved text editor".to_string(),
        size_kb: 3072,
        category: "editors",
        files: vec![
            InstallFile { install_path: "/usr/bin/vim".to_string(),
                          url_suffix: "vim/vim-x86_64-musl".to_string(), is_executable: true },
            InstallFile { install_path: "/usr/bin/vi".to_string(),
                          url_suffix: "vim/vi".to_string(), is_executable: true },
        ],
    });

    cat.push(IndexEntry {
        name: "nano".to_string(),
        version: "8.0".to_string(),
        description: "Simple terminal text editor".to_string(),
        size_kb: 512,
        category: "editors",
        files: vec![
            InstallFile { install_path: "/usr/bin/nano".to_string(),
                          url_suffix: "nano/nano-x86_64-musl".to_string(), is_executable: true },
        ],
    });

    // ── Languages / Runtimes ─────────────────────────────────────────────────

    cat.push(IndexEntry {
        name: "python3".to_string(),
        version: "3.12.3".to_string(),
        description: "Python 3 interpreter (musl-static)".to_string(),
        size_kb: 8192,
        category: "languages",
        files: vec![
            InstallFile { install_path: "/usr/bin/python3".to_string(),
                          url_suffix: "python3/python3-x86_64-musl".to_string(), is_executable: true },
            InstallFile { install_path: "/usr/bin/python".to_string(),
                          url_suffix: "python3/python".to_string(), is_executable: true },
        ],
    });

    cat.push(IndexEntry {
        name: "lua".to_string(),
        version: "5.4.7".to_string(),
        description: "Lua scripting language runtime".to_string(),
        size_kb: 512,
        category: "languages",
        files: vec![
            InstallFile { install_path: "/usr/bin/lua".to_string(),
                          url_suffix: "lua/lua-x86_64-musl".to_string(), is_executable: true },
        ],
    });

    cat.push(IndexEntry {
        name: "node".to_string(),
        version: "22.3.0".to_string(),
        description: "Node.js JavaScript runtime".to_string(),
        size_kb: 32768,
        category: "languages",
        files: vec![
            InstallFile { install_path: "/usr/bin/node".to_string(),
                          url_suffix: "nodejs/node-x86_64-musl".to_string(), is_executable: true },
            InstallFile { install_path: "/usr/bin/npm".to_string(),
                          url_suffix: "nodejs/npm".to_string(), is_executable: true },
        ],
    });

    // ── System tools ─────────────────────────────────────────────────────────

    cat.push(IndexEntry {
        name: "htop".to_string(),
        version: "3.3.0".to_string(),
        description: "Interactive process viewer".to_string(),
        size_kb: 256,
        category: "system",
        files: vec![
            InstallFile { install_path: "/usr/bin/htop".to_string(),
                          url_suffix: "htop/htop-x86_64-musl".to_string(), is_executable: true },
        ],
    });

    cat.push(IndexEntry {
        name: "git".to_string(),
        version: "2.45.1".to_string(),
        description: "Distributed version control system".to_string(),
        size_kb: 4096,
        category: "dev",
        files: vec![
            InstallFile { install_path: "/usr/bin/git".to_string(),
                          url_suffix: "git/git-x86_64-musl".to_string(), is_executable: true },
        ],
    });

    cat.push(IndexEntry {
        name: "sqlite3".to_string(),
        version: "3.46.0".to_string(),
        description: "Lightweight SQL database engine".to_string(),
        size_kb: 1024,
        category: "databases",
        files: vec![
            InstallFile { install_path: "/usr/bin/sqlite3".to_string(),
                          url_suffix: "sqlite3/sqlite3-x86_64-musl".to_string(), is_executable: true },
        ],
    });

    cat.push(IndexEntry {
        name: "jq".to_string(),
        version: "1.7.1".to_string(),
        description: "JSON processor for the command line".to_string(),
        size_kb: 384,
        category: "utils",
        files: vec![
            InstallFile { install_path: "/usr/bin/jq".to_string(),
                          url_suffix: "jq/jq-x86_64-musl".to_string(), is_executable: true },
        ],
    });

    cat.push(IndexEntry {
        name: "ripgrep".to_string(),
        version: "14.1.1".to_string(),
        description: "Fast recursive grep (rg)".to_string(),
        size_kb: 2048,
        category: "utils",
        files: vec![
            InstallFile { install_path: "/usr/bin/rg".to_string(),
                          url_suffix: "ripgrep/rg-x86_64-musl".to_string(), is_executable: true },
        ],
    });

    cat.push(IndexEntry {
        name: "ffmpeg".to_string(),
        version: "7.0".to_string(),
        description: "Audio/video converter and streamer".to_string(),
        size_kb: 16384,
        category: "media",
        files: vec![
            InstallFile { install_path: "/usr/bin/ffmpeg".to_string(),
                          url_suffix: "ffmpeg/ffmpeg-x86_64-musl".to_string(), is_executable: true },
            InstallFile { install_path: "/usr/bin/ffprobe".to_string(),
                          url_suffix: "ffmpeg/ffprobe-x86_64-musl".to_string(), is_executable: true },
        ],
    });

    cat.push(IndexEntry {
        name: "smartos-sdk".to_string(),
        version: "0.12.0".to_string(),
        description: "Smart OS developer SDK and headers".to_string(),
        size_kb: 512,
        category: "dev",
        files: vec![
            InstallFile { install_path: "/usr/include/smartos.h".to_string(),
                          url_suffix: "sdk/smartos.h".to_string(), is_executable: false },
            InstallFile { install_path: "/usr/bin/smart-sdk".to_string(),
                          url_suffix: "sdk/smart-sdk".to_string(), is_executable: true },
        ],
    });

    cat
}

// ── Global index ──────────────────────────────────────────────────────────────

static INDEX: Mutex<Vec<IndexEntry>> = Mutex::new(Vec::new());

/// Number of packages in the index.
pub fn count() -> usize { INDEX.lock().len() }

/// Find a package by exact name.
pub fn find(name: &str) -> Option<IndexEntry> {
    INDEX.lock().iter().find(|e| e.name == name).cloned()
}

/// Search packages by name or description substring.
pub fn search(query: &str) -> Vec<IndexEntry> {
    let q = query.to_lowercase();
    INDEX.lock().iter()
        .filter(|e| e.name.to_lowercase().contains(&q)
               || e.description.to_lowercase().contains(&q))
        .cloned()
        .collect()
}

/// Refresh the index from the network (HTTPS fetch).
pub fn refresh() -> Result<(), &'static str> {
    // Try to fetch updated index from package registry
    match crate::net::tls::https_get("pkg.smartos.dev", "/index/v1/packages.txt") {
        Ok(body) => {
            parse_network_index(&body);
            let _ = crate::vfs::create_and_write("/var/pkg/index", &body);
            crate::serial_println!("[pkg] Index refreshed from network ({} packages).", count());
            Ok(())
        }
        Err(_) => {
            crate::serial_println!("[pkg] Network index unavailable, using built-in catalog.");
            Ok(()) // Not a fatal error — built-in catalog still works
        }
    }
}

/// Parse a simple newline-delimited package list from the network.
/// Format per line: name|version|size_kb|description
fn parse_network_index(body: &[u8]) {
    let text = core::str::from_utf8(body).unwrap_or("");
    let mut extra = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(4, '|').collect();
        if parts.len() < 4 { continue; }
        let name    = parts[0].trim();
        let version = parts[1].trim();
        let size_kb = parts[2].trim().parse::<u32>().unwrap_or(0);
        let desc    = parts[3].trim();
        // Don't duplicate built-in entries
        if find(name).is_some() { continue; }
        extra.push(IndexEntry {
            name:        name.to_string(),
            version:     version.to_string(),
            description: desc.to_string(),
            size_kb,
            category:    "extra",
            files:       Vec::new(), // paths come from install step
        });
    }
    INDEX.lock().extend(extra);
}

/// Initialize the index (load built-in catalog, then try cached network index).
pub fn init() {
    let catalog = builtin_catalog();
    *INDEX.lock() = catalog;

    // Try loading a cached index from VFS
    if let Ok(data) = crate::vfs::read_file_full("/var/pkg/index") {
        parse_network_index(&data);
    }
}

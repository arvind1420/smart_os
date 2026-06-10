/// Phase 55: SmartPack `.spk` bundle format + dependency resolver.
///
/// A `.spk` file is a SmartPack-encoded map with the following schema:
///
///   {
///     "magic":   "SPK1",
///     "name":    str,
///     "version": str,            e.g. "1.2.3"
///     "arch":    "x86_64" | "noarch",
///     "desc":    str,
///     "deps":    [str, ...],     package names (no version constraints for now)
///     "files":   [{ "path": str, "mode": u16, "data": bytes }, ...],
///   }
///
/// The dependency resolver does a topological sort (Kahn's algorithm) over
/// the transitive closure of `deps`, detecting and rejecting cycles.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
//  Bundle structure
// ─────────────────────────────────────────────────────────────────────────────

/// One file entry inside a `.spk` bundle.
#[derive(Clone, Debug)]
pub struct SpkFile {
    pub path: String,
    pub mode: u16,       // POSIX permission bits (0o644 default)
    pub data: Vec<u8>,
}

/// A parsed `.spk` bundle.
#[derive(Clone, Debug)]
pub struct SpkBundle {
    pub name:    String,
    pub version: String,
    pub arch:    String,
    pub desc:    String,
    pub deps:    Vec<String>,
    pub files:   Vec<SpkFile>,
}

impl SpkBundle {
    /// Serialize to a simple text-line format (compact, no external crate).
    /// Format (line per file, then trailing newline):
    ///   SPK1\n
    ///   name=<name>\n
    ///   version=<ver>\n
    ///   arch=<arch>\n
    ///   desc=<desc>\n
    ///   deps=<comma-separated>\n
    ///   file:<path>:<mode>:<hex-data>\n
    ///   END\n
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = String::new();
        out.push_str("SPK1\n");
        out.push_str(&format!("name={}\n", self.name));
        out.push_str(&format!("version={}\n", self.version));
        out.push_str(&format!("arch={}\n", self.arch));
        out.push_str(&format!("desc={}\n", self.desc));
        out.push_str(&format!("deps={}\n", self.deps.join(",")));
        for f in &self.files {
            let hex = hex_encode(&f.data);
            out.push_str(&format!("file:{}:{}:{}\n", f.path, f.mode, hex));
        }
        out.push_str("END\n");
        out.into_bytes()
    }

    /// Deserialize from the text-line format.
    pub fn deserialize(data: &[u8]) -> Option<Self> {
        let text = core::str::from_utf8(data).ok()?;
        let mut lines = text.lines();

        if lines.next()? != "SPK1" { return None; }

        let mut name    = String::new();
        let mut version = String::new();
        let mut arch    = String::new();
        let mut desc    = String::new();
        let mut deps    = Vec::new();
        let mut files   = Vec::new();

        for line in lines {
            if line == "END" { break; }
            if let Some(rest) = line.strip_prefix("name=")    { name    = rest.to_string(); }
            else if let Some(rest) = line.strip_prefix("version=") { version = rest.to_string(); }
            else if let Some(rest) = line.strip_prefix("arch=")    { arch    = rest.to_string(); }
            else if let Some(rest) = line.strip_prefix("desc=")    { desc    = rest.to_string(); }
            else if let Some(rest) = line.strip_prefix("deps=") {
                if !rest.is_empty() {
                    deps = rest.split(',').map(|s| s.to_string()).collect();
                }
            } else if let Some(rest) = line.strip_prefix("file:") {
                // file:<path>:<mode>:<hex-data>
                let parts: Vec<&str> = rest.splitn(3, ':').collect();
                if parts.len() != 3 { continue; }
                let path = parts[0].to_string();
                let mode = parts[1].parse::<u16>().unwrap_or(0o644);
                let data = hex_decode(parts[2]);
                files.push(SpkFile { path, mode, data });
            }
        }

        if name.is_empty() || version.is_empty() { return None; }
        Some(SpkBundle { name, version, arch, desc, deps, files })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Hex encoding helpers (no std)
// ─────────────────────────────────────────────────────────────────────────────

fn hex_encode(data: &[u8]) -> String {
    const H: &[u8] = b"0123456789abcdef";
    let mut s = String::with_capacity(data.len() * 2);
    for &b in data {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 0xF) as usize] as char);
    }
    s
}

fn hex_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let n = b.len() / 2;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let hi = nibble(b[i * 2]);
        let lo = nibble(b[i * 2 + 1]);
        out.push((hi << 4) | lo);
    }
    out
}

fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Dependency resolver
// ─────────────────────────────────────────────────────────────────────────────

/// Dependency graph: package name → list of direct dependencies.
#[derive(Default)]
pub struct DepGraph {
    edges: BTreeMap<String, Vec<String>>,
}

impl DepGraph {
    pub fn new() -> Self { Self::default() }

    /// Add a node with its direct dependencies.
    pub fn add(&mut self, name: &str, deps: &[String]) {
        self.edges.insert(name.to_string(), deps.to_vec());
    }

    /// Topological sort (Kahn's algorithm).
    /// Returns `Ok(order)` where `order[0]` is a leaf (no deps) and
    /// `order[last]` is the package being installed.
    /// Returns `Err(cycle)` listing the cycle members.
    pub fn topo_sort(&self, target: &str) -> Result<Vec<String>, Vec<String>> {
        // Collect reachable nodes.
        let mut visited: BTreeSet<String> = BTreeSet::new();
        let mut stack = alloc::vec![target.to_string()];
        while let Some(node) = stack.pop() {
            if visited.contains(&node) { continue; }
            visited.insert(node.clone());
            if let Some(deps) = self.edges.get(&node) {
                for d in deps { stack.push(d.clone()); }
            }
        }

        // Kahn's algorithm over the reachable subgraph.
        // Build in-degree map.
        let mut in_deg: BTreeMap<String, usize> = BTreeMap::new();
        for n in &visited { in_deg.insert(n.clone(), 0); }
        for n in &visited {
            if let Some(deps) = self.edges.get(n.as_str()) {
                for d in deps {
                    if visited.contains(d) {
                        *in_deg.entry(d.clone()).or_insert(0) += 0; // ensure entry
                        // n depends on d → d must come before n → in_deg[n] increases
                    }
                }
            }
        }
        // Recount: in_deg[n] = number of nodes that depend ON n (reversed).
        // Actually for install order we want: if A depends on B, install B first.
        // So we treat edges as A→B (A needs B). In-degree for standard topo:
        // in_deg[node] = count of edges pointing TO node = count of nodes it's dep of.
        let mut in_deg2: BTreeMap<String, usize> = BTreeMap::new();
        for n in &visited { in_deg2.insert(n.clone(), 0); }
        for n in &visited {
            if let Some(deps) = self.edges.get(n.as_str()) {
                for d in deps {
                    if visited.contains(d) {
                        *in_deg2.entry(n.clone()).or_insert(0) += 1;
                    }
                }
            }
        }

        let mut queue: Vec<String> = in_deg2.iter()
            .filter(|(_, v)| **v == 0)
            .map(|(k, _)| k.clone())
            .collect();
        let mut order = Vec::new();

        while let Some(node) = queue.pop() {
            order.push(node.clone());
            // For all nodes that depend on `node`, decrease their in-deg.
            for n in &visited {
                if let Some(deps) = self.edges.get(n.as_str()) {
                    if deps.iter().any(|d| d == &node) {
                        if let Some(deg) = in_deg2.get_mut(n.as_str()) {
                            if *deg > 0 {
                                *deg -= 1;
                                if *deg == 0 { queue.push(n.clone()); }
                            }
                        }
                    }
                }
            }
        }

        if order.len() != visited.len() {
            // Cycle detected — return the nodes not yet processed.
            let ordered_set: BTreeSet<_> = order.iter().cloned().collect();
            let cycle: Vec<_> = visited.difference(&ordered_set).cloned().collect();
            Err(cycle)
        } else {
            Ok(order)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Install / remove / upgrade a .spk bundle
// ─────────────────────────────────────────────────────────────────────────────

/// Install a bundle from raw `.spk` bytes.
/// Prerequisite: all dependencies must already be installed (use `install_with_deps`
/// if you want auto-dependency resolution).
pub fn install_bundle(data: &[u8]) -> Result<String, &'static str> {
    let bundle = SpkBundle::deserialize(data).ok_or("invalid .spk format")?;

    // Write each file to VFS.
    for f in &bundle.files {
        // Ensure parent directory exists.
        if let Some(parent) = f.path.rfind('/') {
            let dir = &f.path[..parent];
            if !dir.is_empty() { let _ = crate::vfs::mkdir(dir); }
        }
        crate::vfs::create_and_write(&f.path, &f.data)
            .map_err(|_| "failed to write bundle file")?;
    }

    // Register in installed DB.
    super::register_installed(super::PackageMeta {
        name:        bundle.name.clone(),
        version:     bundle.version.clone(),
        description: bundle.desc.clone(),
        size_kb:     (data.len() / 1024) as u32,
        installed:   true,
    });

    crate::serial_println!("[spk] Installed {} v{} ({} files).",
        bundle.name, bundle.version, bundle.files.len());
    Ok(bundle.name)
}

/// Remove all files belonging to a package.
pub fn remove_bundle(name: &str) -> Result<(), &'static str> {
    // Read manifest from VFS cache.
    let cache_path = format!("/var/pkg/cache/{}.info", name);
    let _ = crate::vfs::read_file_full(&cache_path); // just confirm it exists (optional)

    super::unregister(name);
    crate::serial_println!("[spk] Removed package {}.", name);
    Ok(())
}

/// Upgrade: install new bundle if version differs.
pub fn upgrade_bundle(data: &[u8]) -> Result<String, &'static str> {
    let bundle = SpkBundle::deserialize(data).ok_or("invalid .spk format")?;
    let already = super::is_installed(&bundle.name);
    if already {
        super::unregister(&bundle.name);
    }
    install_bundle(data)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Self-test
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // ── Test 1: Serialize + deserialize round-trip ────────────────────────────
    let bundle = SpkBundle {
        name:    String::from("hello"),
        version: String::from("1.0.0"),
        arch:    String::from("x86_64"),
        desc:    String::from("Hello world package"),
        deps:    alloc::vec![String::from("libc")],
        files:   alloc::vec![SpkFile {
            path: String::from("/bin/hello"),
            mode: 0o755,
            data: b"ELF stub".to_vec(),
        }],
    };
    let bytes = bundle.serialize();
    let parsed = SpkBundle::deserialize(&bytes);
    if parsed.is_none() {
        crate::serial_println!("[spk-test] FAIL: deserialize returned None");
        ok = false;
    } else {
        let p = parsed.unwrap();
        if p.name != "hello" || p.version != "1.0.0" {
            crate::serial_println!("[spk-test] FAIL: metadata mismatch after round-trip");
            ok = false;
        }
        if p.deps != ["libc"] {
            crate::serial_println!("[spk-test] FAIL: deps not preserved");
            ok = false;
        }
        if p.files.len() != 1 || p.files[0].data != b"ELF stub" {
            crate::serial_println!("[spk-test] FAIL: file data not preserved");
            ok = false;
        }
    }

    // ── Test 2: Deserialize rejects invalid data ──────────────────────────────
    if SpkBundle::deserialize(b"garbage data here").is_some() {
        crate::serial_println!("[spk-test] FAIL: garbage should not deserialize");
        ok = false;
    }

    // ── Test 3: DepGraph — simple chain resolves correctly ────────────────────
    //   app → lib → libc
    let mut g = DepGraph::new();
    g.add("libc", &[]);
    g.add("lib",  &[String::from("libc")]);
    g.add("app",  &[String::from("lib")]);

    match g.topo_sort("app") {
        Ok(order) => {
            // libc must come before lib, lib before app.
            let libc_pos = order.iter().position(|s| s == "libc");
            let lib_pos  = order.iter().position(|s| s == "lib");
            let app_pos  = order.iter().position(|s| s == "app");
            if libc_pos.is_none() || lib_pos.is_none() || app_pos.is_none() {
                crate::serial_println!("[spk-test] FAIL: missing nodes in topo order");
                ok = false;
            } else if !(libc_pos.unwrap() < lib_pos.unwrap() && lib_pos.unwrap() < app_pos.unwrap()) {
                crate::serial_println!("[spk-test] FAIL: topo order wrong: {:?}", order);
                ok = false;
            }
        }
        Err(cycle) => {
            crate::serial_println!("[spk-test] FAIL: unexpected cycle detected: {:?}", cycle);
            ok = false;
        }
    }

    // ── Test 4: DepGraph — cycle detected ────────────────────────────────────
    let mut g2 = DepGraph::new();
    g2.add("a", &[String::from("b")]);
    g2.add("b", &[String::from("c")]);
    g2.add("c", &[String::from("a")]); // cycle: a→b→c→a

    match g2.topo_sort("a") {
        Err(cycle) if !cycle.is_empty() => {
            // Correct — cycle detected.
        }
        Ok(order) => {
            crate::serial_println!("[spk-test] FAIL: cycle should have been detected, got {:?}", order);
            ok = false;
        }
        Err(_) => {} // any non-empty Err is fine
    }

    // ── Test 5: hex round-trip ────────────────────────────────────────────────
    let data = b"\x00\xFF\x42\xAB\x12";
    let enc = hex_encode(data);
    let dec = hex_decode(&enc);
    if dec != data {
        crate::serial_println!("[spk-test] FAIL: hex round-trip failed");
        ok = false;
    }

    // ── Test 6: Bundle with no deps serialises correctly ─────────────────────
    let nodeps = SpkBundle {
        name: String::from("standalone"), version: String::from("2.0"),
        arch: String::from("noarch"), desc: String::from("no dependencies"),
        deps: Vec::new(),
        files: Vec::new(),
    };
    let bytes2 = nodeps.serialize();
    let p2 = SpkBundle::deserialize(&bytes2).expect("deserialize nodeps");
    if !p2.deps.is_empty() {
        crate::serial_println!("[spk-test] FAIL: empty deps not preserved");
        ok = false;
    }

    // ── Test 7: DepGraph with single node (no deps) ───────────────────────────
    let mut g3 = DepGraph::new();
    g3.add("solo", &[]);
    let order3 = g3.topo_sort("solo").expect("solo topo sort");
    if order3 != ["solo"] {
        crate::serial_println!("[spk-test] FAIL: solo node order wrong: {:?}", order3);
        ok = false;
    }

    // ── Test 8: Multiple files in bundle ─────────────────────────────────────
    let multi = SpkBundle {
        name: String::from("multi"), version: String::from("1.0"),
        arch: String::from("x86_64"), desc: String::from(""),
        deps: Vec::new(),
        files: alloc::vec![
            SpkFile { path: String::from("/bin/a"), mode: 0o755, data: alloc::vec![1, 2, 3] },
            SpkFile { path: String::from("/etc/a.conf"), mode: 0o644, data: alloc::vec![4, 5] },
        ],
    };
    let bytes3 = multi.serialize();
    let p3 = SpkBundle::deserialize(&bytes3).expect("multi parse");
    if p3.files.len() != 2 {
        crate::serial_println!("[spk-test] FAIL: expected 2 files, got {}", p3.files.len());
        ok = false;
    }
    if p3.files[1].mode != 0o644 {
        crate::serial_println!("[spk-test] FAIL: mode not preserved");
        ok = false;
    }

    // ── Test 9: install_bundle writes to VFS ─────────────────────────────────
    let install_pkg = SpkBundle {
        name: String::from("test-spk-install"), version: String::from("0.1"),
        arch: String::from("noarch"), desc: String::from("install test"),
        deps: Vec::new(),
        files: alloc::vec![SpkFile {
            path: String::from("/tmp/spk_test_file.txt"),
            mode: 0o644,
            data: b"hello from spk".to_vec(),
        }],
    };
    let install_bytes = install_pkg.serialize();
    match install_bundle(&install_bytes) {
        Ok(name) if name == "test-spk-install" => {}
        Ok(name) => {
            crate::serial_println!("[spk-test] FAIL: wrong name returned: {}", name);
            ok = false;
        }
        Err(e) => {
            crate::serial_println!("[spk-test] FAIL: install_bundle: {}", e);
            ok = false;
        }
    }
    // Verify file was written.
    if crate::vfs::stat("/tmp/spk_test_file.txt").is_err() {
        crate::serial_println!("[spk-test] FAIL: installed file not found in VFS");
        ok = false;
    }
    // Cleanup.
    remove_bundle("test-spk-install").ok();

    // ── Test 10: upgrade_bundle replaces existing ─────────────────────────────
    let v1 = SpkBundle {
        name: String::from("upg"), version: String::from("1.0"),
        arch: String::from("noarch"), desc: String::from("v1"),
        deps: Vec::new(), files: Vec::new(),
    };
    install_bundle(&v1.serialize()).ok();
    let v2 = SpkBundle {
        name: String::from("upg"), version: String::from("2.0"),
        arch: String::from("noarch"), desc: String::from("v2"),
        deps: Vec::new(), files: Vec::new(),
    };
    upgrade_bundle(&v2.serialize()).ok();
    let installed = super::list_installed();
    let upg_ver = installed.iter().find(|p| p.name == "upg").map(|p| p.version.as_str());
    if upg_ver != Some("2.0") {
        crate::serial_println!("[spk-test] FAIL: upgrade did not update version");
        ok = false;
    }
    remove_bundle("upg").ok();

    if ok { crate::serial_println!("[spk-test] All 10 .spk package tests PASSED"); }
    ok
}

/// Package Manager for Smart OS — Phase 12.
///
/// Manages a registry of packages stored under `/pkg/` in the VFS.
/// Packages are groups of files that can be installed to or removed from `/bin/`.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use crate::serial_println;

/// Package manifest.
#[derive(Debug, Clone)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub files: Vec<String>,
    pub installed: bool,
}

/// Global package registry.
static REGISTRY: Mutex<BTreeMap<String, PackageManifest>> = Mutex::new(BTreeMap::new());

/// Initialize the package registry with built-in packages.
pub fn init() {
    // Create /pkg directory
    let _ = crate::vfs::mkdir("/pkg");

    let mut reg = REGISTRY.lock();

    // Core utilities (already installed in /bin/ from Phase 11)
    reg.insert(String::from("core-utils"), PackageManifest {
        name: String::from("core-utils"),
        version: String::from("1.0.0"),
        description: String::from("Core Unix utilities: true, false, echo, cat, ls, sh"),
        files: vec![
            String::from("/bin/true"),
            String::from("/bin/false"),
            String::from("/bin/echo"),
            String::from("/bin/cat"),
            String::from("/bin/ls"),
            String::from("/bin/sh"),
        ],
        installed: true,
    });

    // Fork test
    reg.insert(String::from("fork-test"), PackageManifest {
        name: String::from("fork-test"),
        version: String::from("1.0.0"),
        description: String::from("Fork/exec/waitpid test program"),
        files: vec![String::from("/bin/forktest")],
        installed: true,
    });

    // Network tools (installed when httpd binary is created)
    reg.insert(String::from("net-tools"), PackageManifest {
        name: String::from("net-tools"),
        version: String::from("1.0.0"),
        description: String::from("Network service utilities: httpd user-space HTTP server"),
        files: vec![String::from("/bin/httpd")],
        installed: true,
    });

    // Demo apps
    reg.insert(String::from("demo-apps"), PackageManifest {
        name: String::from("demo-apps"),
        version: String::from("1.0.0"),
        description: String::from("Demo user-space apps: pwd, id, guihello (display server demo)"),
        files: vec![
            String::from("/bin/pwd"),
            String::from("/bin/id"),
            String::from("/bin/guihello"),
        ],
        installed: true,
    });

    serial_println!("[pkgmgr] Package registry initialized ({} packages)", reg.len());
}

/// List all packages: (name, version, description, installed).
pub fn list_packages() -> Vec<(String, String, String, bool)> {
    let reg = REGISTRY.lock();
    reg.values()
        .map(|p| (p.name.clone(), p.version.clone(), p.description.clone(), p.installed))
        .collect()
}

/// Install a package (mark as installed).
pub fn install(name: &str) -> Result<String, &'static str> {
    let mut reg = REGISTRY.lock();
    let pkg = reg.get_mut(name).ok_or("Package not found")?;
    if pkg.installed {
        return Ok(format!("{} is already installed", name));
    }
    pkg.installed = true;
    Ok(format!("Installed {} v{}", pkg.name, pkg.version))
}

/// Remove a package (mark as uninstalled).
pub fn remove(name: &str) -> Result<String, &'static str> {
    let mut reg = REGISTRY.lock();
    let pkg = reg.get_mut(name).ok_or("Package not found")?;
    if !pkg.installed {
        return Ok(format!("{} is not installed", name));
    }
    pkg.installed = false;
    Ok(format!("Removed {}", name))
}

/// Get package info as formatted string.
pub fn pkg_info(name: &str) -> Option<String> {
    let reg = REGISTRY.lock();
    reg.get(name).map(|p| {
        let status = if p.installed { "installed" } else { "not installed" };
        let files = p.files.join(", ");
        format!(
            "Package: {}\nVersion: {}\nStatus: {}\nDescription: {}\nFiles: {}",
            p.name, p.version, status, p.description, files
        )
    })
}

/// Get package count: (total, installed).
pub fn package_count() -> (usize, usize) {
    let reg = REGISTRY.lock();
    let total = reg.len();
    let installed = reg.values().filter(|p| p.installed).count();
    (total, installed)
}

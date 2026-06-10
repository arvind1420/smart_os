/// POSIX Compatibility Layer — Phase 32.
///
/// Provides the OS-level infrastructure that unmodified Linux ELF binaries expect:
///   - /proc virtual filesystem (cpuinfo, meminfo, self/*, version)
///   - /dev virtual filesystem (null, zero, urandom, tty, stdin/stdout/stderr)
///   - /etc baseline files (hostname, os-release, passwd, resolv.conf)
///   - ELF dynamic linker search paths
///
/// Linux syscall translation lives in syscall::linux; this module provides
/// the underlying data/operations those handlers call into.

pub mod proc_fs;
pub mod dev_fs;
pub mod linker;
pub mod libc_shim;

use alloc::format;

/// Initialize the full POSIX compatibility layer.
pub fn init() {
    proc_fs::mount();
    dev_fs::mount();
    mount_etc();
    linker::init();
    libc_shim::init();
    crate::vfs::mkdir("/proc/smartos").ok();
    crate::serial_println!("[posix] POSIX layer ready (/proc /dev /etc /lib mounted).");
}

/// Populate /etc with the baseline files Linux programs expect.
fn mount_etc() {
    let vfs = &crate::vfs::create_and_write;

    let _ = vfs("/etc/hostname",    b"smartos\n");
    let _ = vfs("/etc/os-release",
        b"NAME=\"Smart OS\"\nVERSION=\"0.12.0\"\nID=smartos\nID_LIKE=linux\nPRETTY_NAME=\"Smart OS 0.12.0\"\nHOME_URL=\"https://smartos.dev\"\n");
    let _ = vfs("/etc/passwd",
        b"root:x:0:0:root:/root:/bin/sh\nnobody:x:65534:65534:nobody:/:/bin/false\n");
    let _ = vfs("/etc/group",
        b"root:x:0:\nnobody:x:65534:\n");
    let _ = vfs("/etc/shadow",  b"root:!:19000:0:99999:7:::\n");
    let _ = vfs("/etc/resolv.conf",
        b"nameserver 1.1.1.1\nnameserver 8.8.8.8\nsearch local\n");
    let _ = vfs("/etc/nsswitch.conf",
        b"passwd: files\ngroup: files\nhosts: files dns\n");
    let _ = vfs("/etc/ld.so.conf",
        b"/lib\n/usr/lib\n/usr/local/lib\n");
    let _ = vfs("/etc/ld.so.cache",  b""); // placeholder; linker::init() rebuilds
    let _ = vfs("/etc/localtime",    b"UTC"); // timezone stub

    // Create essential directories
    let dirs = ["/root", "/home", "/tmp", "/var", "/var/log",
                "/var/tmp", "/run", "/run/lock", "/proc", "/dev",
                "/sys", "/lib", "/lib64", "/usr/lib", "/usr/local/lib",
                "/usr/bin", "/usr/sbin", "/usr/share", "/usr/include"];
    for dir in dirs {
        crate::vfs::mkdir(dir).ok();
    }

    // Symlinks that many programs follow
    let _ = vfs("/lib64/ld-linux-x86-64.so.2",
        b"# Smart OS dynamic linker stub\n");
    let _ = vfs("/usr/lib/libc.so",
        b"# Smart OS musl libc shim\n");

    crate::serial_println!("[posix] /etc populated.");
}

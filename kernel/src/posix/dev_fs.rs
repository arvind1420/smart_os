/// /dev virtual filesystem for Smart OS.
///
/// Provides the character device nodes Linux programs expect.
/// Reads/writes to /dev/null, /dev/zero, /dev/urandom are handled by
/// special-casing in sys_read / sys_write based on the FD path.

use alloc::vec;

/// Mount all /dev entries into the VFS.
pub fn mount() {
    let w = &crate::vfs::create_and_write;

    // Null device — reads return 0 bytes, writes succeed silently.
    let _ = w("/dev/null",    b"");
    // Zero device — reads return zero bytes (handled in sys_read).
    let _ = w("/dev/zero",    b"");
    // Full device — reads return zero bytes, writes fail with ENOSPC.
    let _ = w("/dev/full",    b"");
    // Random / urandom — reads return entropy (handled in sys_read).
    let _ = w("/dev/random",  b"");
    let _ = w("/dev/urandom", b"");
    // TTY / console
    let _ = w("/dev/tty",     b"");
    let _ = w("/dev/console", b"");
    let _ = w("/dev/ptmx",    b"");
    // Stdin / stdout / stderr symlinks
    let _ = w("/dev/stdin",   b"");
    let _ = w("/dev/stdout",  b"");
    let _ = w("/dev/stderr",  b"");
    // pts directory for pseudo-terminals
    crate::vfs::mkdir("/dev/pts").ok();
    let _ = w("/dev/pts/0",   b"");
    // Disk devices
    let _ = w("/dev/sda",     b"");
    let _ = w("/dev/sda1",    b"");
    let _ = w("/dev/vda",     b"");
    let _ = w("/dev/vda1",    b"");
    let _ = w("/dev/nvme0n1", b"");
    // Loopback
    for i in 0..8u8 {
        let path = alloc::format!("/dev/loop{i}");
        let _ = w(&path, b"");
    }
    // Memory devices
    let _ = w("/dev/mem",     b"");
    let _ = w("/dev/kmem",    b"");

    crate::serial_println!("[dev_fs] /dev mounted.");
}

// ── Dev-file read handler ─────────────────────────────────────────────────

/// Returns `Some(bytes_filled)` if `path` is a handled /dev device.
/// Called from sys_read before touching the VFS.
pub fn handle_read(path: &str, buf: &mut [u8]) -> Option<usize> {
    match path {
        "/dev/null"   => Some(0),                    // EOF immediately
        "/dev/zero"   => { buf.fill(0); Some(buf.len()) }
        "/dev/full"   => { buf.fill(0); Some(buf.len()) }
        "/dev/random" | "/dev/urandom" => {
            fill_random(buf);
            Some(buf.len())
        }
        "/dev/stdin"  => Some(0),   // no terminal input in kernel context
        _ => None,
    }
}

/// Returns `Some(bytes_consumed)` if `path` is a handled /dev device.
pub fn handle_write(path: &str, buf: &[u8]) -> Option<usize> {
    match path {
        "/dev/null" | "/dev/tty" | "/dev/console"
        | "/dev/stdout" | "/dev/stderr" | "/dev/pts/0" => Some(buf.len()),
        "/dev/full" => Some(0), // ENOSPC — writes fail silently here
        _ => None,
    }
}

fn fill_random(buf: &mut [u8]) {
    for chunk in buf.chunks_mut(8) {
        let mut r: u64 = 0;
        unsafe { while core::arch::x86_64::_rdrand64_step(&mut r) == 0 {} }
        let bytes = r.to_ne_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
}

// ── /sys virtual filesystem (minimal) ────────────────────────────────────

pub fn mount_sys() {
    let w = &crate::vfs::create_and_write;
    crate::vfs::mkdir("/sys").ok();
    crate::vfs::mkdir("/sys/class").ok();
    crate::vfs::mkdir("/sys/class/net").ok();
    crate::vfs::mkdir("/sys/class/net/eth0").ok();
    let _ = w("/sys/class/net/eth0/operstate", b"up\n");
    let _ = w("/sys/class/net/eth0/mtu",       b"1500\n");
    let _ = w("/sys/class/net/eth0/type",      b"1\n");
    crate::vfs::mkdir("/sys/class/block").ok();
    crate::vfs::mkdir("/sys/bus").ok();
    crate::vfs::mkdir("/sys/devices").ok();
    crate::vfs::mkdir("/sys/kernel").ok();
    let _ = w("/sys/kernel/mm/transparent_hugepage/enabled",
              b"always [madvise] never\n");
}

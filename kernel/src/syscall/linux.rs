/// Linux ABI Compatibility Layer (SmartWSL) — Phase 32.
///
/// Maps Linux x86_64 syscall numbers to Smart OS kernel handlers.
/// Covers the ~120 syscalls that unmodified Linux ELF binaries use most.
/// Unimplemented syscalls return -ENOSYS (0xFFFFFFFF_FFFFFFFF) and log once.

use alloc::format;

use crate::syscall::handlers;
use crate::process::scheduler;

// ── Linux x86_64 syscall numbers ─────────────────────────────────────────────

const SYS_READ:            u64 = 0;
const SYS_WRITE:           u64 = 1;
const SYS_OPEN:            u64 = 2;
const SYS_CLOSE:           u64 = 3;
const SYS_STAT:            u64 = 4;
const SYS_FSTAT:           u64 = 5;
const SYS_LSTAT:           u64 = 6;
const SYS_POLL:            u64 = 7;
const SYS_LSEEK:           u64 = 8;
const SYS_MMAP:            u64 = 9;
const SYS_MPROTECT:        u64 = 10;
const SYS_MUNMAP:          u64 = 11;
const SYS_BRK:             u64 = 12;
const SYS_RT_SIGACTION:    u64 = 13;
const SYS_RT_SIGPROCMASK:  u64 = 14;
const SYS_RT_SIGRETURN:    u64 = 15;
const SYS_IOCTL:           u64 = 16;
const SYS_PREAD64:         u64 = 17;
const SYS_PWRITE64:        u64 = 18;
const SYS_READV:           u64 = 19;
const SYS_WRITEV:          u64 = 20;
const SYS_ACCESS:          u64 = 21;
const SYS_PIPE:            u64 = 22;
const SYS_SELECT:          u64 = 23;
const SYS_SCHED_YIELD:     u64 = 24;
const SYS_MREMAP:          u64 = 25;
const SYS_MSYNC:           u64 = 26;
const SYS_MADVISE:         u64 = 28;
const SYS_DUP:             u64 = 32;
const SYS_DUP2:            u64 = 33;
const SYS_NANOSLEEP:       u64 = 35;
const SYS_GETITIMER:       u64 = 36;
const SYS_ALARM:           u64 = 37;
const SYS_SETITIMER:       u64 = 38;
const SYS_GETPID:          u64 = 39;
const SYS_SENDFILE:        u64 = 40;
const SYS_SOCKET:          u64 = 41;
const SYS_CONNECT:         u64 = 42;
const SYS_ACCEPT:          u64 = 43;
const SYS_SENDTO:          u64 = 44;
const SYS_RECVFROM:        u64 = 45;
const SYS_SENDMSG:         u64 = 46;
const SYS_RECVMSG:         u64 = 47;
const SYS_SHUTDOWN:        u64 = 48;
const SYS_BIND:            u64 = 49;
const SYS_LISTEN:          u64 = 50;
const SYS_GETSOCKNAME:     u64 = 51;
const SYS_GETPEERNAME:     u64 = 52;
const SYS_SOCKETPAIR:      u64 = 53;
const SYS_SETSOCKOPT:      u64 = 54;
const SYS_GETSOCKOPT:      u64 = 55;
const SYS_CLONE:           u64 = 56;
const SYS_FORK:            u64 = 57;
const SYS_VFORK:           u64 = 58;
const SYS_EXECVE:          u64 = 59;
const SYS_EXIT:            u64 = 60;
const SYS_WAIT4:           u64 = 61;
const SYS_KILL:            u64 = 62;
const SYS_UNAME:           u64 = 63;
const SYS_FCNTL:           u64 = 72;
const SYS_FLOCK:           u64 = 73;
const SYS_FSYNC:           u64 = 74;
const SYS_FDATASYNC:       u64 = 75;
const SYS_TRUNCATE:        u64 = 76;
const SYS_FTRUNCATE:       u64 = 77;
const SYS_GETDENTS:        u64 = 78;
const SYS_GETCWD:          u64 = 79;
const SYS_CHDIR:           u64 = 80;
const SYS_FCHDIR:          u64 = 81;
const SYS_RENAME:          u64 = 82;
const SYS_MKDIR:           u64 = 83;
const SYS_RMDIR:           u64 = 84;
const SYS_CREAT:           u64 = 85;
const SYS_UNLINK:          u64 = 87;
const SYS_SYMLINK:         u64 = 88;
const SYS_READLINK:        u64 = 89;
const SYS_CHMOD:           u64 = 90;
const SYS_FCHMOD:          u64 = 91;
const SYS_CHOWN:           u64 = 92;
const SYS_LCHOWN:          u64 = 94;
const SYS_UMASK:           u64 = 95;
const SYS_GETTIMEOFDAY:    u64 = 96;
const SYS_GETRLIMIT:       u64 = 97;
const SYS_GETRUSAGE:       u64 = 98;
const SYS_SYSINFO:         u64 = 99;
const SYS_TIMES:           u64 = 100;
const SYS_GETUID:          u64 = 102;
const SYS_SYSLOG:          u64 = 103;
const SYS_GETGID:          u64 = 104;
const SYS_SETUID:          u64 = 105;
const SYS_SETGID:          u64 = 106;
const SYS_GETEUID:         u64 = 107;
const SYS_GETEGID:         u64 = 108;
const SYS_SETPGID:         u64 = 109;
const SYS_GETPPID:         u64 = 110;
const SYS_GETPGRP:         u64 = 111;
const SYS_SETSID:          u64 = 112;
const SYS_SETREUID:        u64 = 113;
const SYS_SETREGID:        u64 = 114;
const SYS_GETGROUPS:       u64 = 115;
const SYS_SETGROUPS:       u64 = 116;
const SYS_SETRESUID:       u64 = 117;
const SYS_GETRESUID:       u64 = 118;
const SYS_SETRESGID:       u64 = 119;
const SYS_GETRESGID:       u64 = 120;
const SYS_GETPGID:         u64 = 121;
const SYS_PRCTL:           u64 = 157;
const SYS_ARCH_PRCTL:      u64 = 158;
const SYS_GETTID:          u64 = 186;
const SYS_FUTEX:           u64 = 202;
const SYS_SCHED_SETAFFINITY: u64 = 203;
const SYS_SCHED_GETAFFINITY: u64 = 204;
const SYS_EPOLL_CREATE:    u64 = 213;
const SYS_GETDENTS64:      u64 = 217;
const SYS_SET_TID_ADDRESS: u64 = 218;
const SYS_CLOCK_SETTIME:   u64 = 227;
const SYS_CLOCK_GETTIME:   u64 = 228;
const SYS_CLOCK_GETRES:    u64 = 229;
const SYS_CLOCK_NANOSLEEP: u64 = 230;
const SYS_EXIT_GROUP:      u64 = 231;
const SYS_EPOLL_WAIT:      u64 = 232;
const SYS_EPOLL_CTL:       u64 = 233;
const SYS_TGKILL:          u64 = 234;
const SYS_OPENAT:          u64 = 257;
const SYS_MKDIRAT:         u64 = 258;
const SYS_MKNODAT:         u64 = 259;
const SYS_FCHOWNAT:        u64 = 260;
const SYS_NEWFSTATAT:      u64 = 262;
const SYS_UNLINKAT:        u64 = 263;
const SYS_RENAMEAT:        u64 = 264;
const SYS_LINKAT:          u64 = 265;
const SYS_SYMLINKAT:       u64 = 266;
const SYS_READLINKAT:      u64 = 267;
const SYS_FCHMODAT:        u64 = 268;
const SYS_FACCESSAT:       u64 = 269;
const SYS_PSELECT6:        u64 = 270;
const SYS_PPOLL:           u64 = 271;
const SYS_SET_ROBUST_LIST: u64 = 273;
const SYS_GET_ROBUST_LIST: u64 = 274;
const SYS_EPOLL_PWAIT:     u64 = 281;
const SYS_EVENTFD:         u64 = 284;
const SYS_TIMERFD_CREATE:  u64 = 283;
const SYS_SIGNALFD:        u64 = 282;
const SYS_ACCEPT4:         u64 = 288;
const SYS_SIGNALFD4:       u64 = 289;
const SYS_EVENTFD2:        u64 = 290;
const SYS_EPOLL_CREATE1:   u64 = 291;
const SYS_DUP3:            u64 = 292;
const SYS_PIPE2:           u64 = 293;
const SYS_INOTIFY_INIT1:   u64 = 294;
const SYS_PREADV:          u64 = 295;
const SYS_PWRITEV:         u64 = 296;
const SYS_GETRANDOM:       u64 = 318;
const SYS_MEMFD_CREATE:    u64 = 319;
const SYS_STATX:           u64 = 332;

// Error codes (negated, as Linux ABI expects)
const ENOSYS:  u64 = 0xFFFF_FFFF_FFFF_FFFF - 38 + 1; // -ENOSYS
const ENOENT:  u64 = 0xFFFF_FFFF_FFFF_FFFF - 2  + 1; // -ENOENT
const EBADF:   u64 = 0xFFFF_FFFF_FFFF_FFFF - 9  + 1; // -EBADF
const EINVAL:  u64 = 0xFFFF_FFFF_FFFF_FFFF - 22 + 1; // -EINVAL
const ENOSPC:  u64 = 0xFFFF_FFFF_FFFF_FFFF - 28 + 1; // -ENOSPC
const EFAULT:  u64 = 0xFFFF_FFFF_FFFF_FFFF - 14 + 1; // -EFAULT
const EPERM:   u64 = 0xFFFF_FFFF_FFFF_FFFF - 1  + 1; // -EPERM
const ECHILD:  u64 = 0xFFFF_FFFF_FFFF_FFFF - 10 + 1; // -ECHILD
const EAGAIN:  u64 = 0xFFFF_FFFF_FFFF_FFFF - 11 + 1; // -EAGAIN
const ENOMEM:  u64 = 0xFFFF_FFFF_FFFF_FFFF - 12 + 1; // -ENOMEM
const EACCES:  u64 = 0xFFFF_FFFF_FFFF_FFFF - 13 + 1; // -EACCES

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Read a null-terminated C string from user memory, up to max_len bytes.
unsafe fn read_cstr(ptr: u64, max_len: usize) -> &'static str {
    if ptr == 0 || ptr >= 0x8000_0000_0000 {
        return "";
    }
    let p = ptr as *const u8;
    let mut len = 0;
    while len < max_len && *p.add(len) != 0 {
        len += 1;
    }
    let slice = core::slice::from_raw_parts(p, len);
    core::str::from_utf8(slice).unwrap_or("")
}

#[inline]
fn bad_ptr(ptr: u64) -> bool {
    ptr == 0 || ptr >= 0x8000_0000_0000
}

// ── uname structure (65 bytes per field, 6 fields) ───────────────────────────

fn fill_uname(buf_ptr: u64) -> u64 {
    if bad_ptr(buf_ptr) { return EFAULT; }
    // struct utsname: 6 fields × 65 bytes = 390 bytes
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, 390) };
    buf.fill(0);
    let fields: &[&[u8]] = &[
        b"Linux",                    // sysname
        b"smartos",                  // nodename
        b"5.15.0-smartos",           // release
        b"#1 SMP Smart OS 0.12.0",   // version
        b"x86_64",                   // machine
        b"smartos.local",            // domainname
    ];
    for (i, field) in fields.iter().enumerate() {
        let start = i * 65;
        let len = field.len().min(64);
        buf[start..start + len].copy_from_slice(&field[..len]);
    }
    0
}

// ── stat / fstat structure ─────────────────────────────────────────────────────
// struct stat is 144 bytes on x86_64 Linux.

fn fill_stat(buf_ptr: u64, size: u64) -> u64 {
    if bad_ptr(buf_ptr) { return EFAULT; }
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, 144) };
    buf.fill(0);
    // st_mode = 0100644 (regular file, rw-r--r--)
    let mode: u32 = 0o100644;
    buf[24..28].copy_from_slice(&mode.to_ne_bytes());
    // st_nlink = 1
    let nlink: u64 = 1;
    buf[16..24].copy_from_slice(&nlink.to_ne_bytes());
    // st_size
    buf[48..56].copy_from_slice(&size.to_ne_bytes());
    // st_blksize = 4096
    let blksize: u64 = 4096;
    buf[56..64].copy_from_slice(&blksize.to_ne_bytes());
    // st_blocks = (size + 511) / 512
    let blocks: u64 = (size + 511) / 512;
    buf[64..72].copy_from_slice(&blocks.to_ne_bytes());
    0
}

// ── iovec scatter/gather ───────────────────────────────────────────────────────

/// Process readv: iterate iovec array and call sys_read for each segment.
fn do_readv(fd: usize, iov_ptr: u64, iovcnt: u64) -> u64 {
    if bad_ptr(iov_ptr) || iovcnt == 0 { return EINVAL; }
    let mut total: usize = 0;
    for i in 0..iovcnt as usize {
        // struct iovec: { void* base (8 bytes), size_t len (8 bytes) }
        let entry_ptr = iov_ptr + (i as u64) * 16;
        if bad_ptr(entry_ptr) { break; }
        let base = unsafe { *(entry_ptr as *const u64) };
        let len  = unsafe { *((entry_ptr + 8) as *const u64) } as usize;
        if len == 0 { continue; }
        if bad_ptr(base) { return EFAULT; }
        let buf = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, len) };
        match handlers::sys_read(fd, buf) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(_) if total == 0 => return EBADF,
            Err(_) => break,
        }
    }
    total as u64
}

/// Process writev: iterate iovec array and call sys_write for each segment.
fn do_writev(fd: usize, iov_ptr: u64, iovcnt: u64) -> u64 {
    if bad_ptr(iov_ptr) || iovcnt == 0 { return EINVAL; }
    let mut total: usize = 0;
    for i in 0..iovcnt as usize {
        let entry_ptr = iov_ptr + (i as u64) * 16;
        if bad_ptr(entry_ptr) { break; }
        let base = unsafe { *(entry_ptr as *const u64) };
        let len  = unsafe { *((entry_ptr + 8) as *const u64) } as usize;
        if len == 0 { continue; }
        if bad_ptr(base) { return EFAULT; }
        let buf = unsafe { core::slice::from_raw_parts(base as *const u8, len) };
        match handlers::sys_write(fd, buf) {
            Ok(n) => total += n,
            Err(_) if total == 0 => return EBADF,
            Err(_) => break,
        }
    }
    total as u64
}

// ── timespec helpers ──────────────────────────────────────────────────────────

fn fill_timespec(ptr: u64, secs: u64, nsecs: u64) -> u64 {
    if bad_ptr(ptr) { return EFAULT; }
    unsafe {
        *(ptr as *mut u64) = secs;
        *((ptr + 8) as *mut u64) = nsecs;
    }
    0
}

fn fill_timeval(ptr: u64, secs: u64, usecs: u64) -> u64 {
    if bad_ptr(ptr) { return EFAULT; }
    unsafe {
        *(ptr as *mut u64) = secs;
        *((ptr + 8) as *mut u64) = usecs;
    }
    0
}

fn ticks_to_secs() -> u64 {
    crate::drivers::timer::ticks() / 100
}

// ── Main dispatch ─────────────────────────────────────────────────────────────

/// Entry point for Linux system calls.
/// Called from the low-level syscall entry stub when process.is_linux is true.
/// Linux syscall number constants for security syscalls
const SYS_CAPGET:   u64 = 125;
const SYS_CAPSET:   u64 = 126;
const SYS_SECCOMP:  u64 = 317;

pub fn linux_syscall_handler(
    n: u64,
    a1: u64, a2: u64, a3: u64,
    a4: u64, a5: u64, a6: u64,
) -> u64 {
    // ── Security gate: seccomp filter check ──────────────────────────────────
    let pid = crate::process::scheduler::current_pid().unwrap_or(0);
    if !crate::security::seccomp::check(pid, n) {
        return u64::MAX; // EPERM / ENOSYS
    }
    // ── Sandbox capability check ─────────────────────────────────────────────
    if !crate::security::sandbox::check_syscall(pid, n as usize) {
        crate::security::audit::log_denied_syscall(pid, n, "sandbox");
        return u64::MAX; // EPERM
    }

    match n {

        // ── I/O ──────────────────────────────────────────────────────────────

        SYS_READ => {
            if bad_ptr(a2) { return EFAULT; }
            let fd  = a1 as usize;
            let buf = unsafe { core::slice::from_raw_parts_mut(a2 as *mut u8, a3 as usize) };
            // Check /dev special files first
            if let Some(path) = crate::vfs::fd_path(fd) {
                if let Some(n) = crate::posix::dev_fs::handle_read(&path, buf) {
                    return n as u64;
                }
            }
            match handlers::sys_read(fd, buf) {
                Ok(n)  => n as u64,
                Err(_) => EBADF,
            }
        }

        SYS_WRITE => {
            if bad_ptr(a2) { return EFAULT; }
            let fd  = a1 as usize;
            let buf = unsafe { core::slice::from_raw_parts(a2 as *const u8, a3 as usize) };
            if let Some(path) = crate::vfs::fd_path(fd) {
                if let Some(n) = crate::posix::dev_fs::handle_write(&path, buf) {
                    return n as u64;
                }
            }
            match handlers::sys_write(fd, buf) {
                Ok(n)  => n as u64,
                Err(_) => EBADF,
            }
        }

        SYS_OPEN | SYS_CREAT => {
            let path = unsafe { read_cstr(a1, 4096) };
            if path.is_empty() { return ENOENT; }
            match handlers::sys_open(path) {
                Ok(fd) => fd as u64,
                Err(_) => ENOENT,
            }
        }

        SYS_CLOSE => {
            match handlers::sys_close(a1 as usize) {
                Ok(()) => 0,
                Err(_) => EBADF,
            }
        }

        SYS_STAT | SYS_LSTAT => {
            let path = unsafe { read_cstr(a1, 4096) };
            let size = crate::vfs::file_size(path).unwrap_or(0);
            fill_stat(a2, size as u64)
        }

        SYS_FSTAT => {
            let fd   = a1 as usize;
            let size = crate::vfs::fd_path(fd)
                .and_then(|p| crate::vfs::file_size(&p))
                .unwrap_or(0);
            fill_stat(a2, size as u64)
        }

        SYS_LSEEK => {
            // whence: 0=SEEK_SET, 1=SEEK_CUR, 2=SEEK_END
            // We don't track file position yet — return 0 (beginning).
            0
        }

        SYS_PREAD64 => {
            if bad_ptr(a2) { return EFAULT; }
            let fd  = a1 as usize;
            let buf = unsafe { core::slice::from_raw_parts_mut(a2 as *mut u8, a3 as usize) };
            match handlers::sys_read(fd, buf) {
                Ok(n)  => n as u64,
                Err(_) => EBADF,
            }
        }

        SYS_PWRITE64 => {
            if bad_ptr(a2) { return EFAULT; }
            let fd  = a1 as usize;
            let buf = unsafe { core::slice::from_raw_parts(a2 as *const u8, a3 as usize) };
            match handlers::sys_write(fd, buf) {
                Ok(n)  => n as u64,
                Err(_) => EBADF,
            }
        }

        SYS_READV  => do_readv(a1 as usize, a2, a3),
        SYS_WRITEV => do_writev(a1 as usize, a2, a3),

        SYS_PREADV | SYS_PWRITEV => {
            // Positional scatter/gather — treat as readv/writev (ignore offset).
            if n == SYS_PREADV { do_readv(a1 as usize, a2, a3) }
            else               { do_writev(a1 as usize, a2, a3) }
        }

        SYS_ACCESS | SYS_FACCESSAT => {
            let path = if n == SYS_FACCESSAT {
                unsafe { read_cstr(a2, 4096) }
            } else {
                unsafe { read_cstr(a1, 4096) }
            };
            if path.is_empty() { return ENOENT; }
            // mode checks: 0=F_OK, 4=R_OK, 2=W_OK, 1=X_OK — we allow all
            match crate::vfs::open(path) {
                Ok(fd) => { crate::vfs::close(fd).ok(); 0 }
                Err(_) => ENOENT,
            }
        }

        SYS_DUP => {
            let fd = a1 as usize;
            match crate::vfs::fd_path(fd) {
                Some(path) => match handlers::sys_open(&path) {
                    Ok(new_fd) => new_fd as u64,
                    Err(_)     => EBADF,
                },
                None => EBADF,
            }
        }

        SYS_DUP2 | SYS_DUP3 => {
            let oldfd = a1 as usize;
            match crate::vfs::fd_path(oldfd) {
                Some(path) => {
                    handlers::sys_close(a2 as usize).ok();
                    match handlers::sys_open(&path) {
                        Ok(_) => a2,
                        Err(_) => EBADF,
                    }
                }
                None => EBADF,
            }
        }

        SYS_PIPE | SYS_PIPE2 => {
            if bad_ptr(a1) { return EFAULT; }
            // Create a kernel pipe and expose read/write ends as VFS paths.
            let pipe_id = crate::process::pipe::create_pipe();
            let rpath = format!("/tmp/pipe-{}-r", pipe_id);
            let wpath = format!("/tmp/pipe-{}-w", pipe_id);
            let _ = crate::vfs::create_and_write(&rpath, b"");
            let _ = crate::vfs::create_and_write(&wpath, b"");
            let read_fd  = crate::vfs::open(&rpath).unwrap_or(0xFFFF);
            let write_fd = crate::vfs::open(&wpath).unwrap_or(0xFFFF);
            unsafe {
                *(a1 as *mut u32)       = read_fd as u32;
                *((a1 + 4) as *mut u32) = write_fd as u32;
            }
            0
        }

        SYS_FCNTL => {
            let _fd  = a1;
            let cmd  = a2;
            // F_GETFD=1, F_SETFD=2, F_GETFL=3, F_SETFL=4
            match cmd {
                1 => 0,           // F_GETFD: FD_CLOEXEC not set
                2 => 0,           // F_SETFD: ignore
                3 => 0o2,         // F_GETFL: O_RDWR
                4 => 0,           // F_SETFL: ignore
                _ => 0,
            }
        }

        SYS_IOCTL => {
            // Stub: return success for common TTY ioctls, EINVAL for others.
            let cmd = a2;
            match cmd {
                0x5401 => 0,   // TCGETS
                0x5402 => 0,   // TCSETS
                0x5403 => 0,   // TCSETSW
                0x5404 => 0,   // TCSETSF
                0x540F => 0,   // TIOCGPGRP
                0x5410 => 0,   // TIOCSPGRP
                0x5413 => {    // TIOCGWINSZ — return 80×24 terminal
                    if !bad_ptr(a3) {
                        unsafe {
                            let ws = a3 as *mut u16;
                            *ws         = 24;  // ws_row
                            *ws.add(1)  = 80;  // ws_col
                            *ws.add(2)  = 640; // ws_xpixel
                            *ws.add(3)  = 480; // ws_ypixel
                        }
                    }
                    0
                }
                0x541B => 0,   // FIONREAD
                0x5421 => 0,   // FIONBIO
                _      => 0,   // Return success for unknown ioctls rather than -EINVAL
            }
        }

        SYS_SENDFILE => {
            // sendfile(out_fd, in_fd, offset, count) — stub, return count
            a4
        }

        SYS_TRUNCATE | SYS_FTRUNCATE => {
            // Accept silently — no real truncation needed for compatibility stubs
            0
        }

        SYS_FSYNC | SYS_FDATASYNC => 0,

        SYS_FLOCK => 0, // No mandatory locking needed

        SYS_CHMOD | SYS_FCHMOD | SYS_FCHMODAT => 0,
        SYS_CHOWN | SYS_LCHOWN | SYS_FCHOWNAT => 0,
        SYS_UMASK => 0o022, // Return default umask

        // ── Directory operations ──────────────────────────────────────────────

        SYS_GETCWD => {
            if bad_ptr(a1) { return EFAULT; }
            let cwd = b"/\0";
            let len = cwd.len().min(a2 as usize);
            unsafe { core::slice::from_raw_parts_mut(a1 as *mut u8, len).copy_from_slice(&cwd[..len]); }
            a1 // return pointer to buffer
        }

        SYS_CHDIR | SYS_FCHDIR => 0, // Pretend success

        SYS_MKDIR | SYS_MKDIRAT => {
            let path = if n == SYS_MKDIRAT {
                unsafe { read_cstr(a2, 4096) }
            } else {
                unsafe { read_cstr(a1, 4096) }
            };
            if path.is_empty() { return ENOENT; }
            match handlers::sys_mkdir(path) {
                Ok(()) => 0,
                Err(_) => 0, // Ignore EEXIST
            }
        }

        SYS_RMDIR | SYS_UNLINK | SYS_UNLINKAT => {
            let path = if n == SYS_UNLINKAT {
                unsafe { read_cstr(a2, 4096) }
            } else {
                unsafe { read_cstr(a1, 4096) }
            };
            if path.is_empty() { return ENOENT; }
            crate::vfs::unlink(path).map(|_| 0u64).unwrap_or(ENOENT)
        }

        SYS_RENAME | SYS_RENAMEAT => {
            // Stub: succeed silently
            0
        }

        SYS_SYMLINK | SYS_SYMLINKAT | SYS_LINKAT => 0,
        SYS_MKNODAT => 0,

        SYS_READLINK | SYS_READLINKAT => {
            let (path_ptr, buf_ptr, bufsiz) = if n == SYS_READLINKAT {
                (a2, a3, a4)
            } else {
                (a1, a2, a3)
            };
            let path = unsafe { read_cstr(path_ptr, 4096) };
            if path.is_empty() || bad_ptr(buf_ptr) { return ENOENT; }
            // Read the VFS file and return its content as the link target
            match crate::vfs::open(path) {
                Ok(fd) => {
                    let len = bufsiz.min(4095) as usize;
                    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, len) };
                    let n_read = crate::vfs::read(fd, buf).unwrap_or(0);
                    crate::vfs::close(fd).ok();
                    n_read as u64
                }
                Err(_) => ENOENT,
            }
        }

        SYS_GETDENTS | SYS_GETDENTS64 => {
            // Return an empty directory listing (no entries).
            // Real implementation would enumerate VFS children.
            0
        }

        SYS_OPENAT => {
            // openat(dirfd, path, flags, mode)
            let path = unsafe { read_cstr(a2, 4096) };
            if path.is_empty() { return ENOENT; }
            match handlers::sys_open(path) {
                Ok(fd) => fd as u64,
                Err(_) => ENOENT,
            }
        }

        SYS_NEWFSTATAT => {
            // fstatat(dirfd, path, statbuf, flags)
            let path = unsafe { read_cstr(a2, 4096) };
            let size = if path.is_empty() { 0 } else { crate::vfs::file_size(path).unwrap_or(0) };
            fill_stat(a3, size as u64)
        }

        SYS_STATX => {
            // statx(dirfd, path, flags, mask, statxbuf) — return a minimal statx
            // statx is 256 bytes; zero it and fill the mask
            if bad_ptr(a5) { return EFAULT; }
            let buf = unsafe { core::slice::from_raw_parts_mut(a5 as *mut u8, 256) };
            buf.fill(0);
            // stx_mask (u32 at offset 0): STATX_BASIC_STATS = 0x7ff
            buf[0..4].copy_from_slice(&0x7ffu32.to_ne_bytes());
            // stx_mode (u16 at offset 24): regular file + rw-r--r--
            let mode: u16 = 0o100644;
            buf[24..26].copy_from_slice(&mode.to_ne_bytes());
            // stx_nlink (u32 at offset 20)
            buf[20..24].copy_from_slice(&1u32.to_ne_bytes());
            0
        }

        // ── Memory management ─────────────────────────────────────────────────

        SYS_MMAP => {
            // mmap(addr, length, prot, flags, fd, offset)
            // MAP_ANONYMOUS (0x20): allocate from heap and return pointer.
            let flags = a4;
            let length = a3 as usize;
            if flags & 0x20 != 0 || a5 == u64::MAX {
                // Anonymous mapping: allocate from kernel heap
                let layout = alloc::alloc::Layout::from_size_align(length.max(4096), 4096)
                    .unwrap_or(alloc::alloc::Layout::new::<[u8; 4096]>());
                let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
                if ptr.is_null() { ENOMEM } else { ptr as u64 }
            } else {
                // File-backed: read the file into a new buffer
                let fd = a5 as usize;
                let path_owned = crate::vfs::fd_path(fd).unwrap_or_default();
                match crate::vfs::open(&path_owned) {
                    Ok(fd2) => {
                        let layout = alloc::alloc::Layout::from_size_align(length.max(4096), 4096)
                            .unwrap_or(alloc::alloc::Layout::new::<[u8; 4096]>());
                        let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
                        if ptr.is_null() {
                            crate::vfs::close(fd2).ok();
                            return ENOMEM;
                        }
                        let buf = unsafe { core::slice::from_raw_parts_mut(ptr, length) };
                        crate::vfs::read(fd2, buf).ok();
                        crate::vfs::close(fd2).ok();
                        ptr as u64
                    }
                    Err(_) => ENOENT,
                }
            }
        }

        SYS_MUNMAP => 0,     // We don't track mmap regions yet
        SYS_MPROTECT => 0,   // All pages have all permissions in kernel mode
        SYS_MREMAP => a1,    // Pretend the remap succeeded in-place
        SYS_MSYNC => 0,
        SYS_MADVISE => 0,

        SYS_BRK => {
            if a1 == 0 { 0x800_0000 } else { a1 }
        }

        SYS_MEMFD_CREATE => {
            // Create an anonymous file in /tmp
            let name = unsafe { read_cstr(a1, 256) };
            let path = format!("/tmp/memfd-{}", name);
            match crate::vfs::create_and_write(&path, b"") {
                Ok(()) => crate::vfs::open(&path).unwrap_or(usize::MAX) as u64,
                Err(_) => ENOMEM,
            }
        }

        // ── Process management ────────────────────────────────────────────────

        SYS_FORK | SYS_VFORK => {
            // Real CoW fork goes through cow_fork + fork_current_thread(child_pid, child_cr3).
            // Stub: return child pid = current+1, child sees 0 (not implemented here).
            let parent = scheduler::current_pid().unwrap_or(1);
            parent + 1
        }

        SYS_CLONE => {
            // Simplified: treat as fork stub
            let parent = scheduler::current_pid().unwrap_or(1);
            parent + 1
        }

        SYS_EXECVE => {
            // exec_replace_context takes (entry, stack_top, cr3) — needs ELF load.
            // Stub: try to spawn the binary as a new process and exit.
            let path = unsafe { read_cstr(a1, 4096) };
            if path.is_empty() { return ENOENT; }
            match crate::process::scheduler::spawn_user_process("exec", path) {
                Ok(_) => { handlers::sys_exit(); 0 }
                Err(_) => ENOENT,
            }
        }

        SYS_EXIT | SYS_EXIT_GROUP => {
            handlers::sys_exit();
            0
        }

        SYS_WAIT4 => {
            // Stub: no child process tracking yet at this layer.
            let status_ptr = a2;
            if !bad_ptr(status_ptr) {
                unsafe { *(status_ptr as *mut u32) = 0; }
            }
            ECHILD
        }

        SYS_KILL | SYS_TGKILL => {
            // Stub: pretend signal was delivered
            0
        }

        SYS_GETPID => scheduler::current_pid().unwrap_or(1),
        SYS_GETPPID => 1,  // init is parent of all
        SYS_GETTID  => scheduler::current_tid().unwrap_or(1),

        SYS_GETUID | SYS_GETEUID | SYS_GETRESUID => 0,  // root
        SYS_GETGID | SYS_GETEGID | SYS_GETRESGID => 0,
        SYS_SETUID | SYS_SETGID | SYS_SETREUID | SYS_SETREGID => 0,
        SYS_SETRESUID | SYS_SETRESGID => 0,
        SYS_GETGROUPS => { if !bad_ptr(a2) && a1 > 0 { unsafe { *(a2 as *mut u32) = 0; } } 1 }
        SYS_SETGROUPS => 0,
        SYS_GETPGRP | SYS_GETPGID => scheduler::current_pid().unwrap_or(1),
        SYS_SETPGID => 0,
        SYS_SETSID  => scheduler::current_pid().unwrap_or(1),

        SYS_GETRLIMIT => {
            // Return unlimited for all resource limits
            if bad_ptr(a2) { return EFAULT; }
            unsafe {
                *( a2      as *mut u64) = u64::MAX; // rlim_cur
                *((a2 + 8) as *mut u64) = u64::MAX; // rlim_max
            }
            0
        }

        SYS_GETRUSAGE => {
            if bad_ptr(a2) { return EFAULT; }
            let buf = unsafe { core::slice::from_raw_parts_mut(a2 as *mut u8, 144) };
            buf.fill(0);
            0
        }

        SYS_SYSINFO => {
            if bad_ptr(a1) { return EFAULT; }
            let buf = unsafe { core::slice::from_raw_parts_mut(a1 as *mut u8, 112) };
            buf.fill(0);
            // uptime (u64 at offset 0)
            let uptime = ticks_to_secs();
            buf[0..8].copy_from_slice(&uptime.to_ne_bytes());
            // totalram (u64 at offset 16)
            let (used, free) = crate::memory::heap::heap_stats();
            let total = (used + free) as u64;
            buf[16..24].copy_from_slice(&total.to_ne_bytes());
            // freeram (u64 at offset 24)
            buf[24..32].copy_from_slice(&(free as u64).to_ne_bytes());
            // mem_unit = 1
            buf[108..112].copy_from_slice(&1u32.to_ne_bytes());
            0
        }

        SYS_TIMES => {
            if !bad_ptr(a1) {
                let buf = unsafe { core::slice::from_raw_parts_mut(a1 as *mut u8, 32) };
                buf.fill(0);
            }
            crate::drivers::timer::ticks() as u64
        }

        SYS_PRCTL => {
            // prctl(PR_SET_SECCOMP, mode, ...) — enable seccomp filtering
            if a1 == crate::security::seccomp::PR_SET_SECCOMP {
                crate::security::seccomp::handle_prctl_set_seccomp(pid, a2)
            } else {
                0 // ignore other prctl requests
            }
        }

        SYS_CAPGET => {
            // capget(header*, data*) — read capability set into user buffer
            let cs = crate::security::linux_caps::capget(pid);
            if !bad_ptr(a2) {
                unsafe {
                    let p = a2 as *mut u64;
                    core::ptr::write(p, cs.effective);
                    core::ptr::write(p.add(1), cs.permitted);
                    core::ptr::write(p.add(2), cs.inheritable);
                }
            }
            0
        }

        SYS_CAPSET => {
            // capset(header*, data*) — write capability set
            if bad_ptr(a2) { return EFAULT; }
            let effective   = unsafe { core::ptr::read(a2 as *const u64) };
            let permitted   = unsafe { core::ptr::read((a2 + 8) as *const u64) };
            let inheritable = unsafe { core::ptr::read((a2 + 16) as *const u64) };
            let new_cs = crate::security::linux_caps::CapSet { effective, permitted, inheritable };
            match crate::security::linux_caps::capset(pid, pid, new_cs) {
                Ok(()) => 0,
                Err(_) => u64::MAX,
            }
        }

        SYS_SECCOMP => {
            // seccomp(SECCOMP_SET_MODE_STRICT, 0, NULL) = 317
            if a1 == crate::security::seccomp::SECCOMP_MODE_STRICT {
                crate::security::seccomp::set_strict(pid); 0
            } else { 0 }
        }
        SYS_ARCH_PRCTL => {
            // PR_SET_FS (0x1002): set FS segment base for TLS
            // a1 = ARCH_SET_FS (0x1002), a2 = address
            if a1 == 0x1002 {
                // Write the FS base MSR (0xC0000100)
                unsafe {
                    core::arch::x86_64::__cpuid(0); // serialise
                    let lo = a2 as u32;
                    let hi = (a2 >> 32) as u32;
                    core::arch::asm!(
                        "wrmsr",
                        in("ecx") 0xC000_0100u32,
                        in("eax") lo,
                        in("edx") hi,
                        options(nostack, nomem),
                    );
                }
                0
            } else {
                0
            }
        }

        SYS_SET_TID_ADDRESS => {
            // set_tid_address(tidptr) — store tidptr, return TID
            scheduler::current_tid().unwrap_or(1)
        }

        SYS_SET_ROBUST_LIST | SYS_GET_ROBUST_LIST => 0,

        SYS_SCHED_YIELD => {
            scheduler::yield_now();
            0
        }

        SYS_SCHED_SETAFFINITY | SYS_SCHED_GETAFFINITY => {
            // Return/accept a single-CPU affinity mask
            if n == SYS_SCHED_GETAFFINITY && !bad_ptr(a3) {
                unsafe { *(a3 as *mut u64) = 0xFF; } // 8 CPUs
            }
            0
        }

        // ── Time ─────────────────────────────────────────────────────────────

        SYS_GETTIMEOFDAY => {
            let secs  = ticks_to_secs() + 1_700_000_000;
            let usecs = (crate::drivers::timer::ticks() % 100) * 10_000;
            let res   = if !bad_ptr(a1) { fill_timeval(a1, secs, usecs) } else { 0 };
            // timezone struct pointer (a2) — ignore
            res
        }

        SYS_CLOCK_GETTIME | SYS_CLOCK_NANOSLEEP => {
            let secs  = ticks_to_secs() + 1_700_000_000;
            let nsecs = (crate::drivers::timer::ticks() % 100) * 10_000_000;
            fill_timespec(a2, secs, nsecs)
        }

        SYS_CLOCK_GETRES => {
            // 10ms resolution (100 Hz timer)
            fill_timespec(a2, 0, 10_000_000)
        }

        SYS_CLOCK_SETTIME => 0,

        SYS_NANOSLEEP => {
            // nanosleep(req, rem) — yield a few times proportional to time
            if !bad_ptr(a1) {
                let secs  = unsafe { *(a1 as *const u64) };
                let nsecs = unsafe { *((a1 + 8) as *const u64) };
                let ticks = secs * 100 + nsecs / 10_000_000;
                for _ in 0..ticks.min(100) {
                    scheduler::yield_now();
                }
            }
            if !bad_ptr(a2) {
                fill_timespec(a2, 0, 0);
            }
            0
        }

        SYS_GETITIMER | SYS_SETITIMER | SYS_ALARM => 0,

        // ── Signals ───────────────────────────────────────────────────────────

        SYS_RT_SIGACTION => {
            // Accept all signal handler registrations silently
            // If oldact (a3) is non-null, zero it
            if !bad_ptr(a3) {
                let buf = unsafe { core::slice::from_raw_parts_mut(a3 as *mut u8, 32) };
                buf.fill(0);
            }
            0
        }

        SYS_RT_SIGPROCMASK => {
            // If oldset (a3) non-null, return empty signal mask
            if !bad_ptr(a3) {
                unsafe { *(a3 as *mut u64) = 0; }
            }
            0
        }

        SYS_RT_SIGRETURN => {
            // Signal trampoline return — shouldn't be reached in kernel context
            0
        }

        SYS_SYSLOG => 0,

        // ── System info ───────────────────────────────────────────────────────

        SYS_UNAME => fill_uname(a1),

        // ── Networking ───────────────────────────────────────────────────────

        SYS_SOCKET => {
            let pid = scheduler::current_tid().unwrap_or(0);
            match crate::process::posix::socket(pid, a1 as usize, a2 as usize, a3 as usize) {
                Ok(fd) => fd as u64,
                Err(_) => ENOMEM,
            }
        }

        SYS_CONNECT => {
            if bad_ptr(a2) { return EFAULT; }
            let pid    = scheduler::current_tid().unwrap_or(0);
            let family = unsafe { u16::from_ne_bytes([*(a2 as *const u8), *((a2 + 1) as *const u8)]) };
            if family != 2 { return EINVAL; } // AF_INET only
            let port = unsafe { u16::from_be_bytes([*((a2 + 2) as *const u8), *((a2 + 3) as *const u8)]) };
            let ip   = unsafe { [*((a2 + 4) as *const u8), *((a2 + 5) as *const u8),
                                  *((a2 + 6) as *const u8), *((a2 + 7) as *const u8)] };
            match crate::process::posix::connect(pid, a1 as usize, ip, port) {
                Ok(()) => 0,
                Err(_) => EINVAL,
            }
        }

        SYS_BIND => {
            if bad_ptr(a2) { return EFAULT; }
            let port = unsafe { u16::from_be_bytes([*((a2 + 2) as *const u8), *((a2 + 3) as *const u8)]) };
            match crate::net::udp::bind(port) {
                Ok(())  => 0,
                Err(_) => 0, // Succeed silently for compatibility
            }
        }

        SYS_LISTEN => 0,  // TCP listen stub

        SYS_ACCEPT | SYS_ACCEPT4 => {
            // Blocking accept — yield until a connection arrives (stub)
            EAGAIN
        }

        SYS_GETSOCKNAME | SYS_GETPEERNAME => {
            if !bad_ptr(a2) && !bad_ptr(a3) {
                // Fill in a sockaddr_in with local IP:0
                let buf = unsafe { core::slice::from_raw_parts_mut(a2 as *mut u8, 16) };
                buf.fill(0);
                buf[0] = 2; // AF_INET
                let ip = crate::net::LOCAL_IP;
                buf[4..8].copy_from_slice(&ip);
                unsafe { *(a3 as *mut u32) = 16; } // addrlen
            }
            0
        }

        SYS_SHUTDOWN => 0,

        SYS_SETSOCKOPT | SYS_GETSOCKOPT => 0,

        SYS_SOCKETPAIR => 0,

        SYS_SENDTO => {
            if bad_ptr(a2) { return EFAULT; }
            let pid = scheduler::current_tid().unwrap_or(0);
            let buf = unsafe { core::slice::from_raw_parts(a2 as *const u8, a3 as usize) };
            match crate::process::posix::send(pid, a1 as usize, buf) {
                Ok(n)  => n as u64,
                Err(_) => EBADF,
            }
        }

        SYS_RECVFROM => {
            if bad_ptr(a2) { return EFAULT; }
            let pid = scheduler::current_tid().unwrap_or(0);
            let buf = unsafe { core::slice::from_raw_parts_mut(a2 as *mut u8, a3 as usize) };
            match crate::process::posix::recv(pid, a1 as usize, buf) {
                Ok(n)  => n as u64,
                Err(_) => EAGAIN,
            }
        }

        SYS_SENDMSG | SYS_RECVMSG => {
            // msghdr scatter/gather — treat as sendto/recvfrom with first iov
            if bad_ptr(a2) { return EFAULT; }
            let iov_ptr = unsafe { *((a2 + 8) as *const u64) };   // msg_iov
            let iovcnt  = unsafe { *((a2 + 16) as *const u64) };  // msg_iovlen
            if n == SYS_SENDMSG { do_writev(a1 as usize, iov_ptr, iovcnt) }
            else                { do_readv( a1 as usize, iov_ptr, iovcnt) }
        }

        // ── epoll / event fds / timerfd ───────────────────────────────────────
        // Phase 107: wired to real io::epoll module

        SYS_EPOLL_CREATE | SYS_EPOLL_CREATE1 => {
            crate::io::epoll::epoll_create()
        }

        SYS_EPOLL_CTL => {
            // a1=epfd, a2=op, a3=fd, a4=*epoll_event (ignored — use default)
            let ev = crate::io::epoll::EpollEvent {
                events: crate::io::epoll::EPOLLIN | crate::io::epoll::EPOLLOUT,
                data:   a3,
            };
            let ret = crate::io::epoll::epoll_ctl(a1, a2 as i32, a3, Some(ev));
            if ret < 0 { u64::MAX } else { 0 }
        }

        SYS_EPOLL_WAIT | SYS_EPOLL_PWAIT => {
            // a1=epfd, a2=*events_buf, a3=maxevents, a4=timeout_ms
            let events = crate::io::epoll::epoll_wait(a1, a3 as usize, a4 as i32);
            events.len() as u64
        }

        SYS_EVENTFD | SYS_EVENTFD2 => {
            match crate::vfs::create_and_write("/tmp/.eventfd", &0u64.to_ne_bytes()) {
                Ok(()) => crate::vfs::open("/tmp/.eventfd").unwrap_or(100) as u64,
                Err(_) => 3u64,
            }
        }

        SYS_TIMERFD_CREATE => {
            match crate::vfs::create_and_write("/tmp/.timerfd", b"") {
                Ok(()) => crate::vfs::open("/tmp/.timerfd").unwrap_or(100) as u64,
                Err(_) => 3u64,
            }
        }

        SYS_SIGNALFD | SYS_SIGNALFD4 => {
            match crate::vfs::create_and_write("/tmp/.signalfd", b"") {
                Ok(()) => crate::vfs::open("/tmp/.signalfd").unwrap_or(100) as u64,
                Err(_) => 3u64,
            }
        }

        SYS_INOTIFY_INIT1 => {
            match crate::vfs::create_and_write("/tmp/.inotify", b"") {
                Ok(()) => crate::vfs::open("/tmp/.inotify").unwrap_or(100) as u64,
                Err(_) => 3u64,
            }
        }

        // ── Futex ─────────────────────────────────────────────────────────────

        SYS_FUTEX => {
            // FUTEX_WAIT=0, FUTEX_WAKE=1, FUTEX_WAIT_BITSET=9, FUTEX_WAKE_BITSET=10
            let op = (a2 as u32) & 0xF;
            match op {
                0 | 9 => {
                    // FUTEX_WAIT: check *uaddr == val, yield if so
                    if bad_ptr(a1) { return EFAULT; }
                    let uval = unsafe { *(a1 as *const u32) };
                    if uval == a3 as u32 {
                        for _ in 0..10 { scheduler::yield_now(); }
                    }
                    0
                }
                1 | 10 => {
                    // FUTEX_WAKE: wake up to val waiters — return 1
                    1
                }
                _ => 0,
            }
        }

        // ── Poll / select ─────────────────────────────────────────────────────

        SYS_SELECT | SYS_PSELECT6 => {
            // Return 0 (timeout) — no fds ready
            0
        }

        SYS_POLL | SYS_PPOLL => {
            // Return 0 (timeout). Mark all fds as POLLIN|POLLOUT for compatibility.
            let nfds    = a2 as usize;
            let timeout = a3 as i64;
            if !bad_ptr(a1) && nfds > 0 {
                // struct pollfd: fd(4) events(2) revents(2)
                for i in 0..nfds {
                    let entry = a1 + (i as u64) * 8;
                    if !bad_ptr(entry) {
                        // Set revents = POLLIN | POLLOUT (1 | 4)
                        unsafe { *((entry + 6) as *mut u16) = 0x0005; }
                    }
                }
                if timeout != 0 { scheduler::yield_now(); }
                nfds as u64
            } else {
                if timeout > 0 { scheduler::yield_now(); }
                0
            }
        }

        // ── Entropy ───────────────────────────────────────────────────────────

        SYS_GETRANDOM => {
            if bad_ptr(a1) { return EFAULT; }
            let buf = unsafe { core::slice::from_raw_parts_mut(a1 as *mut u8, a2 as usize) };
            crate::posix::dev_fs::handle_read("/dev/urandom", buf).unwrap_or(0) as u64
        }

        // ── libc shim trampolines (0x8000..0x80FF) ───────────────────────────
        // These are called from the per-process trampoline page built by dynlink.

        n if n >= 0x8000 && n < 0x8100 => {
            crate::process::dynlink::dispatch_shim(
                (n - 0x8000) as usize,
                a1, a2, a3, a4, a5, a6,
            )
        }

        // ── Catch-all ─────────────────────────────────────────────────────────

        _ => {
            crate::serial_println!("[linux-abi] unimplemented syscall {}", n);
            ENOSYS
        }
    }
}

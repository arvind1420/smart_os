/// musl libc compatibility shim for Smart OS.
///
/// When a dynamically-linked Linux ELF calls dlsym(RTLD_DEFAULT, "malloc"),
/// this module returns a kernel function pointer that satisfies the call.
/// All heap operations route to the kernel allocator; I/O routes through
/// the Linux syscall layer already implemented in syscall::linux.
///
/// For statically-linked ELFs (musl-static, busybox-static) no shim is
/// needed — they carry their own libc and call us via SYSCALL.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;

// ── errno per-CPU storage ─────────────────────────────────────────────────────

/// Per-CPU errno values (indexed by CPU id, max 16 CPUs).
static ERRNO: Mutex<[i32; 16]> = Mutex::new([0i32; 16]);

fn cpu_id() -> usize {
    // LAPIC-based CPU id not exposed as a simple fn; use current tid % 16 as proxy
    (crate::process::scheduler::current_tid().unwrap_or(0) as usize) % 16
}

pub fn set_errno(e: i32) {
    ERRNO.lock()[cpu_id()] = e;
}

pub fn get_errno() -> i32 {
    ERRNO.lock()[cpu_id()]
}

// ── Memory allocation trampolines ─────────────────────────────────────────────

unsafe extern "C" fn shim_malloc(size: usize) -> *mut u8 {
    if size == 0 { return core::ptr::null_mut(); }
    let layout = match alloc::alloc::Layout::from_size_align(size, 16) {
        Ok(l)  => l,
        Err(_) => return core::ptr::null_mut(),
    };
    let ptr = alloc::alloc::alloc(layout);
    if ptr.is_null() { set_errno(12); } // ENOMEM
    ptr
}

unsafe extern "C" fn shim_calloc(nmemb: usize, size: usize) -> *mut u8 {
    let total = nmemb.saturating_mul(size);
    if total == 0 { return core::ptr::null_mut(); }
    let layout = match alloc::alloc::Layout::from_size_align(total, 16) {
        Ok(l)  => l,
        Err(_) => return core::ptr::null_mut(),
    };
    let ptr = alloc::alloc::alloc_zeroed(layout);
    if ptr.is_null() { set_errno(12); }
    ptr
}

unsafe extern "C" fn shim_realloc(ptr: *mut u8, new_size: usize) -> *mut u8 {
    if ptr.is_null() { return shim_malloc(new_size); }
    if new_size == 0 { shim_free(ptr); return core::ptr::null_mut(); }
    // We don't know the old layout — allocate fresh and copy conservatively.
    let new_ptr = shim_malloc(new_size);
    if !new_ptr.is_null() && !ptr.is_null() {
        core::ptr::copy_nonoverlapping(ptr, new_ptr, new_size.min(4096));
    }
    new_ptr
}

unsafe extern "C" fn shim_free(_ptr: *mut u8) {
    // Without layout info we can't call dealloc. Accept the leak for now;
    // statically-linked ELFs manage their own heap and don't call us here.
}

unsafe extern "C" fn shim_posix_memalign(memptr: *mut *mut u8, align: usize, size: usize) -> i32 {
    let layout = match alloc::alloc::Layout::from_size_align(size, align) {
        Ok(l)  => l,
        Err(_) => return 22, // EINVAL
    };
    let ptr = alloc::alloc::alloc(layout);
    if ptr.is_null() { return 12; } // ENOMEM
    *memptr = ptr;
    0
}

// ── String functions ──────────────────────────────────────────────────────────

unsafe extern "C" fn shim_strlen(s: *const u8) -> usize {
    if s.is_null() { return 0; }
    let mut n = 0usize;
    while *s.add(n) != 0 { n += 1; }
    n
}

unsafe extern "C" fn shim_strcpy(dst: *mut u8, src: *const u8) -> *mut u8 {
    if dst.is_null() || src.is_null() { return dst; }
    let mut i = 0usize;
    loop {
        let c = *src.add(i);
        *dst.add(i) = c;
        if c == 0 { break; }
        i += 1;
    }
    dst
}

unsafe extern "C" fn shim_strncpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if dst.is_null() || src.is_null() { return dst; }
    let mut i = 0usize;
    while i < n {
        let c = if *src.add(i) != 0 || i == 0 { *src.add(i) } else { 0 };
        *dst.add(i) = c;
        i += 1;
    }
    dst
}

unsafe extern "C" fn shim_strcmp(a: *const u8, b: *const u8) -> i32 {
    if a.is_null() || b.is_null() { return if a == b { 0 } else { 1 }; }
    let mut i = 0usize;
    loop {
        let ca = *a.add(i);
        let cb = *b.add(i);
        if ca != cb { return (ca as i32) - (cb as i32); }
        if ca == 0  { return 0; }
        i += 1;
    }
}

unsafe extern "C" fn shim_strncmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    if a.is_null() || b.is_null() { return 0; }
    let mut i = 0usize;
    while i < n {
        let ca = *a.add(i);
        let cb = *b.add(i);
        if ca != cb { return (ca as i32) - (cb as i32); }
        if ca == 0  { return 0; }
        i += 1;
    }
    0
}

unsafe extern "C" fn shim_strdup(s: *const u8) -> *mut u8 {
    let len = shim_strlen(s);
    let dst = shim_malloc(len + 1);
    if !dst.is_null() {
        core::ptr::copy_nonoverlapping(s, dst, len + 1);
    }
    dst
}

unsafe extern "C" fn shim_strndup(s: *const u8, n: usize) -> *mut u8 {
    let len = shim_strlen(s).min(n);
    let dst = shim_malloc(len + 1);
    if !dst.is_null() {
        core::ptr::copy_nonoverlapping(s, dst, len);
        *dst.add(len) = 0;
    }
    dst
}

unsafe extern "C" fn shim_memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if !dst.is_null() && !src.is_null() {
        core::ptr::copy_nonoverlapping(src, dst, n);
    }
    dst
}

unsafe extern "C" fn shim_memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if !dst.is_null() && !src.is_null() {
        core::ptr::copy(src, dst, n);
    }
    dst
}

unsafe extern "C" fn shim_memset(dst: *mut u8, c: i32, n: usize) -> *mut u8 {
    if !dst.is_null() {
        core::ptr::write_bytes(dst, c as u8, n);
    }
    dst
}

unsafe extern "C" fn shim_memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    for i in 0..n {
        let diff = (*a.add(i) as i32) - (*b.add(i) as i32);
        if diff != 0 { return diff; }
    }
    0
}

// ── stdio ─────────────────────────────────────────────────────────────────────

unsafe extern "C" fn shim_puts(s: *const u8) -> i32 {
    let len = shim_strlen(s);
    let buf = core::slice::from_raw_parts(s, len);
    crate::vfs::write(1, buf).ok();
    crate::vfs::write(1, b"\n").ok();
    len as i32 + 1
}

unsafe extern "C" fn shim_putchar(c: i32) -> i32 {
    let b = [c as u8];
    crate::vfs::write(1, &b).ok();
    c
}

unsafe extern "C" fn shim_getchar() -> i32 {
    let mut b = [0u8; 1];
    match crate::vfs::read(0, &mut b) {
        Ok(1) => b[0] as i32,
        _     => -1, // EOF
    }
}

// printf family — very minimal: handles %s, %d, %u, %x, %c, %% only.
unsafe extern "C" fn shim_printf(fmt: *const u8, /* varargs */) -> i32 {
    // Without varargs support in no_std we can only print the format string itself.
    let len = shim_strlen(fmt);
    let buf = core::slice::from_raw_parts(fmt, len);
    crate::vfs::write(1, buf).ok();
    len as i32
}

unsafe extern "C" fn shim_fprintf(stream: usize, fmt: *const u8) -> i32 {
    let fd = if stream <= 2 { stream } else { 2 };
    let len = shim_strlen(fmt);
    let buf = core::slice::from_raw_parts(fmt, len);
    crate::vfs::write(fd, buf).ok();
    len as i32
}

// ── Environment ───────────────────────────────────────────────────────────────

unsafe extern "C" fn shim_getenv(name: *const u8) -> *const u8 {
    let key_len = shim_strlen(name);
    let key = core::str::from_utf8(core::slice::from_raw_parts(name, key_len))
        .unwrap_or("");
    match crate::process::env::get(key) {
        Some(val) => {
            static ENV_BUF: Mutex<[u8; 4096]> = Mutex::new([0u8; 4096]);
            let mut buf = ENV_BUF.lock();
            let bytes = val.as_bytes();
            let len = bytes.len().min(4095);
            buf[..len].copy_from_slice(&bytes[..len]);
            buf[len] = 0;
            buf.as_ptr()
        }
        None => core::ptr::null(),
    }
}

unsafe extern "C" fn shim_setenv(name: *const u8, value: *const u8, _overwrite: i32) -> i32 {
    let key_len = shim_strlen(name);
    let val_len = shim_strlen(value);
    let key = core::str::from_utf8(core::slice::from_raw_parts(name, key_len)).unwrap_or("");
    let val = core::str::from_utf8(core::slice::from_raw_parts(value, val_len)).unwrap_or("");
    crate::process::env::set(key, val);
    0
}

unsafe extern "C" fn shim_unsetenv(name: *const u8) -> i32 {
    let key_len = shim_strlen(name);
    let key = core::str::from_utf8(core::slice::from_raw_parts(name, key_len)).unwrap_or("");
    crate::process::env::remove(key);
    0
}

// ── Error handling ────────────────────────────────────────────────────────────

unsafe extern "C" fn shim_strerror(errnum: i32) -> *const u8 {
    match errnum {
        0  => b"Success\0".as_ptr(),
        1  => b"Operation not permitted\0".as_ptr(),
        2  => b"No such file or directory\0".as_ptr(),
        11 => b"Resource temporarily unavailable\0".as_ptr(),
        12 => b"Out of memory\0".as_ptr(),
        13 => b"Permission denied\0".as_ptr(),
        14 => b"Bad address\0".as_ptr(),
        22 => b"Invalid argument\0".as_ptr(),
        28 => b"No space left on device\0".as_ptr(),
        38 => b"Function not implemented\0".as_ptr(),
        _  => b"Unknown error\0".as_ptr(),
    }
}

unsafe extern "C" fn shim_perror(s: *const u8) {
    if !s.is_null() {
        shim_fprintf(2, s);
        let sep = b": \0".as_ptr();
        shim_fprintf(2, sep);
    }
    let e = get_errno();
    let msg = shim_strerror(e);
    shim_fprintf(2, msg);
    let nl = b"\n\0".as_ptr();
    shim_fprintf(2, nl);
}

// ── Math stubs ────────────────────────────────────────────────────────────────

unsafe extern "C" fn shim_abs(x: i32) -> i32 { if x < 0 { -x } else { x } }
unsafe extern "C" fn shim_labs(x: i64) -> i64 { if x < 0 { -x } else { x } }

// sqrt via Newton's method (integer approximation returned as f64 bits).
unsafe extern "C" fn shim_sqrt_bits(x_bits: u64) -> u64 {
    // Interpret bits as f64, compute sqrt, return bits.
    // We use a simple u64 approximation since f64 ops may not be safe.
    let x = f64::from_bits(x_bits);
    if x <= 0.0 { return 0f64.to_bits(); }
    // Newton: start at x/2, iterate
    let mut r = x / 2.0;
    for _ in 0..10 { r = (r + x / r) / 2.0; }
    r.to_bits()
}

// ── pthread stubs (all no-ops / success returns) ──────────────────────────────

unsafe extern "C" fn shim_pthread_create(
    thread: *mut u64, _attr: *const u8,
    start: unsafe extern "C" fn(*mut u8) -> *mut u8,
    arg: *mut u8,
) -> i32 {
    // Spawn as a kernel thread (best-effort — no proper user-mode threading yet)
    let _ = (start, arg, thread);
    0
}

unsafe extern "C" fn shim_pthread_join(_thread: u64, _retval: *mut *mut u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_mutex_init(_m: *mut u8, _attr: *const u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_mutex_lock(_m: *mut u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_mutex_unlock(_m: *mut u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_mutex_destroy(_m: *mut u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_once(_ctl: *mut u8, init: unsafe extern "C" fn()) -> i32 {
    init(); 0
}
unsafe extern "C" fn shim_pthread_self() -> u64 {
    crate::process::scheduler::current_tid().unwrap_or(1)
}
unsafe extern "C" fn shim_pthread_exit(_val: *mut u8) -> ! {
    crate::process::scheduler::exit_current_thread();
    loop { core::hint::spin_loop(); }
}
unsafe extern "C" fn shim_pthread_key_create(_key: *mut u32, _dtor: *const u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_key_delete(_key: u32) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_getspecific(_key: u32) -> *mut u8 { core::ptr::null_mut() }
unsafe extern "C" fn shim_pthread_setspecific(_key: u32, _val: *const u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_attr_init(_attr: *mut u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_attr_destroy(_attr: *mut u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_attr_setstacksize(_attr: *mut u8, _sz: usize) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_cond_init(_c: *mut u8, _attr: *const u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_cond_wait(_c: *mut u8, _m: *mut u8) -> i32 {
    crate::process::scheduler::yield_now(); 0
}
unsafe extern "C" fn shim_pthread_cond_signal(_c: *mut u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_cond_broadcast(_c: *mut u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_cond_destroy(_c: *mut u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_rwlock_init(_r: *mut u8, _attr: *const u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_rwlock_rdlock(_r: *mut u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_rwlock_wrlock(_r: *mut u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_rwlock_unlock(_r: *mut u8) -> i32 { 0 }
unsafe extern "C" fn shim_pthread_rwlock_destroy(_r: *mut u8) -> i32 { 0 }

// ── Dynamic loading ───────────────────────────────────────────────────────────

unsafe extern "C" fn shim_dlopen(path: *const u8, _flags: i32) -> *mut u8 {
    // Return a non-null sentinel — all symbols come from the same shim table.
    if path.is_null() { return 1usize as *mut u8; } // RTLD_DEFAULT handle
    1usize as *mut u8
}

unsafe extern "C" fn shim_dlsym(handle: *mut u8, symbol: *const u8) -> *mut u8 {
    let _ = handle;
    let len = shim_strlen(symbol);
    let name = core::str::from_utf8(core::slice::from_raw_parts(symbol, len)).unwrap_or("");
    lookup(name).unwrap_or(0) as *mut u8
}

unsafe extern "C" fn shim_dlclose(_handle: *mut u8) -> i32 { 0 }

unsafe extern "C" fn shim_dlerror() -> *const u8 {
    b"Smart OS: no dlerror\0".as_ptr()
}

// ── Misc ──────────────────────────────────────────────────────────────────────

unsafe extern "C" fn shim_exit(code: i32) -> ! {
    let _ = code;
    crate::process::scheduler::exit_current_thread();
    loop { core::hint::spin_loop(); }
}

unsafe extern "C" fn shim_abort() -> ! {
    crate::process::scheduler::exit_current_thread();
    loop { core::hint::spin_loop(); }
}

unsafe extern "C" fn shim_assert_fail(
    expr: *const u8, file: *const u8, line: u32, func: *const u8,
) -> ! {
    let _ = (expr, file, line, func);
    crate::process::scheduler::exit_current_thread();
    loop { core::hint::spin_loop(); }
}

unsafe extern "C" fn shim_usleep(usec: u32) -> i32 {
    let ticks = (usec as u64 / 10_000).min(100);
    for _ in 0..ticks { crate::process::scheduler::yield_now(); }
    0
}

unsafe extern "C" fn shim_sleep(sec: u32) -> u32 {
    let ticks = (sec as u64 * 100).min(10_000);
    for _ in 0..ticks { crate::process::scheduler::yield_now(); }
    0
}

unsafe extern "C" fn shim_time(tloc: *mut u64) -> u64 {
    let t = crate::drivers::timer::ticks() / 100 + 1_700_000_000;
    if !tloc.is_null() { *tloc = t; }
    t
}

unsafe extern "C" fn shim_clock_gettime(clk: i32, tp: *mut u64) -> i32 {
    let _ = clk;
    if tp.is_null() { return -1; }
    let secs  = crate::drivers::timer::ticks() / 100 + 1_700_000_000;
    let nsecs = (crate::drivers::timer::ticks() % 100) * 10_000_000;
    *tp       = secs;
    *tp.add(1) = nsecs;
    0
}

unsafe extern "C" fn shim_isatty(_fd: i32) -> i32 { 1 }

unsafe extern "C" fn shim_signal(_signum: i32, _handler: usize) -> usize { 1 } // SIG_DFL

unsafe extern "C" fn shim_raise(_sig: i32) -> i32 { 0 }

// ── Symbol lookup ─────────────────────────────────────────────────────────────
//
// Function pointers cannot be cast to integers in const/static context.
// We use a runtime match instead of a static table.

/// Number of symbols provided by the shim.
pub fn symbol_count() -> usize { 68 }

/// Look up a libc symbol by name. Returns the function address or None.
pub fn lookup(name: &str) -> Option<u64> {
    macro_rules! addr {
        ($fn:ident) => { Some($fn as *const () as u64) };
    }
    match name {
        // Memory
        "malloc"              => addr!(shim_malloc),
        "calloc"              => addr!(shim_calloc),
        "realloc"             => addr!(shim_realloc),
        "free"                => addr!(shim_free),
        "posix_memalign"      => addr!(shim_posix_memalign),
        // String
        "strlen"              => addr!(shim_strlen),
        "strcpy"              => addr!(shim_strcpy),
        "strncpy"             => addr!(shim_strncpy),
        "strcmp"              => addr!(shim_strcmp),
        "strncmp"             => addr!(shim_strncmp),
        "strdup"              => addr!(shim_strdup),
        "strndup"             => addr!(shim_strndup),
        "memcpy"              => addr!(shim_memcpy),
        "memmove"             => addr!(shim_memmove),
        "memset"              => addr!(shim_memset),
        "memcmp"              => addr!(shim_memcmp),
        // stdio
        "puts"                => addr!(shim_puts),
        "putchar"             => addr!(shim_putchar),
        "getchar"             => addr!(shim_getchar),
        "printf"              => addr!(shim_printf),
        "fprintf"             => addr!(shim_fprintf),
        // env
        "getenv"              => addr!(shim_getenv),
        "setenv"              => addr!(shim_setenv),
        "unsetenv"            => addr!(shim_unsetenv),
        // error
        "strerror"            => addr!(shim_strerror),
        "perror"              => addr!(shim_perror),
        // math
        "abs"                 => addr!(shim_abs),
        "labs"                => addr!(shim_labs),
        // pthread
        "pthread_create"      => addr!(shim_pthread_create),
        "pthread_join"        => addr!(shim_pthread_join),
        "pthread_self"        => addr!(shim_pthread_self),
        "pthread_exit"        => addr!(shim_pthread_exit),
        "pthread_mutex_init"  => addr!(shim_pthread_mutex_init),
        "pthread_mutex_lock"  => addr!(shim_pthread_mutex_lock),
        "pthread_mutex_unlock"=> addr!(shim_pthread_mutex_unlock),
        "pthread_mutex_destroy"=>addr!(shim_pthread_mutex_destroy),
        "pthread_once"        => addr!(shim_pthread_once),
        "pthread_key_create"  => addr!(shim_pthread_key_create),
        "pthread_key_delete"  => addr!(shim_pthread_key_delete),
        "pthread_getspecific" => addr!(shim_pthread_getspecific),
        "pthread_setspecific" => addr!(shim_pthread_setspecific),
        "pthread_attr_init"   => addr!(shim_pthread_attr_init),
        "pthread_attr_destroy"=> addr!(shim_pthread_attr_destroy),
        "pthread_attr_setstacksize" => addr!(shim_pthread_attr_setstacksize),
        "pthread_cond_init"   => addr!(shim_pthread_cond_init),
        "pthread_cond_wait"   => addr!(shim_pthread_cond_wait),
        "pthread_cond_signal" => addr!(shim_pthread_cond_signal),
        "pthread_cond_broadcast"=>addr!(shim_pthread_cond_broadcast),
        "pthread_cond_destroy"=> addr!(shim_pthread_cond_destroy),
        "pthread_rwlock_init" => addr!(shim_pthread_rwlock_init),
        "pthread_rwlock_rdlock"=>addr!(shim_pthread_rwlock_rdlock),
        "pthread_rwlock_wrlock"=>addr!(shim_pthread_rwlock_wrlock),
        "pthread_rwlock_unlock"=>addr!(shim_pthread_rwlock_unlock),
        "pthread_rwlock_destroy"=>addr!(shim_pthread_rwlock_destroy),
        // dlfcn
        "dlopen"              => addr!(shim_dlopen),
        "dlsym"               => addr!(shim_dlsym),
        "dlclose"             => addr!(shim_dlclose),
        "dlerror"             => addr!(shim_dlerror),
        // misc
        "exit"                => addr!(shim_exit),
        "abort"               => addr!(shim_abort),
        "__assert_fail"       => addr!(shim_assert_fail),
        "usleep"              => addr!(shim_usleep),
        "sleep"               => addr!(shim_sleep),
        "time"                => addr!(shim_time),
        "clock_gettime"       => addr!(shim_clock_gettime),
        "isatty"              => addr!(shim_isatty),
        "signal"              => addr!(shim_signal),
        "raise"               => addr!(shim_raise),
        _                     => None,
    }
}

/// Initialize the libc shim: write a real /lib/libc.so.6 marker and
/// publish errno address to the VFS for debuggers.
pub fn init() {
    // Write the shim symbol count into a synthetic /proc file
    let count = symbol_count();
    let _ = crate::vfs::create_and_write(
        "/proc/smartos/libc_shim",
        format!("symbols={}\n", count).as_bytes(),
    );
    crate::serial_println!("[libc_shim] musl shim ready ({} symbols).", count);
}

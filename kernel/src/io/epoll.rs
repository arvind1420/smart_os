//! epoll — Phase 107 System Hardening
//!
//! Linux-compatible epoll(7) for the Smart OS kernel.
//!
//! In a bare-metal synchronous kernel all I/O is non-blocking and always
//! "ready", so epoll_wait() is a lightweight immediate poll rather than a
//! true sleeping wait.  The API contract (ADD/MOD/DEL, EPOLLIN/EPOLLOUT,
//! EPOLLONESHOT, EPOLLET) is preserved so existing event-loop code compiles.

#![allow(dead_code)]

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ─── Event flags (mirrors <sys/epoll.h>) ─────────────────────────────────────

pub const EPOLLIN:      u32 = 0x0001;
pub const EPOLLPRI:     u32 = 0x0002;
pub const EPOLLOUT:     u32 = 0x0004;
pub const EPOLLERR:     u32 = 0x0008;
pub const EPOLLHUP:     u32 = 0x0010;
pub const EPOLLRDNORM:  u32 = 0x0040;
pub const EPOLLWRNORM:  u32 = 0x0100;
pub const EPOLLRDHUP:   u32 = 0x2000;
pub const EPOLLONESHOT: u32 = 1 << 30;
pub const EPOLLET:      u32 = 1 << 31;

// ─── epoll_ctl operations ─────────────────────────────────────────────────────

pub const EPOLL_CTL_ADD: i32 = 1;
pub const EPOLL_CTL_DEL: i32 = 2;
pub const EPOLL_CTL_MOD: i32 = 3;

// ─── EpollEvent (mirrors struct epoll_event) ──────────────────────────────────

/// Matches Linux `struct epoll_event` — packed for ABI compatibility.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EpollEvent {
    pub events: u32,
    /// User data tag passed back verbatim in wait results.
    pub data:   u64,
}

// ─── Interest entry ───────────────────────────────────────────────────────────

struct Interest {
    events:   u32,
    data:     u64,
    /// true after a EPOLLONESHOT event has been delivered.
    one_fired: bool,
}

// ─── EpollInstance ────────────────────────────────────────────────────────────

struct EpollInstance {
    /// fd → interest
    interests: BTreeMap<u64, Interest>,
}

impl EpollInstance {
    fn new() -> Self { Self { interests: BTreeMap::new() } }
}

// ─── Global table ─────────────────────────────────────────────────────────────

static EPOLL_TABLE: Mutex<BTreeMap<u64, EpollInstance>> = Mutex::new(BTreeMap::new());

/// epoll fd counter starts above the VFS fd range to avoid confusion.
static EPOLL_NEXT_FD: AtomicU64 = AtomicU64::new(10_000);

// ─── Public API ───────────────────────────────────────────────────────────────

/// Create a new epoll instance.  Returns an opaque "epoll fd" ≥ 10000.
pub fn epoll_create() -> u64 {
    let fd = EPOLL_NEXT_FD.fetch_add(1, Ordering::Relaxed);
    EPOLL_TABLE.lock().insert(fd, EpollInstance::new());
    fd
}

/// Close an epoll instance and release all interest registrations.
pub fn epoll_close(epfd: u64) {
    EPOLL_TABLE.lock().remove(&epfd);
}

/// `epoll_ctl(epfd, op, fd, event)`.
/// Returns 0 on success, negative errno on error:
///   -22 (EINVAL) — invalid epfd or op
///   -17 (EEXIST) — ADD on already-watched fd
///   -2  (ENOENT) — MOD/DEL on unwatched fd
pub fn epoll_ctl(epfd: u64, op: i32, fd: u64, event: Option<EpollEvent>) -> i64 {
    let mut table = EPOLL_TABLE.lock();
    let inst = match table.get_mut(&epfd) {
        Some(i) => i,
        None    => return -22, // EINVAL
    };

    match op {
        EPOLL_CTL_ADD => {
            if inst.interests.contains_key(&fd) { return -17; } // EEXIST
            let ev = event.unwrap_or_default();
            inst.interests.insert(fd, Interest {
                events:    ev.events,
                data:      ev.data,
                one_fired: false,
            });
            0
        }
        EPOLL_CTL_MOD => {
            let entry = match inst.interests.get_mut(&fd) {
                Some(e) => e,
                None    => return -2, // ENOENT
            };
            let ev = event.unwrap_or_default();
            entry.events    = ev.events;
            entry.data      = ev.data;
            entry.one_fired = false; // re-arm
            0
        }
        EPOLL_CTL_DEL => {
            if inst.interests.remove(&fd).is_none() { return -2; } // ENOENT
            0
        }
        _ => -22, // EINVAL
    }
}

/// Simulate per-fd readiness.
///
/// In the synchronous SmartOS kernel every fd is always ready for both
/// reading and writing.  This is correct for VFS fds, pipes, and UDP sockets.
/// TCP sockets could be extended to consult a receive-queue length in future.
#[inline]
fn ready_mask(fd: u64, interest: u32) -> u32 {
    let _ = fd;
    let mut ready = 0u32;
    if interest & (EPOLLIN | EPOLLRDNORM) != 0 { ready |= EPOLLIN; }
    if interest & (EPOLLOUT | EPOLLWRNORM) != 0 { ready |= EPOLLOUT; }
    ready
}

/// `epoll_wait(epfd, maxevents, timeout_ms)`.
///
/// Returns immediately with all currently-ready events (up to `maxevents`).
/// `timeout_ms` is accepted but not honoured — returns without sleeping.
/// Returns empty Vec on invalid epfd.
pub fn epoll_wait(epfd: u64, maxevents: usize, _timeout_ms: i32) -> Vec<EpollEvent> {
    let cap = maxevents.min(1024);
    let mut out = Vec::with_capacity(cap);

    let mut table = EPOLL_TABLE.lock();
    let inst = match table.get_mut(&epfd) {
        Some(i) => i,
        None    => return out,
    };

    for (&fd, entry) in inst.interests.iter_mut() {
        if out.len() >= cap { break; }
        // Skip fired one-shot entries
        if entry.events & EPOLLONESHOT != 0 && entry.one_fired { continue; }

        // Compute ready mask against interest (strip meta-flags)
        let interest = entry.events & !(EPOLLET | EPOLLONESHOT);
        let ready    = ready_mask(fd, interest);
        if ready == 0 { continue; }

        out.push(EpollEvent { events: ready, data: entry.data });

        // Mark ONESHOT as fired so next wait skips it
        if entry.events & EPOLLONESHOT != 0 { entry.one_fired = true; }
    }
    out
}

/// Re-arm a one-shot fd after it has been delivered.
/// Equivalent to `epoll_ctl(epfd, EPOLL_CTL_MOD, fd, ev)`.
pub fn epoll_rearm(epfd: u64, fd: u64) {
    let mut table = EPOLL_TABLE.lock();
    if let Some(inst) = table.get_mut(&epfd) {
        if let Some(entry) = inst.interests.get_mut(&fd) {
            entry.one_fired = false;
        }
    }
}

/// Return the number of watched fds on a given epoll instance.
pub fn epoll_interest_count(epfd: u64) -> usize {
    EPOLL_TABLE.lock()
        .get(&epfd)
        .map(|i| i.interests.len())
        .unwrap_or(0)
}

// ─── Init ─────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[epoll] Phase 107: epoll ready (synchronous, always-ready semantics).");
}

// ─── Self-test ────────────────────────────────────────────────────────────────

pub fn self_test() {
    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! check {
        ($desc:expr, $val:expr) => {
            if $val { passed += 1; }
            else    { failed += 1; crate::serial_println!("[epoll] FAIL: {}", $desc); }
        };
    }

    // T1: create
    let epfd = epoll_create();
    check!("epoll_create fd >= 10000", epfd >= 10_000);

    // T2: ADD a fd
    let ev = EpollEvent { events: EPOLLIN | EPOLLOUT, data: 42 };
    check!("CTL_ADD returns 0", epoll_ctl(epfd, EPOLL_CTL_ADD, 7, Some(ev)) == 0);
    check!("interest count = 1", epoll_interest_count(epfd) == 1);

    // T3: duplicate ADD returns -17 (EEXIST)
    check!("CTL_ADD duplicate returns -17", epoll_ctl(epfd, EPOLL_CTL_ADD, 7, Some(ev)) == -17);

    // T4: MOD
    let ev2 = EpollEvent { events: EPOLLOUT, data: 99 };
    check!("CTL_MOD returns 0", epoll_ctl(epfd, EPOLL_CTL_MOD, 7, Some(ev2)) == 0);

    // T5: wait → EPOLLOUT, data = 99
    let events = epoll_wait(epfd, 8, 0);
    check!("wait returns 1 event",        events.len() == 1);
    check!("event data = 99",             events[0].data == 99);
    check!("event has EPOLLOUT",          events[0].events & EPOLLOUT != 0);

    // T6: DEL
    check!("CTL_DEL returns 0", epoll_ctl(epfd, EPOLL_CTL_DEL, 7, None) == 0);
    check!("interest count = 0 after DEL", epoll_interest_count(epfd) == 0);
    let empty = epoll_wait(epfd, 8, 0);
    check!("wait after DEL is empty", empty.is_empty());

    // T7: DEL non-existent returns -2 (ENOENT)
    check!("CTL_DEL non-existent returns -2", epoll_ctl(epfd, EPOLL_CTL_DEL, 7, None) == -2);

    // T8: invalid epfd
    check!("invalid epfd returns -22", epoll_ctl(99_999, EPOLL_CTL_ADD, 1, Some(ev)) == -22);

    // T9: EPOLLONESHOT fires only once
    let epfd2 = epoll_create();
    let ev_os = EpollEvent { events: EPOLLOUT | EPOLLONESHOT, data: 7 };
    epoll_ctl(epfd2, EPOLL_CTL_ADD, 1, Some(ev_os));
    let first  = epoll_wait(epfd2, 8, 0);
    let second = epoll_wait(epfd2, 8, 0);
    check!("ONESHOT first  fires",  !first.is_empty());
    check!("ONESHOT second silent",  second.is_empty());

    // T10: re-arm ONESHOT
    epoll_rearm(epfd2, 1);
    let third = epoll_wait(epfd2, 8, 0);
    check!("ONESHOT re-arm fires again", !third.is_empty());

    // T11: EPOLLIN included in returned mask when requested
    let epfd3 = epoll_create();
    let ev_in = EpollEvent { events: EPOLLIN, data: 5 };
    epoll_ctl(epfd3, EPOLL_CTL_ADD, 2, Some(ev_in));
    let rin = epoll_wait(epfd3, 8, 0);
    check!("EPOLLIN in returned events", !rin.is_empty() && rin[0].events & EPOLLIN != 0);

    // T12: maxevents cap
    let epfd4 = epoll_create();
    for i in 0..10u64 {
        let e = EpollEvent { events: EPOLLOUT, data: i };
        epoll_ctl(epfd4, EPOLL_CTL_ADD, i + 20, Some(e));
    }
    let limited = epoll_wait(epfd4, 3, 0);
    check!("maxevents=3 caps at 3", limited.len() == 3);

    epoll_close(epfd);
    epoll_close(epfd2);
    epoll_close(epfd3);
    epoll_close(epfd4);

    crate::serial_println!("[epoll] self_test: {}/{} passed", passed, passed + failed);
}

/// Session manager for Smart OS.
///
/// Tracks the currently logged-in user and provides login/logout/lock APIs.
/// The login screen calls `start_session`; logout and lock screen use `end_session`
/// and `lock` / `unlock`.

use alloc::string::String;
use spin::Mutex;

// ── Session state ─────────────────────────────────────────────────────────────

pub struct Session {
    pub uid:      u32,
    pub username: String,
    pub home_dir: String,
}

static CURRENT: Mutex<Option<Session>> = Mutex::new(None);

/// True while the screen is locked (session exists but desktop is covered).
static LOCKED: Mutex<bool> = Mutex::new(false);

// ── Public API ────────────────────────────────────────────────────────────────

/// Start a session for `username` (must already be authenticated).
/// Creates home dir if needed. Returns false if user not found.
pub fn start_session(username: &str) -> bool {
    let user = match crate::users::find_user(username) {
        Some(u) => u,
        None    => return false,
    };

    // Ensure home dir exists
    let _ = crate::vfs::mkdir(&user.home);

    *CURRENT.lock() = Some(Session {
        uid:      user.uid,
        username: user.username.clone(),
        home_dir: user.home.clone(),
    });
    *LOCKED.lock() = false;

    crate::serial_println!("[session] Started session for '{}' (uid={})", user.username, user.uid);
    true
}

/// Destroy the current session (logout).
pub fn end_session() {
    let name = current_user().unwrap_or_default();
    *CURRENT.lock() = None;
    *LOCKED.lock() = false;
    crate::serial_println!("[session] Session ended for '{}'", name);
}

/// Lock the screen (session remains, desktop is covered).
pub fn lock() {
    *LOCKED.lock() = true;
    crate::serial_println!("[session] Screen locked");
}

/// Unlock after verifying the current user's password.
pub fn unlock(password: &str) -> bool {
    let username = match current_user() {
        Some(u) => u,
        None    => return false,
    };
    if crate::users::authenticate(&username, password) {
        *LOCKED.lock() = false;
        crate::serial_println!("[session] Screen unlocked");
        true
    } else {
        false
    }
}

pub fn is_logged_in() -> bool { CURRENT.lock().is_some() }

pub fn is_locked() -> bool { *LOCKED.lock() }

pub fn current_user() -> Option<String> {
    CURRENT.lock().as_ref().map(|s| s.username.clone())
}

pub fn current_uid() -> Option<u32> {
    CURRENT.lock().as_ref().map(|s| s.uid)
}

pub fn current_home() -> Option<String> {
    CURRENT.lock().as_ref().map(|s| s.home_dir.clone())
}

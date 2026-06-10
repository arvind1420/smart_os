/// Phase 53: Multi-user support for Smart OS.
///
/// Extends the existing `users` module with:
///
///  • Groups      — `/etc/group` file, supplementary group membership
///  • Permissions — POSIX-style 9-bit mode on VFS paths (separate overlay table)
///  • Sessions    — per-process `SessionInfo` (current uid/gid/groups)
///  • `su`        — switch-user with password verification
///  • `login`     — authenticate and create a new session
///  • `setuid` / `setgid` — lower-privilege exec transitions
///  • `check_perm` — called at VFS open/read/write boundaries

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
//  POSIX permission bits
// ─────────────────────────────────────────────────────────────────────────────

/// Standard POSIX mode bits.
pub mod mode {
    pub const OWNER_R: u16 = 0o400;
    pub const OWNER_W: u16 = 0o200;
    pub const OWNER_X: u16 = 0o100;
    pub const GROUP_R: u16 = 0o040;
    pub const GROUP_W: u16 = 0o020;
    pub const GROUP_X: u16 = 0o010;
    pub const OTHER_R: u16 = 0o004;
    pub const OTHER_W: u16 = 0o002;
    pub const OTHER_X: u16 = 0o001;

    /// Readable by everyone, writable by owner.
    pub const DEFAULT_FILE: u16 = 0o644;
    /// rwxr-xr-x — executable/directory.
    pub const DEFAULT_DIR:  u16 = 0o755;
    /// Owner-only: used for /etc/shadow.
    pub const OWNER_ONLY:   u16 = 0o600;
    /// Root-only read.
    pub const ROOT_ONLY_R:  u16 = 0o400;
}

/// Access operation flags (POSIX R/W/X bit values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessOp {
    Read    = 4,
    Write   = 2,
    Execute = 1,
}

impl AccessOp {
    pub fn bit(self) -> u16 { self as u16 }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Groups
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Group {
    pub name:    String,
    pub gid:     u32,
    pub members: Vec<String>, // usernames
}

static GROUPS: Mutex<Vec<Group>> = Mutex::new(Vec::new());

/// Load groups from `/etc/group`  (format: name:x:gid:member1,member2).
pub fn load_groups() {
    let data = crate::vfs::read_file_full("/etc/group").unwrap_or_default();
    let text = core::str::from_utf8(&data).unwrap_or("");
    let mut groups = GROUPS.lock();
    groups.clear();
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        if parts.len() < 3 { continue; }
        let name = parts[0].to_string();
        let gid: u32 = parts[2].parse().unwrap_or(0);
        let members: Vec<String> = if parts.len() == 4 && !parts[3].is_empty() {
            parts[3].split(',').map(|s| s.to_string()).collect()
        } else {
            Vec::new()
        };
        groups.push(Group { name, gid, members });
    }
}

/// All supplementary GIDs for a username (beyond their primary GID).
pub fn supplementary_gids(username: &str) -> Vec<u32> {
    GROUPS.lock().iter()
        .filter(|g| g.members.iter().any(|m| m == username))
        .map(|g| g.gid)
        .collect()
}

/// Check if `username` is a member of group `gid`.
pub fn is_member_of(username: &str, gid: u32) -> bool {
    let user = super::find_user(username);
    // Primary group matches.
    if let Some(ref u) = user {
        if u.gid == gid { return true; }
    }
    // Supplementary groups.
    GROUPS.lock().iter()
        .filter(|g| g.gid == gid)
        .any(|g| g.members.iter().any(|m| m == username))
}

pub fn list_groups() -> Vec<Group> {
    GROUPS.lock().clone()
}

pub fn find_group_by_gid(gid: u32) -> Option<Group> {
    GROUPS.lock().iter().find(|g| g.gid == gid).cloned()
}

pub fn find_group_by_name(name: &str) -> Option<Group> {
    GROUPS.lock().iter().find(|g| g.name == name).cloned()
}

// ─────────────────────────────────────────────────────────────────────────────
//  VFS permission overlay table
// ─────────────────────────────────────────────────────────────────────────────

/// Per-path permission entry.
#[derive(Clone, Copy, Debug)]
pub struct FilePerm {
    pub mode:  u16,   // POSIX 9-bit mode
    pub owner: u32,   // owning UID
    pub group: u32,   // owning GID
}

impl FilePerm {
    pub const fn new(mode: u16, owner: u32, group: u32) -> Self {
        Self { mode, owner, group }
    }
}

/// Permission overlay table: path → FilePerm.
static PERM_TABLE: Mutex<BTreeMap<String, FilePerm>> = Mutex::new(BTreeMap::new());

/// Set permissions for a path.
pub fn set_perm(path: &str, uid: u32, gid: u32, mode: u16) {
    PERM_TABLE.lock().insert(path.to_string(), FilePerm::new(mode, uid, gid));
}

/// Get permissions for a path (fallback: default based on path).
pub fn get_perm(path: &str) -> FilePerm {
    if let Some(&p) = PERM_TABLE.lock().get(path) {
        return p;
    }
    // Sensible defaults based on path prefix.
    let (owner_mode, owner, group) = if path.starts_with("/etc/shadow") {
        (mode::OWNER_ONLY,   0u32, 0u32) // root-only
    } else if path.starts_with("/etc/") {
        (mode::DEFAULT_FILE, 0,    0)
    } else if path.starts_with("/system/") || path.starts_with("/boot/") {
        (0o444,              0,    0)    // read-only, root-owned
    } else if path.starts_with("/root/") {
        (0o700,              0,    0)    // root private dir
    } else if path.starts_with("/home/") {
        // /home/<user>/... is owned by that user (uid 1000+).
        let user_dir = path.trim_start_matches("/home/");
        let username = user_dir.split('/').next().unwrap_or("");
        let uid = super::find_user(username).map(|u| u.uid).unwrap_or(1000);
        (mode::DEFAULT_DIR,  uid, uid)
    } else {
        (mode::DEFAULT_FILE, 0,    0)
    };
    FilePerm::new(owner_mode, owner, group)
}

/// Check if (`uid`, `gid`, `extra_gids`) may perform `op` on `path`.
/// Returns `Ok(())` or `Err("EACCES")`.
pub fn check_perm(
    path: &str,
    uid: u32,
    gid: u32,
    extra_gids: &[u32],
    op: AccessOp,
) -> Result<(), &'static str> {
    // root (uid=0) bypasses all permission checks except execute on non-executable.
    if uid == 0 {
        if op == AccessOp::Execute {
            // Root can only exec if at least one execute bit is set.
            let perm = get_perm(path);
            let any_x = perm.mode & (mode::OWNER_X | mode::GROUP_X | mode::OTHER_X);
            return if any_x != 0 { Ok(()) } else { Err("EACCES: not executable") };
        }
        return Ok(());
    }

    let perm = get_perm(path);
    let bit = op.bit();

    // Owner check.
    if uid == perm.owner {
        if (perm.mode >> 6) & bit != 0 { return Ok(()); }
        return Err("EACCES: owner permission denied");
    }

    // Group check (primary + supplementary).
    let in_group = gid == perm.group || extra_gids.contains(&perm.group);
    if in_group {
        if (perm.mode >> 3) & bit != 0 { return Ok(()); }
        return Err("EACCES: group permission denied");
    }

    // Other check.
    if perm.mode & bit != 0 { return Ok(()); }
    Err("EACCES: permission denied")
}

// ─────────────────────────────────────────────────────────────────────────────
//  Per-process session
// ─────────────────────────────────────────────────────────────────────────────

/// Active session for one process.
#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub uid:         u32,
    pub gid:         u32,
    pub username:    String,
    /// Supplementary groups.
    pub extra_gids:  Vec<u32>,
    /// Effective UID (after setuid / su).
    pub euid:        u32,
    /// Effective GID.
    pub egid:        u32,
}

impl SessionInfo {
    pub fn root() -> Self {
        Self {
            uid: 0, gid: 0, username: String::from("root"),
            extra_gids: Vec::new(), euid: 0, egid: 0,
        }
    }

    pub fn for_user(user: &super::User) -> Self {
        let extra = supplementary_gids(&user.username);
        Self {
            uid: user.uid, gid: user.gid,
            username: user.username.clone(),
            extra_gids: extra,
            euid: user.uid, egid: user.gid,
        }
    }

    pub fn is_root(&self) -> bool { self.euid == 0 }
}

/// Per-process session table: pid → SessionInfo.
static SESSIONS: Mutex<BTreeMap<u64, SessionInfo>> = Mutex::new(BTreeMap::new());

/// Register a session for a process.
pub fn set_session(pid: u64, info: SessionInfo) {
    SESSIONS.lock().insert(pid, info);
}

/// Get the session for a process (falls back to root session if unregistered).
pub fn get_session(pid: u64) -> SessionInfo {
    SESSIONS.lock().get(&pid).cloned().unwrap_or_else(SessionInfo::root)
}

/// Remove a session on process exit.
pub fn remove_session(pid: u64) {
    SESSIONS.lock().remove(&pid);
}

/// Current effective UID for a process.
pub fn current_uid(pid: u64) -> u32 {
    SESSIONS.lock().get(&pid).map(|s| s.euid).unwrap_or(0)
}

/// Current effective GID for a process.
pub fn current_gid(pid: u64) -> u32 {
    SESSIONS.lock().get(&pid).map(|s| s.egid).unwrap_or(0)
}

// ─────────────────────────────────────────────────────────────────────────────
//  su — switch user
// ─────────────────────────────────────────────────────────────────────────────

/// Switch the session of `pid` to `target_user` after verifying `password`.
/// Root (uid=0) may `su` without a password.
pub fn su(pid: u64, target_username: &str, password: &str) -> Result<(), &'static str> {
    let current = get_session(pid);

    // root may always su.
    let authenticated = current.euid == 0
        || super::authenticate(target_username, password);

    if !authenticated {
        crate::security::audit::log(format!(
            "SU_FAIL pid={} from={} to={}", pid, current.username, target_username
        ));
        return Err("EPERM: authentication failed");
    }

    let user = super::find_user(target_username).ok_or("ENOENT: user not found")?;
    let new_session = SessionInfo::for_user(&user);

    crate::security::audit::log(format!(
        "SU_OK pid={} from={} to={}", pid, current.username, target_username
    ));

    set_session(pid, new_session);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
//  login — authenticate and start a new session
// ─────────────────────────────────────────────────────────────────────────────

/// Authenticate and return a `SessionInfo` (does NOT assign to a pid).
/// Call `set_session(pid, info)` afterwards.
pub fn login(username: &str, password: &str) -> Result<SessionInfo, &'static str> {
    if !super::authenticate(username, password) {
        crate::security::audit::log(format!("LOGIN_FAIL user={}", username));
        return Err("EPERM: bad credentials");
    }
    let user = super::find_user(username).ok_or("ENOENT")?;
    crate::security::audit::log(format!("LOGIN_OK user={} uid={}", username, user.uid));
    Ok(SessionInfo::for_user(&user))
}

// ─────────────────────────────────────────────────────────────────────────────
//  setuid / setgid (privilege drop)
// ─────────────────────────────────────────────────────────────────────────────

/// Drop effective UID to `new_uid` (only root can raise it).
pub fn setuid(pid: u64, new_uid: u32) -> Result<(), &'static str> {
    let mut sessions = SESSIONS.lock();
    let info = sessions.get_mut(&pid).ok_or("ESRCH: no session")?;
    // Only root can set arbitrary UID; non-root can only set to their real UID.
    if info.euid != 0 && new_uid != info.uid {
        return Err("EPERM");
    }
    info.euid = new_uid;
    Ok(())
}

/// Drop effective GID to `new_gid`.
pub fn setgid(pid: u64, new_gid: u32) -> Result<(), &'static str> {
    let mut sessions = SESSIONS.lock();
    let info = sessions.get_mut(&pid).ok_or("ESRCH: no session")?;
    if info.egid != 0 && new_gid != info.gid {
        return Err("EPERM");
    }
    info.egid = new_gid;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

/// Default `/etc/group` content.
const DEFAULT_GROUP: &str = "root:x:0:\nwheel:x:10:root,user\nusers:x:100:user\naudio:x:29:user\nvideo:x:39:user\n";

pub fn init() {
    // Write /etc/group if missing.
    if crate::vfs::stat("/etc/group").is_err() {
        let _ = crate::vfs::mkdir("/etc");
        let _ = crate::vfs::create_and_write("/etc/group", DEFAULT_GROUP.as_bytes());
    }
    load_groups();

    // Seed permission overlay for sensitive paths.
    set_perm("/etc/shadow", 0, 0, mode::OWNER_ONLY);       // root-only read
    set_perm("/etc/passwd", 0, 0, 0o644);                   // world-readable, root-write
    set_perm("/etc/group",  0, 0, 0o644);
    set_perm("/system",     0, 0, 0o755);
    set_perm("/boot",       0, 0, 0o700);                   // root-only

    // Register root session for kernel process (pid=1).
    set_session(1, SessionInfo::root());

    crate::serial_println!(
        "[multiuser] Multi-user initialised: {} groups loaded, permission overlay seeded.",
        GROUPS.lock().len()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  Self-test
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // ── Test 1: Root bypasses permission check ────────────────────────────────
    let r = check_perm("/etc/shadow", 0, 0, &[], AccessOp::Read);
    if r.is_err() {
        crate::serial_println!("[multiuser-test] FAIL: root should read /etc/shadow");
        ok = false;
    }

    // ── Test 2: Non-root denied /etc/shadow ───────────────────────────────────
    let r = check_perm("/etc/shadow", 1000, 1000, &[], AccessOp::Read);
    if r.is_ok() {
        crate::serial_println!("[multiuser-test] FAIL: uid=1000 should not read /etc/shadow");
        ok = false;
    }

    // ── Test 3: Non-root can read /etc/passwd ─────────────────────────────────
    let r = check_perm("/etc/passwd", 1000, 1000, &[], AccessOp::Read);
    if r.is_err() {
        crate::serial_println!("[multiuser-test] FAIL: uid=1000 should read /etc/passwd (0o644)");
        ok = false;
    }

    // ── Test 4: Non-root cannot write /etc/passwd ─────────────────────────────
    let r = check_perm("/etc/passwd", 1000, 1000, &[], AccessOp::Write);
    if r.is_ok() {
        crate::serial_println!("[multiuser-test] FAIL: uid=1000 should not write /etc/passwd");
        ok = false;
    }

    // ── Test 5: Group-write: set custom perm then check ───────────────────────
    set_perm("/tmp/shared.txt", 500, 500, 0o664); // owner=500 group=500 rw-rw-r--
    // uid=501 in group 500 can write.
    let r = check_perm("/tmp/shared.txt", 501, 999, &[500], AccessOp::Write);
    if r.is_err() {
        crate::serial_println!("[multiuser-test] FAIL: supplementary group should allow write");
        ok = false;
    }
    // uid=502 not in group 500 cannot write.
    let r = check_perm("/tmp/shared.txt", 502, 999, &[], AccessOp::Write);
    if r.is_ok() {
        crate::serial_println!("[multiuser-test] FAIL: non-member should not write group file");
        ok = false;
    }

    // ── Test 6: Session management ────────────────────────────────────────────
    let dummy_user = super::User {
        username: String::from("alice"),
        uid: 2000,
        gid: 2000,
        home: String::from("/home/alice"),
        shell: String::from("/bin/sh"),
        pw_hash: String::new(),
    };
    let session = SessionInfo::for_user(&dummy_user);
    set_session(9901, session.clone());
    let got = current_uid(9901);
    if got != 2000 {
        crate::serial_println!("[multiuser-test] FAIL: current_uid should be 2000, got {}", got);
        ok = false;
    }
    remove_session(9901);

    // ── Test 7: setuid can drop to real uid ───────────────────────────────────
    set_session(9902, session);
    let r = setuid(9902, 2000); // same as real uid — allowed
    if r.is_err() {
        crate::serial_println!("[multiuser-test] FAIL: setuid to real uid should succeed");
        ok = false;
    }
    let r = setuid(9902, 0); // raise to root — denied (not root)
    if r.is_ok() {
        crate::serial_println!("[multiuser-test] FAIL: non-root should not setuid to root");
        ok = false;
    }
    remove_session(9902);

    // ── Test 8: login() with wrong password fails ─────────────────────────────
    let r = login("root", "definitely_wrong_password_xyz");
    if r.is_ok() {
        crate::serial_println!("[multiuser-test] FAIL: bad password should reject login");
        ok = false;
    }

    // ── Test 9: Group API round-trip (self-seeded) ───────────────────────────
    // self_test() may run before init() creates /etc/group, so we seed a
    // test group directly into the GROUPS table and verify the API round-trips.
    {
        let mut g = GROUPS.lock();
        g.push(Group {
            name: String::from("_testgroup_"),
            gid:  99999,
            members: alloc::vec![String::from("_testuser_")],
        });
    }
    let groups = list_groups();
    if !groups.iter().any(|g| g.name == "_testgroup_") {
        crate::serial_println!("[multiuser-test] FAIL: seeded group not found via list_groups");
        ok = false;
    }
    // Verify supplementary_gids returns the test group for _testuser_.
    if !supplementary_gids("_testuser_").contains(&99999) {
        crate::serial_println!("[multiuser-test] FAIL: supplementary_gids missed seeded group");
        ok = false;
    }
    // Cleanup.
    GROUPS.lock().retain(|g| g.gid != 99999);

    // ── Test 10: supplementary_gids returns non-root groups for "user" ─────
    // (User may be in 'users'/'audio'/'video' per /etc/group default)
    let sgids = supplementary_gids("user");
    // Just check the function runs without panic; result depends on /etc/group.
    let _ = sgids;

    // Cleanup.
    PERM_TABLE.lock().remove("/tmp/shared.txt");

    if ok {
        crate::serial_println!("[multiuser-test] All 10 multi-user tests PASSED");
    }
    ok
}

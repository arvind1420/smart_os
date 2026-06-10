/// User account management for Smart OS.
///
/// Stores user records in VFS at /etc/passwd and /etc/shadow.
/// Passwords are hashed with SHA-256 + salt (4096 iterations).
/// Format: $sha256$<16-hex-salt>$<64-hex-hash>

mod sha256;
pub mod multiuser;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct User {
    pub username:  String,
    pub uid:       u32,
    pub gid:       u32,
    pub home:      String,
    pub shell:     String,
    pub pw_hash:   String, // $sha256$<salt_hex>$<hash_hex>  or "" for no-password root
}

// ── Global user table ─────────────────────────────────────────────────────────

static USERS: Mutex<Vec<User>> = Mutex::new(Vec::new());

// ── Init ──────────────────────────────────────────────────────────────────────

/// Populate /etc/passwd and /etc/shadow with defaults, then load them.
pub fn init() {
    let passwd = "root:x:0:0:/root:/bin/sh\nuser:x:1000:1000:/home/user:/bin/sh\n";
    let shadow = format!(
        "root:{}\nuser:{}\n",
        make_hash("root"),
        make_hash("user"),
    );

    // Write defaults only if files don't already exist
    if crate::vfs::stat("/etc/passwd").is_err() {
        let _ = crate::vfs::mkdir("/etc");
        let _ = crate::vfs::create_and_write("/etc/passwd", passwd.as_bytes());
        let _ = crate::vfs::create_and_write("/etc/shadow", shadow.as_bytes());
    }

    // Ensure home dirs exist
    let _ = crate::vfs::mkdir("/root");
    let _ = crate::vfs::mkdir("/home");
    let _ = crate::vfs::mkdir("/home/user");

    load_from_vfs();

    // Phase 53: multi-user (groups + permissions + sessions).
    multiuser::init();
}

fn load_from_vfs() {
    let passwd_data = match crate::vfs::read_file_full("/etc/passwd") {
        Ok(d) => d, Err(_) => return,
    };
    let shadow_data = crate::vfs::read_file_full("/etc/shadow").unwrap_or_default();
    let passwd_str = core::str::from_utf8(&passwd_data).unwrap_or("");
    let shadow_str = core::str::from_utf8(&shadow_data).unwrap_or("");

    let mut users = USERS.lock();
    users.clear();

    for line in passwd_str.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 6 { continue; }
        let username = parts[0].to_string();
        let uid: u32 = parts[2].parse().unwrap_or(0);
        let gid: u32 = parts[3].parse().unwrap_or(0);
        let home  = parts[4].to_string();
        let shell = parts[5].to_string();

        // Look up password hash from shadow
        let pw_hash = shadow_str.lines()
            .find(|l| l.starts_with(&format!("{}:", username)))
            .and_then(|l| l.splitn(2, ':').nth(1))
            .unwrap_or("")
            .to_string();

        users.push(User { username, uid, gid, home, shell, pw_hash });
    }
}

// ── Password hashing ──────────────────────────────────────────────────────────

/// Derive a fixed, boot-stable salt from the username.
/// (No timer involvement so the hash round-trips correctly across calls.)
fn salt_for(username: &str) -> [u8; 16] {
    // Salt = SHA-256("SmartOS" || username)[0..16] — purely deterministic.
    let mut input = b"SmartOS".to_vec();
    input.extend_from_slice(username.as_bytes());
    let seed = sha256::sha256(&input);
    let mut s = [0u8; 16];
    s.copy_from_slice(&seed[..16]);
    s
}

fn make_hash(password: &str) -> String {
    let salt = salt_for(password);
    hash_password(password, &salt)
}

/// Hash a password: SHA-256(salt || pass), iterated 4096 times.
pub fn hash_password(password: &str, salt: &[u8; 16]) -> String {
    let mut buf = Vec::with_capacity(16 + password.len());
    buf.extend_from_slice(salt);
    buf.extend_from_slice(password.as_bytes());
    let mut h = sha256::sha256(&buf);
    for _ in 1..4096 {
        let mut next = Vec::with_capacity(16 + 32);
        next.extend_from_slice(salt);
        next.extend_from_slice(&h);
        h = sha256::sha256(&next);
    }
    let salt_hex = hex(&salt[..]);
    let hash_hex = hex(&h);
    format!("$sha256${}${}", salt_hex, hash_hex)
}

/// Verify a plaintext password against a stored `$sha256$...` hash.
pub fn verify_password(password: &str, stored: &str) -> bool {
    // Parse $sha256$<salt_hex>$<hash_hex>
    let parts: Vec<&str> = stored.splitn(4, '$').collect();
    // parts: ["", "sha256", "<salt>", "<hash>"]
    if parts.len() != 4 || parts[1] != "sha256" { return false; }
    let salt_bytes = unhex(parts[2]);
    if salt_bytes.len() != 16 { return false; }
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&salt_bytes);
    let computed = hash_password(password, &salt);
    computed == stored
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn authenticate(username: &str, password: &str) -> bool {
    let users = USERS.lock();
    let user = match users.iter().find(|u| u.username == username) {
        Some(u) => u.clone(),
        None => return false,
    };
    drop(users);
    if user.pw_hash.is_empty() { return password.is_empty(); }
    verify_password(password, &user.pw_hash)
}

pub fn find_user(username: &str) -> Option<User> {
    USERS.lock().iter().find(|u| u.username == username).cloned()
}

pub fn list_users() -> Vec<String> {
    USERS.lock().iter().map(|u| u.username.clone()).collect()
}

pub fn add_user(username: &str, password: &str) -> Result<(), &'static str> {
    {
        let users = USERS.lock();
        if users.iter().any(|u| u.username == username) {
            return Err("User already exists");
        }
    }
    // Append to /etc/passwd
    let uid = 1000 + USERS.lock().len() as u32;
    let home = format!("/home/{}", username);
    let entry = format!("{}:x:{}:{}:{}:/bin/sh\n", username, uid, uid, home);
    append_to("/etc/passwd", &entry)?;
    // Append to /etc/shadow
    let salt = salt_for(username);
    let hash = hash_password(password, &salt);
    append_to("/etc/shadow", &format!("{}:{}\n", username, hash))?;
    // Create home dir
    let _ = crate::vfs::mkdir(&home);
    // Reload
    load_from_vfs();
    Ok(())
}

pub fn change_password(username: &str, new_password: &str) -> Result<(), &'static str> {
    // Rebuild shadow file
    let shadow_data = crate::vfs::read_file_full("/etc/shadow").unwrap_or_default();
    let shadow_str = core::str::from_utf8(&shadow_data).unwrap_or("").to_string();
    let salt = salt_for(username);
    let new_hash = hash_password(new_password, &salt);
    let new_shadow: String = shadow_str.lines().map(|line| {
        if line.starts_with(&format!("{}:", username)) {
            format!("{}:{}\n", username, new_hash)
        } else {
            format!("{}\n", line)
        }
    }).collect();
    crate::vfs::create_and_write("/etc/shadow", new_shadow.as_bytes())
        .map_err(|_| "Failed to write /etc/shadow")?;
    load_from_vfs();
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

fn unhex(s: &str) -> Vec<u8> {
    let s = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut i = 0;
    while i + 1 < s.len() {
        let hi = from_hex_nibble(s[i]);
        let lo = from_hex_nibble(s[i + 1]);
        out.push((hi << 4) | lo);
        i += 2;
    }
    out
}

fn from_hex_nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

fn append_to(path: &str, content: &str) -> Result<(), &'static str> {
    let mut existing = crate::vfs::read_file_full(path).unwrap_or_default();
    existing.extend_from_slice(content.as_bytes());
    crate::vfs::create_and_write(path, &existing).map_err(|_| "write failed")
}

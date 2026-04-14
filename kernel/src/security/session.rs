/// Multi-User Session Manager for Smart OS.
///
/// Phase 25: Enterprise Sovereign Identity.
/// Tracks active user sessions, isolated environments, and their
/// respective home directory encryption keys.

use spin::Mutex;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Active,
    Locked,
    LoggedOut,
}

pub struct UserSession {
    pub session_id: u32,
    pub username: String,
    pub user_id: u32,
    pub state: SessionState,
    pub active_tty: u32,
    /// Placeholder for the AES-NI master key for the user's /home isolation
    pub fde_key_hash: [u8; 32], 
}

pub struct SessionManager {
    sessions: BTreeMap<u32, UserSession>,
    next_id: u32,
    active_session_id: Option<u32>,
}

impl SessionManager {
    pub const fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
            next_id: 1,
            active_session_id: None,
        }
    }

    pub fn login(&mut self, username: &str, user_id: u32) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        
        let session = UserSession {
            session_id: id,
            username: String::from(username),
            user_id,
            state: SessionState::Active,
            active_tty: 1,
            fde_key_hash: [0; 32],
        };

        self.sessions.insert(id, session);
        self.active_session_id = Some(id);
        
        crate::serial_println!("[session] User '{}' logged in. Session ID: {}", username, id);
        id
    }

    pub fn lock(&mut self, session_id: u32) {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.state = SessionState::Locked;
            crate::serial_println!("[session] Session {} locked.", session_id);
        }
    }

    pub fn unlock(&mut self, session_id: u32) {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.state = SessionState::Active;
            self.active_session_id = Some(session_id);
            crate::serial_println!("[session] Session {} unlocked.", session_id);
        }
    }
}

pub static SESSIONS: Mutex<SessionManager> = Mutex::new(SessionManager::new());

pub fn init() {
    let mut manager = SESSIONS.lock();
    // Auto-login the default root/admin for now, later replaced by SmartID biometric.
    manager.login("admin", 0);
    crate::serial_println!("[session] Multi-user session manager initialized.");
}

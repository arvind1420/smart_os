/// Environment variables for Smart OS processes.
///
/// Phase 9: Global environment variable store with default system variables.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

/// Global environment variable store.
static ENV: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// Initialize the environment with default system variables.
pub fn init() {
    let mut env = ENV.lock();
    env.insert(String::from("HOME"), String::from("/home/user"));
    env.insert(String::from("PATH"), String::from("/bin"));
    env.insert(String::from("SHELL"), String::from("smartsh"));
    env.insert(String::from("USER"), String::from("user"));
    env.insert(String::from("HOSTNAME"), String::from("smartos"));
    env.insert(String::from("VERSION"), String::from("0.9.0"));
    env.insert(String::from("TERM"), String::from("smartterm"));
    let count = env.len();
    crate::serial_println!("[env] Environment initialized ({} variables).", count);
}

/// Set an environment variable (insert or update).
pub fn set(key: &str, value: &str) {
    ENV.lock().insert(String::from(key), String::from(value));
}

/// Get the value of an environment variable.
pub fn get(key: &str) -> Option<String> {
    ENV.lock().get(key).cloned()
}

/// Remove an environment variable. Returns true if the variable existed.
pub fn remove(key: &str) -> bool {
    ENV.lock().remove(key).is_some()
}

/// List all environment variables as a sorted vector of (key, value) pairs.
pub fn list() -> Vec<(String, String)> {
    let env = ENV.lock();
    // BTreeMap is already sorted by key, so iteration order is sorted.
    env.iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

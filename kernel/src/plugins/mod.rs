/// Plugin system for Smart OS.
///
/// Provides capability-based plugins with lifecycle management.
/// Plugins are registered with a manifest describing their permissions,
/// then verified, started (as kernel threads), and managed.

pub mod capability;
pub mod manifest;
pub mod registry;

/// Initialize the plugin system.
pub fn init() {
    registry::init();
    crate::serial_println!("[plugins] Plugin system initialized.");
}

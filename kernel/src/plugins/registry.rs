/// Plugin registry and lifecycle management.
///
/// Manages plugin instances: register → verify → start → stop → unload.
/// Built-in plugins include the PS/2 mouse driver wrapper.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use super::manifest::{PluginManifest, PluginState};
use super::capability::{Capability, CapabilitySet};

/// A loaded plugin instance.
pub struct PluginInstance {
    pub manifest: PluginManifest,
    pub state: PluginState,
    pub loaded_at: u64,
}

/// The global plugin registry.
struct PluginRegistry {
    plugins: BTreeMap<String, PluginInstance>,
    /// Master set of allowed capabilities (all caps allowed in kernel mode).
    allowed_caps: CapabilitySet,
}

impl PluginRegistry {
    fn new() -> Self {
        // In kernel mode, all capabilities are allowed
        let mut allowed = CapabilitySet::new();
        allowed.grant(Capability::ReadFile);
        allowed.grant(Capability::WriteFile);
        allowed.grant(Capability::IpcSend);
        allowed.grant(Capability::IpcRecv);
        allowed.grant(Capability::IoPort);
        allowed.grant(Capability::Interrupt);
        allowed.grant(Capability::GuiRender);
        allowed.grant(Capability::Timer);
        allowed.grant(Capability::Memory);

        Self {
            plugins: BTreeMap::new(),
            allowed_caps: allowed,
        }
    }
}

static REGISTRY: Mutex<Option<PluginRegistry>> = Mutex::new(None);

/// Initialize the plugin registry.
pub fn init() {
    *REGISTRY.lock() = Some(PluginRegistry::new());
}

/// Register a plugin manifest.
pub fn register(manifest: PluginManifest) -> Result<(), &'static str> {
    let mut reg = REGISTRY.lock();
    let registry = reg.as_mut().ok_or("Plugin registry not initialized")?;

    if registry.plugins.contains_key(&manifest.name) {
        return Err("Plugin already registered");
    }

    let name = manifest.name.clone();
    crate::serial_println!("[plugins] Registered '{}' v{} ({} caps)",
        name, manifest.version, manifest.capabilities.len());

    registry.plugins.insert(name, PluginInstance {
        manifest,
        state: PluginState::Registered,
        loaded_at: crate::drivers::timer::ticks(),
    });

    Ok(())
}

/// Verify a plugin's capabilities against allowed set.
pub fn verify(name: &str) -> Result<(), &'static str> {
    let mut reg = REGISTRY.lock();
    let registry = reg.as_mut().ok_or("Plugin registry not initialized")?;

    let instance = registry.plugins.get_mut(name).ok_or("Plugin not found")?;

    if !instance.manifest.capabilities.is_subset_of(&registry.allowed_caps) {
        instance.state = PluginState::Error;
        return Err("Plugin requests disallowed capabilities");
    }

    instance.state = PluginState::Verified;
    crate::serial_println!("[plugins] Verified '{}'", name);
    Ok(())
}

/// Start a plugin by spawning its entry point as a kernel thread.
pub fn start(name: &str) -> Result<(), &'static str> {
    let mut reg = REGISTRY.lock();
    let registry = reg.as_mut().ok_or("Plugin registry not initialized")?;

    let instance = registry.plugins.get_mut(name).ok_or("Plugin not found")?;

    if instance.state != PluginState::Verified {
        return Err("Plugin not verified");
    }

    let entry = instance.manifest.entry_point.ok_or("No entry point")?;

    instance.state = PluginState::Running;
    let thread_name = alloc::format!("plugin:{}", name);

    // Drop the lock before spawning to avoid deadlock
    drop(reg);

    crate::process::scheduler::spawn(
        // Use a leaked string for the thread name since spawn takes &str
        // This is acceptable for kernel-lifetime plugins
        Box::leak(thread_name.into_boxed_str()),
        entry,
        6, // Medium priority
    );

    crate::serial_println!("[plugins] Started '{}'", name);
    Ok(())
}

/// Stop a plugin.
pub fn stop(name: &str) -> Result<(), &'static str> {
    let mut reg = REGISTRY.lock();
    let registry = reg.as_mut().ok_or("Plugin registry not initialized")?;

    let instance = registry.plugins.get_mut(name).ok_or("Plugin not found")?;
    instance.state = PluginState::Stopped;
    crate::serial_println!("[plugins] Stopped '{}'", name);
    Ok(())
}

/// List all plugins with their states.
pub fn list_plugins() -> Vec<(String, PluginState)> {
    let reg = REGISTRY.lock();
    reg.as_ref()
        .map(|r| r.plugins.iter().map(|(k, v)| (k.clone(), v.state)).collect())
        .unwrap_or_default()
}

/// Get a plugin's state.
pub fn get_state(name: &str) -> Option<PluginState> {
    let reg = REGISTRY.lock();
    reg.as_ref().and_then(|r| r.plugins.get(name).map(|p| p.state))
}

/// Load and start all built-in plugins.
pub fn load_builtin_plugins() {
    // PS/2 Mouse driver plugin
    let mouse_manifest = PluginManifest::new("ps2-mouse", "1.0.0")
        .with_description("PS/2 mouse input to GUI events")
        .with_capability(Capability::IoPort)
        .with_capability(Capability::Interrupt)
        .with_capability(Capability::GuiRender)
        .with_entry(mouse_plugin_entry);

    register(mouse_manifest).ok();
    verify("ps2-mouse").ok();
    start("ps2-mouse").ok();

    // Keyboard input plugin
    let kb_manifest = PluginManifest::new("keyboard-input", "1.0.0")
        .with_description("Keyboard input to GUI widgets")
        .with_capability(Capability::Interrupt)
        .with_capability(Capability::GuiRender)
        .with_entry(keyboard_plugin_entry);

    register(kb_manifest).ok();
    verify("keyboard-input").ok();
    start("keyboard-input").ok();

    crate::serial_println!("[plugins] Built-in plugins loaded.");
}

/// Mouse plugin entry point — forwards mouse events to GUI.
fn mouse_plugin_entry() {
    crate::serial_println!("[plugin:ps2-mouse] Mouse driver plugin running.");
    loop {
        if let Some(event) = crate::drivers::mouse::read_event() {
            crate::gui::handle_mouse_event(event);
        }
        crate::process::scheduler::yield_now();
    }
}

/// Keyboard plugin entry point — forwards keyboard events to GUI widgets.
fn keyboard_plugin_entry() {
    crate::serial_println!("[plugin:keyboard-input] Keyboard input plugin running.");
    loop {
        if let Some(event) = crate::drivers::keyboard::read_key() {
            crate::gui::handle_key_event(event);
        }
        crate::process::scheduler::yield_now();
    }
}

use alloc::boxed::Box;

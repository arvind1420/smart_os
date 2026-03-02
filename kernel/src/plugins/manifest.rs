/// Plugin manifest — describes a plugin's identity and requirements.

use alloc::string::String;
use super::capability::{Capability, CapabilitySet};

/// Plugin lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    Registered,
    Verified,
    Running,
    Stopped,
    Error,
}

impl PluginState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PluginState::Registered => "registered",
            PluginState::Verified => "verified",
            PluginState::Running => "running",
            PluginState::Stopped => "stopped",
            PluginState::Error => "error",
        }
    }
}

/// Plugin manifest describing a plugin's identity and permissions.
#[derive(Debug, Clone)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub capabilities: CapabilitySet,
    /// Entry point for built-in (kernel-compiled) plugins.
    pub entry_point: Option<fn()>,
}

impl PluginManifest {
    pub fn new(name: &str, version: &str) -> Self {
        Self {
            name: String::from(name),
            version: String::from(version),
            author: String::from("Smart OS"),
            description: String::new(),
            capabilities: CapabilitySet::new(),
            entry_point: None,
        }
    }

    pub fn with_author(mut self, author: &str) -> Self {
        self.author = String::from(author);
        self
    }

    pub fn with_description(mut self, desc: &str) -> Self {
        self.description = String::from(desc);
        self
    }

    pub fn with_capability(mut self, cap: Capability) -> Self {
        self.capabilities.grant(cap);
        self
    }

    pub fn with_entry(mut self, entry: fn()) -> Self {
        self.entry_point = Some(entry);
        self
    }

    /// Serialize manifest info to SmartPack Value.
    pub fn to_smartpack(&self) -> smartpack::Value {
        use alloc::vec::Vec;
        use smartpack::Value;

        let caps: Vec<Value> = self.capabilities.list()
            .iter()
            .map(|&s| Value::String(String::from(s)))
            .collect();

        let mut map = Vec::new();
        map.push((Value::String(String::from("name")), Value::String(self.name.clone())));
        map.push((Value::String(String::from("version")), Value::String(self.version.clone())));
        map.push((Value::String(String::from("author")), Value::String(self.author.clone())));
        map.push((Value::String(String::from("capabilities")), Value::Array(caps)));

        Value::Map(map)
    }
}

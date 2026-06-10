//! SmartPkg format based on SmartPack.
//!
//! A SmartPkg (.spk) is a single SmartPack file containing:
//! - Manifest: metadata like name, version, description, and required capabilities.
//! - Payload: the ELF binary data.
//! - Signature: cryptographic signature for verification.

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::types::Value;

/// A parsed SmartPkg.
#[derive(Debug, Clone)]
pub struct SmartPkg {
    pub name: String,
    pub version: String,
    pub description: String,
    pub capabilities: Vec<String>,
    pub binary_payload: Vec<u8>,
    pub signature: Vec<u8>,
}

impl SmartPkg {
    /// Create a new SmartPkg from its components.
    pub fn new(name: &str, version: &str, description: &str, binary: Vec<u8>) -> Self {
        Self {
            name: String::from(name),
            version: String::from(version),
            description: String::from(description),
            capabilities: Vec::new(),
            binary_payload: binary,
            signature: Vec::new(),
        }
    }

    /// Encode the SmartPkg into a SmartPack byte vector.
    pub fn encode(&self) -> Result<Vec<u8>, crate::encode::EncodeError> {
        let manifest = Value::Map(alloc::vec![
            (Value::from("name"), Value::from(self.name.clone())),
            (Value::from("version"), Value::from(self.version.clone())),
            (Value::from("description"), Value::from(self.description.clone())),
            (Value::from("capabilities"), Value::Array(self.capabilities.iter().map(|c| Value::from(c.clone())).collect())),
        ]);

        let root = Value::Map(alloc::vec![
            (Value::from("manifest"), manifest),
            (Value::from("payload"), Value::Binary(self.binary_payload.clone())),
            (Value::from("signature"), Value::Binary(self.signature.clone())),
        ]);

        crate::encode::encode(&root)
    }

    /// Decode a SmartPkg from SmartPack bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        let root = crate::decode::decode(bytes).map_err(|_| "Failed to decode SmartPack")?;
        
        let map = root.as_map().ok_or("Root is not a map")?;
        
        let manifest_val = map.iter().find(|(k, _)| k.as_str() == Some("manifest")).map(|(_, v)| v).ok_or("Missing manifest")?;
        let payload_val = map.iter().find(|(k, _)| k.as_str() == Some("payload")).map(|(_, v)| v).ok_or("Missing payload")?;
        let signature_val = map.iter().find(|(k, _)| k.as_str() == Some("signature")).map(|(_, v)| v).ok_or("Missing signature")?;

        let manifest_map = manifest_val.as_map().ok_or("Manifest is not a map")?;
        
        let name = manifest_map.iter().find(|(k, _)| k.as_str() == Some("name")).map(|(_, v)| v.as_str()).flatten().ok_or("Missing name")?.to_string();
        let version = manifest_map.iter().find(|(k, _)| k.as_str() == Some("version")).map(|(_, v)| v.as_str()).flatten().ok_or("Missing version")?.to_string();
        let description = manifest_map.iter().find(|(k, _)| k.as_str() == Some("description")).map(|(_, v)| v.as_str()).flatten().ok_or("Missing description")?.to_string();
        
        let capabilities = if let Some(cap_val) = manifest_map.iter().find(|(k, _)| k.as_str() == Some("capabilities")).map(|(_, v)| v.as_array()).flatten() {
            cap_val.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
        } else {
            Vec::new()
        };

        let binary_payload = payload_val.as_bytes().ok_or("Payload is not binary")?.to_vec();
        let signature = signature_val.as_bytes().ok_or("Signature is not binary")?.to_vec();

        Ok(Self {
            name,
            version,
            description,
            capabilities,
            binary_payload,
            signature,
        })
    }
}

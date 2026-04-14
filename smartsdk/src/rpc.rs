/// Remote Procedure Call (RPC) Framework.
///
/// Uses the IPC message-passing layer and SmartPack serialization
/// to provide strongly-typed service calls between processes.

use alloc::string::String;
use alloc::vec::Vec;
use smartpack::{Value, encode, decode};
use crate::ipc::IpcPort;

pub struct RpcClient {
    target_port: u64,
}

impl RpcClient {
    /// Connect to a named service.
    pub fn connect(service_name: &str) -> Result<Self, &'static str> {
        let port = IpcPort::lookup(service_name)?;
        Ok(Self { target_port: port })
    }

    /// Call a method on the service with arguments, returning the response.
    /// In a real system, we'd wait for a reply on a dedicated reply port.
    /// For this MVP, we just send. To make it a true RPC, the server would 
    /// reply to the client's port. We simulate this by having a generic "call"
    /// that packs the data.
    pub fn call(&self, method: &str, args: Value) -> Result<(), &'static str> {
        let mut msg = alloc::vec::Vec::new();
        msg.push((Value::String(String::from("method")), Value::String(String::from(method))));
        msg.push((Value::String(String::from("args")), args));
        
        let encoded = encode(&Value::Map(msg)).map_err(|_| "RPC encode failed")?;
        
        IpcPort::send(self.target_port, &encoded)
    }
}

/// SmartPack-based IPC channels.
///
/// A channel is a bounded FIFO queue that carries SmartPack Values
/// between kernel threads (and eventually userspace processes).

use alloc::collections::VecDeque;
use alloc::string::String;
use smartpack::Value;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Unique channel ID.
pub type ChannelId = u64;

static NEXT_CHANNEL_ID: AtomicU64 = AtomicU64::new(1);

/// A bounded message channel.
pub struct Channel {
    pub id: ChannelId,
    pub name: String,
    queue: Mutex<VecDeque<Message>>,
    capacity: usize,
}

/// A message sent through a channel.
#[derive(Debug, Clone)]
pub struct Message {
    /// Sender thread ID.
    pub sender_tid: u64,
    /// The payload — a SmartPack Value.
    pub payload: Value,
}

/// Channel errors.
#[derive(Debug)]
pub enum ChannelError {
    Full,
    Empty,
    NotFound,
}

impl Channel {
    /// Create a new channel with the given name and capacity.
    pub fn new(name: &str, capacity: usize) -> Self {
        Self {
            id: NEXT_CHANNEL_ID.fetch_add(1, Ordering::Relaxed),
            name: String::from(name),
            queue: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    /// Send a message (non-blocking). Returns Err(Full) if the queue is full.
    pub fn send(&self, sender_tid: u64, payload: Value) -> Result<(), ChannelError> {
        let mut queue = self.queue.lock();
        if queue.len() >= self.capacity {
            return Err(ChannelError::Full);
        }
        queue.push_back(Message { sender_tid, payload });
        Ok(())
    }

    /// Receive a message (non-blocking). Returns Err(Empty) if no messages.
    pub fn recv(&self) -> Result<Message, ChannelError> {
        let mut queue = self.queue.lock();
        queue.pop_front().ok_or(ChannelError::Empty)
    }

    /// Check if there are pending messages.
    pub fn has_messages(&self) -> bool {
        !self.queue.lock().is_empty()
    }

    /// Number of pending messages.
    pub fn len(&self) -> usize {
        self.queue.lock().len()
    }
}

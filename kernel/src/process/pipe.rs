/// Kernel pipe implementation for Smart OS.
///
/// Provides byte-stream IPC between processes via ring buffers.

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

const PIPE_BUF_SIZE: usize = 4096;

/// A kernel pipe: fixed-size ring buffer.
pub struct Pipe {
    buffer: [u8; PIPE_BUF_SIZE],
    read_pos: usize,
    write_pos: usize,
    count: usize,
    pub closed_write: bool,
    pub closed_read: bool,
}

impl Pipe {
    fn new() -> Self {
        Self {
            buffer: [0u8; PIPE_BUF_SIZE],
            read_pos: 0,
            write_pos: 0,
            count: 0,
            closed_write: false,
            closed_read: false,
        }
    }

    /// Read up to `buf.len()` bytes. Returns number of bytes read.
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let to_read = buf.len().min(self.count);
        for i in 0..to_read {
            buf[i] = self.buffer[self.read_pos];
            self.read_pos = (self.read_pos + 1) % PIPE_BUF_SIZE;
        }
        self.count -= to_read;
        to_read
    }

    /// Write up to `data.len()` bytes. Returns number of bytes written.
    pub fn write(&mut self, data: &[u8]) -> usize {
        let space = PIPE_BUF_SIZE - self.count;
        let to_write = data.len().min(space);
        for i in 0..to_write {
            self.buffer[self.write_pos] = data[i];
            self.write_pos = (self.write_pos + 1) % PIPE_BUF_SIZE;
        }
        self.count += to_write;
        to_write
    }

    /// Available bytes to read.
    pub fn available(&self) -> usize {
        self.count
    }
}

/// Global pipe table.
pub static PIPES: Mutex<BTreeMap<u64, Pipe>> = Mutex::new(BTreeMap::new());
static NEXT_PIPE_ID: AtomicU64 = AtomicU64::new(1);

/// Create a new pipe. Returns the pipe ID.
pub fn create_pipe() -> u64 {
    let id = NEXT_PIPE_ID.fetch_add(1, Ordering::Relaxed);
    PIPES.lock().insert(id, Pipe::new());
    id
}

/// Read from a pipe.
pub fn pipe_read(pipe_id: u64, buf: &mut [u8]) -> Result<usize, &'static str> {
    let mut pipes = PIPES.lock();
    let pipe = pipes.get_mut(&pipe_id).ok_or("Invalid pipe")?;
    if pipe.count == 0 && pipe.closed_write {
        return Ok(0); // EOF
    }
    Ok(pipe.read(buf))
}

/// Write to a pipe.
pub fn pipe_write(pipe_id: u64, data: &[u8]) -> Result<usize, &'static str> {
    let mut pipes = PIPES.lock();
    let pipe = pipes.get_mut(&pipe_id).ok_or("Invalid pipe")?;
    if pipe.closed_read {
        return Err("Broken pipe");
    }
    Ok(pipe.write(data))
}

/// Close the write end of a pipe.
pub fn close_write(pipe_id: u64) {
    if let Some(pipe) = PIPES.lock().get_mut(&pipe_id) {
        pipe.closed_write = true;
    }
}

/// Close the read end of a pipe.
pub fn close_read(pipe_id: u64) {
    if let Some(pipe) = PIPES.lock().get_mut(&pipe_id) {
        pipe.closed_read = true;
    }
}

/// Destroy a pipe if both ends are closed.
pub fn maybe_destroy(pipe_id: u64) {
    let mut pipes = PIPES.lock();
    if let Some(pipe) = pipes.get(&pipe_id) {
        if pipe.closed_read && pipe.closed_write {
            pipes.remove(&pipe_id);
        }
    }
}

/// Check if a pipe has data available to read (for select/poll).
pub fn pipe_readable(pipe_id: u64) -> bool {
    let pipes = PIPES.lock();
    pipes.get(&pipe_id).map(|p| p.count > 0 || p.closed_write).unwrap_or(false)
}

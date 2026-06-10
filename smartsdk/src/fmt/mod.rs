/// Minimal zero-allocation string formatting for Smart OS SDK.

use core::fmt::{self, Write};

pub mod locale;
pub mod translate;

pub struct StringBuffer<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> StringBuffer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl<'a> Write for StringBuffer<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let remaining = self.buf.len() - self.len;
        if remaining < bytes.len() {
            return Err(fmt::Error);
        }
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

/// A macro that formats into a fixed-size stack buffer and yields a `&str`.
#[macro_export]
macro_rules! format_buf {
    ($buf:expr, $($arg:tt)*) => {{
        use core::fmt::Write;
        let mut sb = $crate::fmt::StringBuffer::new($buf);
        let _ = write!(&mut sb, $($arg)*);
        let len = sb.len();
        unsafe { core::str::from_utf8_unchecked(&$buf[..len]) }
    }};
}

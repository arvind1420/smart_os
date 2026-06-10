//! License Client API for Smart OS Apps.

use crate::syscall::{syscall0, SYS_LICENSE_CHECK};

/// Check if the system has a Pro/Enterprise license active.
pub fn is_pro() -> bool {
    syscall0(SYS_LICENSE_CHECK) == 1
}

/// Helper for apps to display a "Community Edition" watermark if not licensed.
pub fn watermark() -> &'static str {
    if is_pro() { "" } else { "Community Edition" }
}

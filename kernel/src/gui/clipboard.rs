/// System clipboard for Smart OS.
///
/// A simple global string clipboard supporting copy, paste, cut, and clear.

use alloc::string::String;
use spin::Mutex;

/// Global clipboard storage.
static CLIPBOARD: Mutex<Option<String>> = Mutex::new(None);

/// Copy text into the clipboard.
pub fn copy(text: &str) {
    *CLIPBOARD.lock() = Some(String::from(text));
}

/// Paste: return a clone of the clipboard contents, or None if empty.
pub fn paste() -> Option<String> {
    CLIPBOARD.lock().clone()
}

/// Cut: store text in the clipboard (caller is responsible for removing from source).
pub fn cut(text: &str) {
    *CLIPBOARD.lock() = Some(String::from(text));
}

/// Check whether the clipboard has content.
pub fn has_content() -> bool {
    CLIPBOARD.lock().is_some()
}

/// Clear the clipboard.
pub fn clear() {
    *CLIPBOARD.lock() = None;
}

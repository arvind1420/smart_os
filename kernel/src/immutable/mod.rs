/// Immutable System Core for Smart OS.
///
/// Implements an A/B system partition approach where the OS core is
/// read-only. Updates are applied via SmartPack deltas to a secondary
/// partition, ensuring the system never "rots" or slows down over time.

pub mod partition;
pub mod protection;

/// Initialize the immutable system core.
pub fn init() {
    protection::init();
    partition::init();
    crate::serial_println!("[immutable] Immutable system core initialized.");
}

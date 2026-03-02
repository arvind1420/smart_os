/// Predictive prefetching for Smart OS.
///
/// Pre-loads predicted app data into kernel page cache during idle time.
/// Runs as a kernel thread, periodically checking predictions and
/// warming the cache for apps the user is likely to launch.

use alloc::collections::BTreeSet;
use alloc::string::String;
use spin::Mutex;

/// Set of app names whose data is currently warm in cache.
pub static WARM_CACHE: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Only prefetch if prediction confidence exceeds this threshold.
const PREFETCH_THRESHOLD: f32 = 0.15;

/// Maximum number of apps to prefetch concurrently.
const MAX_PREFETCH: usize = 3;

/// Run one prefetch cycle.
/// Called from the prefetch-worker kernel thread.
pub fn prefetch_cycle() {
    let predictions = super::predictor::predict_next(MAX_PREFETCH);

    for (app_name, confidence) in &predictions {
        if *confidence < PREFETCH_THRESHOLD {
            break; // Predictions are sorted by confidence
        }

        let mut cache = WARM_CACHE.lock();
        if cache.contains(app_name) {
            continue; // Already warm
        }

        // "Pre-load" the app data by reading its VFS entry.
        // For kernel apps (terminal, file-manager, sysmon) this is a no-op
        // since they are compiled into the kernel. For user-space ELFs,
        // reading the binary touches VFS pages and warms the cache.
        let elf_path = alloc::format!("/bin/{}", app_name);
        if let Ok(fd) = crate::vfs::open(&elf_path) {
            let mut buf = [0u8; 4096];
            let _ = crate::vfs::read(fd, &mut buf);
            crate::vfs::close(fd).ok();
        }

        cache.insert(app_name.clone());
        crate::serial_println!(
            "[prefetch] Pre-loaded '{}' (confidence: {:.0}%)",
            app_name,
            confidence * 100.0,
        );
    }
}

/// Invalidate a cache entry when an app exits.
pub fn invalidate(app_name: &str) {
    WARM_CACHE.lock().remove(app_name);
}

/// Get the number of currently warm cache entries.
pub fn warm_count() -> usize {
    WARM_CACHE.lock().len()
}

/// Get the list of currently warm apps.
pub fn warm_apps() -> alloc::vec::Vec<String> {
    WARM_CACHE.lock().iter().cloned().collect()
}

/// The prefetch worker thread entry point.
/// Runs in a loop, periodically checking predictions and pre-loading.
pub fn prefetch_worker() {
    crate::serial_println!("[prefetch] Prefetch worker thread started.");

    loop {
        // Run a prefetch cycle
        prefetch_cycle();

        // Yield to other threads, then wait before checking again.
        // We run prefetch infrequently to avoid wasting CPU.
        for _ in 0..50 {
            crate::process::scheduler::yield_now();
        }
    }
}

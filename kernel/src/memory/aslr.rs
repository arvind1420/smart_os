/// Address Space Layout Randomization (ASLR) for Smart OS.
///
/// Phase 10: Randomizes the base addresses of user-space process
/// memory regions (stack, heap, mmap, ELF load address) to prevent
/// code-reuse attacks like ROP.
///
/// Uses a simple LFSR-based PRNG seeded from the PIT timer counter
/// and RTC clock for entropy.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::serial_println;

/// PRNG state (LFSR-based).
static PRNG_STATE: AtomicU64 = AtomicU64::new(0xDEAD_BEEF_CAFE_1337);

/// Whether ASLR is enabled.
static ASLR_ENABLED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(true);

/// Initialize ASLR with entropy from hardware.
pub fn init() {
    // Collect entropy from multiple sources
    let tsc = read_tsc();
    let pit = crate::drivers::timer::uptime_ticks();
    let rtc_raw = crate::drivers::rtc::read_rtc_raw();

    // Mix entropy sources
    let seed = tsc
        .wrapping_mul(0x517CC1B7_27220A95)
        .wrapping_add(pit)
        .wrapping_mul(0x6C62272E_07BB0142)
        .wrapping_add(rtc_raw as u64);

    if seed != 0 {
        PRNG_STATE.store(seed, Ordering::Relaxed);
    }

    serial_println!("[aslr] ASLR initialized (entropy seed: {:#X}).", seed & 0xFFFF);
}

/// Read the CPU timestamp counter for entropy.
fn read_tsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Generate a pseudo-random 64-bit number using xorshift64.
pub fn random_u64() -> u64 {
    let mut state = PRNG_STATE.load(Ordering::Relaxed);
    // xorshift64
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    PRNG_STATE.store(state, Ordering::Relaxed);
    state
}

/// Generate a random number in [0, max).
pub fn random_range(max: u64) -> u64 {
    if max == 0 { return 0; }
    random_u64() % max
}

/// Randomize a user-space stack address.
///
/// The stack top is in the range [0x7FFF_FF00_0000, 0x7FFF_FFFF_0000)
/// with page-aligned offsets.
pub fn randomize_stack_top() -> u64 {
    if !ASLR_ENABLED.load(Ordering::Relaxed) {
        return 0x7FFF_FFFF_0000; // Default fixed address
    }

    let base = 0x7FFF_FF00_0000u64;
    let range_pages = 255u64; // ~1MiB range
    let offset = random_range(range_pages) * 4096;
    base + offset
}

/// Randomize the ELF load base address.
///
/// Range: [0x0040_0000, 0x0100_0000) — standard low user-space.
pub fn randomize_elf_base() -> u64 {
    if !ASLR_ENABLED.load(Ordering::Relaxed) {
        return 0x0040_0000; // Default fixed address
    }

    let base = 0x0040_0000u64;
    let range_pages = 768u64; // 3MiB range
    let offset = random_range(range_pages) * 4096;
    base + offset
}

/// Randomize the mmap/shmem base address.
///
/// Range: [0x1000_0000, 0x3000_0000).
pub fn randomize_mmap_base() -> u64 {
    if !ASLR_ENABLED.load(Ordering::Relaxed) {
        return 0x1000_0000;
    }

    let base = 0x1000_0000u64;
    let range_pages = 0x2000u64; // 32MiB range
    let offset = random_range(range_pages) * 4096;
    base + offset
}

/// Randomize the heap start address.
///
/// Placed after ELF segments, with a random gap.
pub fn randomize_heap_start(elf_end: u64) -> u64 {
    if !ASLR_ENABLED.load(Ordering::Relaxed) {
        return (elf_end + 0xFFF) & !0xFFF; // Page-align
    }

    let aligned = (elf_end + 0xFFF) & !0xFFF;
    let gap_pages = 16 + random_range(64); // 64KiB to 320KiB gap
    aligned + gap_pages * 4096
}

/// Enable or disable ASLR.
pub fn set_enabled(enabled: bool) {
    ASLR_ENABLED.store(enabled, Ordering::Relaxed);
    serial_println!("[aslr] ASLR {}", if enabled { "enabled" } else { "disabled" });
}

/// Check if ASLR is enabled.
pub fn is_enabled() -> bool {
    ASLR_ENABLED.load(Ordering::Relaxed)
}

/// Get ASLR entropy bits used for each region type.
pub fn entropy_info() -> &'static str {
    "Stack: 8 bits, ELF: 10 bits, mmap: 13 bits, heap: 6 bits"
}

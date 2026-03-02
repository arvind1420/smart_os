/// Programmable Interval Timer (PIT) driver.
///
/// Configures the PIT to fire at ~100Hz, providing the heartbeat
/// for preemptive scheduling and timekeeping.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::port::Port;

/// PIT oscillator frequency (1.193182 MHz)
const PIT_FREQUENCY: u32 = 1_193_182;
/// Desired tick rate in Hz
const TARGET_HZ: u32 = 100;
/// PIT divisor = oscillator_freq / target_hz
const DIVISOR: u16 = (PIT_FREQUENCY / TARGET_HZ) as u16;

/// PIT I/O ports
const PIT_CHANNEL0: u16 = 0x40;
const PIT_CMD: u16 = 0x43;

/// Global tick counter — incremented every timer interrupt (~100Hz).
pub static TICKS: AtomicU64 = AtomicU64::new(0);

/// Initialize the PIT to fire at ~100Hz.
pub fn init() {
    unsafe {
        // Channel 0, lobyte/hibyte, rate generator mode
        Port::<u8>::new(PIT_CMD).write(0x36);
        // Send divisor (low byte first, then high byte)
        Port::<u8>::new(PIT_CHANNEL0).write((DIVISOR & 0xFF) as u8);
        Port::<u8>::new(PIT_CHANNEL0).write((DIVISOR >> 8) as u8);
    }
    crate::serial_println!("[drivers] PIT initialized at ~{}Hz (divisor={}).", TARGET_HZ, DIVISOR);
}

/// Called from the timer interrupt handler.
pub fn tick() {
    let t = TICKS.fetch_add(1, Ordering::Relaxed);
    // Update RTC cached time once per second (~100 ticks)
    if t % 100 == 0 {
        super::rtc::update();
    }
}

/// Get the current tick count.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Get approximate uptime in seconds.
pub fn uptime_secs() -> u64 {
    ticks() / TARGET_HZ as u64
}

/// Get approximate uptime in milliseconds.
pub fn uptime_ms() -> u64 {
    ticks() * 1000 / TARGET_HZ as u64
}

/// Alias for ticks() — used by ASLR for entropy.
pub fn uptime_ticks() -> u64 {
    ticks()
}

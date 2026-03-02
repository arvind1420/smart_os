/// Real-Time Clock (RTC) driver.
///
/// Reads date/time from the CMOS RTC via I/O ports 0x70/0x71.
/// Provides a cached DateTime updated once per second from the timer interrupt.

use spin::Mutex;
use x86_64::instructions::port::Port;

/// A date-time snapshot from the RTC.
#[derive(Debug, Clone)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    pub const fn zero() -> Self {
        Self { year: 2000, month: 1, day: 1, hour: 0, minute: 0, second: 0 }
    }
}

/// Cached time, updated every ~1 second by the timer interrupt.
pub static CACHED_TIME: Mutex<DateTime> = Mutex::new(DateTime::zero());

/// CMOS I/O ports.
const CMOS_INDEX: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

/// Read a CMOS register by index.
fn cmos_read(reg: u8) -> u8 {
    unsafe {
        // Select register (preserve NMI disable bit — clear bit 7 to keep NMI enabled)
        Port::<u8>::new(CMOS_INDEX).write(reg & 0x7F);
        Port::<u8>::new(CMOS_DATA).read()
    }
}

/// Convert BCD-encoded byte to binary.
fn bcd_to_bin(val: u8) -> u8 {
    (val & 0x0F) + ((val >> 4) * 10)
}

/// Read the current date/time from the RTC hardware.
pub fn read_rtc() -> DateTime {
    // Wait for any update-in-progress to finish (register 0x0A, bit 7).
    while cmos_read(0x0A) & 0x80 != 0 {}

    let raw_sec = cmos_read(0x00);
    let raw_min = cmos_read(0x02);
    let raw_hour = cmos_read(0x04);
    let raw_day = cmos_read(0x07);
    let raw_month = cmos_read(0x08);
    let raw_year = cmos_read(0x09);

    let status_b = cmos_read(0x0B);
    let is_binary = status_b & 0x04 != 0;
    let is_24h = status_b & 0x02 != 0;

    let (second, minute, mut hour, day, month, year_2d) = if is_binary {
        (raw_sec, raw_min, raw_hour, raw_day, raw_month, raw_year)
    } else {
        (
            bcd_to_bin(raw_sec),
            bcd_to_bin(raw_min),
            bcd_to_bin(raw_hour & 0x7F) | (raw_hour & 0x80), // preserve PM bit
            bcd_to_bin(raw_day),
            bcd_to_bin(raw_month),
            bcd_to_bin(raw_year),
        )
    };

    // Handle 12-hour mode: if PM bit (0x80) set, add 12 (unless it's 12 PM)
    if !is_24h && hour & 0x80 != 0 {
        hour = ((hour & 0x7F) % 12) + 12;
    }

    DateTime {
        year: 2000 + year_2d as u16,
        month,
        day,
        hour,
        minute,
        second,
    }
}

/// Initialize the RTC driver: read initial time and cache it.
pub fn init() {
    let dt = read_rtc();
    crate::serial_println!(
        "[drivers] RTC initialized: {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
    );
    *CACHED_TIME.lock() = dt;
}

/// Update the cached time (called from timer interrupt, every ~100 ticks = 1 second).
pub fn update() {
    *CACHED_TIME.lock() = read_rtc();
}

/// Return a clone of the cached date-time.
pub fn cached_time() -> DateTime {
    CACHED_TIME.lock().clone()
}

/// Alias for cached_time().
pub fn now() -> DateTime {
    cached_time()
}

/// Read a raw RTC value for entropy (seconds + minutes combined).
pub fn read_rtc_raw() -> u32 {
    let sec = cmos_read(0x00) as u32;
    let min = cmos_read(0x02) as u32;
    let hour = cmos_read(0x04) as u32;
    sec | (min << 8) | (hour << 16)
}

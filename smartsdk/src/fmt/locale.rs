//! Locale-aware formatting for Smart OS — Phase 44.

use alloc::string::String;
use alloc::format;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    EnUS,
    FrFR,
    DeDE,
    EsES,
    JaJP,
}

impl Locale {
    /// Get the decimal separator for this locale.
    pub fn decimal_sep(&self) -> char {
        match self {
            Locale::EnUS | Locale::JaJP => '.',
            Locale::FrFR | Locale::DeDE | Locale::EsES => ',',
        }
    }

    /// Get the thousands separator for this locale.
    pub fn thousands_sep(&self) -> char {
        match self {
            Locale::EnUS | Locale::JaJP => ',',
            Locale::DeDE | Locale::EsES => '.',
            Locale::FrFR => ' ',
        }
    }

    /// Get current system locale.
    pub fn current() -> Self {
        // In a real OS, we'd query a system setting.
        Locale::EnUS
    }
}

fn abs_f64(x: f64) -> f64 {
    if x < 0.0 { -x } else { x }
}

fn pow10(precision: usize) -> i64 {
    let mut res = 1;
    for _ in 0..precision {
        res *= 10;
    }
    res
}

/// Format a number with locale-specific separators.
pub fn format_number(val: f64, precision: usize, locale: Locale) -> String {
    let is_negative = val < 0.0;
    let abs_val = abs_f64(val);
    
    let mult = pow10(precision);
    let total_rounded = (abs_val * (mult as f64) + 0.5) as i64;
    
    let rounded_int = if precision > 0 {
        total_rounded / mult
    } else {
        total_rounded
    };
    
    let rounded_fract = if precision > 0 {
        total_rounded % mult
    } else {
        0
    };

    let int_str = format!("{}", rounded_int);
    let thousands_sep = locale.thousands_sep();
    
    // Insert thousands separator
    let mut formatted_int = String::new();
    let char_count = int_str.len();
    for (i, c) in int_str.chars().enumerate() {
        formatted_int.push(c);
        let remaining = char_count - 1 - i;
        if remaining > 0 && remaining % 3 == 0 {
            formatted_int.push(thousands_sep);
        }
    }

    let prefix = if is_negative { "-" } else { "" };

    if precision > 0 {
        format!("{}{}{}{:0width$}", prefix, formatted_int, locale.decimal_sep(), rounded_fract, width = precision)
    } else {
        format!("{}{}", prefix, formatted_int)
    }
}

/// Format currency based on the locale.
pub fn format_currency(val: f64, locale: Locale) -> String {
    match locale {
        Locale::EnUS => {
            format!("${}", format_number(val, 2, locale))
        }
        Locale::JaJP => {
            // Japanese Yen typically has no fractional part
            format!("¥{}", format_number(val, 0, locale))
        }
        Locale::FrFR | Locale::DeDE | Locale::EsES => {
            format!("{} €", format_number(val, 2, locale))
        }
    }
}

/// Format a date according to the locale.
pub fn format_date(year: i32, month: u8, day: u8, locale: Locale) -> String {
    match locale {
        Locale::EnUS => {
            format!("{:02}/{:02}/{:04}", month, day, year)
        }
        Locale::DeDE => {
            format!("{:02}.{:02}.{:04}", day, month, year)
        }
        Locale::FrFR | Locale::EsES => {
            format!("{:02}/{:02}/{:04}", day, month, year)
        }
        Locale::JaJP => {
            format!("{:04}/{:02}/{:02}", year, month, day)
        }
    }
}

/// Format a time according to the locale.
pub fn format_time(hour: u8, minute: u8, second: u8, locale: Locale) -> String {
    match locale {
        Locale::EnUS => {
            let ampm = if hour >= 12 { "PM" } else { "AM" };
            let h12 = if hour == 0 {
                12
            } else if hour > 12 {
                hour - 12
            } else {
                hour
            };
            format!("{:02}:{:02}:{:02} {}", h12, minute, second, ampm)
        }
        _ => {
            format!("{:02}:{:02}:{:02}", hour, minute, second)
        }
    }
}

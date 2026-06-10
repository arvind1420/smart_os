//! Internationalization & Locale System — Phase 60: Unicode & Localization (v0.20.0).
//!
//! Provides locale-aware formatting for numbers, dates, times, and currencies.
//! Ships 10 built-in locales (en-US, en-GB, de-DE, fr-FR, es-ES, ar-SA,
//! zh-CN, ja-JP, pt-BR, ru-RU).  The active locale is stored as an AtomicU8
//! so it can be read from any context without locking.

#![allow(dead_code)]

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, Ordering};

// ─── Locale identifier ───────────────────────────────────────────────────────

/// Built-in locale identifiers.  The `u8` discriminant is used as an index
/// into `LOCALES` and stored in `CURRENT_LOCALE_ID`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LocaleId {
    EnUs = 0,
    EnGb = 1,
    DeDe = 2,
    FrFr = 3,
    EsEs = 4,
    ArSa = 5,
    ZhCn = 6,
    JaJp = 7,
    PtBr = 8,
    RuRu = 9,
}

impl LocaleId {
    pub fn from_u8(n: u8) -> Self {
        match n {
            1 => LocaleId::EnGb,
            2 => LocaleId::DeDe,
            3 => LocaleId::FrFr,
            4 => LocaleId::EsEs,
            5 => LocaleId::ArSa,
            6 => LocaleId::ZhCn,
            7 => LocaleId::JaJp,
            8 => LocaleId::PtBr,
            9 => LocaleId::RuRu,
            _ => LocaleId::EnUs,
        }
    }

    /// Slice of all supported locale IDs.
    pub fn all() -> &'static [LocaleId] {
        static ALL: [LocaleId; 10] = [
            LocaleId::EnUs, LocaleId::EnGb, LocaleId::DeDe, LocaleId::FrFr,
            LocaleId::EsEs, LocaleId::ArSa, LocaleId::ZhCn, LocaleId::JaJp,
            LocaleId::PtBr, LocaleId::RuRu,
        ];
        &ALL
    }
}

// ─── Date ordering ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateOrder {
    /// Month / Day / Year (e.g., 12/25/2024)
    MDY,
    /// Day / Month / Year (e.g., 25/12/2024)
    DMY,
    /// Year - Month - Day  (e.g., 2024-12-25)
    YMD,
}

// ─── Locale descriptor ───────────────────────────────────────────────────────

pub struct Locale {
    pub id:              LocaleId,
    /// BCP-47 language tag, e.g. `"en-US"`.
    pub code:            &'static str,
    /// Human-readable name in the native language.
    pub name:            &'static str,
    /// Decimal separator character (`'.'` or `','`).
    pub decimal_sep:     char,
    /// Thousands grouping separator (`','`, `'.'`, or `' '`).
    pub thousands_sep:   char,
    /// Currency symbol string (may be multi-char, e.g. `"R$"`).
    pub currency_sym:    &'static str,
    /// `true` → symbol precedes amount (`"$1.00"`), `false` → follows (`"1,00 €"`).
    pub currency_before: bool,
    /// Ordering of year / month / day components.
    pub date_order:      DateOrder,
    /// Separator between date components.
    pub date_sep:        char,
    /// `true` → 24-hour clock, `false` → 12-hour AM/PM.
    pub time_24h:        bool,
    /// `true` if the locale is Right-to-Left (Arabic, Hebrew, …).
    pub is_rtl:          bool,
}

// ─── Locale table ────────────────────────────────────────────────────────────
// Indexed by LocaleId discriminant.

static LOCALES: [Locale; 10] = [
    // 0 – en-US
    Locale {
        id: LocaleId::EnUs, code: "en-US",
        name: "English (United States)",
        decimal_sep: '.', thousands_sep: ',',
        currency_sym: "$", currency_before: true,
        date_order: DateOrder::MDY, date_sep: '/',
        time_24h: false, is_rtl: false,
    },
    // 1 – en-GB
    Locale {
        id: LocaleId::EnGb, code: "en-GB",
        name: "English (United Kingdom)",
        decimal_sep: '.', thousands_sep: ',',
        currency_sym: "\u{00A3}", currency_before: true, // £
        date_order: DateOrder::DMY, date_sep: '/',
        time_24h: false, is_rtl: false,
    },
    // 2 – de-DE
    Locale {
        id: LocaleId::DeDe, code: "de-DE",
        name: "Deutsch (Deutschland)",
        decimal_sep: ',', thousands_sep: '.',
        currency_sym: "\u{20AC}", currency_before: false, // €
        date_order: DateOrder::DMY, date_sep: '.',
        time_24h: true, is_rtl: false,
    },
    // 3 – fr-FR
    Locale {
        id: LocaleId::FrFr, code: "fr-FR",
        name: "Fran\u{00E7}ais (France)",
        decimal_sep: ',', thousands_sep: ' ',
        currency_sym: "\u{20AC}", currency_before: false, // €
        date_order: DateOrder::DMY, date_sep: '/',
        time_24h: true, is_rtl: false,
    },
    // 4 – es-ES
    Locale {
        id: LocaleId::EsEs, code: "es-ES",
        name: "Espa\u{00F1}ol (Espa\u{00F1}a)",
        decimal_sep: ',', thousands_sep: '.',
        currency_sym: "\u{20AC}", currency_before: false, // €
        date_order: DateOrder::DMY, date_sep: '/',
        time_24h: true, is_rtl: false,
    },
    // 5 – ar-SA
    Locale {
        id: LocaleId::ArSa, code: "ar-SA",
        name: "\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064A}\u{0629}",
        decimal_sep: '.', thousands_sep: ',',
        currency_sym: "\u{FDFC}", currency_before: false, // ﷼
        date_order: DateOrder::DMY, date_sep: '/',
        time_24h: false, is_rtl: true,
    },
    // 6 – zh-CN
    Locale {
        id: LocaleId::ZhCn, code: "zh-CN",
        name: "\u{4E2D}\u{6587} (\u{4E2D}\u{56FD})", // 中文 (中国)
        decimal_sep: '.', thousands_sep: ',',
        currency_sym: "\u{00A5}", currency_before: true, // ¥
        date_order: DateOrder::YMD, date_sep: '-',
        time_24h: true, is_rtl: false,
    },
    // 7 – ja-JP
    Locale {
        id: LocaleId::JaJp, code: "ja-JP",
        name: "\u{65E5}\u{672C}\u{8A9E} (\u{65E5}\u{672C})", // 日本語 (日本)
        decimal_sep: '.', thousands_sep: ',',
        currency_sym: "\u{00A5}", currency_before: true, // ¥
        date_order: DateOrder::YMD, date_sep: '/',
        time_24h: true, is_rtl: false,
    },
    // 8 – pt-BR
    Locale {
        id: LocaleId::PtBr, code: "pt-BR",
        name: "Portugu\u{00EA}s (Brasil)",
        decimal_sep: ',', thousands_sep: '.',
        currency_sym: "R$", currency_before: true,
        date_order: DateOrder::DMY, date_sep: '/',
        time_24h: true, is_rtl: false,
    },
    // 9 – ru-RU
    Locale {
        id: LocaleId::RuRu, code: "ru-RU",
        name: "\u{0420}\u{0443}\u{0441}\u{0441}\u{043A}\u{0438}\u{0439} (\u{0420}\u{043E}\u{0441}\u{0441}\u{0438}\u{044F})",
        decimal_sep: ',', thousands_sep: ' ',
        currency_sym: "\u{20BD}", currency_before: false, // ₽
        date_order: DateOrder::DMY, date_sep: '.',
        time_24h: true, is_rtl: false,
    },
];

// ─── Active-locale state ──────────────────────────────────────────────────────

/// Currently active locale (stored as the `LocaleId` discriminant u8).
static CURRENT_LOCALE_ID: AtomicU8 = AtomicU8::new(0); // default: en-US

/// Retrieve the locale descriptor for `id`.
pub fn locale(id: LocaleId) -> &'static Locale {
    &LOCALES[id as usize]
}

/// Return the currently active locale ID.
pub fn get_locale() -> LocaleId {
    LocaleId::from_u8(CURRENT_LOCALE_ID.load(Ordering::Relaxed))
}

/// Set the currently active locale.
pub fn set_locale(id: LocaleId) {
    CURRENT_LOCALE_ID.store(id as u8, Ordering::Relaxed);
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Format an unsigned integer with locale-specific thousands grouping.
fn format_uint_grouped(n: u64, thousands_sep: char) -> String {
    if n == 0 { return String::from("0"); }

    // Build digits in reverse (least-significant first)
    let mut rev: Vec<u8> = Vec::with_capacity(20);
    let mut tmp = n;
    while tmp > 0 {
        rev.push((tmp % 10) as u8);
        tmp /= 10;
    }

    // Insert separators every 3 digits; collect reversed for final LTR order
    let mut rev_chars: Vec<char> = Vec::with_capacity(rev.len() + rev.len() / 3);
    for (i, &d) in rev.iter().enumerate() {
        if i > 0 && i % 3 == 0 {
            rev_chars.push(thousands_sep);
        }
        rev_chars.push((b'0' + d) as char);
    }

    rev_chars.into_iter().rev().collect()
}

// ─── Public formatting API ───────────────────────────────────────────────────

/// Format an integer with locale thousands grouping.
///
/// ```text
/// format_number(1_234_567, LocaleId::EnUs) → "1,234,567"
/// format_number(1_234_567, LocaleId::DeDe) → "1.234.567"
/// format_number(1_234_567, LocaleId::FrFr) → "1 234 567"
/// ```
pub fn format_number(n: i64, id: LocaleId) -> String {
    let loc = locale(id);
    let neg = n < 0;
    let abs_n: u64 = if neg { (n.wrapping_neg()) as u64 } else { n as u64 };
    let grouped = format_uint_grouped(abs_n, loc.thousands_sep);
    if neg {
        let mut s = String::with_capacity(grouped.len() + 1);
        s.push('-');
        s.push_str(&grouped);
        s
    } else {
        grouped
    }
}

/// Format a date with locale-specific ordering and separator.
///
/// ```text
/// format_date(2024, 12, 25, LocaleId::EnUs) → "12/25/2024"
/// format_date(2024, 12, 25, LocaleId::DeDe) → "25.12.2024"
/// format_date(2024, 12, 25, LocaleId::ZhCn) → "2024-12-25"
/// ```
pub fn format_date(year: u32, month: u8, day: u8, id: LocaleId) -> String {
    let loc = locale(id);
    let s = loc.date_sep;
    match loc.date_order {
        DateOrder::MDY => alloc::format!("{:02}{}{:02}{}{:04}", month, s, day,   s, year),
        DateOrder::DMY => alloc::format!("{:02}{}{:02}{}{:04}", day,   s, month, s, year),
        DateOrder::YMD => alloc::format!("{:04}{}{:02}{}{:02}", year,  s, month, s, day),
    }
}

/// Format a wall-clock time with locale-specific 12/24-hour convention.
///
/// ```text
/// format_time(14, 30,  0, LocaleId::EnUs) → "02:30:00 PM"
/// format_time(14, 30,  0, LocaleId::DeDe) → "14:30:00"
/// ```
pub fn format_time(hour: u8, min: u8, sec: u8, id: LocaleId) -> String {
    let loc = locale(id);
    if loc.time_24h {
        alloc::format!("{:02}:{:02}:{:02}", hour, min, sec)
    } else {
        let pm = hour >= 12;
        let h12 = if hour == 0 { 12u8 } else if hour > 12 { hour - 12 } else { hour };
        alloc::format!("{:02}:{:02}:{:02} {}", h12, min, sec, if pm { "PM" } else { "AM" })
    }
}

/// Format an amount expressed in the smallest currency unit (cents / pence / …)
/// using locale-specific grouping, decimal separator, and currency symbol.
///
/// ```text
/// format_currency(123456, LocaleId::EnUs) → "$1,234.56"
/// format_currency(123456, LocaleId::DeDe) → "1.234,56 €"
/// format_currency(    99, LocaleId::EnUs) → "$0.99"
/// ```
pub fn format_currency(cents: i64, id: LocaleId) -> String {
    let loc = locale(id);
    let neg = cents < 0;
    let abs_cents: u64 = if neg { (cents.wrapping_neg()) as u64 } else { cents as u64 };
    let units = abs_cents / 100;
    let frac  = (abs_cents % 100) as u8;

    let int_str = format_uint_grouped(units, loc.thousands_sep);
    let sign    = if neg { "-" } else { "" };

    if loc.currency_before {
        alloc::format!("{}{}{}{}{:02}", sign, loc.currency_sym, int_str, loc.decimal_sep, frac)
    } else {
        alloc::format!("{}{}{}{:02} {}", sign, int_str, loc.decimal_sep, frac, loc.currency_sym)
    }
}

/// Return a short list of locale descriptors (for settings display).
pub fn all_locales() -> Vec<&'static Locale> {
    LocaleId::all().iter().map(|&id| locale(id)).collect()
}

/// No-op init (no mutable globals beyond the atomic locale ID).
pub fn init() {}

/// Boot self-test — verifies number/date/time/currency formatting across
/// several locales and edge cases.
pub fn self_test() -> bool {
    let mut ok = true;

    // ── format_number ────────────────────────────────────────────────
    if format_number(1_234_567, LocaleId::EnUs) != "1,234,567" { ok = false; }
    if format_number(1_234_567, LocaleId::DeDe) != "1.234.567" { ok = false; }
    if format_number(1_234_567, LocaleId::FrFr) != "1 234 567" { ok = false; }
    if format_number(          0, LocaleId::EnUs) != "0"         { ok = false; }
    if format_number(        -42, LocaleId::EnUs) != "-42"       { ok = false; }
    if format_number(       1000, LocaleId::EnUs) != "1,000"     { ok = false; }

    // ── format_date ──────────────────────────────────────────────────
    if format_date(2024,  3, 15, LocaleId::EnUs) != "03/15/2024" { ok = false; }
    if format_date(2024,  3, 15, LocaleId::DeDe) != "15.03.2024" { ok = false; }
    if format_date(2024,  3, 15, LocaleId::ZhCn) != "2024-03-15" { ok = false; }
    if format_date(2024, 12, 25, LocaleId::JaJp) != "2024/12/25" { ok = false; }

    // ── format_time ──────────────────────────────────────────────────
    if format_time(14, 30, 0, LocaleId::EnUs) != "02:30:00 PM" { ok = false; }
    if format_time(14, 30, 0, LocaleId::DeDe) != "14:30:00"    { ok = false; }
    if format_time( 0,  0, 0, LocaleId::EnUs) != "12:00:00 AM" { ok = false; }
    if format_time(12,  0, 0, LocaleId::EnUs) != "12:00:00 PM" { ok = false; }

    // ── format_currency ──────────────────────────────────────────────
    if format_currency(123_456, LocaleId::EnUs) != "$1,234.56"    { ok = false; }
    if format_currency(123_456, LocaleId::DeDe) != "1.234,56 \u{20AC}" { ok = false; }
    if format_currency(     99, LocaleId::EnUs) != "$0.99"         { ok = false; }
    if format_currency(      0, LocaleId::EnUs) != "$0.00"         { ok = false; }

    // ── locale metadata ──────────────────────────────────────────────
    if LocaleId::all().len() != 10 { ok = false; }
    if locale(LocaleId::ArSa).is_rtl != true  { ok = false; }
    if locale(LocaleId::EnUs).is_rtl != false { ok = false; }
    if locale(LocaleId::DeDe).time_24h != true  { ok = false; }
    if locale(LocaleId::EnUs).time_24h != false { ok = false; }

    // ── get/set locale ───────────────────────────────────────────────
    let orig = get_locale();
    set_locale(LocaleId::DeDe);
    if get_locale() != LocaleId::DeDe { ok = false; }
    set_locale(orig);
    if get_locale() != orig { ok = false; }

    ok
}

//! Lightweight Bidirectional (BiDi) text reordering algorithm for Smart OS.
//!
//! Handles basic RTL (Arabic, Hebrew) character detection, neutral character resolution,
//! run segmenting, RTL run reversing, and base direction run ordering.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CharType {
    L, // Left-to-Right (Latin, Cyrillic, CJK, etc.)
    R, // Right-to-Left (Hebrew, Arabic, Syriac, etc.)
    N, // Neutral (spaces, punctuation, symbols)
}

/// Detect the strong type of a character.
fn get_char_type(c: char) -> CharType {
    let val = c as u32;
    // Hebrew: U+0590..=U+05FF, Arabic: U+0600..=U+08FF
    // Arabic presentation forms: U+FB1D..=U+FDFF, U+FE70..=U+FEFC
    if (val >= 0x0590 && val <= 0x08FF) || (val >= 0xFB1D && val <= 0xFDFF) || (val >= 0xFE70 && val <= 0xFEFC) {
        CharType::R
    } else if c.is_alphabetic() {
        CharType::L
    } else if c.is_numeric() {
        CharType::L // digits are drawn LTR in RTL contexts
    } else {
        CharType::N
    }
}

/// Reorders a mixed LTR/RTL text line so it renders correctly when drawn left-to-right.
pub fn bidi_reorder(text: &str) -> String {
    // If there are no RTL characters at all, return as-is immediately to avoid allocations.
    if !text.chars().any(|c| {
        let val = c as u32;
        (val >= 0x0590 && val <= 0x08FF) || (val >= 0xFB1D && val <= 0xFDFF) || (val >= 0xFE70 && val <= 0xFEFC)
    }) {
        return String::from(text);
    }

    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n == 0 {
        return String::new();
    }

    // 1. Assign initial strong/neutral types
    let mut types = alloc::vec![CharType::N; n];
    for i in 0..n {
        types[i] = get_char_type(chars[i]);
    }

    // 2. Resolve base direction (first strong character, defaults to LTR)
    let mut base_dir = CharType::L;
    for i in 0..n {
        if types[i] != CharType::N {
            base_dir = types[i];
            break;
        }
    }

    // 3. Resolve neutral runs using surrounding strong types
    let mut resolved = types.clone();
    for i in 0..n {
        if types[i] == CharType::N {
            // Find left strong type
            let mut left = base_dir;
            for j in (0..i).rev() {
                if types[j] != CharType::N {
                    left = types[j];
                    break;
                }
            }
            // Find right strong type
            let mut right = base_dir;
            for j in (i+1)..n {
                if types[j] != CharType::N {
                    right = types[j];
                    break;
                }
            }
            if left == right {
                resolved[i] = left;
            } else {
                resolved[i] = base_dir;
            }
        }
    }

    // 4. Split into runs of L and R
    struct Run {
        is_rtl: bool,
        chars: Vec<char>,
    }
    let mut runs: Vec<Run> = Vec::new();
    let mut current_rtl = resolved[0] == CharType::R;
    let mut current_chars = alloc::vec![chars[0]];

    for i in 1..n {
        let rtl = resolved[i] == CharType::R;
        if rtl == current_rtl {
            current_chars.push(chars[i]);
        } else {
            runs.push(Run { is_rtl: current_rtl, chars: current_chars });
            current_rtl = rtl;
            current_chars = alloc::vec![chars[i]];
        }
    }
    runs.push(Run { is_rtl: current_rtl, chars: current_chars });

    // 5. Reverse the character order inside RTL runs
    for run in runs.iter_mut() {
        if run.is_rtl {
            run.chars.reverse();
        }
    }

    // 6. If the base paragraph direction is RTL, reverse the order of runs
    if base_dir == CharType::R {
        runs.reverse();
    }

    // 7. Reassemble
    let mut result = String::with_capacity(n);
    for run in runs {
        for c in run.chars {
            result.push(c);
        }
    }
    result
}

/// Returns `true` if the paragraph's base direction is RTL, determined by
/// the first strong-type character (UAX #9 rules P2/P3).
/// Returns `false` (LTR) when no strong character is found.
pub fn bidi_paragraph_dir(text: &str) -> bool {
    for c in text.chars() {
        match get_char_type(c) {
            CharType::R => return true,
            CharType::L => return false,
            CharType::N => {}
        }
    }
    false // default LTR
}

/// Reorder `text` with an explicit base-direction override.
///
/// * `force_rtl = Some(true)`  → treat paragraph as RTL regardless of content.
/// * `force_rtl = Some(false)` → treat paragraph as LTR regardless of content.
/// * `force_rtl = None`        → auto-detect (identical to `bidi_reorder`).
pub fn bidi_reorder_with_base(text: &str, force_rtl: Option<bool>) -> String {
    let has_rtl = text.chars().any(|c| {
        let v = c as u32;
        (v >= 0x0590 && v <= 0x08FF)
            || (v >= 0xFB1D && v <= 0xFDFF)
            || (v >= 0xFE70 && v <= 0xFEFC)
    });

    if !has_rtl && force_rtl != Some(true) {
        return String::from(text);
    }

    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n == 0 { return String::new(); }

    let mut types = alloc::vec![CharType::N; n];
    for i in 0..n { types[i] = get_char_type(chars[i]); }

    let base_dir = match force_rtl {
        Some(true)  => CharType::R,
        Some(false) => CharType::L,
        None => {
            let mut bd = CharType::L;
            for i in 0..n {
                if types[i] != CharType::N { bd = types[i]; break; }
            }
            bd
        }
    };

    let mut resolved = types.clone();
    for i in 0..n {
        if types[i] == CharType::N {
            let mut left = base_dir;
            for j in (0..i).rev() {
                if types[j] != CharType::N { left = types[j]; break; }
            }
            let mut right = base_dir;
            for j in (i + 1)..n {
                if types[j] != CharType::N { right = types[j]; break; }
            }
            resolved[i] = if left == right { left } else { base_dir };
        }
    }

    struct Run { is_rtl: bool, chars: Vec<char> }
    let mut runs: Vec<Run> = Vec::new();
    let mut cur_rtl = resolved[0] == CharType::R;
    let mut cur_chars = alloc::vec![chars[0]];

    for i in 1..n {
        let rtl = resolved[i] == CharType::R;
        if rtl == cur_rtl {
            cur_chars.push(chars[i]);
        } else {
            runs.push(Run { is_rtl: cur_rtl, chars: cur_chars });
            cur_rtl = rtl;
            cur_chars = alloc::vec![chars[i]];
        }
    }
    runs.push(Run { is_rtl: cur_rtl, chars: cur_chars });

    for run in runs.iter_mut() {
        if run.is_rtl { run.chars.reverse(); }
    }
    if base_dir == CharType::R { runs.reverse(); }

    let mut result = String::with_capacity(n);
    for run in runs { for c in run.chars { result.push(c); } }
    result
}

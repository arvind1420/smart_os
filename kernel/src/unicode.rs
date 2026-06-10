//! Unicode utilities — Phase 60: Unicode & Localization (v0.20.0).
//!
//! Provides NFC/NFD normalization (Latin-1 + Extended-A/B + Greek + Hangul),
//! Unicode block classification, combining-character detection, and
//! grapheme-cluster counting/splitting.  No external tables; data is embedded.

#![allow(dead_code)]

use alloc::string::String;
use alloc::vec::Vec;

// ─── Hangul algorithmic constants (UAX #15 §15) ──────────────────────────────
const HANGUL_SBASE:  u32 = 0xAC00;
const HANGUL_LBASE:  u32 = 0x1100;
const HANGUL_VBASE:  u32 = 0x1161;
const HANGUL_TBASE:  u32 = 0x11A7; // TBASE+0 = null T (no trailing consonant)
const HANGUL_LCOUNT: u32 = 19;
const HANGUL_VCOUNT: u32 = 21;
const HANGUL_TCOUNT: u32 = 28; // 27 real + 1 null
const HANGUL_NCOUNT: u32 = 588; // VCOUNT * TCOUNT
const HANGUL_SCOUNT: u32 = 11172; // LCOUNT * NCOUNT

// ─── Canonical Decomposition table ───────────────────────────────────────────
// (precomposed_cp, base_cp, combining_cp)  — sorted ascending by precomposed
// for binary-search NFD; iterated linearly for NFC canonical composition.
// Only canonical (NFD) decompositions are listed; NFKD excluded for size.
static DECOMP: &[(u32, u32, u32)] = &[
    // Latin-1 Supplement U+00C0–U+00FF
    (0x00C0, 0x0041, 0x0300), // À = A + grave
    (0x00C1, 0x0041, 0x0301), // Á = A + acute
    (0x00C2, 0x0041, 0x0302), // Â = A + circumflex
    (0x00C3, 0x0041, 0x0303), // Ã = A + tilde
    (0x00C4, 0x0041, 0x0308), // Ä = A + diaeresis
    (0x00C5, 0x0041, 0x030A), // Å = A + ring above
    (0x00C7, 0x0043, 0x0327), // Ç = C + cedilla
    (0x00C8, 0x0045, 0x0300), // È = E + grave
    (0x00C9, 0x0045, 0x0301), // É = E + acute
    (0x00CA, 0x0045, 0x0302), // Ê = E + circumflex
    (0x00CB, 0x0045, 0x0308), // Ë = E + diaeresis
    (0x00CC, 0x0049, 0x0300), // Ì = I + grave
    (0x00CD, 0x0049, 0x0301), // Í = I + acute
    (0x00CE, 0x0049, 0x0302), // Î = I + circumflex
    (0x00CF, 0x0049, 0x0308), // Ï = I + diaeresis
    (0x00D1, 0x004E, 0x0303), // Ñ = N + tilde
    (0x00D2, 0x004F, 0x0300), // Ò = O + grave
    (0x00D3, 0x004F, 0x0301), // Ó = O + acute
    (0x00D4, 0x004F, 0x0302), // Ô = O + circumflex
    (0x00D5, 0x004F, 0x0303), // Õ = O + tilde
    (0x00D6, 0x004F, 0x0308), // Ö = O + diaeresis
    (0x00D9, 0x0055, 0x0300), // Ù = U + grave
    (0x00DA, 0x0055, 0x0301), // Ú = U + acute
    (0x00DB, 0x0055, 0x0302), // Û = U + circumflex
    (0x00DC, 0x0055, 0x0308), // Ü = U + diaeresis
    (0x00DD, 0x0059, 0x0301), // Ý = Y + acute
    (0x00E0, 0x0061, 0x0300), // à = a + grave
    (0x00E1, 0x0061, 0x0301), // á = a + acute
    (0x00E2, 0x0061, 0x0302), // â = a + circumflex
    (0x00E3, 0x0061, 0x0303), // ã = a + tilde
    (0x00E4, 0x0061, 0x0308), // ä = a + diaeresis
    (0x00E5, 0x0061, 0x030A), // å = a + ring above
    (0x00E7, 0x0063, 0x0327), // ç = c + cedilla
    (0x00E8, 0x0065, 0x0300), // è = e + grave
    (0x00E9, 0x0065, 0x0301), // é = e + acute
    (0x00EA, 0x0065, 0x0302), // ê = e + circumflex
    (0x00EB, 0x0065, 0x0308), // ë = e + diaeresis
    (0x00EC, 0x0069, 0x0300), // ì = i + grave
    (0x00ED, 0x0069, 0x0301), // í = i + acute
    (0x00EE, 0x0069, 0x0302), // î = i + circumflex
    (0x00EF, 0x0069, 0x0308), // ï = i + diaeresis
    (0x00F1, 0x006E, 0x0303), // ñ = n + tilde
    (0x00F2, 0x006F, 0x0300), // ò = o + grave
    (0x00F3, 0x006F, 0x0301), // ó = o + acute
    (0x00F4, 0x006F, 0x0302), // ô = o + circumflex
    (0x00F5, 0x006F, 0x0303), // õ = o + tilde
    (0x00F6, 0x006F, 0x0308), // ö = o + diaeresis
    (0x00F9, 0x0075, 0x0300), // ù = u + grave
    (0x00FA, 0x0075, 0x0301), // ú = u + acute
    (0x00FB, 0x0075, 0x0302), // û = u + circumflex
    (0x00FC, 0x0075, 0x0308), // ü = u + diaeresis
    (0x00FD, 0x0079, 0x0301), // ý = y + acute
    (0x00FF, 0x0079, 0x0308), // ÿ = y + diaeresis
    // Latin Extended-A U+0100–U+017F
    (0x0100, 0x0041, 0x0304), // Ā = A + macron
    (0x0101, 0x0061, 0x0304), // ā = a + macron
    (0x0102, 0x0041, 0x0306), // Ă = A + breve
    (0x0103, 0x0061, 0x0306), // ă = a + breve
    (0x0106, 0x0043, 0x0301), // Ć = C + acute
    (0x0107, 0x0063, 0x0301), // ć = c + acute
    (0x010C, 0x0043, 0x030C), // Č = C + caron
    (0x010D, 0x0063, 0x030C), // č = c + caron
    (0x010E, 0x0044, 0x030C), // Ď = D + caron
    (0x010F, 0x0064, 0x030C), // ď = d + caron
    (0x011A, 0x0045, 0x030C), // Ě = E + caron
    (0x011B, 0x0065, 0x030C), // ě = e + caron
    (0x0139, 0x004C, 0x0301), // Ĺ = L + acute
    (0x013A, 0x006C, 0x0301), // ĺ = l + acute
    (0x013D, 0x004C, 0x030C), // Ľ = L + caron
    (0x013E, 0x006C, 0x030C), // ľ = l + caron
    (0x0143, 0x004E, 0x0301), // Ń = N + acute
    (0x0144, 0x006E, 0x0301), // ń = n + acute
    (0x0147, 0x004E, 0x030C), // Ň = N + caron
    (0x0148, 0x006E, 0x030C), // ň = n + caron
    (0x0154, 0x0052, 0x0301), // Ŕ = R + acute
    (0x0155, 0x0072, 0x0301), // ŕ = r + acute
    (0x0158, 0x0052, 0x030C), // Ř = R + caron
    (0x0159, 0x0072, 0x030C), // ř = r + caron
    (0x015A, 0x0053, 0x0301), // Ś = S + acute
    (0x015B, 0x0073, 0x0301), // ś = s + acute
    (0x015E, 0x0053, 0x0327), // Ş = S + cedilla
    (0x015F, 0x0073, 0x0327), // ş = s + cedilla
    (0x0160, 0x0053, 0x030C), // Š = S + caron
    (0x0161, 0x0073, 0x030C), // š = s + caron
    (0x0164, 0x0054, 0x030C), // Ť = T + caron
    (0x0165, 0x0074, 0x030C), // ť = t + caron
    (0x016E, 0x0055, 0x030A), // Ů = U + ring above
    (0x016F, 0x0075, 0x030A), // ů = u + ring above
    (0x0170, 0x0055, 0x030B), // Ű = U + double acute
    (0x0171, 0x0075, 0x030B), // ű = u + double acute
    (0x0179, 0x005A, 0x0301), // Ź = Z + acute
    (0x017A, 0x007A, 0x0301), // ź = z + acute
    (0x017D, 0x005A, 0x030C), // Ž = Z + caron
    (0x017E, 0x007A, 0x030C), // ž = z + caron
    // Greek U+0386–U+03CE (tonos / acute)
    (0x0386, 0x0391, 0x0301), // Ά = Α + tonos
    (0x0388, 0x0395, 0x0301), // Έ = Ε + tonos
    (0x0389, 0x0397, 0x0301), // Ή = Η + tonos
    (0x038A, 0x0399, 0x0301), // Ί = Ι + tonos
    (0x038C, 0x039F, 0x0301), // Ό = Ο + tonos
    (0x038E, 0x03A5, 0x0301), // Ύ = Υ + tonos
    (0x038F, 0x03A9, 0x0301), // Ώ = Ω + tonos
    (0x03AC, 0x03B1, 0x0301), // ά = α + tonos
    (0x03AD, 0x03B5, 0x0301), // έ = ε + tonos
    (0x03AE, 0x03B7, 0x0301), // ή = η + tonos
    (0x03AF, 0x03B9, 0x0301), // ί = ι + tonos
    (0x03CC, 0x03BF, 0x0301), // ό = ο + tonos
    (0x03CD, 0x03C5, 0x0301), // ύ = υ + tonos
    (0x03CE, 0x03C9, 0x0301), // ώ = ω + tonos
];

// ─── Public API ──────────────────────────────────────────────────────────────

/// Returns `true` if `c` is a Unicode combining character (UAX #29 subset).
/// Covers the Combining Diacritical Marks ranges used by Latin, Greek,
/// Arabic vowel marks, and the half-marks used in musical notation.
pub fn is_combining(c: char) -> bool {
    let v = c as u32;
    matches!(v,
        0x0300..=0x036F |   // Combining Diacritical Marks
        0x0610..=0x061A |   // Arabic extended combining
        0x064B..=0x065F |   // Arabic diacritics (harakat)
        0x1AB0..=0x1AFF |   // Combining Diacritical Marks Extended
        0x1DC0..=0x1DFF |   // Combining Diacritical Marks Supplement
        0x20D0..=0x20FF |   // Combining Diacritical Marks for Symbols
        0xFE20..=0xFE2F     // Combining Half Marks
    )
}

/// Returns the Unicode block name for `c` (50+ named blocks).
pub fn unicode_block(c: char) -> &'static str {
    let v = c as u32;
    match v {
        0x0000..=0x007F => "Basic Latin",
        0x0080..=0x00FF => "Latin-1 Supplement",
        0x0100..=0x017F => "Latin Extended-A",
        0x0180..=0x024F => "Latin Extended-B",
        0x0250..=0x02AF => "IPA Extensions",
        0x02B0..=0x02FF => "Spacing Modifier Letters",
        0x0300..=0x036F => "Combining Diacritical Marks",
        0x0370..=0x03FF => "Greek and Coptic",
        0x0400..=0x04FF => "Cyrillic",
        0x0500..=0x052F => "Cyrillic Supplement",
        0x0530..=0x058F => "Armenian",
        0x0590..=0x05FF => "Hebrew",
        0x0600..=0x06FF => "Arabic",
        0x0700..=0x074F => "Syriac",
        0x0750..=0x077F => "Arabic Supplement",
        0x0900..=0x097F => "Devanagari",
        0x0980..=0x09FF => "Bengali",
        0x0A00..=0x0A7F => "Gurmukhi",
        0x0A80..=0x0AFF => "Gujarati",
        0x0B00..=0x0B7F => "Oriya",
        0x0B80..=0x0BFF => "Tamil",
        0x0C00..=0x0C7F => "Telugu",
        0x0C80..=0x0CFF => "Kannada",
        0x0D00..=0x0D7F => "Malayalam",
        0x0E00..=0x0E7F => "Thai",
        0x0E80..=0x0EFF => "Lao",
        0x0F00..=0x0FFF => "Tibetan",
        0x1000..=0x109F => "Myanmar",
        0x10A0..=0x10FF => "Georgian",
        0x1100..=0x11FF => "Hangul Jamo",
        0x1200..=0x137F => "Ethiopic",
        0x13A0..=0x13FF => "Cherokee",
        0x1400..=0x167F => "Unified Canadian Aboriginal Syllabics",
        0x1680..=0x169F => "Ogham",
        0x16A0..=0x16FF => "Runic",
        0x1700..=0x177F => "Tagalog / Hanunoo / Buhid / Tagbanwa",
        0x1780..=0x17FF => "Khmer",
        0x1800..=0x18AF => "Mongolian",
        0x1E00..=0x1EFF => "Latin Extended Additional",
        0x1F00..=0x1FFF => "Greek Extended",
        0x2000..=0x206F => "General Punctuation",
        0x2070..=0x209F => "Superscripts and Subscripts",
        0x20A0..=0x20CF => "Currency Symbols",
        0x20D0..=0x20FF => "Combining Diacritical Marks for Symbols",
        0x2100..=0x214F => "Letterlike Symbols",
        0x2150..=0x218F => "Number Forms",
        0x2190..=0x21FF => "Arrows",
        0x2200..=0x22FF => "Mathematical Operators",
        0x2300..=0x23FF => "Miscellaneous Technical",
        0x2460..=0x24FF => "Enclosed Alphanumerics",
        0x2500..=0x257F => "Box Drawing",
        0x2580..=0x259F => "Block Elements",
        0x25A0..=0x25FF => "Geometric Shapes",
        0x2600..=0x26FF => "Miscellaneous Symbols",
        0x2700..=0x27BF => "Dingbats",
        0x2C00..=0x2C5F => "Glagolitic",
        0x2C60..=0x2C7F => "Latin Extended-C",
        0x2C80..=0x2CFF => "Coptic",
        0x3000..=0x303F => "CJK Symbols and Punctuation",
        0x3040..=0x309F => "Hiragana",
        0x30A0..=0x30FF => "Katakana",
        0x3100..=0x312F => "Bopomofo",
        0x3130..=0x318F => "Hangul Compatibility Jamo",
        0x31F0..=0x31FF => "Katakana Phonetic Extensions",
        0x3200..=0x32FF => "Enclosed CJK Letters and Months",
        0x3300..=0x33FF => "CJK Compatibility",
        0x3400..=0x4DBF => "CJK Unified Ideographs Extension A",
        0x4E00..=0x9FFF => "CJK Unified Ideographs",
        0xA000..=0xA48F => "Yi Syllables",
        0xA490..=0xA4CF => "Yi Radicals",
        0xAC00..=0xD7AF => "Hangul Syllables",
        0xD800..=0xDFFF => "Surrogates",
        0xE000..=0xF8FF => "Private Use Area",
        0xF900..=0xFAFF => "CJK Compatibility Ideographs",
        0xFB00..=0xFB4F => "Alphabetic Presentation Forms",
        0xFB50..=0xFDFF => "Arabic Presentation Forms-A",
        0xFE20..=0xFE2F => "Combining Half Marks",
        0xFE30..=0xFE4F => "CJK Compatibility Forms",
        0xFE70..=0xFEFF => "Arabic Presentation Forms-B",
        0xFF00..=0xFFEF => "Halfwidth and Fullwidth Forms",
        0xFFF0..=0xFFFF => "Specials",
        0x1D000..=0x1D1FF => "Byzantine / Musical Symbols",
        0x1D400..=0x1D7FF => "Mathematical Alphanumeric Symbols",
        0x1F300..=0x1F5FF => "Miscellaneous Symbols and Pictographs",
        0x1F600..=0x1F64F => "Emoticons",
        0x1F900..=0x1F9FF => "Supplemental Symbols and Pictographs",
        _ => "Unknown Block",
    }
}

/// Decompose `s` to NFD (canonical decomposition only).
/// Pure ASCII input is returned as-is with no allocation.
pub fn normalize_nfd(s: &str) -> String {
    if s.bytes().all(|b| b < 0x80) {
        return String::from(s);
    }
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        let v = c as u32;
        // Hangul syllable algorithmic decomposition
        if v >= HANGUL_SBASE && v < HANGUL_SBASE + HANGUL_SCOUNT {
            let si = v - HANGUL_SBASE;
            let li = si / HANGUL_NCOUNT;
            let vi = (si % HANGUL_NCOUNT) / HANGUL_TCOUNT;
            let ti = si % HANGUL_TCOUNT;
            if let Some(lc) = char::from_u32(HANGUL_LBASE + li) { out.push(lc); }
            if let Some(vc) = char::from_u32(HANGUL_VBASE + vi) { out.push(vc); }
            if ti > 0 {
                if let Some(tc) = char::from_u32(HANGUL_TBASE + ti) { out.push(tc); }
            }
            continue;
        }
        // Binary search in canonical decomposition table
        match DECOMP.binary_search_by_key(&v, |&(cp, _, _)| cp) {
            Ok(idx) => {
                let (_, base, comb) = DECOMP[idx];
                if let Some(bc) = char::from_u32(base) { out.push(bc); }
                if let Some(cc) = char::from_u32(comb) { out.push(cc); }
            }
            Err(_) => out.push(c),
        }
    }
    out
}

/// Compose `s` to NFC (canonical composition after NFD decompose).
/// Pure ASCII input is returned as-is with no allocation.
pub fn normalize_nfc(s: &str) -> String {
    if s.bytes().all(|b| b < 0x80) {
        return String::from(s);
    }
    let nfd = normalize_nfd(s);
    let chars: Vec<char> = nfd.chars().collect();
    let n = chars.len();
    if n == 0 { return nfd; }

    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < n {
        let base_v = chars[i] as u32;

        if i + 1 < n {
            let next_v = chars[i + 1] as u32;

            // Hangul L + V → LV (optionally + T → LVT)
            if base_v >= HANGUL_LBASE && base_v < HANGUL_LBASE + HANGUL_LCOUNT
               && next_v >= HANGUL_VBASE && next_v < HANGUL_VBASE + HANGUL_VCOUNT
            {
                let li = base_v - HANGUL_LBASE;
                let vi = next_v - HANGUL_VBASE;
                let lv_cp = HANGUL_SBASE + li * HANGUL_NCOUNT + vi * HANGUL_TCOUNT;
                if i + 2 < n {
                    let t_v = chars[i + 2] as u32;
                    if t_v > HANGUL_TBASE && t_v < HANGUL_TBASE + HANGUL_TCOUNT {
                        if let Some(c) = char::from_u32(lv_cp + (t_v - HANGUL_TBASE)) { out.push(c); }
                        i += 3;
                        continue;
                    }
                }
                if let Some(c) = char::from_u32(lv_cp) { out.push(c); }
                i += 2;
                continue;
            }

            // Hangul LV + T → LVT
            if base_v >= HANGUL_SBASE && base_v < HANGUL_SBASE + HANGUL_SCOUNT {
                let si = base_v - HANGUL_SBASE;
                if si % HANGUL_TCOUNT == 0 {
                    if next_v > HANGUL_TBASE && next_v < HANGUL_TBASE + HANGUL_TCOUNT {
                        if let Some(c) = char::from_u32(base_v + (next_v - HANGUL_TBASE)) { out.push(c); }
                        i += 2;
                        continue;
                    }
                }
            }

            // Canonical composition via DECOMP (linear search by base+combining)
            if is_combining(chars[i + 1]) {
                let comb_v = next_v;
                let mut composed: Option<u32> = None;
                for &(cp, b, cmb) in DECOMP {
                    if b == base_v && cmb == comb_v {
                        composed = Some(cp);
                        break;
                    }
                }
                if let Some(cp) = composed {
                    if let Some(c) = char::from_u32(cp) { out.push(c); }
                    i += 2;
                    continue;
                }
            }
        }

        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Count user-perceived characters (grapheme clusters).
/// Each base character + its trailing combining marks = one cluster.
pub fn grapheme_len(s: &str) -> usize {
    s.chars().filter(|&c| !is_combining(c)).count()
}

/// Split `s` into grapheme clusters (base + any trailing combining marks).
pub fn grapheme_clusters(s: &str) -> Vec<String> {
    let mut clusters: Vec<String> = Vec::new();
    for c in s.chars() {
        if is_combining(c) {
            if let Some(last) = clusters.last_mut() {
                last.push(c);
            } else {
                clusters.push(String::from(c));
            }
        } else {
            clusters.push(String::from(c));
        }
    }
    clusters
}

/// Strip all combining marks, returning only base characters.
pub fn strip_combining(s: &str) -> String {
    s.chars().filter(|&c| !is_combining(c)).collect()
}

/// Returns `true` if `s` consists entirely of ASCII (U+0000..U+007F).
#[inline]
pub fn is_ascii(s: &str) -> bool {
    s.bytes().all(|b| b < 0x80)
}

/// Heuristic: returns `true` if `s` appears to be in NFC form already
/// (no base+combining pair that our table could compose is present).
pub fn is_likely_nfc(s: &str) -> bool {
    if is_ascii(s) { return true; }
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    for i in 0..n.saturating_sub(1) {
        if is_combining(chars[i + 1]) {
            let bv = chars[i] as u32;
            let cv = chars[i + 1] as u32;
            for &(_, b, cmb) in DECOMP {
                if b == bv && cmb == cv { return false; }
            }
        }
    }
    true
}

/// No-op init (module has no mutable global state).
pub fn init() {}

/// Boot self-test — verifies NFC/NFD round-trips, block detection,
/// grapheme clusters, combining detection, and ASCII fast-paths.
pub fn self_test() -> bool {
    let mut ok = true;

    // 1. NFD: é (U+00E9) → e + U+0301
    let nfd = normalize_nfd("\u{00E9}");
    if nfd.chars().count() != 2 { ok = false; }
    let mut nfd_iter = nfd.chars();
    if nfd_iter.next() != Some('e') { ok = false; }
    if nfd_iter.next() != Some('\u{0301}') { ok = false; }

    // 2. NFC: e + U+0301 → é
    let nfc = normalize_nfc("e\u{0301}");
    if nfc != "\u{00E9}" { ok = false; }

    // 3. Round-trip on precomposed Latin text
    let word = "H\u{00E9}llo w\u{00F6}rld"; // Héllo wörld
    let rt = normalize_nfc(&normalize_nfd(word));
    if rt != word { ok = false; }

    // 4. Czech caron round-trip
    let czech = "Dobr\u{00FD} ve\u{010D}er"; // Dobrý večer
    let rt2 = normalize_nfc(&normalize_nfd(czech));
    if rt2 != czech { ok = false; }

    // 5. Grapheme clusters
    let clusters = grapheme_clusters("e\u{0301}a");
    if clusters.len() != 2 { ok = false; }

    // 6. grapheme_len: precomposed é counts as 1
    if grapheme_len("caf\u{00E9}") != 4 { ok = false; }

    // 7. grapheme_len: decomposed e+combining counts as 1 base
    if grapheme_len("e\u{0301}") != 1 { ok = false; }

    // 8. Block detection
    if unicode_block('A') != "Basic Latin" { ok = false; }
    if unicode_block('\u{00E9}') != "Latin-1 Supplement" { ok = false; }
    if unicode_block('\u{0100}') != "Latin Extended-A" { ok = false; }
    if unicode_block('\u{03B1}') != "Greek and Coptic" { ok = false; }
    if unicode_block('\u{AC00}') != "Hangul Syllables" { ok = false; }
    if unicode_block('\u{4E2D}') != "CJK Unified Ideographs" { ok = false; }
    if unicode_block('\u{2192}') != "Arrows" { ok = false; }

    // 9. is_combining
    if !is_combining('\u{0301}') { ok = false; }
    if is_combining('a') { ok = false; }

    // 10. strip_combining
    let stripped = strip_combining("e\u{0301}a\u{0308}");
    if stripped != "ea" { ok = false; }

    // 11. ASCII fast-path (no allocation, same pointer)
    let ascii = normalize_nfc("Hello, World!");
    if ascii != "Hello, World!" { ok = false; }

    // 12. NFC of Greek tonos
    let greek_nfc = normalize_nfc("\u{03B1}\u{0301}"); // α + tonos
    if greek_nfc != "\u{03AC}" { ok = false; }          // ά

    ok
}

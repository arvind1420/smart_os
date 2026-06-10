#![allow(dead_code)]
/// Smart OS — WOFF / WOFF2 Web Font Parser (Phase 99, v0.59.0)
///
/// Parses WOFF (Web Open Font Format) and WOFF2 containers and extracts
/// the embedded TrueType/OpenType sfnt data for use with the TTF renderer.
///
/// Public API:
///   `detect_format(data)` → `FontFormat`
///   `parse_woff(data)` → `Result<WoffFont, WoffError>`
///   `extract_sfnt(data)` → `Option<Vec<u8>>`   (raw TTF bytes, uncompressed)
///   `WoffFont::family_name()` → `&str`
///   `WoffFont::to_sfnt()` → `Option<&[u8]>`     (ready for TTF renderer)
///
/// Limitations (no_std kernel):
///   • WOFF table decompression (zlib) is stubbed — uncompressed tables only.
///   • WOFF2 brotli decompression is not implemented; format is detected but
///     tables are returned as-is (the font won't render unless the browser
///     falls back to a system font).
///   • CFF (PostScript outlines) fonts are not supported by the TTF renderer.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;

// ─── Font format detection ─────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum FontFormat {
    /// WOFF1 (RFC 8081): magic 0x774F4646 "wOFF"
    Woff1,
    /// WOFF2 (RFC 8081): magic 0x774F4632 "wOF2"
    Woff2,
    /// Bare TrueType sfnt: magic 0x00010000 or "true"
    TrueType,
    /// OpenType CFF: magic "OTTO"
    OpenTypeCff,
    /// OpenType collection: magic "ttcf"
    TrueTypeCollection,
    /// Unknown / not a font
    Unknown,
}

impl FontFormat {
    pub fn name(&self) -> &'static str {
        match self {
            FontFormat::Woff1             => "WOFF1",
            FontFormat::Woff2             => "WOFF2",
            FontFormat::TrueType          => "TrueType/OpenType",
            FontFormat::OpenTypeCff       => "OpenType CFF",
            FontFormat::TrueTypeCollection=> "TTC",
            FontFormat::Unknown           => "Unknown",
        }
    }
    pub fn is_woff(&self) -> bool {
        matches!(self, FontFormat::Woff1 | FontFormat::Woff2)
    }
}

pub fn detect_format(data: &[u8]) -> FontFormat {
    if data.len() < 4 { return FontFormat::Unknown; }
    match &data[0..4] {
        b"wOFF" => FontFormat::Woff1,
        b"wOF2" => FontFormat::Woff2,
        b"OTTO" => FontFormat::OpenTypeCff,
        b"ttcf" => FontFormat::TrueTypeCollection,
        [0x00, 0x01, 0x00, 0x00] => FontFormat::TrueType,
        b"true" | b"typ1" => FontFormat::TrueType,
        _ => FontFormat::Unknown,
    }
}

// ─── WOFF table descriptor ─────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct WoffTable {
    /// 4-byte tag (e.g., b"cmap", b"glyf", b"head")
    pub tag:      [u8; 4],
    /// Checksum of original uncompressed data
    pub checksum: u32,
    /// Offset in WOFF file to (possibly compressed) table data
    pub offset:   u32,
    /// Compressed size in the WOFF file
    pub comp_len: u32,
    /// Original (uncompressed) size
    pub orig_len: u32,
}

impl WoffTable {
    /// Returns the tag as a string (ASCII)
    pub fn tag_str(&self) -> String {
        self.tag.iter().map(|&b| b as char).collect()
    }
    /// Whether this table is stored compressed (compLength < origLength)
    pub fn is_compressed(&self) -> bool {
        self.comp_len < self.orig_len
    }
}

// ─── WOFF font ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct WoffFont {
    pub format:     FontFormat,
    pub flavor:     u32,           // sfnt version (0x00010000 = TrueType, 0x4F54544F = CFF)
    pub num_tables: u16,
    pub tables:     Vec<WoffTable>,
    /// Reconstructed sfnt bytes (None if any table was compressed)
    pub sfnt_data:  Option<Vec<u8>>,
    /// Family name extracted from the `name` table (if available)
    pub family:     String,
    /// Original raw WOFF bytes (needed for table slice access)
    raw:            Vec<u8>,
}

impl WoffFont {
    pub fn family_name(&self) -> &str { &self.family }
    pub fn flavor_name(&self) -> &'static str {
        match self.flavor {
            0x00010000 | 0x74727565 => "TrueType",
            0x4F54544F => "CFF/PostScript",
            _ => "unknown",
        }
    }
    /// Returns the reconstructed sfnt data (raw TTF/OTF), if all tables
    /// were uncompressed.
    pub fn to_sfnt(&self) -> Option<&[u8]> {
        self.sfnt_data.as_deref()
    }
}

// ─── Parse errors ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum WoffError {
    TooShort,
    BadMagic,
    BadTableCount,
    TableOutOfBounds,
    CompressedTable(String),   // table tag that required decompression
    NotAWoff,
}

impl WoffError {
    pub fn description(&self) -> String {
        match self {
            WoffError::TooShort           => "Data too short".to_string(),
            WoffError::BadMagic           => "Bad WOFF magic bytes".to_string(),
            WoffError::BadTableCount      => "Invalid table count".to_string(),
            WoffError::TableOutOfBounds   => "Table offset/length out of bounds".to_string(),
            WoffError::CompressedTable(t) => format!("Table '{}' is zlib-compressed (not supported in no_std)", t),
            WoffError::NotAWoff           => "Not a WOFF file".to_string(),
        }
    }
}

// ─── WOFF1 parser ─────────────────────────────────────────────────────────

fn read_u16_be(d: &[u8], off: usize) -> Option<u16> {
    if off + 2 > d.len() { return None; }
    Some(u16::from_be_bytes([d[off], d[off+1]]))
}
fn read_u32_be(d: &[u8], off: usize) -> Option<u32> {
    if off + 4 > d.len() { return None; }
    Some(u32::from_be_bytes([d[off], d[off+1], d[off+2], d[off+3]]))
}

/// Parse a WOFF1 file.
/// Returns a `WoffFont` whose `sfnt_data` is `Some(bytes)` only if ALL tables
/// are uncompressed (compLength == origLength for every table).
pub fn parse_woff(data: &[u8]) -> Result<WoffFont, WoffError> {
    if data.len() < 48 { return Err(WoffError::TooShort); }
    // Verify magic
    let fmt = detect_format(data);
    if fmt != FontFormat::Woff1 && fmt != FontFormat::Woff2 {
        return Err(WoffError::BadMagic);
    }

    // ─── WOFF1 header (44 bytes) ──────────────────────────────────────────
    // Offset  Size  Name
    //  0       4    signature  "wOFF"
    //  4       4    flavor     sfntVersion
    //  8       4    length     total size of WOFF file
    // 12       2    numTables
    // 14       2    reserved   (must be 0)
    // 16       4    totalSfntSize
    // 20       2    majorVersion
    // 22       2    minorVersion
    // 24       4    metaOffset
    // 28       4    metaLength
    // 32       4    metaOrigLength
    // 36       4    privOffset
    // 40       4    privLength
    //         44   (header ends)

    let flavor     = read_u32_be(data, 4).ok_or(WoffError::TooShort)?;
    let num_tables = read_u16_be(data, 12).ok_or(WoffError::TooShort)? as usize;
    if num_tables == 0 || num_tables > 128 { return Err(WoffError::BadTableCount); }

    // Table directory starts at offset 44, each entry is 20 bytes
    // Entry layout:
    //  0  4  tag
    //  4  4  offset
    //  8  4  compLength
    // 12  4  origLength
    // 16  4  origChecksum
    let dir_start = 44usize;
    let dir_end   = dir_start + num_tables * 20;
    if dir_end > data.len() { return Err(WoffError::TableOutOfBounds); }

    let mut tables: Vec<WoffTable> = Vec::new();
    for i in 0..num_tables {
        let base = dir_start + i * 20;
        let tag: [u8; 4] = data[base..base+4].try_into().unwrap_or([0u8; 4]);
        let offset    = read_u32_be(data, base +  4).ok_or(WoffError::TooShort)?;
        let comp_len  = read_u32_be(data, base +  8).ok_or(WoffError::TooShort)?;
        let orig_len  = read_u32_be(data, base + 12).ok_or(WoffError::TooShort)?;
        let checksum  = read_u32_be(data, base + 16).ok_or(WoffError::TooShort)?;

        // Validate bounds
        let end = offset as usize + comp_len as usize;
        if end > data.len() { return Err(WoffError::TableOutOfBounds); }

        tables.push(WoffTable { tag, checksum, offset, comp_len, orig_len });
    }

    // Reconstruct sfnt only if ALL tables are uncompressed
    let all_uncompressed = tables.iter().all(|t| !t.is_compressed());
    let sfnt_data = if all_uncompressed {
        Some(reconstruct_sfnt(data, flavor, &tables))
    } else {
        None
    };

    // Try to extract family name from the `name` table (if uncompressed)
    let family = extract_family_name(data, &tables).unwrap_or_else(|| "Unknown".to_string());

    Ok(WoffFont {
        format: fmt,
        flavor,
        num_tables: num_tables as u16,
        tables,
        sfnt_data,
        family,
        raw: data.to_vec(),
    })
}

// ─── Reconstruct sfnt from uncompressed WOFF tables ──────────────────────

/// Build a valid TrueType/OpenType binary from the WOFF table directory.
/// All tables must be uncompressed (compLen == origLen).
fn reconstruct_sfnt(woff: &[u8], flavor: u32, tables: &[WoffTable]) -> Vec<u8> {
    let n = tables.len() as u16;
    // sfnt search parameters
    let search_range   = largest_pow2_le(n) * 16;
    let entry_selector = log2_floor(largest_pow2_le(n));
    let range_shift    = n * 16 - search_range;

    // sfnt header (12 bytes) + table directory (n * 16 bytes)
    let header_size = 12 + n as usize * 16;

    // Sort tables by tag for correct sfnt layout
    let mut sorted: Vec<&WoffTable> = tables.iter().collect();
    sorted.sort_by_key(|t| t.tag);

    // Compute data offsets (4-byte aligned after header)
    let mut data_offset = header_size;
    let mut offsets: Vec<usize> = Vec::new();
    for t in &sorted {
        offsets.push(data_offset);
        data_offset += (t.orig_len as usize + 3) & !3; // round up to 4
    }
    let total_size = data_offset;
    let mut out = vec![0u8; total_size];

    // Write sfnt offset table
    let fv = flavor.to_be_bytes();
    out[0..4].copy_from_slice(&fv);
    out[4..6].copy_from_slice(&n.to_be_bytes());
    out[6..8].copy_from_slice(&search_range.to_be_bytes());
    out[8..10].copy_from_slice(&entry_selector.to_be_bytes());
    out[10..12].copy_from_slice(&range_shift.to_be_bytes());

    // Write table directory + table data
    for (idx, t) in sorted.iter().enumerate() {
        let dir_off = 12 + idx * 16;
        let data_off = offsets[idx] as u32;
        out[dir_off..dir_off+4].copy_from_slice(&t.tag);
        out[dir_off+4..dir_off+8].copy_from_slice(&t.checksum.to_be_bytes());
        out[dir_off+8..dir_off+12].copy_from_slice(&data_off.to_be_bytes());
        out[dir_off+12..dir_off+16].copy_from_slice(&t.orig_len.to_be_bytes());

        // Copy table data (uncompressed, same as comp_len == orig_len)
        let src_start = t.offset as usize;
        let src_end   = src_start + t.comp_len as usize;
        let dst_start = offsets[idx];
        let dst_end   = dst_start + t.orig_len as usize;
        if src_end <= woff.len() && dst_end <= out.len() {
            out[dst_start..dst_end].copy_from_slice(&woff[src_start..src_end]);
        }
    }
    out
}

fn largest_pow2_le(n: u16) -> u16 {
    if n == 0 { return 0; }
    let mut p = 1u16;
    while p * 2 <= n { p *= 2; }
    p
}
fn log2_floor(n: u16) -> u16 {
    if n <= 1 { return 0; }
    let mut r = 0u16;
    let mut v = n;
    while v > 1 { v >>= 1; r += 1; }
    r
}

// ─── Extract family name from `name` table ─────────────────────────────────

/// Try to read the `name` table's platform-0/3 family name (nameID=1).
fn extract_family_name(woff: &[u8], tables: &[WoffTable]) -> Option<String> {
    let name_table = tables.iter().find(|t| &t.tag == b"name")?;
    if name_table.is_compressed() { return None; }

    let start = name_table.offset as usize;
    let end   = start + name_table.orig_len as usize;
    if end > woff.len() { return None; }
    let n = &woff[start..end];

    // name table layout:
    //  0  2  format (0 or 1)
    //  2  2  count (number of name records)
    //  4  2  stringOffset (from start of table to string storage)
    if n.len() < 6 { return None; }
    let count         = read_u16_be(n, 2)? as usize;
    let string_offset = read_u16_be(n, 4)? as usize;

    // Each name record is 12 bytes:
    //  0  2  platformID
    //  2  2  encodingID
    //  4  2  languageID
    //  6  2  nameID
    //  8  2  length
    // 10  2  offset (from stringOffset)
    let record_start = 6usize;
    for i in 0..count {
        let r = record_start + i * 12;
        if r + 12 > n.len() { break; }
        let platform_id = read_u16_be(n, r)?;
        let encoding_id = read_u16_be(n, r + 2)?;
        let name_id     = read_u16_be(n, r + 6)?;
        let length      = read_u16_be(n, r + 8)? as usize;
        let str_off     = read_u16_be(n, r + 10)? as usize;

        // nameID 1 = Family name, 4 = Full name, 6 = PostScript name
        // platformID 3 (Windows), encodingID 1 (UTF-16BE) — most common
        // platformID 1 (Mac), encodingID 0 (ASCII) — fallback
        if name_id != 1 { continue; }

        let abs_off = string_offset + str_off;
        let abs_end = abs_off + length;
        if abs_end > n.len() { continue; }
        let raw = &n[abs_off..abs_end];

        if platform_id == 3 && encoding_id == 1 {
            // UTF-16BE: every two bytes = one char (BMP only)
            let s: String = raw.chunks(2)
                .filter_map(|ch| {
                    if ch.len() == 2 {
                        let code = u16::from_be_bytes([ch[0], ch[1]]) as u32;
                        char::from_u32(code)
                    } else { None }
                })
                .collect();
            if !s.is_empty() { return Some(s); }
        } else if platform_id == 1 {
            // Mac ASCII (Latin-1 subset)
            let s: String = raw.iter().map(|&b| b as char).collect();
            if !s.is_empty() { return Some(s); }
        }
    }
    None
}

// ─── Convenience wrapper ──────────────────────────────────────────────────

/// Detect and parse any supported font format.
/// For bare TTF/OTF, wraps the data directly (no conversion needed).
/// For WOFF1/WOFF2, calls `parse_woff`.
pub fn extract_sfnt(data: &[u8]) -> Option<Vec<u8>> {
    match detect_format(data) {
        FontFormat::TrueType | FontFormat::OpenTypeCff => {
            // Already raw sfnt; return a copy
            Some(data.to_vec())
        }
        FontFormat::Woff1 => {
            parse_woff(data).ok()?.sfnt_data
        }
        FontFormat::Woff2 => {
            // WOFF2 requires brotli decompression — stub: return None
            None
        }
        FontFormat::TrueTypeCollection => None, // TTC: multi-font, not yet supported
        FontFormat::Unknown => None,
    }
}

// ─── @font-face registry ──────────────────────────────────────────────────

/// A registered web font (from `@font-face` CSS).
#[derive(Clone, Debug)]
pub struct RegisteredFont {
    /// `font-family` value from the @font-face rule
    pub family: String,
    /// Raw sfnt bytes (TrueType/OpenType, ready for the TTF renderer)
    pub sfnt:   Vec<u8>,
    /// Optional weight (100–900, 400 = normal)
    pub weight: u16,
    /// Optional style: "normal" | "italic" | "oblique"
    pub style:  String,
}

/// Parse a `@font-face` CSS block and return a `RegisteredFont`.
/// `css_block` is the content inside `{ ... }` of the `@font-face` rule.
/// `resolve_data(url)` fetches the font file bytes for a given URL.
pub fn parse_font_face(css_block: &str) -> Option<RegisteredFontDecl> {
    let mut family = String::new();
    let mut src_urls: Vec<String> = Vec::new();
    let mut weight: u16 = 400;
    let mut style = "normal".to_string();

    for line in css_block.split(';') {
        let line = line.trim();
        if line.is_empty() { continue; }
        if let Some(val) = css_value_of(line, "font-family") {
            family = val.trim_matches(|c| c == '"' || c == '\'').to_string();
        } else if let Some(val) = css_value_of(line, "src") {
            // Parse one or more url() values
            let mut rest = val.as_str();
            while let Some(start) = rest.find("url(") {
                rest = &rest[start + 4..];
                let end = rest.find(')').unwrap_or(rest.len());
                let url = rest[..end].trim_matches(|c| c == '"' || c == '\'').to_string();
                if !url.is_empty() { src_urls.push(url); }
                rest = &rest[end..];
            }
        } else if let Some(val) = css_value_of(line, "font-weight") {
            weight = val.trim().parse().unwrap_or(400);
        } else if let Some(val) = css_value_of(line, "font-style") {
            style = val.trim().to_string();
        }
    }

    if family.is_empty() || src_urls.is_empty() { return None; }
    Some(RegisteredFontDecl { family, src_urls, weight, style })
}

/// Declaration parsed from @font-face (before font data is fetched)
#[derive(Clone, Debug)]
pub struct RegisteredFontDecl {
    pub family:   String,
    pub src_urls: Vec<String>,
    pub weight:   u16,
    pub style:    String,
}

fn css_value_of<'a>(declaration: &'a str, property: &str) -> Option<String> {
    let lower = declaration.to_ascii_lowercase();
    let prefix = format!("{}:", property);
    if lower.trim_start().starts_with(&prefix) {
        let colon = declaration.find(':')? + 1;
        Some(declaration[colon..].trim().to_string())
    } else {
        None
    }
}

// ─── Self-test ─────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // T1: detect TrueType magic
    let ttf_magic = &[0x00u8, 0x01, 0x00, 0x00, 0x00, 0x10];
    if detect_format(ttf_magic) != FontFormat::TrueType { ok = false; }

    // T2: detect WOFF1 magic
    let woff_magic = b"wOFF\x00\x01\x00\x00\x00\x00\x00\x00\x00\x03\x00\x00";
    if detect_format(woff_magic) != FontFormat::Woff1 { ok = false; }

    // T3: detect WOFF2 magic
    let woff2_magic = b"wOF2\x00\x01\x00\x00";
    if detect_format(woff2_magic) != FontFormat::Woff2 { ok = false; }

    // T4: detect OpenType CFF
    if detect_format(b"OTTO\x00") != FontFormat::OpenTypeCff { ok = false; }

    // T5: parse_woff on too-short data returns error
    if !matches!(parse_woff(b"wOFF"), Err(WoffError::TooShort)) { ok = false; }

    // T6: parse_woff on bad magic returns error
    let bad = vec![0u8; 50];
    if !matches!(parse_woff(&bad), Err(WoffError::BadMagic)) { ok = false; }

    // T7: extract_sfnt on raw TTF returns Some
    let fake_ttf = vec![0x00u8, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    if extract_sfnt(&fake_ttf).is_none() { ok = false; }

    // T8: extract_sfnt on WOFF2 returns None (brotli not implemented)
    let fake_woff2 = b"wOF2\x00\x01\x00\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
    if extract_sfnt(fake_woff2).is_some() { ok = false; }

    // T9: parse_font_face extracts family + src_url
    let decl = parse_font_face(
        r#"font-family: "MyFont"; src: url("fonts/myfont.woff2") format("woff2"), url("fonts/myfont.woff"); font-weight: 700;"#
    );
    if let Some(d) = decl {
        if d.family != "MyFont" { ok = false; }
        if d.src_urls.is_empty() { ok = false; }
        if d.weight != 700 { ok = false; }
    } else { ok = false; }

    // T10: log2_floor / largest_pow2_le
    if largest_pow2_le(7)  != 4  { ok = false; }
    if largest_pow2_le(8)  != 8  { ok = false; }
    if largest_pow2_le(16) != 16 { ok = false; }
    if log2_floor(8)  != 3 { ok = false; }
    if log2_floor(16) != 4 { ok = false; }

    // T11: format names are non-empty
    for fmt in [FontFormat::Woff1, FontFormat::Woff2, FontFormat::TrueType,
                FontFormat::OpenTypeCff, FontFormat::TrueTypeCollection] {
        if fmt.name().is_empty() { ok = false; }
    }

    ok
}

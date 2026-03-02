/// Content-aware file classification for SmartFS.
///
/// Two-tier approach:
/// 1. Fast heuristic (magic bytes, extensions, byte analysis)
/// 2. AI-assisted (neural network) when heuristic is uncertain

use super::metadata::FileCategory;

/// Classify file content using combined heuristic + AI approach.
/// Returns (category, confidence) where confidence is 0.0-1.0.
pub fn classify(data: &[u8], filename: &str) -> (FileCategory, f32) {
    // Tier 1: Heuristic classification
    let (heuristic_cat, heuristic_conf) = classify_heuristic(data, filename);

    // If heuristic is confident enough, use it directly
    if heuristic_conf >= 0.8 {
        return (heuristic_cat, heuristic_conf);
    }

    // Tier 2: AI-assisted classification (when heuristic is uncertain)
    if let Some((ai_cat, ai_conf)) = classify_with_ai(data) {
        // Use AI result if it's more confident
        if ai_conf > heuristic_conf {
            return (ai_cat, ai_conf);
        }
    }

    // Fall back to heuristic result
    (heuristic_cat, heuristic_conf)
}

/// Tier 1: Heuristic classification using magic bytes, extensions, byte analysis.
fn classify_heuristic(data: &[u8], filename: &str) -> (FileCategory, f32) {
    // Check magic bytes first (highest confidence)
    if let Some(cat) = check_magic_bytes(data) {
        return (cat, 0.95);
    }

    // Check file extension
    if let Some(cat) = check_extension(filename) {
        return (cat, 0.85);
    }

    // Byte analysis
    if data.is_empty() {
        return (FileCategory::Unknown, 0.0);
    }

    let sample = &data[..data.len().min(512)];
    let n = sample.len() as f32;

    // Count byte classes
    let mut printable = 0u32;
    let mut whitespace = 0u32;
    let mut high_bytes = 0u32;
    let mut null_bytes = 0u32;

    for &b in sample {
        match b {
            0 => null_bytes += 1,
            9..=13 | 32 => { whitespace += 1; printable += 1; }
            33..=126 => printable += 1,
            128..=255 => high_bytes += 1,
            _ => {}
        }
    }

    let printable_ratio = printable as f32 / n;
    let high_ratio = high_bytes as f32 / n;
    let null_ratio = null_bytes as f32 / n;

    if printable_ratio > 0.90 {
        // Mostly printable ASCII — likely text or source
        if whitespace as f32 / n > 0.05 {
            (FileCategory::Text, 0.7)
        } else {
            (FileCategory::Data, 0.5)
        }
    } else if high_ratio > 0.3 || null_ratio > 0.1 {
        (FileCategory::Binary, 0.6)
    } else {
        (FileCategory::Unknown, 0.3)
    }
}

/// Check magic bytes for known file formats.
fn check_magic_bytes(data: &[u8]) -> Option<FileCategory> {
    if data.len() < 4 { return None; }

    let magic4 = &data[..4];
    let magic2 = &data[..2];

    // Image formats
    if magic4 == &[0x89, b'P', b'N', b'G'] { return Some(FileCategory::Image); }
    if magic2 == &[0xFF, 0xD8] { return Some(FileCategory::Image); }
    if magic4 == &[b'G', b'I', b'F', b'8'] { return Some(FileCategory::Image); }
    if magic4 == &[b'B', b'M', 0, 0] || (magic2 == &[b'B', b'M'] && data.len() > 14) {
        return Some(FileCategory::Image);
    }

    // Archive formats
    if magic4 == &[b'P', b'K', 0x03, 0x04] { return Some(FileCategory::Archive); }
    if magic4 == &[0x1F, 0x8B, 0x08, 0x00] { return Some(FileCategory::Archive); } // gzip
    if &data[..6.min(data.len())] == b"7z\xBC\xAF\x27\x1C" { return Some(FileCategory::Archive); }

    // Binary executables
    if magic4 == &[0x7F, b'E', b'L', b'F'] { return Some(FileCategory::Binary); }
    if magic2 == &[b'M', b'Z'] { return Some(FileCategory::Binary); } // PE/DOS

    // Document formats
    if magic4 == &[b'%', b'P', b'D', b'F'] { return Some(FileCategory::Data); }

    // SmartPack
    if magic2 == &[0x53, 0x50] { return Some(FileCategory::Data); }

    // Shebang (script/source)
    if magic2 == &[b'#', b'!'] { return Some(FileCategory::SourceCode); }

    None
}

/// Check file extension for known types.
fn check_extension(filename: &str) -> Option<FileCategory> {
    let ext = filename.rsplit('.').next()?;

    match ext {
        // Source code
        "rs" | "c" | "cpp" | "h" | "py" | "js" | "ts" | "java" | "go" | "asm" | "sh" | "rb" =>
            Some(FileCategory::SourceCode),

        // Config
        "toml" | "yaml" | "yml" | "json" | "xml" | "ini" | "cfg" | "conf" | "sp" =>
            Some(FileCategory::Config),

        // Text
        "txt" | "md" | "log" | "csv" | "readme" =>
            Some(FileCategory::Text),

        // Image
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "ico" | "svg" | "webp" =>
            Some(FileCategory::Image),

        // Archive
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" =>
            Some(FileCategory::Archive),

        // Binary
        "exe" | "elf" | "bin" | "so" | "dll" | "o" | "obj" =>
            Some(FileCategory::Binary),

        _ => None,
    }
}

/// Tier 2: AI-assisted classification using the neural network.
fn classify_with_ai(data: &[u8]) -> Option<(FileCategory, f32)> {
    let result = crate::ai::inference::classify_file_content(data).ok()?;
    let category = FileCategory::from_str(&result.label);
    Some((category, result.confidence))
}

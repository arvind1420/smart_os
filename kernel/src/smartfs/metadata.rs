/// Rich file metadata for SmartFS.
///
/// Every file stored through SmartFS gets metadata describing its content type,
/// compression method, chunk hashes, and optional AI classification confidence.

use alloc::string::String;
use alloc::vec::Vec;

/// File category (determined by heuristics + optional AI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCategory {
    Text,
    SourceCode,
    Config,
    Binary,
    Image,
    Archive,
    Data,
    Unknown,
}

impl FileCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            FileCategory::Text => "text",
            FileCategory::SourceCode => "source",
            FileCategory::Config => "config",
            FileCategory::Binary => "binary",
            FileCategory::Image => "image",
            FileCategory::Archive => "archive",
            FileCategory::Data => "data",
            FileCategory::Unknown => "unknown",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "text" => FileCategory::Text,
            "source" => FileCategory::SourceCode,
            "config" => FileCategory::Config,
            "binary" => FileCategory::Binary,
            "image" => FileCategory::Image,
            "archive" => FileCategory::Archive,
            "data" => FileCategory::Data,
            _ => FileCategory::Unknown,
        }
    }
}

/// Compression method used for a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionMethod {
    None,
    Rle,
    Lz4Simple,
}

impl CompressionMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompressionMethod::None => "none",
            CompressionMethod::Rle => "rle",
            CompressionMethod::Lz4Simple => "lz4",
        }
    }
}

/// Rich metadata for a SmartFS file.
#[derive(Debug, Clone)]
pub struct SmartMetadata {
    pub category: FileCategory,
    pub compression: CompressionMethod,
    pub original_size: usize,
    pub compressed_size: usize,
    pub chunk_count: usize,
    pub chunk_hashes: Vec<u32>,
    pub tags: Vec<String>,
    pub ai_confidence: Option<f32>,
}

impl SmartMetadata {
    /// Compression ratio (1.0 = no compression, <1.0 = compressed).
    pub fn compression_ratio(&self) -> f32 {
        if self.original_size == 0 { return 1.0; }
        self.compressed_size as f32 / self.original_size as f32
    }

    /// Serialize metadata to SmartPack Value.
    pub fn to_smartpack(&self) -> smartpack::Value {
        use smartpack::Value;

        let mut map = Vec::new();
        map.push((
            Value::String(String::from("category")),
            Value::String(String::from(self.category.as_str())),
        ));
        map.push((
            Value::String(String::from("compression")),
            Value::String(String::from(self.compression.as_str())),
        ));
        map.push((
            Value::String(String::from("original_size")),
            Value::UInt32(self.original_size as u32),
        ));
        map.push((
            Value::String(String::from("compressed_size")),
            Value::UInt32(self.compressed_size as u32),
        ));
        map.push((
            Value::String(String::from("chunks")),
            Value::UInt32(self.chunk_count as u32),
        ));
        if let Some(conf) = self.ai_confidence {
            map.push((
                Value::String(String::from("ai_confidence")),
                Value::Float32(conf),
            ));
        }

        Value::Map(map)
    }
}

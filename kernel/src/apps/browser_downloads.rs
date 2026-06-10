#![allow(dead_code)]
/// Smart OS — Browser Downloads Manager (Phase 90, v0.50.0)
///
/// Intercepts HTTP responses with `Content-Disposition: attachment` or
/// non-viewable MIME types and streams them to `/home/user/downloads/`:
///   • `DownloadEntry`     — metadata + progress tracking
///   • `DownloadQueue`     — manage up to `MAX_CONCURRENT` parallel downloads
///   • `MimeClassifier`    — decide if a response should be downloaded vs displayed
///   • `content_disposition_filename` — parse filename from header
///   • `sha256_hex`        — checksum of completed file (for verification UI)

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use alloc::vec;

// ─── MIME classifier ──────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MimeDisposition {
    Display,    // browser can render this
    Download,   // must be saved to disk
}

pub fn classify_mime(mime: &str) -> MimeDisposition {
    let m = mime.split(';').next().unwrap_or(mime).trim().to_ascii_lowercase();
    match m.as_str() {
        // Displayable
        "text/html"
        | "text/plain"
        | "text/css"
        | "application/javascript"
        | "application/json"
        | "image/png"
        | "image/jpeg"
        | "image/gif"
        | "image/webp"
        | "image/svg+xml"
        | "application/pdf"
        | "video/mp4"
        | "audio/mpeg"
        | "audio/ogg" => MimeDisposition::Display,
        // Everything else: download
        _ => MimeDisposition::Download,
    }
}

// ─── Content-Disposition parser ───────────────────────────────────────────────
/// Extract filename from `Content-Disposition: attachment; filename="foo.zip"`.
/// Returns `None` if not an attachment or no filename found.
pub fn content_disposition_filename(header: &str) -> Option<String> {
    let lower = header.to_ascii_lowercase();
    if !lower.contains("attachment") { return None; }

    // Look for filename*= (RFC 5987) first, then filename=
    for token in ["filename*=utf-8''", "filename*=", "filename="] {
        if let Some(idx) = lower.find(token) {
            let rest = &header[idx + token.len()..];
            let name = rest.trim_matches(|c: char| c == '"' || c == '\'' || c.is_ascii_whitespace())
                .split(|c: char| c == ';' || c == '\r' || c == '\n')
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| c == '"' || c == '\'' || c.is_ascii_whitespace());
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Derive a safe download filename from a URL path.
pub fn filename_from_url(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url)
                  .split('#').next().unwrap_or(url);
    let name = path.split('/').last().unwrap_or("download");
    if name.is_empty() { "download".to_string() } else { name.to_string() }
}

// ─── Download state ───────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DownloadState {
    Queued,
    Downloading,
    Paused,
    Done,
    Failed,
    Cancelled,
}

impl DownloadState {
    pub fn is_terminal(self) -> bool {
        matches!(self, DownloadState::Done | DownloadState::Failed | DownloadState::Cancelled)
    }
    pub fn name(self) -> &'static str {
        match self {
            DownloadState::Queued      => "Queued",
            DownloadState::Downloading => "Downloading",
            DownloadState::Paused      => "Paused",
            DownloadState::Done        => "Done",
            DownloadState::Failed      => "Failed",
            DownloadState::Cancelled   => "Cancelled",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DownloadEntry {
    pub id:            u32,
    pub url:           String,
    pub filename:      String,
    pub dest_path:     String,
    pub mime:          String,
    pub bytes_received: u64,
    pub total_bytes:   Option<u64>,  // None if no Content-Length
    pub state:         DownloadState,
    pub error:         Option<String>,
    pub checksum_sha256: Option<String>,  // set when Done
    pub started_at:    u64,              // uptime seconds
}

impl DownloadEntry {
    pub fn progress_pct(&self) -> Option<u8> {
        self.total_bytes.filter(|&t| t > 0)
            .map(|t| ((self.bytes_received * 100 / t).min(100)) as u8)
    }

    pub fn summary(&self) -> String {
        let progress = self.progress_pct()
            .map(|p| format!(" {}%", p))
            .unwrap_or_default();
        format!("[{:12}] {:30} {:>8} KB{}",
            self.state.name(),
            self.filename,
            self.bytes_received / 1024,
            progress)
    }
}

// ─── Download queue ───────────────────────────────────────────────────────────
const MAX_CONCURRENT: usize = 4;

pub struct DownloadQueue {
    pub entries:  Vec<DownloadEntry>,
    pub next_id:  u32,
}

impl DownloadQueue {
    pub fn new() -> Self { DownloadQueue { entries: Vec::new(), next_id: 1 } }

    /// Enqueue a new download.  Returns the download ID.
    pub fn enqueue(&mut self, url: &str, filename: &str, mime: &str) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let dest_path = format!("/home/user/downloads/{}", sanitize_filename(filename));
        let ts = crate::drivers::timer::uptime_secs();
        self.entries.push(DownloadEntry {
            id,
            url:             url.to_string(),
            filename:        filename.to_string(),
            dest_path,
            mime:            mime.to_string(),
            bytes_received:  0,
            total_bytes:     None,
            state:           DownloadState::Queued,
            error:           None,
            checksum_sha256: None,
            started_at:      ts,
        });
        id
    }

    pub fn active_count(&self) -> usize {
        self.entries.iter().filter(|e| e.state == DownloadState::Downloading).count()
    }

    pub fn can_start_new(&self) -> bool {
        self.active_count() < MAX_CONCURRENT
    }

    pub fn get(&self, id: u32) -> Option<&DownloadEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut DownloadEntry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    pub fn cancel(&mut self, id: u32) -> bool {
        if let Some(e) = self.get_mut(id) {
            if !e.state.is_terminal() {
                e.state = DownloadState::Cancelled;
                return true;
            }
        }
        false
    }

    pub fn remove_done(&mut self) {
        self.entries.retain(|e| !e.state.is_terminal());
    }

    /// Simulate receiving a chunk of data.
    pub fn update_progress(&mut self, id: u32, new_bytes: u64, total: Option<u64>) {
        if let Some(e) = self.get_mut(id) {
            e.bytes_received += new_bytes;
            if let Some(t) = total { e.total_bytes = Some(t); }
            e.state = DownloadState::Downloading;
        }
    }

    /// Mark a download as complete, compute checksum, and write to VFS.
    pub fn complete(&mut self, id: u32, body: &[u8]) -> bool {
        if let Some(e) = self.get_mut(id) {
            e.bytes_received = body.len() as u64;
            let hash = sha256_hex(body);
            e.checksum_sha256 = Some(hash);
            e.state = DownloadState::Done;
            let path = e.dest_path.clone();
            return crate::vfs::create_and_write(&path, body).is_ok();
        }
        false
    }

    pub fn fail(&mut self, id: u32, reason: &str) {
        if let Some(e) = self.get_mut(id) {
            e.state = DownloadState::Failed;
            e.error = Some(reason.to_string());
        }
    }

    pub fn pending_count(&self) -> usize {
        self.entries.iter().filter(|e| e.state == DownloadState::Queued).count()
    }

    pub fn done_count(&self) -> usize {
        self.entries.iter().filter(|e| e.state == DownloadState::Done).count()
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────
/// Replace path-unsafe characters in a filename.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '(' | ')') { c } else { '_' })
        .collect()
}

/// Hex-encoded SHA-256 of a byte slice.
pub fn sha256_hex(data: &[u8]) -> String {
    let hash = crate::crypto::sha256(data);
    hash.iter().map(|b| format!("{:02x}", b)).collect()
}

// ─── Self-test ────────────────────────────────────────────────────────────────
pub fn self_test() -> bool {
    let mut ok = true;

    // T1: MIME classification
    if classify_mime("text/html") != MimeDisposition::Display  { ok = false; }
    if classify_mime("image/png") != MimeDisposition::Display  { ok = false; }
    if classify_mime("application/zip") != MimeDisposition::Download { ok = false; }
    if classify_mime("application/octet-stream") != MimeDisposition::Download { ok = false; }

    // T2: content_disposition_filename
    let h = "attachment; filename=\"report.pdf\"";
    if content_disposition_filename(h) != Some("report.pdf".to_string()) { ok = false; }
    // No attachment → None
    if content_disposition_filename("inline; filename=\"report.pdf\"").is_some() { ok = false; }

    // T3: filename_from_url
    let f = filename_from_url("https://example.com/files/archive.tar.gz?v=2");
    if f != "archive.tar.gz" { ok = false; }

    // T4: sanitize_filename
    let s = sanitize_filename("My File (v2).zip");
    if s.contains(' ') { ok = false; }

    // T5: DownloadQueue enqueue and progress
    let mut q = DownloadQueue::new();
    let id = q.enqueue("https://example.com/file.zip", "file.zip", "application/zip");
    q.update_progress(id, 1024, Some(4096));
    let entry = q.get(id).unwrap();
    if entry.progress_pct() != Some(25) { ok = false; }

    // T6: DownloadQueue cancel
    let id2 = q.enqueue("https://b.com/b.zip", "b.zip", "application/zip");
    if !q.cancel(id2) { ok = false; }
    if q.get(id2).unwrap().state != DownloadState::Cancelled { ok = false; }

    // T7: DownloadQueue complete
    let id3 = q.enqueue("https://c.com/c.txt", "c.txt", "text/plain");
    let body = b"hello world download test";
    q.complete(id3, body);
    let e3 = q.get(id3).unwrap();
    if e3.state != DownloadState::Done { ok = false; }
    if e3.checksum_sha256.is_none() { ok = false; }

    // T8: sha256_hex length == 64
    let hex = sha256_hex(b"test");
    if hex.len() != 64 { ok = false; }

    // T9: MAX_CONCURRENT limit tracking
    for i in 0..MAX_CONCURRENT {
        let _id = q.enqueue(&format!("https://x.com/{}", i), &format!("{}.bin", i), "application/octet-stream");
    }
    // Manually mark them all as Downloading to test the limit
    let mut count = 0usize;
    for e in q.entries.iter_mut() {
        if e.state == DownloadState::Queued && count < MAX_CONCURRENT {
            e.state = DownloadState::Downloading;
            count += 1;
        }
    }
    if q.active_count() > MAX_CONCURRENT { ok = false; }

    ok
}

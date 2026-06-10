//! HTTP/1.1 Client — Phase 29 for Smart OS.
//!
//! Provides:
//!  • URL parsing (scheme, host, port, path, query)
//!  • HTTP/1.1 request builder  (GET, POST, PUT, DELETE, HEAD)
//!  • HTTP/1.1 response parser  (status line, headers, body)
//!  • Chunked transfer-encoding decoder
//!  • Connection: keep-alive
//!  • Basic authentication  (base64 user:pass)
//!  • TLS 1.3 first, TLS 1.2 fallback (via crypto modules)
//!  • Raw TCP fallback for http:// URLs
//!
//! All I/O is synchronous.  The caller provides a `ClientIo` implementation
//! backed by the kernel TCP stack.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::borrow::ToOwned;

// ─────────────────────────────────────────────────────────────────────────────
//  URL
// ─────────────────────────────────────────────────────────────────────────────

/// Parsed HTTP/HTTPS URL.
#[derive(Debug, Clone)]
pub struct Url {
    pub scheme: String,   // "http" | "https"
    pub host:   String,
    pub port:   u16,
    pub path:   String,   // always starts with "/"
    pub query:  String,   // without "?"
}

impl Url {
    /// Parse a URL string.  Returns `None` on malformed input.
    pub fn parse(raw: &str) -> Option<Self> {
        // scheme
        let (scheme, rest) = if let Some(pos) = raw.find("://") {
            (&raw[..pos], &raw[pos+3..])
        } else {
            return None;
        };
        let scheme_lc = scheme.to_ascii_lowercase();
        if scheme_lc != "http" && scheme_lc != "https" { return None; }

        // host[:port] and path
        let (authority, path_query) = if let Some(p) = rest.find('/') {
            (&rest[..p], &rest[p..])
        } else {
            (rest, "/")
        };

        let (host, port) = if let Some(cp) = authority.rfind(':') {
            let port_str = &authority[cp+1..];
            if let Ok(p) = port_str.parse::<u16>() {
                (authority[..cp].to_string(), p)
            } else {
                (authority.to_string(), if scheme_lc == "https" { 443 } else { 80 })
            }
        } else {
            (authority.to_string(), if scheme_lc == "https" { 443 } else { 80 })
        };

        // path + query
        let (path, query) = if let Some(qp) = path_query.find('?') {
            (path_query[..qp].to_string(), path_query[qp+1..].to_string())
        } else {
            (path_query.to_string(), String::new())
        };

        Some(Url { scheme: scheme_lc, host, port, path, query })
    }

    /// Return the request target (path + optional query string).
    pub fn request_target(&self) -> String {
        if self.query.is_empty() {
            self.path.clone()
        } else {
            format!("{}?{}", self.path, self.query)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  HTTP method
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Options,
    Patch,
}

impl Method {
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get     => "GET",
            Method::Post    => "POST",
            Method::Put     => "PUT",
            Method::Delete  => "DELETE",
            Method::Head    => "HEAD",
            Method::Options => "OPTIONS",
            Method::Patch   => "PATCH",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  HTTP Request
// ─────────────────────────────────────────────────────────────────────────────

/// An HTTP/1.1 request ready to be serialized onto the wire.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method:  Method,
    pub target:  String,
    pub headers: BTreeMap<String, String>,
    pub body:    Vec<u8>,
}

impl HttpRequest {
    pub fn new(method: Method, target: impl Into<String>) -> Self {
        HttpRequest {
            method,
            target: target.into(),
            headers: BTreeMap::new(),
            body:    Vec::new(),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn body(mut self, data: Vec<u8>) -> Self {
        self.body = data;
        self
    }

    /// Serialize to wire bytes.
    pub fn serialize(&self, host: &str) -> Vec<u8> {
        let mut out = Vec::new();
        // Request line.
        let line = format!("{} {} HTTP/1.1\r\n", self.method.as_str(), self.target);
        out.extend_from_slice(line.as_bytes());
        // Host header (always required in HTTP/1.1).
        if !self.headers.contains_key("Host") {
            let h = format!("Host: {}\r\n", host);
            out.extend_from_slice(h.as_bytes());
        }
        // Content-Length if body present.
        if !self.body.is_empty() && !self.headers.contains_key("Content-Length") {
            let cl = format!("Content-Length: {}\r\n", self.body.len());
            out.extend_from_slice(cl.as_bytes());
        }
        // Connection: keep-alive.
        if !self.headers.contains_key("Connection") {
            out.extend_from_slice(b"Connection: keep-alive\r\n");
        }
        // User-Agent.
        if !self.headers.contains_key("User-Agent") {
            out.extend_from_slice(b"User-Agent: SmartOS/0.17 SmartBrowser\r\n");
        }
        // Accept + Accept-Encoding — let the server know we can read gzip.
        if !self.headers.contains_key("Accept") {
            out.extend_from_slice(b"Accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/png,image/jpeg,*/*;q=0.5\r\n");
        }
        if !self.headers.contains_key("Accept-Encoding") {
            // NOTE: gzip/deflate work only if the server honours `Accept-Encoding`
            // *and* our inflate path stays correct.  Until both ends are battle-
            // tested we declare `identity` to keep the body in plain text.
            out.extend_from_slice(b"Accept-Encoding: identity\r\n");
        }
        if !self.headers.contains_key("Accept-Language") {
            out.extend_from_slice(b"Accept-Language: en-US,en;q=0.9\r\n");
        }
        // Caller-supplied headers.
        for (name, value) in &self.headers {
            let h = format!("{}: {}\r\n", name, value);
            out.extend_from_slice(h.as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  HTTP Response
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct HttpResponse {
    pub status:  u16,
    pub reason:  String,
    pub headers: BTreeMap<String, String>,
    pub body:    Vec<u8>,
}

impl HttpResponse {
    /// Returns the value of a header (case-insensitive lookup).
    pub fn header(&self, name: &str) -> Option<&str> {
        let lc = name.to_ascii_lowercase();
        self.headers.get(&lc).map(|s| s.as_str())
    }

    /// Returns `true` when the response carried a successful 2xx status.
    pub fn is_success(&self) -> bool { (200..300).contains(&self.status) }
}

// ─────────────────────────────────────────────────────────────────────────────
//  HTTP client error
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum HttpError {
    InvalidUrl,
    ConnectionFailed,
    SendFailed,
    RecvFailed,
    ParseError(&'static str),
    TooManyRedirects,
    TlsError,
}

// ─────────────────────────────────────────────────────────────────────────────
//  I/O trait (implemented by TCP connection wrapper)
// ─────────────────────────────────────────────────────────────────────────────

pub trait ClientIo {
    /// Write all bytes.
    fn write_all(&mut self, data: &[u8]) -> bool;
    /// Read up to `buf.len()` bytes.  Returns number of bytes read (0 = EOF).
    fn read(&mut self, buf: &mut [u8]) -> usize;
    /// Read until CRLF or buffer full.  Returns the line including CRLF.
    fn read_line(&mut self) -> Option<String>;
    /// Read exactly `n` bytes.
    fn read_exact(&mut self, n: usize) -> Option<Vec<u8>>;
}

// ─────────────────────────────────────────────────────────────────────────────
//  Response parser
// ─────────────────────────────────────────────────────────────────────────────

/// Parse an HTTP/1.1 response from the I/O stream.
pub fn parse_response<IO: ClientIo>(io: &mut IO) -> Result<HttpResponse, HttpError> {
    // Status line.
    let status_line = io.read_line().ok_or(HttpError::RecvFailed)?;
    let status_line = status_line.trim_end_matches('\n').trim_end_matches('\r');
    // "HTTP/1.1 200 OK"
    let mut parts = status_line.splitn(3, ' ');
    let _version = parts.next().ok_or(HttpError::ParseError("no version"))?;
    let code_str = parts.next().ok_or(HttpError::ParseError("no status code"))?;
    let reason   = parts.next().unwrap_or("").to_string();
    let status   = code_str.parse::<u16>().map_err(|_| HttpError::ParseError("bad status code"))?;

    // Headers.
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    loop {
        let line = io.read_line().ok_or(HttpError::RecvFailed)?;
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        if trimmed.is_empty() { break; } // blank line = end of headers
        if let Some(colon) = trimmed.find(':') {
            let name  = trimmed[..colon].trim().to_ascii_lowercase();
            let value = trimmed[colon+1..].trim().to_string();
            headers.insert(name, value);
        }
    }

    // Body.
    let mut body = read_body(io, &headers)?;

    // Content-Encoding handling — decompress in place.
    if let Some(enc) = headers.get("content-encoding").cloned() {
        let enc_lc = enc.to_ascii_lowercase();
        let enc_lc = enc_lc.trim().to_string();
        crate::serial_println!(
            "[http_client] response content-encoding={:?} body_len={}",
            enc_lc, body.len()
        );
        let decoded = if enc_lc.contains("gzip") || enc_lc.contains("x-gzip") {
            match super::inflate::inflate_gzip(&body) {
                Ok(d) => Some(d),
                Err(e) => {
                    crate::serial_println!("[http_client] gzip inflate failed: {:?}", e);
                    None
                }
            }
        } else if enc_lc.contains("deflate") {
            // Per RFC 7230 §4.2.2 deflate should be zlib-wrapped, but many
            // servers send raw deflate.  Try both.
            super::inflate::inflate_zlib(&body)
                .or_else(|_| super::inflate::inflate(&body))
                .map_err(|e| {
                    crate::serial_println!("[http_client] deflate inflate failed: {:?}", e);
                    e
                })
                .ok()
        } else if enc_lc.contains("identity") || enc_lc.is_empty() {
            None
        } else {
            crate::serial_println!("[http_client] unsupported content-encoding: {}", enc);
            None
        };
        if let Some(d) = decoded {
            crate::serial_println!(
                "[http_client] decompressed {}B → {}B ({})",
                body.len(), d.len(), enc_lc
            );
            body = d;
            headers.remove("content-encoding");
            headers.insert("content-length".into(), alloc::format!("{}", body.len()));
        }
    }

    Ok(HttpResponse { status, reason, headers, body })
}

fn read_body<IO: ClientIo>(
    io:      &mut IO,
    headers: &BTreeMap<String, String>,
) -> Result<Vec<u8>, HttpError> {
    // Check for chunked transfer encoding.
    let chunked = headers.get("transfer-encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false);

    if chunked {
        return read_chunked_body(io);
    }

    // Content-Length based.
    if let Some(cl) = headers.get("content-length") {
        let n = cl.trim().parse::<usize>().map_err(|_| HttpError::ParseError("bad content-length"))?;
        return io.read_exact(n).ok_or(HttpError::RecvFailed);
    }

    // No content-length and not chunked → read until EOF (for responses without body).
    let mut body = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = io.read(&mut chunk);
        if n == 0 { break; }
        body.extend_from_slice(&chunk[..n]);
        if body.len() > 16 * 1024 * 1024 { break; } // 16 MiB safety limit
    }
    Ok(body)
}

/// RFC 7230 §4.1 chunked transfer decoding.
fn read_chunked_body<IO: ClientIo>(io: &mut IO) -> Result<Vec<u8>, HttpError> {
    let mut body = Vec::new();
    loop {
        // Read chunk-size line (hex digits followed by CRLF).
        let line = io.read_line().ok_or(HttpError::RecvFailed)?;
        let hex  = line.trim_end_matches('\n').trim_end_matches('\r');
        // Strip chunk-extension (;…)
        let hex  = hex.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(hex, 16).map_err(|_| HttpError::ParseError("bad chunk size"))?;
        if size == 0 { break; } // last chunk
        let chunk = io.read_exact(size).ok_or(HttpError::RecvFailed)?;
        body.extend_from_slice(&chunk);
        // Consume trailing CRLF after chunk data.
        let _ = io.read_line();
    }
    // Consume trailing headers (empty line).
    loop {
        let line = io.read_line().unwrap_or_default();
        if line.trim().is_empty() { break; }
    }
    Ok(body)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Base64 encoder (for Basic auth)
// ─────────────────────────────────────────────────────────────────────────────

const B64_CHARS: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = Vec::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let combined = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_CHARS[((combined >> 18) & 63) as usize]);
        out.push(B64_CHARS[((combined >> 12) & 63) as usize]);
        out.push(if chunk.len() > 1 { B64_CHARS[((combined >>  6) & 63) as usize] } else { b'=' });
        out.push(if chunk.len() > 2 { B64_CHARS[( combined        & 63) as usize] } else { b'=' });
    }
    String::from_utf8(out).unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────────────
//  High-level client helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Build a GET request for the given URL.
pub fn get_request(url: &Url) -> HttpRequest {
    HttpRequest::new(Method::Get, url.request_target())
}

/// Build a POST request with a JSON body.
pub fn post_json(url: &Url, json: &str) -> HttpRequest {
    HttpRequest::new(Method::Post, url.request_target())
        .header("Content-Type", "application/json")
        .body(json.as_bytes().to_vec())
}

/// Convenience: add Basic auth header.
pub fn with_basic_auth(req: HttpRequest, user: &str, pass: &str) -> HttpRequest {
    let creds = format!("{}:{}", user, pass);
    let encoded = base64_encode(creds.as_bytes());
    req.header("Authorization", format!("Basic {}", encoded))
}

// ─────────────────────────────────────────────────────────────────────────────
//  Kernel TCP-backed I/O adapter
// ─────────────────────────────────────────────────────────────────────────────

/// Adapter that wraps a raw byte slice (e.g. already-received TLS data).
pub struct SliceIo<'a> {
    pub data: &'a [u8],
    pub pos:  usize,
}

impl<'a> ClientIo for SliceIo<'a> {
    fn write_all(&mut self, _data: &[u8]) -> bool { true } // no-op for read-only
    fn read(&mut self, buf: &mut [u8]) -> usize {
        let rem = &self.data[self.pos..];
        let n = rem.len().min(buf.len());
        buf[..n].copy_from_slice(&rem[..n]);
        self.pos += n;
        n
    }
    fn read_line(&mut self) -> Option<String> {
        let rem = &self.data[self.pos..];
        let end = rem.windows(2).position(|w| w == b"\r\n")?;
        let s = alloc::string::String::from_utf8_lossy(&rem[..end + 2]).into_owned();
        self.pos += end + 2;
        Some(s)
    }
    fn read_exact(&mut self, n: usize) -> Option<Vec<u8>> {
        let rem = &self.data[self.pos..];
        if rem.len() < n { return None; }
        let v = rem[..n].to_vec();
        self.pos += n;
        Some(v)
    }
}

/// Adapter that wraps a kernel TCP connection ID for use with `parse_response`.
pub struct TcpClientIo {
    pub conn_id: u64,
    buf:         Vec<u8>,
    pos:         usize,
}

impl TcpClientIo {
    pub fn new(conn_id: u64) -> Self {
        TcpClientIo { conn_id, buf: Vec::new(), pos: 0 }
    }

    /// Fill internal buffer from TCP stack.
    fn fill(&mut self) -> bool {
        let mut tmp = [0u8; 4096];
        if let Ok(n) = super::tcp::recv(self.conn_id, &mut tmp) {
            if n > 0 {
                self.buf.extend_from_slice(&tmp[..n]);
                return true;
            }
        }
        false
    }
}

impl ClientIo for TcpClientIo {
    fn write_all(&mut self, data: &[u8]) -> bool {
        super::tcp::send(self.conn_id, data).is_ok()
    }

    fn read(&mut self, out: &mut [u8]) -> usize {
        // Serve from buffer first.
        let avail = self.buf.len() - self.pos;
        if avail == 0 {
            if !self.fill() { return 0; }
        }
        let avail = self.buf.len() - self.pos;
        let n = avail.min(out.len());
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos+n]);
        self.pos += n;
        if self.pos == self.buf.len() { self.buf.clear(); self.pos = 0; }
        n
    }

    fn read_line(&mut self) -> Option<String> {
        loop {
            // Search for \n in buffer.
            if let Some(nl) = self.buf[self.pos..].iter().position(|&b| b == b'\n') {
                let end = self.pos + nl + 1;
                let line = String::from_utf8_lossy(&self.buf[self.pos..end]).into_owned();
                self.pos = end;
                if self.pos == self.buf.len() { self.buf.clear(); self.pos = 0; }
                return Some(line);
            }
            if !self.fill() { break; }
        }
        None
    }

    fn read_exact(&mut self, n: usize) -> Option<Vec<u8>> {
        // Keep filling until we have enough data.
        while self.buf.len() - self.pos < n {
            if !self.fill() { return None; }
        }
        let out = self.buf[self.pos..self.pos+n].to_vec();
        self.pos += n;
        if self.pos == self.buf.len() { self.buf.clear(); self.pos = 0; }
        Some(out)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Simple HTTP GET helper (plain TCP, for http:// URLs)
// ─────────────────────────────────────────────────────────────────────────────

/// Perform a synchronous HTTP GET request over plain TCP or TLS (https://).
/// Follows redirects (301, 302, 303, 307, 308) up to 5 hops.
pub fn http_get(url_str: &str) -> Result<HttpResponse, HttpError> {
    http_get_recursive(url_str, 0)
}

fn http_get_recursive(url_str: &str, depth: usize) -> Result<HttpResponse, HttpError> {
    if depth >= 5 {
        return Err(HttpError::TooManyRedirects);
    }

    let url = Url::parse(url_str).ok_or(HttpError::InvalidUrl)?;

    // Build the request: get_request adds gzip/UA/Accept-Encoding, here we add cookies.
    let mut req = get_request(&url);
    let cookie_hdr = super::cookies::header_for(&url.host, &url.path, &url.scheme);
    if !cookie_hdr.is_empty() {
        req.headers.insert("Cookie".into(), cookie_hdr);
    }
    // Default Connection: close so the server sends EOF when done.
    req.headers.insert("Connection".into(), "close".into());
    let request_bytes = req.serialize(&url.host);

    let resp = if url.scheme == "https" {
        crate::serial_println!("[http_client] HTTPS GET {} (depth={})", url_str, depth);
        let raw = super::tls::https_request(&url.host, url.port, &request_bytes)
            .map_err(|_| HttpError::ConnectionFailed)?;
        let mut io = SliceIo { data: &raw, pos: 0 };
        parse_response(&mut io)?
    } else if url.scheme == "http" {
        crate::serial_println!("[http_client] HTTP GET {} (depth={})", url_str, depth);
        let ip = crate::net::dns::resolve(&url.host)
            .map_err(|_| HttpError::ConnectionFailed)?;
        let local_port: u16 = 49152 + (url.port % 1024);
        let conn_id = super::tcp::connect(ip, url.port, local_port)
            .map_err(|_| HttpError::ConnectionFailed)?;
        let mut io = TcpClientIo::new(conn_id);
        if !io.write_all(&request_bytes) {
            let _ = super::tcp::close(conn_id);
            return Err(HttpError::SendFailed);
        }
        let r = parse_response(&mut io)?;
        let _ = super::tcp::close(conn_id);
        r
    } else {
        return Err(HttpError::InvalidUrl);
    };

    // Ingest cookies from the response into the jar.
    super::cookies::ingest_set_cookies(&resp.headers, &url.host, &url.path);

    // Follow redirect (301, 302, 303, 307, 308)
    if [301, 302, 303, 307, 308].contains(&resp.status) {
        if let Some(loc) = resp.header("location") {
            let next_url = if loc.starts_with('/') {
                // Relative redirect from root
                format!("{}://{}{}", url.scheme, url.host, loc)
            } else if !loc.contains("://") {
                // Relative redirect from current path
                if let Some(slash_idx) = url.path.rfind('/') {
                    format!("{}://{}{}/{}", url.scheme, url.host, &url.path[..slash_idx], loc)
                } else {
                    format!("{}://{}{}/{}", url.scheme, url.host, url.path, loc)
                }
            } else {
                // Absolute redirect
                loc.to_string()
            };
            crate::serial_println!("[http_client] redirect: {} → {}", url_str, next_url);
            return http_get_recursive(&next_url, depth + 1);
        }
    }

    crate::serial_println!(
        "[http_client] {} {} ({}B body, {} cookies in jar)",
        url_str, resp.status, resp.body.len(), super::cookies::len()
    );
    Ok(resp)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Statistics
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::{AtomicU64, Ordering};

static HTTP_CLIENT_REQUESTS:   AtomicU64 = AtomicU64::new(0);
static HTTP_CLIENT_ERRORS:     AtomicU64 = AtomicU64::new(0);
static HTTP_CLIENT_BYTES_SENT: AtomicU64 = AtomicU64::new(0);
static HTTP_CLIENT_BYTES_RECV: AtomicU64 = AtomicU64::new(0);

pub fn bump_request()     { HTTP_CLIENT_REQUESTS.fetch_add(1, Ordering::Relaxed); }
pub fn bump_error()       { HTTP_CLIENT_ERRORS.fetch_add(1,   Ordering::Relaxed); }
pub fn bump_sent(n: u64)  { HTTP_CLIENT_BYTES_SENT.fetch_add(n, Ordering::Relaxed); }
pub fn bump_recv(n: u64)  { HTTP_CLIENT_BYTES_RECV.fetch_add(n, Ordering::Relaxed); }

pub fn print_stats() {
    crate::serial_println!(
        "[http_client] reqs={} errs={} sent={} recv={}",
        HTTP_CLIENT_REQUESTS.load(Ordering::Relaxed),
        HTTP_CLIENT_ERRORS.load(Ordering::Relaxed),
        HTTP_CLIENT_BYTES_SENT.load(Ordering::Relaxed),
        HTTP_CLIENT_BYTES_RECV.load(Ordering::Relaxed),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  Module init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[http_client] HTTP/1.1 client ready (GET/POST/PUT/DELETE, chunked, basic-auth).");
}

/// HTTP/1.0 Server + Echo Server for Smart OS — Phase 12.
///
/// Kernel-space HTTP server that serves static files from the VFS.
/// Spawns as a kernel thread, listens on a configurable port.
/// Also includes a simple echo/telnet server.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::serial_println;

// ═══════════════════════════════════════════════════════════════
//  HTTP Server
// ═══════════════════════════════════════════════════════════════

const DOC_ROOT: &str = "/home/user/www";
const MAX_REQUEST_SIZE: usize = 4096;
const MAX_RESPONSE_SIZE: usize = 32768;

static HTTP_RUNNING: AtomicBool = AtomicBool::new(false);
static HTTP_REQUESTS: AtomicU64 = AtomicU64::new(0);
static HTTP_ERRORS: AtomicU64 = AtomicU64::new(0);
static HTTP_PORT: AtomicU64 = AtomicU64::new(0);

/// Initialize and start the HTTP server on the given port.
pub fn init(port: u16) {
    if HTTP_RUNNING.load(Ordering::Relaxed) {
        serial_println!("[http] Server already running on port {}", HTTP_PORT.load(Ordering::Relaxed));
        return;
    }

    // Start listening
    if let Err(e) = super::tcp::listen(port) {
        serial_println!("[http] Failed to listen on port {}: {}", port, e);
        return;
    }

    HTTP_PORT.store(port as u64, Ordering::Relaxed);
    HTTP_RUNNING.store(true, Ordering::Relaxed);

    // Spawn the HTTP server thread (reads port from HTTP_PORT atomic)
    crate::process::scheduler::spawn("http-server", http_server_thread, 6);

    serial_println!("[http] HTTP server started on port {}", port);
}

fn http_server_thread() {
    let port = HTTP_PORT.load(Ordering::Relaxed) as u16;
    serial_println!("[http] Server thread running, accepting on port {}", port);

    loop {
        // Try to accept a connection
        if let Some(conn_id) = super::tcp::accept(port) {
            handle_connection(conn_id);
        }

        // Yield to other threads between polls
        crate::process::scheduler::yield_now();
    }
}

fn handle_connection(conn_id: super::tcp::TcpSocketId) {
    // Read the request (poll with yields)
    let mut buf = vec![0u8; MAX_REQUEST_SIZE];
    let mut total_read = 0;
    let mut attempts = 0;

    while attempts < 100 && total_read == 0 {
        match super::tcp::recv(conn_id, &mut buf[total_read..]) {
            Ok(n) if n > 0 => {
                total_read += n;
                // Check if we have a complete request (ends with \r\n\r\n)
                if total_read >= 4 {
                    let end = &buf[total_read - 4..total_read];
                    if end == b"\r\n\r\n" {
                        break;
                    }
                }
            }
            _ => {
                attempts += 1;
                crate::process::scheduler::yield_now();
            }
        }
    }

    if total_read == 0 {
        HTTP_ERRORS.fetch_add(1, Ordering::Relaxed);
        let _ = super::tcp::close(conn_id);
        return;
    }

    // Parse the request
    let request = &buf[..total_read];
    let response = match parse_request(request) {
        Some((method, path)) => {
            if method == "GET" {
                serve_file(path)
            } else {
                build_response(405, "Method Not Allowed", "text/plain", b"405 Method Not Allowed")
            }
        }
        None => {
            build_response(400, "Bad Request", "text/plain", b"400 Bad Request")
        }
    };

    HTTP_REQUESTS.fetch_add(1, Ordering::Relaxed);

    // Send response
    let resp_bytes = response.as_bytes();
    let _ = super::tcp::send(conn_id, resp_bytes);

    // Close connection
    let _ = super::tcp::close(conn_id);
}

fn parse_request(data: &[u8]) -> Option<(&str, &str)> {
    let text = core::str::from_utf8(data).ok()?;
    let first_line = text.lines().next()?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next()?;
    let path = parts.next()?;
    // Ignore HTTP version
    Some((method, path))
}

fn serve_file(url_path: &str) -> String {
    // Map URL path to VFS path
    let path = if url_path == "/" {
        format!("{}/index.html", DOC_ROOT)
    } else {
        format!("{}{}", DOC_ROOT, url_path)
    };

    // Try to read from VFS
    match crate::vfs::open(&path) {
        Ok(fd) => {
            let mut buf = vec![0u8; MAX_RESPONSE_SIZE];
            match crate::vfs::read(fd, &mut buf) {
                Ok(n) => {
                    let _ = crate::vfs::close(fd);
                    let body = &buf[..n];
                    let ctype = content_type(url_path);
                    build_response(200, "OK", ctype, body)
                }
                Err(_) => {
                    let _ = crate::vfs::close(fd);
                    HTTP_ERRORS.fetch_add(1, Ordering::Relaxed);
                    build_response(500, "Internal Server Error", "text/plain", b"500 Read Error")
                }
            }
        }
        Err(_) => {
            build_response(404, "Not Found", "text/html",
                format!("<html><body><h1>404 Not Found</h1><p>{}</p></body></html>", url_path).as_bytes())
        }
    }
}

fn content_type(path: &str) -> &'static str {
    if path.ends_with(".html") || path.ends_with(".htm") {
        "text/html"
    } else if path.ends_with(".txt") {
        "text/plain"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".js") {
        "application/javascript"
    } else if path.ends_with(".rs") || path.ends_with(".c") || path.ends_with(".h") {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

fn build_response(status: u16, status_text: &str, ctype: &str, body: &[u8]) -> String {
    // Build response as a string (we need to handle binary body carefully)
    let header = format!(
        "HTTP/1.0 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nServer: SmartOS/0.12.0\r\nConnection: close\r\n\r\n",
        status, status_text, ctype, body.len()
    );
    let mut response = header;
    // Append body as UTF-8 (for text content; binary will be lossy but HTTP is text-based here)
    if let Ok(body_str) = core::str::from_utf8(body) {
        response.push_str(body_str);
    } else {
        response.push_str("[binary content]");
    }
    response
}

/// Check if HTTP server is running.
pub fn is_running() -> bool {
    HTTP_RUNNING.load(Ordering::Relaxed)
}

/// Get HTTP server stats: (requests_served, errors).
pub fn stats() -> (u64, u64) {
    (HTTP_REQUESTS.load(Ordering::Relaxed), HTTP_ERRORS.load(Ordering::Relaxed))
}

/// Get the port the HTTP server is listening on.
pub fn port() -> u16 {
    HTTP_PORT.load(Ordering::Relaxed) as u16
}

// ═══════════════════════════════════════════════════════════════
//  Echo Server
// ═══════════════════════════════════════════════════════════════

static ECHO_RUNNING: AtomicBool = AtomicBool::new(false);
static ECHO_PORT: AtomicU64 = AtomicU64::new(0);

/// Start an echo server on the given port.
pub fn init_echo(port: u16) {
    if ECHO_RUNNING.load(Ordering::Relaxed) {
        serial_println!("[echo] Echo server already running");
        return;
    }

    if let Err(e) = super::tcp::listen(port) {
        serial_println!("[echo] Failed to listen on port {}: {}", port, e);
        return;
    }

    ECHO_PORT.store(port as u64, Ordering::Relaxed);
    ECHO_RUNNING.store(true, Ordering::Relaxed);

    crate::process::scheduler::spawn("echo-server", echo_server_thread, 7);

    serial_println!("[echo] Echo server started on port {}", port);
}

fn echo_server_thread() {
    let port = ECHO_PORT.load(Ordering::Relaxed) as u16;
    loop {
        if let Some(conn_id) = super::tcp::accept(port) {
            // Read data and echo it back
            let mut buf = vec![0u8; 1024];
            let mut attempts = 0;
            while attempts < 50 {
                match super::tcp::recv(conn_id, &mut buf) {
                    Ok(n) if n > 0 => {
                        let _ = super::tcp::send(conn_id, &buf[..n]);
                        break;
                    }
                    _ => {
                        attempts += 1;
                        crate::process::scheduler::yield_now();
                    }
                }
            }
            let _ = super::tcp::close(conn_id);
        }
        crate::process::scheduler::yield_now();
    }
}

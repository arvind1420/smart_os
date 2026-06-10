#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use smartsdk::gui::{Window, EVENT_MOUSE_CLICK};
use smartsdk::io::print;
use smartsdk::syscall::{syscall1, syscall2, syscall3, syscall4, SYS_TCP_CONNECT, SYS_TCP_SEND, SYS_TCP_RECV, SYS_TCP_CLOSE, SYS_GETHOSTBYNAME, SYS_AI_INFER, SYS_TLS_CONNECT, SYS_TLS_SEND, SYS_TLS_RECV, SYS_TLS_CLOSE};
use core::alloc::{GlobalAlloc, Layout};

// Standard Smart OS User-space Allocator (Bump)
struct BumpAllocator {
    heap: core::cell::UnsafeCell<[u8; 2 * 1024 * 1024]>, // 2MB heap for browser
    next: core::sync::atomic::AtomicUsize,
}

unsafe impl Sync for BumpAllocator {}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();
        let mut next = self.next.load(core::sync::atomic::Ordering::Relaxed);
        let padding = next % align;
        let offset = if padding == 0 { 0 } else { align - padding };
        next += offset;
        if next + size > 2 * 1024 * 1024 { return core::ptr::null_mut(); }
        self.next.store(next + size, core::sync::atomic::Ordering::Relaxed);
        self.heap.get().cast::<u8>().add(next)
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

impl BumpAllocator {
    fn reset(&self) {
        self.next.store(0, core::sync::atomic::Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    heap: core::cell::UnsafeCell::new([0; 2 * 1024 * 1024]),
    next: core::sync::atomic::AtomicUsize::new(0),
};

// ═══════════════════════════════════════════════════════════════
//  Wrappers
// ═══════════════════════════════════════════════════════════════

fn resolve_host(name: &str) -> Result<[u8; 4], ()> {
    let mut ip = [0u8; 4];
    let res = syscall2(SYS_GETHOSTBYNAME, name.as_ptr() as u64, name.len() as u64);
    if res == u64::MAX { return Err(()); }
    let ip_val = res as u32;
    ip[0] = (ip_val >> 24) as u8;
    ip[1] = (ip_val >> 16) as u8;
    ip[2] = (ip_val >> 8) as u8;
    ip[3] = (ip_val & 0xFF) as u8;
    Ok(ip)
}

fn tcp_connect(ip: [u8; 4], port: u16) -> Result<usize, ()> {
    let ip_val = u32::from_be_bytes(ip) as u64;
    let res = syscall2(SYS_TCP_CONNECT, ip_val, port as u64);
    if res == u64::MAX { Err(()) } else { Ok(res as usize) }
}

fn tcp_send(fd: usize, data: &[u8]) -> Result<usize, ()> {
    let res = syscall3(SYS_TCP_SEND, fd as u64, data.as_ptr() as u64, data.len() as u64);
    if res == u64::MAX { Err(()) } else { Ok(res as usize) }
}

fn tcp_recv(fd: usize, buf: &mut [u8]) -> Result<usize, ()> {
    let res = syscall3(SYS_TCP_RECV, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64);
    if res == u64::MAX { Err(()) } else { Ok(res as usize) }
}

fn tls_connect(host: &str, port: u16) -> Result<u32, ()> {
    let res = syscall3(SYS_TLS_CONNECT, host.as_ptr() as u64, host.len() as u64, port as u64);
    if res == u64::MAX { Err(()) } else { Ok(res as u32) }
}

fn tls_send(id: u32, data: &[u8]) -> Result<(), ()> {
    let res = syscall3(SYS_TLS_SEND, id as u64, data.as_ptr() as u64, data.len() as u64);
    if res == u64::MAX { Err(()) } else { Ok(()) }
}

fn tls_recv(id: u32, buf: &mut [u8]) -> Result<usize, ()> {
    let res = syscall3(SYS_TLS_RECV, id as u64, buf.as_mut_ptr() as u64, buf.len() as u64);
    if res == u64::MAX { Err(()) } else { Ok(res as usize) }
}

fn tls_close(id: u32) -> Result<(), ()> {
    let res = syscall1(SYS_TLS_CLOSE, id as u64);
    if res == u64::MAX { Err(()) } else { Ok(()) }
}

// ═══════════════════════════════════════════════════════════════
//  HTML Parsing
// ═══════════════════════════════════════════════════════════════

fn parse_html(html: &str) -> String {
    let mut text = String::new();
    let mut tag_content = String::new();
    let mut in_tag = false;
    let mut body_found = false;
    let mut in_body = false;

    let mut i = 0;
    while i < html.len() {
        let c = html.as_bytes()[i] as char;
        if c == '<' {
            in_tag = true;
            tag_content.clear();
        } else if c == '>' {
            in_tag = false;
            
            let mut tag_lower = tag_content.clone();
            tag_lower.make_ascii_lowercase();

            if tag_lower.starts_with("body") {
                body_found = true;
                in_body = true;
            } else if tag_lower.starts_with("/body") {
                in_body = false;
            } else if !body_found || in_body {
                if tag_lower.starts_with("h1") {
                    if !text.ends_with('\n') && !text.is_empty() { text.push('\n'); }
                    text.push_str("# ");
                } else if tag_lower.starts_with("h2") {
                    if !text.ends_with('\n') && !text.is_empty() { text.push('\n'); }
                    text.push_str("## ");
                } else if tag_lower.starts_with("h3") {
                    if !text.ends_with('\n') && !text.is_empty() { text.push('\n'); }
                    text.push_str("### ");
                } else if tag_lower.starts_with("p") || tag_lower.starts_with("br") || tag_lower.starts_with("div") || tag_lower.starts_with("/h1") || tag_lower.starts_with("/h2") || tag_lower.starts_with("/h3") {
                    if !text.ends_with('\n') && !text.is_empty() { text.push('\n'); }
                }
            }
        } else if in_tag {
            tag_content.push(c);
        } else if !in_tag && (!body_found || in_body) {
            if c.is_ascii_whitespace() {
                if !text.ends_with(' ') && !text.ends_with('\n') && !text.is_empty() {
                    text.push(' ');
                }
            } else {
                text.push(c);
            }
        }
        i += 1;
    }

    if text.is_empty() {
        html.chars().take(200).collect()
    } else {
        text.trim().to_string()
    }
}

// ═══════════════════════════════════════════════════════════════
//  Main
// ═══════════════════════════════════════════════════════════════

fn draw_ui(win: &Window, url: &str, status_msg: &str) {
    win.clear();
    win.draw_text_ttf(10, 20, 16, "Smart Browser");
    win.draw_text_ttf(200, 18, 12, &format!("URL: {}", url));
    win.draw_text_ttf(22, 46, 11, "Fetch URL");
    win.draw_text_ttf(130, 46, 11, "AI Summarize");
    win.draw_text_ttf(240, 46, 11, status_msg);
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Smart Browser...\n");

    if let Some(win) = Window::new(600, 500) {
        let fetch_btn = win.add_button(10, 30, 100, 24);
        let ai_btn = win.add_button(120, 30, 100, 24);

        let mut url = String::from("http://10.0.2.2:8080/");
        let mut status_msg = String::from("Status: Idle");
        let mut page_content = String::from("Welcome to Smart OS Browser. Click 'Fetch' or type a URL and press Enter.");
        let mut fetched_html = String::new();

        // Initial render
        draw_ui(&win, &url, &status_msg);
        win.draw_text_ttf(10, 80, 12, &page_content);

        loop {
            let mut got_event = false;
            let mut should_fetch = false;

            while let Some(ev) = Window::poll_event() {
                got_event = true;
                if ev.event_type == EVENT_MOUSE_CLICK {
                    let btn_id = ev.data[2] as u8;
                    if btn_id == fetch_btn {
                        should_fetch = true;
                    } else if btn_id == ai_btn {
                        if page_content.is_empty() || page_content.starts_with("Welcome") {
                            status_msg = String::from("Status: No content to summarize.");
                        } else {
                            status_msg = String::from("Status: AI summarizing...");
                            // Display status immediately
                            draw_ui(&win, &url, &status_msg);

                            let mut ai_buf = [0u8; 512];
                            let res = syscall4(SYS_AI_INFER, page_content.as_ptr() as u64, page_content.len() as u64, ai_buf.as_mut_ptr() as u64, ai_buf.len() as u64);
                            if res != u64::MAX {
                                let len = res as usize;
                                page_content = String::from_utf8_lossy(&ai_buf[..len]).into_owned();
                                status_msg = String::from("Status: Summary Generated.");
                            } else {
                                status_msg = String::from("Status: AI Inference Failed.");
                            }
                        }
                    }
                } else if ev.event_type == 1 { // EVENT_KEY_PRESS
                    let ascii = ev.data[0] as u8;
                    if ascii == 8 || ascii == 127 { // Backspace
                        url.pop();
                    } else if ascii == 10 || ascii == 13 { // Enter
                        should_fetch = true;
                    } else if ascii >= 32 && ascii <= 126 { // Printable
                        url.push(ascii as char);
                    }
                }
            }

            if should_fetch {
                // Drop variables allocated on heap
                drop(status_msg);
                drop(page_content);
                drop(fetched_html);

                // Reset heap
                ALLOCATOR.reset();

                // Reallocate variables
                status_msg = String::from("Status: Connecting...");
                page_content = String::new();
                fetched_html = String::new();

                draw_ui(&win, &url, &status_msg);

                // Parse the URL: [http://]host[:port]/path
                let mut parsed_host = String::new();
                let mut parsed_port = 80u16;
                let mut parsed_path = String::from("/");

                let mut trimmed = url.as_str();
                if trimmed.starts_with("http://") {
                    trimmed = &trimmed[7..];
                } else if trimmed.starts_with("https://") {
                    trimmed = &trimmed[8..];
                    parsed_port = 443;
                }

                let (host_port, path) = if let Some(slash_pos) = trimmed.find('/') {
                    (&trimmed[..slash_pos], &trimmed[slash_pos..])
                } else {
                    (trimmed, "/")
                };
                parsed_path = path.to_string();

                if let Some(colon_pos) = host_port.find(':') {
                    parsed_host = host_port[..colon_pos].to_string();
                    if let Ok(p) = host_port[colon_pos+1..].parse::<u16>() {
                        parsed_port = p;
                    }
                } else {
                    parsed_host = host_port.to_string();
                }

                let is_https = parsed_port == 443;

                if is_https {
                    status_msg = String::from("Status: TLS connecting...");
                    draw_ui(&win, &url, &status_msg);

                    match tls_connect(&parsed_host, parsed_port) {
                        Ok(tls_id) => {
                            status_msg = String::from("Status: TLS sending GET...");
                            draw_ui(&win, &url, &status_msg);

                            let req_str = format!("GET {} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n", parsed_path, parsed_host);
                            let req = req_str.as_bytes();
                            if tls_send(tls_id, req).is_ok() {
                                let mut response = Vec::new();
                                let mut buf = [0u8; 1024];
                                
                                loop {
                                    for _ in 0..100_000 { core::hint::spin_loop(); }
                                    
                                    match tls_recv(tls_id, &mut buf) {
                                        Ok(0) => break,
                                        Ok(n) => response.extend_from_slice(&buf[..n]),
                                        Err(_) => break,
                                    }
                                }
                                
                                if !response.is_empty() {
                                    let full_text = String::from_utf8_lossy(&response);
                                    if let Some(pos) = full_text.find("\r\n\r\n") {
                                        fetched_html = full_text[pos+4..].to_string();
                                    } else {
                                        fetched_html = full_text.to_string();
                                    }
                                    page_content = parse_html(&fetched_html);
                                    status_msg = String::from("Status: Page Loaded (HTTPS).");
                                } else {
                                    status_msg = String::from("Status: Empty HTTPS Response.");
                                }
                            }
                            let _ = tls_close(tls_id);
                        }
                        Err(_) => {
                            status_msg = String::from("Status: TLS Connection Failed.");
                        }
                    }
                } else {
                    match resolve_host(&parsed_host) {
                        Ok(ip) => {
                            status_msg = String::from("Status: Socket connecting...");
                            draw_ui(&win, &url, &status_msg);

                            match tcp_connect(ip, parsed_port) {
                                Ok(fd) => {
                                    status_msg = String::from("Status: Sending GET...");
                                    draw_ui(&win, &url, &status_msg);

                                    let req_str = format!("GET {} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n", parsed_path, parsed_host);
                                    let req = req_str.as_bytes();
                                    if tcp_send(fd, req).is_ok() {
                                        let mut response = Vec::new();
                                        let mut buf = [0u8; 1024];
                                        
                                        loop {
                                            for _ in 0..100_000 { core::hint::spin_loop(); }
                                            
                                            match tcp_recv(fd, &mut buf) {
                                                Ok(0) => break,
                                                Ok(n) => response.extend_from_slice(&buf[..n]),
                                                Err(_) => break,
                                            }
                                        }
                                        
                                        if !response.is_empty() {
                                            let full_text = String::from_utf8_lossy(&response);
                                            if let Some(pos) = full_text.find("\r\n\r\n") {
                                                fetched_html = full_text[pos+4..].to_string();
                                            } else {
                                                fetched_html = full_text.to_string();
                                            }
                                            page_content = parse_html(&fetched_html);
                                            status_msg = String::from("Status: Page Loaded.");
                                        } else {
                                            status_msg = String::from("Status: Empty Response.");
                                        }
                                    }
                                    syscall1(SYS_TCP_CLOSE, fd as u64);
                                }
                                Err(_) => {
                                    status_msg = String::from("Status: Connection Failed.");
                                }
                            }
                        }
                        Err(_) => {
                            status_msg = String::from("Status: DNS Resolve Failed.");
                        }
                    }
                }
            }

            if got_event || should_fetch {
                draw_ui(&win, &url, &status_msg);

                let mut y = 80;
                for line in page_content.lines() {
                    if y >= 480 { break; }
                    
                    if line.starts_with("# ") {
                        let text = &line[2..];
                        let display_line = if text.len() > 45 { &text[..45] } else { text };
                        win.draw_text_ttf(10, (y + 18) as u16, 18, display_line);
                        y += 26;
                    } else if line.starts_with("## ") {
                        let text = &line[3..];
                        let display_line = if text.len() > 55 { &text[..55] } else { text };
                        win.draw_text_ttf(10, (y + 14) as u16, 15, display_line);
                        y += 22;
                    } else if line.starts_with("### ") {
                        let text = &line[4..];
                        let display_line = if text.len() > 65 { &text[..65] } else { text };
                        win.draw_text_ttf(10, (y + 12) as u16, 13, display_line);
                        y += 20;
                    } else {
                        let display_line = if line.len() > 80 { &line[..80] } else { line };
                        win.draw_text_ttf(10, (y + 10) as u16, 12, display_line);
                        y += 18;
                    }
                }
            }

            for _ in 0..100_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}

#![no_std]
#![no_main]

extern crate alloc;

use smartsdk::gui::{Window, EVENT_MOUSE_CLICK};
use smartsdk::io::print;
use smartsdk::syscall::{syscall2, syscall3, SYS_TCP_CONNECT, SYS_TCP_SEND, SYS_TCP_RECV};
use core::alloc::{GlobalAlloc, Layout};

// Standard Smart OS User-space Allocator (Bump)
struct BumpAllocator {
    heap: core::cell::UnsafeCell<[u8; 1024 * 1024]>, // 1MB heap for browser
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
        if next + size > 1024 * 1024 { return core::ptr::null_mut(); }
        self.next.store(next + size, core::sync::atomic::Ordering::Relaxed);
        self.heap.get().cast::<u8>().add(next)
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    heap: core::cell::UnsafeCell::new([0; 1024 * 1024]),
    next: core::sync::atomic::AtomicUsize::new(0),
};

// Simple wrappers since they aren't in smartsdk::net yet
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

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Web Browser...
");

    if let Some(win) = Window::new(600, 400) {
        win.draw_text(10, 10, "--- SMART BROWSER ---");
        
        let url_btn_id = win.add_button(10, 30, 80, 24);
        win.draw_text(20, 35, "Fetch URL");

        let mut status_msg = "Idle";
        let mut display_text = "Welcome to the Internet.";

        win.draw_text(100, 35, status_msg);
        win.draw_text(10, 70, display_text);

        loop {
            // Check for events
            while let Some(ev) = Window::poll_event() {
                if ev.event_type == EVENT_MOUSE_CLICK {
                    let btn_id = ev.data[2] as u8;
                    if btn_id == url_btn_id {
                        status_msg = "Connecting...";
                        win.draw_text(100, 35, status_msg);
                        
                        // Simple HTTP GET request to a local test server (QEMU Host)
                        // In a real browser we'd use DNS and parse a URL bar
                        let ip = [10, 0, 2, 2]; 
                        let port = 8080;
                        
                        match tcp_connect(ip, port) {
                            Ok(fd) => {
                                status_msg = "Connected. Sending GET...";
                                win.draw_text(100, 35, status_msg);
                                
                                let req = b"GET / HTTP/1.0
Host: localhost

";
                                if tcp_send(fd, req).is_ok() {
                                    let mut buf = [0u8; 512];
                                    // Give network time to respond
                                    for _ in 0..1_000_000 { core::hint::spin_loop(); }
                                    
                                    if let Ok(n) = tcp_recv(fd, &mut buf) {
                                        if n > 0 {
                                            status_msg = "Received Data!";
                                            // Render raw text (MVP renderer)
                                            // In a real browser, parse HTML here
                                            display_text = "Data downloaded.";
                                            win.draw_text(10, 70, "HTTP Response: (truncated)");
                                            // Very basic rendering of first few bytes
                                            // (Requires format_buf or complex string logic in no_std)
                                        } else {
                                            status_msg = "No data received.";
                                        }
                                    }
                                }
                                smartsdk::syscall::syscall1(smartsdk::syscall::SYS_TCP_CLOSE, fd as u64);
                            },
                            Err(_) => {
                                status_msg = "Connection Failed.";
                            }
                        }
                        win.draw_text(100, 35, status_msg);
                    }
                }
            }

            for _ in 0..1_000_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}

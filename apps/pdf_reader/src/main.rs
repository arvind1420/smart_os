#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use smartsdk::gui::{Window, EVENT_MOUSE_CLICK};
use smartsdk::io::print;
use core::alloc::{GlobalAlloc, Layout};

// ═══════════════════════════════════════════════════════════════
//  Bump Allocator (8MB Heap)
// ═══════════════════════════════════════════════════════════════

struct BumpAllocator {
    heap: core::cell::UnsafeCell<[u8; 8 * 1024 * 1024]>,
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
        if next + size > 8 * 1024 * 1024 { return core::ptr::null_mut(); }
        self.next.store(next + size, core::sync::atomic::Ordering::Relaxed);
        unsafe { self.heap.get().cast::<u8>().add(next) }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    heap: core::cell::UnsafeCell::new([0; 8 * 1024 * 1024]),
    next: core::sync::atomic::AtomicUsize::new(0),
};

// ═══════════════════════════════════════════════════════════════
//  VFS Helper Functions
// ═══════════════════════════════════════════════════════════════

fn read_last_pdf_path() -> String {
    let path = "/tmp/last_pdf.txt";
    let fd = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_OPEN,
        path.as_ptr() as u64,
        path.len() as u64,
        0
    );
    if fd == u64::MAX {
        return String::from("/home/user/documents/sample.pdf");
    }
    let mut buf = [0u8; 256];
    let n = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_READ,
        fd,
        buf.as_mut_ptr() as u64,
        buf.len() as u64
    );
    smartsdk::syscall::syscall1(smartsdk::syscall::SYS_CLOSE, fd);
    if n > 0 && n != u64::MAX {
        let mut len = n as usize;
        while len > 0 && (buf[len - 1] == 0 || buf[len - 1] == b'\n' || buf[len - 1] == b'\r') {
            len -= 1;
        }
        let slice = &buf[..len];
        if let Ok(s) = core::str::from_utf8(slice) {
            String::from(s)
        } else {
            String::from("/home/user/documents/sample.pdf")
        }
    } else {
        String::from("/home/user/documents/sample.pdf")
    }
}

fn read_file_bytes(path: &str) -> Result<Vec<u8>, &'static str> {
    let fd = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_OPEN,
        path.as_ptr() as u64,
        path.len() as u64,
        0
    );
    if fd == u64::MAX {
        return Err("Failed to open PDF file");
    }

    let mut size_buf = 0u64;
    smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_STAT,
        path.as_ptr() as u64,
        path.len() as u64,
        &mut size_buf as *mut u64 as u64
    );

    let mut data = Vec::new();
    if size_buf > 0 {
        data.resize(size_buf as usize, 0u8);
        let n = smartsdk::syscall::syscall3(
            smartsdk::syscall::SYS_READ,
            fd,
            data.as_mut_ptr() as u64,
            size_buf
        );
        if n == u64::MAX {
            data.clear();
        } else {
            data.truncate(n as usize);
        }
    } else {
        let mut chunk = [0u8; 4096];
        loop {
            let n = smartsdk::syscall::syscall3(
                smartsdk::syscall::SYS_READ,
                fd,
                chunk.as_mut_ptr() as u64,
                chunk.len() as u64
            );
            if n == 0 || n == u64::MAX {
                break;
            }
            data.extend_from_slice(&chunk[..n as usize]);
        }
    }
    smartsdk::syscall::syscall1(smartsdk::syscall::SYS_CLOSE, fd);
    if data.is_empty() {
        Err("File is empty")
    } else {
        Ok(data)
    }
}

// ═══════════════════════════════════════════════════════════════
//  PDF Document Parser
// ═══════════════════════════════════════════════════════════════

fn get_pdf_metadata(data: &[u8]) -> (String, usize) {
    let mut version = String::from("1.4");
    if data.len() >= 8 && &data[0..5] == b"%PDF-" {
        if let Ok(v) = core::str::from_utf8(&data[5..8]) {
            version = String::from(v);
        }
    }
    
    // Scan for /Count
    let mut page_count = 1;
    let mut pos = 0;
    while pos < data.len().saturating_sub(6) {
        if &data[pos..pos+6] == b"/Count" {
            let mut j = pos + 6;
            while j < data.len() && (data[j].is_ascii_whitespace() || data[j] == b'/' || data[j] == b'#') {
                j += 1;
            }
            let mut digits = Vec::new();
            while j < data.len() && data[j].is_ascii_digit() {
                digits.push(data[j]);
                j += 1;
            }
            if !digits.is_empty() {
                if let Ok(s) = core::str::from_utf8(&digits) {
                    if let Ok(val) = s.parse::<usize>() {
                        page_count = page_count.max(val);
                    }
                }
            }
            pos = j;
        } else {
            pos += 1;
        }
    }
    
    (version, page_count)
}

fn parse_pdf_pages(data: &[u8]) -> Vec<Vec<String>> {
    let mut pages = Vec::new();
    let mut pos = 0;
    
    // Scan for BT ... ET blocks
    while pos < data.len() {
        let mut bt_idx = None;
        for i in pos..data.len().saturating_sub(2) {
            if data[i] == b'B' && data[i+1] == b'T' {
                let before_ok = i == 0 || data[i-1].is_ascii_whitespace() || data[i-1] == b'\n' || data[i-1] == b'\r';
                let after_ok = data[i+2].is_ascii_whitespace() || data[i+2] == b'\n' || data[i+2] == b'\r';
                if before_ok && after_ok {
                    bt_idx = Some(i);
                    break;
                }
            }
        }
        
        let start = match bt_idx {
            Some(idx) => idx + 2,
            None => break,
        };
        
        let mut et_idx = None;
        for i in start..data.len().saturating_sub(2) {
            if data[i] == b'E' && data[i+1] == b'T' {
                let before_ok = data[i-1].is_ascii_whitespace() || data[i-1] == b'\n' || data[i-1] == b'\r';
                let after_ok = i + 2 >= data.len() || data[i+2].is_ascii_whitespace() || data[i+2] == b'\n' || data[i+2] == b'\r';
                if before_ok && after_ok {
                    et_idx = Some(i);
                    break;
                }
            }
        }
        
        let end = match et_idx {
            Some(idx) => idx,
            None => break,
        };
        
        let mut page_lines = Vec::new();
        let mut sub_pos = start;
        while sub_pos < end {
            if data[sub_pos] == b'(' {
                let mut esc = false;
                let mut val = Vec::new();
                let mut j = sub_pos + 1;
                while j < end {
                    let b = data[j];
                    if esc {
                        val.push(b);
                        esc = false;
                    } else if b == b'\\' {
                        esc = true;
                    } else if b == b')' {
                        sub_pos = j;
                        break;
                    } else {
                        val.push(b);
                    }
                    j += 1;
                }
                if let Ok(s) = core::str::from_utf8(&val) {
                    page_lines.push(String::from(s));
                }
            }
            sub_pos += 1;
        }
        
        if !page_lines.is_empty() {
            pages.push(page_lines);
        }
        pos = end + 2;
    }
    
    if pages.is_empty() {
        pages.push(alloc::vec![
            String::from("Smart OS PDF Reader - Catalog Document"),
            String::from("No renderable text stream blocks found."),
            String::from("Please select a standard unencrypted PDF.")
        ]);
    }
    pages
}

// ═══════════════════════════════════════════════════════════════
//  Main Entry Point
// ═══════════════════════════════════════════════════════════════

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Smart PDF Reader...\n");

    let pdf_path = read_last_pdf_path();
    
    let (pages, pdf_ver, pdf_pages_count) = match read_file_bytes(&pdf_path) {
        Ok(bytes) => {
            let (ver, count) = get_pdf_metadata(&bytes);
            let parsed = parse_pdf_pages(&bytes);
            (parsed, ver, count)
        }
        Err(e) => {
            let mut err_pages = Vec::new();
            err_pages.push(alloc::vec![
                format!("Failed to open document: {}", pdf_path),
                format!("Error details: {}", e)
            ]);
            (err_pages, String::from("?.?"), 0)
        }
    };

    if let Some(win) = Window::new(600, 500) {
        // Toolbar controls
        let prev_btn = win.add_button(10, 10, 50, 28);
        win.draw_text(18, 17, "Prev");

        let next_btn = win.add_button(70, 10, 50, 28);
        win.draw_text(78, 17, "Next");

        let zoom_in_btn = win.add_button(130, 10, 70, 28);
        win.draw_text(138, 17, "Zoom +");

        let zoom_out_btn = win.add_button(210, 10, 70, 28);
        win.draw_text(218, 17, "Zoom -");

        let info_btn = win.add_button(290, 10, 50, 28);
        win.draw_text(298, 17, "Info");

        let mut current_page = 0usize;
        let mut zoom_level = 100usize;
        let mut info_active = false;

        let draw_interface = |win: &Window, cur_p: usize, zoom: usize, info: bool| {
            win.clear();
            
            // Draw toolbar background
            win.fill_rect(0, 0, 600, 48, 0x24242C);
            
            // Re-draw toolbar buttons
            win.fill_rect(10, 10, 50, 28, 0x363640);
            win.draw_text(18, 17, "Prev");
            
            win.fill_rect(70, 10, 50, 28, 0x363640);
            win.draw_text(78, 17, "Next");
            
            win.fill_rect(130, 10, 70, 28, 0x363640);
            win.draw_text(138, 17, "Zoom +");
            
            win.fill_rect(210, 10, 70, 28, 0x363640);
            win.draw_text(218, 17, "Zoom -");
            
            win.fill_rect(290, 10, 50, 28, 0x363640);
            win.draw_text(298, 17, "Info");
            
            // Draw page status label in toolbar
            let status = format!("Page {}/{} | Zoom {}%", cur_p + 1, pages.len(), zoom);
            win.draw_text(360, 17, &status);
            
            // Draw page canvas (sheet of paper)
            let base_w = 420;
            let base_h = 380;
            let pw = (base_w * zoom) / 100;
            let ph = (base_h * zoom) / 100;
            
            let ox = 10 + (580 - pw) / 2;
            let oy = 60 + (420 - ph) / 2;
            
            // Shadow
            win.fill_rect((ox + 4) as u16, (oy + 4) as u16, pw as u16, ph as u16, 0x0C0C0E);
            // Page sheet (cream white)
            win.fill_rect(ox as u16, oy as u16, pw as u16, ph as u16, 0xFFFFF8);
            // Page border
            win.fill_rect(ox as u16, oy as u16, pw as u16, 1, 0xCECED6);
            win.fill_rect(ox as u16, (oy + ph - 1) as u16, pw as u16, 1, 0xCECED6);
            win.fill_rect(ox as u16, oy as u16, 1, ph as u16, 0xCECED6);
            win.fill_rect((ox + pw - 1) as u16, oy as u16, 1, ph as u16, 0xCECED6);
            
            // Draw vector text content on the page
            if cur_p < pages.len() {
                let lines = &pages[cur_p];
                let font_size = (13 * zoom) / 100;
                let line_h = (22 * zoom) / 100;
                let start_tx = ox + (30 * zoom) / 100;
                let mut start_ty = oy + (45 * zoom) / 100;
                
                for line in lines {
                    win.draw_text_ttf(start_tx as u16, start_ty as u16, font_size as u16, line);
                    start_ty += line_h;
                }
            }
            
            // Draw metadata overlay if info is active
            if info {
                // Dim overlay background
                win.fill_rect(100, 150, 400, 200, 0x1E1E24);
                // Border
                win.fill_rect(100, 150, 400, 2, 0xFF5555);
                win.fill_rect(100, 348, 400, 2, 0xFF5555);
                win.fill_rect(100, 150, 2, 200, 0xFF5555);
                win.fill_rect(498, 150, 2, 200, 0xFF5555);
                
                win.draw_text(120, 170, "Document Properties");
                win.draw_text(120, 195, "────────────────────────────────");
                
                let version_str = format!("Format Version: PDF-{}", pdf_ver);
                win.draw_text(120, 215, &version_str);
                
                let pages_str = format!("Total Pages (VFS Catalog): {}", pdf_pages_count);
                win.draw_text(120, 240, &pages_str);
                
                win.draw_text(120, 270, "Security: None (Unencrypted)");
                win.draw_text(120, 295, "Reader Engine: SmartOS Native v0.23.0");
            }
        };

        draw_interface(&win, current_page, zoom_level, info_active);

        loop {
            while let Some(ev) = Window::poll_event() {
                if ev.event_type == EVENT_MOUSE_CLICK {
                    let clicked_id = ev.data[2] as u8;
                    if clicked_id == prev_btn {
                        current_page = current_page.saturating_sub(1);
                        draw_interface(&win, current_page, zoom_level, info_active);
                    } else if clicked_id == next_btn {
                        current_page = (current_page + 1).min(pages.len() - 1);
                        draw_interface(&win, current_page, zoom_level, info_active);
                    } else if clicked_id == zoom_in_btn {
                        zoom_level = (zoom_level + 20).min(200);
                        draw_interface(&win, current_page, zoom_level, info_active);
                    } else if clicked_id == zoom_out_btn {
                        zoom_level = (zoom_level - 20).max(60);
                        draw_interface(&win, current_page, zoom_level, info_active);
                    } else if clicked_id == info_btn {
                        info_active = !info_active;
                        draw_interface(&win, current_page, zoom_level, info_active);
                    }
                }
            }
            for _ in 0..1_000_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}

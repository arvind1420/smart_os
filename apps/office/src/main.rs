#![no_std]
#![no_main]

extern crate alloc;

mod gap_buffer;
mod docx;
mod font;

use smartsdk::gui::{Window, EVENT_MOUSE_CLICK, EVENT_KEY_PRESS};
use smartsdk::io::print;
use core::alloc::{GlobalAlloc, Layout};

use gap_buffer::GapBuffer;
use font::{
    RichTextEngine, TextSpan, TextStyle, adjust_spans_for_insert, adjust_spans_for_delete
};

// 8MB Bump Allocator for User-space
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

// VFS Raw System Call Wrappers
fn read_file(path: &str, buf: &mut [u8]) -> Result<usize, &'static str> {
    let fd = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_OPEN,
        path.as_ptr() as u64,
        path.len() as u64,
        0 // O_RDONLY
    );
    if fd == u64::MAX { return Err("Open failed"); }
    let n = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_READ,
        fd,
        buf.as_mut_ptr() as u64,
        buf.len() as u64
    );
    smartsdk::syscall::syscall1(smartsdk::syscall::SYS_CLOSE, fd);
    if n == u64::MAX { return Err("Read failed"); }
    Ok(n as usize)
}

fn write_file(path: &str, data: &[u8]) -> Result<(), &'static str> {
    let fd = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_OPEN,
        path.as_ptr() as u64,
        path.len() as u64,
        2 | 64 // O_RDWR | O_CREAT
    );
    if fd == u64::MAX { return Err("Open failed"); }
    let n = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_WRITE,
        fd,
        data.as_ptr() as u64,
        data.len() as u64
    );
    smartsdk::syscall::syscall1(smartsdk::syscall::SYS_CLOSE, fd);
    if n == u64::MAX { return Err("Write failed"); }
    Ok(())
}

fn trim_path(path: &str) -> &str {
    let mut s = path;
    while s.starts_with(|c| c == ' ' || c == '\r' || c == '\n' || c == '\0') {
        s = &s[1..];
    }
    while s.ends_with(|c| c == ' ' || c == '\r' || c == '\n' || c == '\0') {
        s = &s[..s.len() - 1];
    }
    s
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Smart Office...\n");

    if let Some(win) = Window::new(800, 600) {
        // App Titlebar
        win.fill_rect(0, 0, 800, 30, 0x111115);
        win.draw_text_ttf(10, 22, 14, "Smart Office Professional");

        // UI Toolbar setup
        let new_btn = win.add_button(10, 32, 60, 26);
        let open_btn = win.add_button(80, 32, 60, 26);
        let save_btn = win.add_button(150, 32, 80, 26);
        let bold_btn = win.add_button(240, 32, 60, 26);
        let italic_btn = win.add_button(310, 32, 60, 26);
        let sz_minus_btn = win.add_button(380, 32, 60, 26);
        let sz_plus_btn = win.add_button(450, 32, 60, 26);
        let h1_btn = win.add_button(520, 32, 80, 26);

        // Document memory structure (Gap Buffer)
        let mut document = GapBuffer::new();

        // Style Spans
        let mut spans = [TextSpan { start: 0, end: 65536, style: TextStyle::default() }; 64];
        let mut span_count = 1;

        let mut current_style = TextStyle::default();
        let mut active_filepath = alloc::string::String::new();
        let mut status = "Ready.";

        // Initial default document welcome layout
        let init_text = b"Welcome to Smart Office\n=======================\nEdit text using GapBuffer and format with vector fonts.\n";
        for &b in init_text.iter() {
            document.insert(b);
        }
        adjust_spans_for_insert(&mut spans, &mut span_count, 0, init_text.len(), TextStyle::default());

        // Check if File Manager requested opening a specific file
        let mut path_buf = [0u8; 256];
        if let Ok(len) = read_file("/tmp/last_doc.txt", &mut path_buf) {
            let path = trim_path(unsafe { core::str::from_utf8_unchecked(&path_buf[..len]) });
            if !path.is_empty() {
                // Clear initial setup
                document = GapBuffer::new();
                span_count = 0;
                
                let mut file_buf = alloc::vec![0u8; 128 * 1024]; // 128KB max doc size
                if let Ok(flen) = read_file(path, &mut file_buf) {
                    let mut out_text = alloc::vec::Vec::new();
                    let mut out_spans = alloc::vec::Vec::new();
                    
                    if docx::parse_docx(&file_buf[..flen], &mut out_text, &mut out_spans).is_ok() {
                        for &b in &out_text {
                            document.insert(b);
                        }
                        
                        let copy_len = out_spans.len().min(64);
                        for idx in 0..copy_len {
                            spans[idx] = out_spans[idx];
                        }
                        span_count = copy_len;

                        active_filepath = alloc::string::String::from(path);
                        status = "Loaded document successfully.";
                    } else {
                        status = "Failed to parse document XML.";
                    }
                } else {
                    status = "Failed to read document from disk.";
                }
                
                // Clear the request flag
                let _ = write_file("/tmp/last_doc.txt", b"");
            }
        }

        // Main Draw Loop Helper
        let draw_editor = |
            win: &Window,
            doc: &GapBuffer,
            sps: &[TextSpan],
            sp_count: usize,
            curr_style: TextStyle,
            filepath: &str,
            status_text: &str,
        | {
            // Draw Window Background and Sheet Container
            win.fill_rect(0, 30, 800, 40, 0x1B1B22); // Toolbar base
            win.fill_rect(0, 70, 800, 500, 0x0B0B0E); // Workspace gray
            
            // Draw Toolbar Button Labels & Active Style States
            let draw_btn_decor = |bx: u16, bw: u16, label: &str, active: bool| {
                if active {
                    // Cyan glow border
                    win.fill_rect(bx - 2, 30, bw + 4, 30, 0x00FFD2);
                }
                win.fill_rect(bx, 32, bw, 26, if active { 0x2A2A38 } else { 0x1B1B22 });
                win.draw_text_ttf(bx + 6, 48, 12, label);
            };

            draw_btn_decor(10, 60, "New", false);
            draw_btn_decor(80, 60, "Open", false);
            draw_btn_decor(150, 80, "Save .docx", false);
            draw_btn_decor(240, 60, "Bold", curr_style.bold);
            draw_btn_decor(310, 60, "Italic", curr_style.italic);
            draw_btn_decor(380, 60, "Size -", false);
            draw_btn_decor(450, 60, "Size +", false);
            draw_btn_decor(520, 80, "Heading", curr_style.size == 32);

            // Draw Sheet (Cyber Midnight navy canvas)
            win.fill_rect(38, 78, 724, 474, 0x00FFD2); // Glowing border
            win.fill_rect(40, 80, 720, 470, 0x1A1A24); // Sheet background
            
            // Draw Document Styled Text and Calculate Cursor Coordinates
            let engine = RichTextEngine::new(doc, sps, sp_count);
            let (cx, cy) = engine.render(win, 60, 110, doc.cursor_position());

            // Draw Neon Blinking Cursor
            win.fill_rect(cx, cy - curr_style.size + 2, 2, curr_style.size + 2, 0x00FFD2);

            // Draw Status Bar
            win.fill_rect(0, 570, 800, 30, 0x111115);
            win.draw_text_ttf(10, 588, 12, status_text);

            let mut stats_buf = [0u8; 128];
            let name = if filepath.is_empty() { "Untitled" } else { filepath };
            // Format stats block
            let stats_str = format_stats(&mut stats_buf, name, doc.len(), sp_count);
            win.draw_text_ttf(500, 588, 12, stats_str);
        };

        draw_editor(&win, &document, &spans, span_count, current_style, &active_filepath, status);

        loop {
            while let Some(ev) = Window::poll_event() {
                if ev.event_type == EVENT_MOUSE_CLICK {
                    let clicked_id = ev.data[2] as u8;
                    
                    if clicked_id == new_btn {
                        document = GapBuffer::new();
                        spans = [TextSpan { start: 0, end: 65536, style: TextStyle::default() }; 64];
                        span_count = 1;
                        current_style = TextStyle::default();
                        active_filepath.clear();
                        status = "Created new empty document.";
                    } else if clicked_id == open_btn {
                        // Demo reset back to welcome document
                        document = GapBuffer::new();
                        spans = [TextSpan { start: 0, end: 65536, style: TextStyle::default() }; 64];
                        span_count = 1;
                        current_style = TextStyle::default();
                        active_filepath.clear();
                        let default_welcome = b"Smart OS Word Processor\n=======================\nEnjoy beautiful vector fonts and native rich docx export!\n";
                        for &b in default_welcome.iter() {
                            document.insert(b);
                        }
                        adjust_spans_for_insert(&mut spans, &mut span_count, 0, default_welcome.len(), TextStyle::default());
                        status = "Loaded default template.";
                    } else if clicked_id == save_btn {
                        status = "Saving document...";
                        draw_editor(&win, &document, &spans, span_count, current_style, &active_filepath, status);

                        // Save format
                        let mut text_buf = [0u8; 8192];
                        let len = document.copy_to_slice(&mut text_buf);

                        let mut zip_buf = alloc::vec![0u8; 128 * 1024]; // 128KB target ZIP buffer
                        let zip_size = docx::generate_docx(&text_buf[..len], &spans[..span_count], span_count, &mut zip_buf);

                        let save_path = if active_filepath.is_empty() {
                            "/home/user/documents/Document1.docx"
                        } else {
                            &active_filepath
                        };

                        if write_file(save_path, &zip_buf[..zip_size]).is_ok() {
                            active_filepath = alloc::string::String::from(save_path);
                            status = "Saved document successfully.";
                        } else {
                            status = "Failed to save file to disk.";
                        }
                    } else if clicked_id == bold_btn {
                        current_style.bold = !current_style.bold;
                        status = if current_style.bold { "Bold active" } else { "Regular active" };
                    } else if clicked_id == italic_btn {
                        current_style.italic = !current_style.italic;
                        status = if current_style.italic { "Italic active" } else { "Regular active" };
                    } else if clicked_id == sz_minus_btn {
                        if current_style.size > 10 {
                            current_style.size -= 2;
                        }
                        status = "Font size decreased.";
                    } else if clicked_id == sz_plus_btn {
                        if current_style.size < 48 {
                            current_style.size += 2;
                        }
                        status = "Font size increased.";
                    } else if clicked_id == h1_btn {
                        if current_style.size == 32 {
                            current_style.size = 16;
                            current_style.bold = false;
                            status = "Body layout size.";
                        } else {
                            current_style.size = 32;
                            current_style.bold = true;
                            status = "Heading 1 style.";
                        }
                    }
                    
                    draw_editor(&win, &document, &spans, span_count, current_style, &active_filepath, status);
                } else if ev.event_type == EVENT_KEY_PRESS {
                    let char_code = ev.data[0] as u8;
                    let scancode = ev.data[1] as u8;
                    let cursor_pos = document.cursor_position();

                    if char_code == 0x08 { // Backspace
                        if cursor_pos > 0 {
                            document.delete_before_cursor();
                            adjust_spans_for_delete(&mut spans, &mut span_count, cursor_pos - 1, 1);
                        }
                    } else if scancode == 0x4B { // Left Arrow
                        document.move_cursor_left();
                        // Sync current typing style from character at cursor
                        let new_pos = document.cursor_position();
                        for s in 0..span_count {
                            if new_pos >= spans[s].start && new_pos < spans[s].end {
                                current_style = spans[s].style;
                                break;
                            }
                        }
                    } else if scancode == 0x4D { // Right Arrow
                        document.move_cursor_right();
                        let new_pos = document.cursor_position();
                        for s in 0..span_count {
                            if new_pos >= spans[s].start && new_pos < spans[s].end {
                                current_style = spans[s].style;
                                break;
                            }
                        }
                    } else if (char_code >= 0x20 && char_code <= 0x7E) || char_code == b'\n' {
                        document.insert(char_code);
                        adjust_spans_for_insert(&mut spans, &mut span_count, cursor_pos, 1, current_style);
                    }
                    
                    draw_editor(&win, &document, &spans, span_count, current_style, &active_filepath, status);
                }
            }
            for _ in 0..500_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}

fn format_stats<'a>(buf: &'a mut [u8], path: &str, chars: usize, spans: usize) -> &'a str {
    let mut offset = 0;
    
    // Copy path name
    let path_bytes = path.as_bytes();
    let path_len = path_bytes.len().min(30);
    buf[offset..offset+path_len].copy_from_slice(&path_bytes[..path_len]);
    offset += path_len;
    
    // Copy space
    buf[offset] = b' ';
    offset += 1;
    
    // Copy "| Chars: "
    let label1 = b"| Ch: ";
    buf[offset..offset+label1.len()].copy_from_slice(label1);
    offset += label1.len();
    
    // Copy Chars count
    let mut c_buf = [0u8; 10];
    let c_str = u16_to_str(chars as u16, &mut c_buf);
    let c_bytes = c_str.as_bytes();
    buf[offset..offset+c_bytes.len()].copy_from_slice(c_bytes);
    offset += c_bytes.len();
    
    // Copy " Spans: "
    let label2 = b" Sp: ";
    buf[offset..offset+label2.len()].copy_from_slice(label2);
    offset += label2.len();
    
    // Copy Spans count
    let mut s_buf = [0u8; 10];
    let s_str = u16_to_str(spans as u16, &mut s_buf);
    let s_bytes = s_str.as_bytes();
    buf[offset..offset+s_bytes.len()].copy_from_slice(s_bytes);
    offset += s_bytes.len();
    
    unsafe { core::str::from_utf8_unchecked(&buf[..offset]) }
}

fn u16_to_str(mut val: u16, buf: &mut [u8]) -> &str {
    if val == 0 {
        buf[0] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[..1]) };
    }
    let mut i = buf.len();
    while val > 0 {
        i -= 1;
        buf[i] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[i..]) }
}

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
use font::{RichTextEngine, TextSpan, TextStyle};

// Standard Smart OS User-space Allocator (Bump)
struct BumpAllocator {
    heap: core::cell::UnsafeCell<[u8; 2 * 1024 * 1024]>, // 2MB heap
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

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    heap: core::cell::UnsafeCell::new([0; 2 * 1024 * 1024]),
    next: core::sync::atomic::AtomicUsize::new(0),
};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Smart Office...\n");

    if let Some(win) = Window::new(800, 600) {
        // Toolbar background and title
        win.draw_text(10, 10, "Smart Office Professional - Document1.docx");
        
        let new_btn = win.add_button(10, 30, 60, 24);
        win.draw_text(15, 35, "New");

        let save_btn = win.add_button(80, 30, 60, 24);
        win.draw_text(85, 35, "Save .docx");

        let bold_btn = win.add_button(150, 30, 60, 24);
        win.draw_text(155, 35, "Bold");

        let italic_btn = win.add_button(220, 30, 60, 24);
        win.draw_text(225, 35, "Italic");

        let h1_btn = win.add_button(290, 30, 60, 24);
        win.draw_text(295, 35, "H1 (32pt)");

        // Status bar
        let mut status = "Ready.";
        win.draw_text(10, 570, status);

        // Document memory engine (Gap Buffer)
        let mut document = GapBuffer::new();
        
        // Initial text
        let init_text = b"Smart OS Word Processor\n-----------------------\nWelcome to the future of native editing.\n";
        for &b in init_text.iter() {
            document.insert(b);
        }

        // Current typing style state
        let mut current_style = TextStyle::default();

        let mut draw_document = |win: &Window, doc: &GapBuffer| {
            // Clear document area
            win.fill_rect(10, 70, 780, 490, 0x000000); 
            
            // In a real app we'd build the spans dynamically as we type.
            // For MVP, we pass the raw text to the Rich Text Engine which 
            // uses the fallback bitmap font and the simulated vector font
            // to render the layout.
            let engine = RichTextEngine::new(doc);
            engine.render(win, 20, 80);
        };

        draw_document(&win, &document);

        loop {
            while let Some(ev) = Window::poll_event() {
                if ev.event_type == EVENT_MOUSE_CLICK {
                    let clicked_id = ev.data[2] as u8;
                    win.fill_rect(10, 570, 300, 20, 0x000000); // Clear status
                    
                    if clicked_id == new_btn {
                        document = GapBuffer::new(); // Reset the memory structure
                        status = "Created new document.";
                        draw_document(&win, &document);
                    } else if clicked_id == save_btn {
                        status = "Exporting to .docx...";
                        win.draw_text(10, 570, status);
                        
                        // Copy text from gap buffer
                        let mut text_buf = [0u8; 8192];
                        let len = document.copy_to_slice(&mut text_buf);
                        
                        // Generate the .docx ZIP archive
                        let mut zip_buf = [0u8; 65536];
                        let zip_size = docx::generate_docx(&text_buf[..len], &mut zip_buf);
                        
                        // Simulate writing to VFS
                        print("Generated ");
                        // (Use format_buf macro to print size in real app)
                        print("bytes of .docx archive. Sending to VFS...\n");
                        
                        status = "Saved Document1.docx";
                    } else if clicked_id == bold_btn {
                        current_style.bold = !current_style.bold;
                        status = if current_style.bold { "Bold ON" } else { "Bold OFF" };
                    } else if clicked_id == italic_btn {
                        current_style.italic = !current_style.italic;
                        status = if current_style.italic { "Italic ON" } else { "Italic OFF" };
                    } else if clicked_id == h1_btn {
                        if current_style.size == 32 {
                            current_style.size = 16;
                            status = "Normal Text (16pt)";
                        } else {
                            current_style.size = 32;
                            status = "Heading 1 (32pt)";
                        }
                    }
                    
                    win.draw_text(10, 570, status);
                } else if ev.event_type == EVENT_KEY_PRESS {
                    let char_code = ev.data[0] as u8;
                    
                    if char_code == 0x08 { // Backspace
                        document.delete_before_cursor();
                        draw_document(&win, &document);
                    } else if char_code == 0x1B { // Esc / Left Arrow (mock mapping)
                        document.move_cursor_left();
                        draw_document(&win, &document);
                    } else if char_code == 0x1A { // Right Arrow (mock mapping)
                        document.move_cursor_right();
                        draw_document(&win, &document);
                    } else if (char_code >= 0x20 && char_code <= 0x7E) || char_code == b'\n' { 
                        document.insert(char_code);
                        draw_document(&win, &document);
                    }
                }
            }
            for _ in 0..1_000_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}

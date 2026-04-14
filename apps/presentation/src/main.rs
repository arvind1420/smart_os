#![no_std]
#![no_main]

extern crate alloc;

use smartsdk::gui::{Window, EVENT_MOUSE_CLICK, EVENT_KEY_PRESS};
use smartsdk::io::print;
use smartsdk::format_buf;
use core::alloc::{GlobalAlloc, Layout};

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

const MAX_SLIDES: usize = 10;
const MAX_TITLE: usize = 32;
const MAX_BODY: usize = 256;

struct Slide {
    title: [u8; MAX_TITLE],
    title_len: usize,
    body: [u8; MAX_BODY],
    body_len: usize,
}

impl Slide {
    const fn new() -> Self {
        Self {
            title: [0; MAX_TITLE],
            title_len: 0,
            body: [0; MAX_BODY],
            body_len: 0,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Smart Presentation...\n");

    if let Some(win) = Window::new(800, 600) {
        let mut slides = [
            Slide::new(), Slide::new(), Slide::new(), Slide::new(), Slide::new(),
            Slide::new(), Slide::new(), Slide::new(), Slide::new(), Slide::new(),
        ];
        
        let init_title = b"Smart OS Presentation";
        slides[0].title[..init_title.len()].copy_from_slice(init_title);
        slides[0].title_len = init_title.len();

        let init_body = b"Welcome to the next generation.\nClick here to edit body.";
        slides[0].body[..init_body.len()].copy_from_slice(init_body);
        slides[0].body_len = init_body.len();

        let mut current_slide = 0;
        let mut total_slides = 1;
        
        let mut editing_title = false;
        let mut playing = false;

        win.draw_text(10, 10, "Smart Presentation Professional - Deck1.pptx");
        
        let prev_btn = win.add_button(10, 30, 60, 24);
        win.draw_text(15, 35, "Prev");

        let next_btn = win.add_button(80, 30, 60, 24);
        win.draw_text(85, 35, "Next");
        
        let new_btn = win.add_button(150, 30, 80, 24);
        win.draw_text(155, 35, "New Slide");

        let play_btn = win.add_button(240, 30, 60, 24);
        win.draw_text(245, 35, "Play");

        let draw_editor = |win: &Window, slide_idx: usize, slides: &[Slide; MAX_SLIDES], total: usize, editing_title: bool| {
            // Restore UI if we just stopped playing
            win.draw_text(10, 10, "Smart Presentation Professional - Deck1.pptx");
            win.fill_rect(10, 30, 300, 24, 0x111111); // fake toolbar background
            win.draw_text(15, 35, "Prev");
            win.draw_text(85, 35, "Next");
            win.draw_text(155, 35, "New Slide");
            win.draw_text(245, 35, "Play");

            win.fill_rect(10, 70, 150, 490, 0x222222);
            for i in 0..total {
                let y = 80 + (i as u16 * 40);
                if i == slide_idx {
                    win.fill_rect(20, y, 130, 30, 0x555555);
                } else {
                    win.fill_rect(20, y, 130, 30, 0x333333);
                }
                
                let mut buf = [0u8; 16];
                let label = format_buf!(&mut buf, "Slide {}", i + 1);
                win.draw_text(30, y + 8, label);
            }

            win.fill_rect(170, 70, 610, 490, 0xFFFFFF); 
            let slide = &slides[slide_idx];
            
            if slide.title_len > 0 {
                if let Ok(text) = core::str::from_utf8(&slide.title[..slide.title_len]) {
                    win.draw_text(200, 100, text);
                }
            }
            if editing_title {
                win.draw_text(200 + (slide.title_len as u16 * 8), 100, "_");
            }

            let mut y = 150;
            let mut line_start = 0;
            for i in 0..slide.body_len {
                if slide.body[i] == b'\n' || i == slide.body_len - 1 {
                    let end = if slide.body[i] == b'\n' { i } else { i + 1 };
                    if end > line_start {
                        if let Ok(line) = core::str::from_utf8(&slide.body[line_start..end]) {
                            win.draw_text(200, y, line);
                        }
                    }
                    y += 20;
                    line_start = i + 1;
                }
            }
            if !editing_title {
                let x_off = if slide.body_len > 0 && slide.body[slide.body_len - 1] != b'\n' {
                    (slide.body_len - line_start) as u16 * 8
                } else { 0 };
                let cursor_y = if slide.body_len > 0 && slide.body[slide.body_len-1] == b'\n' { y } else { y - 20 };
                win.draw_text(200 + x_off, cursor_y, "_");
            }
        };

        let draw_fullscreen = |win: &Window, slide_idx: usize, slides: &[Slide; MAX_SLIDES]| {
            win.fill_rect(0, 0, 800, 600, 0x000000);
            
            let slide = &slides[slide_idx];
            if slide.title_len > 0 {
                if let Ok(text) = core::str::from_utf8(&slide.title[..slide.title_len]) {
                    win.draw_text(300, 100, text);
                    win.draw_text(300, 101, text); // Simulate thickness
                }
            }

            let mut y = 200;
            let mut line_start = 0;
            for i in 0..slide.body_len {
                if slide.body[i] == b'\n' || i == slide.body_len - 1 {
                    let end = if slide.body[i] == b'\n' { i } else { i + 1 };
                    if end > line_start {
                        if let Ok(line) = core::str::from_utf8(&slide.body[line_start..end]) {
                            win.draw_text(200, y, line);
                        }
                    }
                    y += 30;
                    line_start = i + 1;
                }
            }
            win.draw_text(10, 580, "Click or press Space to advance. Press ESC to exit.");
        };

        draw_editor(&win, current_slide, &slides, total_slides, editing_title);

        loop {
            while let Some(ev) = Window::poll_event() {
                if playing {
                    if ev.event_type == EVENT_MOUSE_CLICK {
                        if current_slide < total_slides - 1 {
                            current_slide += 1;
                            draw_fullscreen(&win, current_slide, &slides);
                        } else {
                            playing = false;
                            win.fill_rect(0, 0, 800, 600, 0x111111); // clear full
                            draw_editor(&win, current_slide, &slides, total_slides, editing_title);
                        }
                    } else if ev.event_type == EVENT_KEY_PRESS {
                        let code = ev.data[0] as u8;
                        if code == 0x1B { // Esc
                            playing = false;
                            win.fill_rect(0, 0, 800, 600, 0x111111);
                            draw_editor(&win, current_slide, &slides, total_slides, editing_title);
                        } else if code == b' ' || code == 0x0D {
                            if current_slide < total_slides - 1 {
                                current_slide += 1;
                                draw_fullscreen(&win, current_slide, &slides);
                            } else {
                                playing = false;
                                win.fill_rect(0, 0, 800, 600, 0x111111);
                                draw_editor(&win, current_slide, &slides, total_slides, editing_title);
                            }
                        }
                    }
                } else {
                    if ev.event_type == EVENT_MOUSE_CLICK {
                        let clicked_id = ev.data[2] as u8;
                        let mx = ev.data[0] as u16;
                        let my = ev.data[1] as u16;

                        if clicked_id == prev_btn && current_slide > 0 {
                            current_slide -= 1;
                        } else if clicked_id == next_btn && current_slide < total_slides - 1 {
                            current_slide += 1;
                        } else if clicked_id == new_btn && total_slides < MAX_SLIDES {
                            current_slide = total_slides;
                            total_slides += 1;
                        } else if clicked_id == play_btn {
                            playing = true;
                            current_slide = 0;
                            draw_fullscreen(&win, current_slide, &slides);
                            continue;
                        } else {
                            if mx > 170 && mx < 780 {
                                if my >= 70 && my <= 130 {
                                    editing_title = true;
                                } else if my > 130 {
                                    editing_title = false;
                                }
                            }
                            if mx >= 10 && mx <= 160 && my >= 70 {
                                let clicked_idx = ((my - 80) / 40) as usize;
                                if clicked_idx < total_slides {
                                    current_slide = clicked_idx;
                                }
                            }
                        }
                        draw_editor(&win, current_slide, &slides, total_slides, editing_title);
                    } else if ev.event_type == EVENT_KEY_PRESS {
                        let char_code = ev.data[0] as u8;
                        let slide = &mut slides[current_slide];
                        
                        if editing_title {
                            if char_code == 0x08 {
                                if slide.title_len > 0 { slide.title_len -= 1; }
                            } else if char_code >= 0x20 && char_code <= 0x7E {
                                if slide.title_len < MAX_TITLE {
                                    slide.title[slide.title_len] = char_code;
                                    slide.title_len += 1;
                                }
                            }
                        } else {
                            if char_code == 0x08 {
                                if slide.body_len > 0 { slide.body_len -= 1; }
                            } else if (char_code >= 0x20 && char_code <= 0x7E) || char_code == b'\n' {
                                if slide.body_len < MAX_BODY {
                                    slide.body[slide.body_len] = char_code;
                                    slide.body_len += 1;
                                }
                            }
                        }
                        draw_editor(&win, current_slide, &slides, total_slides, editing_title);
                    }
                }
            }
            for _ in 0..1_000_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}

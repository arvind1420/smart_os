#![no_std]
#![no_main]

extern crate alloc;

use smartsdk::gui::{Window, EVENT_MOUSE_CLICK};
use smartsdk::io::print;
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

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Smart Games...
");

    if let Some(win) = Window::new(400, 400) {
        win.draw_text(10, 10, "--- SMART GAMES ---");
        
        let mut target_x = 200;
        let mut target_y = 200;
        let mut score = 0;
        
        // Very simple click-the-target game
        win.draw_text(10, 30, "Click the target!");
        win.draw_text(10, 50, "Score: 0");
        
        // Draw initial target
        win.add_button(target_x, target_y, 40, 40);

        loop {
            while let Some(ev) = Window::poll_event() {
                if ev.event_type == EVENT_MOUSE_CLICK {
                    let mx = ev.data[0] as u16;
                    let my = ev.data[1] as u16;
                    
                    // Check if click is inside the target button (roughly)
                    if mx >= target_x && mx <= target_x + 40 && my >= target_y && my <= target_y + 40 {
                        score += 1;
                        
                        // Clear old text and redraw score
                        win.draw_text(10, 50, "Score:    "); // clear
                        
                        let mut buf = [0u8; 32];
                        let score_str = smartsdk::format_buf!(&mut buf, "Score: {}", score);
                        win.draw_text(10, 50, score_str);
                        
                        // Move target (simple pseudo-random based on current pos)
                        target_x = (target_x + 73) % 360;
                        target_y = (target_y + 111) % 360;
                        if target_y < 100 { target_y += 100; }
                        
                        // The SDK doesn't easily let us move buttons yet without full redraw,
                        // so for MVP we just draw a new button on top. In a real game we'd
                        // use a graphics API.
                        win.add_button(target_x, target_y, 40, 40);
                    }
                }
            }
            for _ in 0..1_000_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}

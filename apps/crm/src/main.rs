#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use smartsdk::ui::{UiApp, VBox, Label, Button, Widget, Rect};
use smartsdk::db::SmartDb;
use smartpack::Value;
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
        unsafe { self.heap.get().cast::<u8>().add(next) }
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
    print("Starting Enterprise CRM...\n");
    
    // We will build a UI using the Retained Mode Layout
    let mut layout = VBox::new();
    layout.add(Label::new("Smart OS Enterprise CRM"));
    layout.add(Label::new("========================="));
    layout.add(Button::new("Sync DB (Save Client)"));
    layout.add(Button::new("RPC Call (Billing)"));
    layout.add(Label::new("Status: Ready"));

    if let Some(mut app) = UiApp::new("Enterprise CRM", 800, 600, alloc::boxed::Box::new(layout)) {
        // App loop handles layout and drawing via `app.run()`.
        // To handle logic without downcasting, we'll manually check the UI tree state.
        
        let base_rect = Rect { x: 10, y: 30, w: 780, h: 560 };
        // app.root.layout(...) is called inside app.run(), but we will inline run() here 
        // so we can access the VBox children directly.
        
        app.run(|_| {});
    }

    smartsdk::syscall::exit(0);
}

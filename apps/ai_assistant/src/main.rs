#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use smartsdk::ui::{UiApp, VBox, HBox, Label, Button, Icon, Widget, Rect};
use smartsdk::io::print;
use smartsdk::syscall::{syscall4, SYS_AI_INFER, SYS_AI_SEARCH};
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

// ═══════════════════════════════════════════════════════════════
//  Wrappers
// ═══════════════════════════════════════════════════════════════

fn ai_infer(input: &str) -> String {
    let mut buf = [0u8; 512];
    let res = syscall4(SYS_AI_INFER, input.as_ptr() as u64, input.len() as u64, buf.as_mut_ptr() as u64, buf.len() as u64);
    if res == u64::MAX {
        String::from("AI error.")
    } else {
        String::from_utf8_lossy(&buf[..res as usize]).into_owned()
    }
}

fn ai_search(query: &str) -> Vec<String> {
    let mut buf = [0u8; 1024];
    let res = syscall4(SYS_AI_SEARCH, query.as_ptr() as u64, query.len() as u64, buf.as_mut_ptr() as u64, buf.len() as u64);
    if res == u64::MAX {
        Vec::new()
    } else {
        if let Ok(smartpack::Value::Array(arr)) = smartpack::decode(&buf[..res as usize]) {
            arr.into_iter().filter_map(|v| v.as_str().map(|s| String::from(s))).collect()
        } else {
            Vec::new()
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  Main
// ═══════════════════════════════════════════════════════════════

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting AI Assistant...\n");

    let mut root = VBox::new();
    root.add(Label::new("--- SMART ASSISTANT ---"));
    root.add(Label::new("Ask me anything about your system or files."));
    
    let mut history = VBox::new();
    history.add(Label::new("User: Hello!"));
    history.add(Label::new("AI: Hello! How can I help you today?"));
    root.add(history);

    let mut input_box = HBox::new();
    input_box.add(Button::new(" Search Files "));
    input_box.add(Button::new(" System Status "));
    root.add(input_box);

    if let Some(mut app) = UiApp::new("AI Assistant", 500, 400, Box::new(root)) {
        app.bg_color = 0x1A1A2E;
        app.run(|_root| {
            // Interactive logic would go here
        });
    }

    smartsdk::syscall::exit(0);
}

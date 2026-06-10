#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use smartsdk::ui::{UiApp, VBox, HBox, Label, Button, Icon, Widget, Rect};
use smartsdk::io::print;
use smartsdk::syscall::{syscall1, syscall2, SYS_DISPLAY_CMD, SYS_DISPLAY_EVENT};
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
//  Main
// ═══════════════════════════════════════════════════════════════

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Smart Game Store...\n");

    let mut root = VBox::new();
    root.add(Label::new("--- SMART GAME STORE ---"));
    root.add(Label::new("Vulkan-Powered Gaming Platform"));
    
    let mut game_list = HBox::new();
    game_list.add(Icon::new("Doom OS", "/bin/doom"));
    game_list.add(Icon::new("RustCraft", "/bin/rustcraft"));
    game_list.add(Icon::new("CyberRun", "/bin/cyberrun"));
    root.add(game_list);

    root.add(Button::new(" Enable Performance Mode "));

    if let Some(mut app) = UiApp::new("Game Store", 600, 400, Box::new(root)) {
        app.bg_color = 0x0A0A0A;
        app.run(|_root| {
            // Logic for launching games or enabling game mode
        });
    }

    smartsdk::syscall::exit(0);
}

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use smartsdk::ui::{UiApp, VBox, HBox, Label, Button, Icon, Widget, Rect};
use smartsdk::io::print;
use smartsdk::syscall::{syscall2, SYS_SPAWN};
use core::alloc::{GlobalAlloc, Layout};

// Standard Smart OS User-space Allocator (Bump)
struct BumpAllocator {
    heap: core::cell::UnsafeCell<[u8; 2 * 1024 * 1024]>, // 2MB heap for shell
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
    print("Initializing Smart Desktop...\n");

    // Root container: A full screen layout
    let mut desktop_root = VBox::new();
    desktop_root.spacing = 0;

    // 1. Top Status Bar (Optional, but let's add one)
    let mut top_bar = HBox::new();
    top_bar.add(Label::new("  Smart OS v1.0 Pro  "));
    top_bar.add(Label::new("  [Enterprise Edition]  "));
    desktop_root.add(top_bar);

    // 2. Main Icon Area (HBox for now, simulating a grid)
    let mut icon_area = HBox::new();
    icon_area.spacing = 40;
    icon_area.add(Icon::new("Web", "/bin/browser"));
    icon_area.add(Icon::new("Files", "/bin/file_manager"));
    icon_area.add(Icon::new("Term", "/bin/terminal"));
    icon_area.add(Icon::new("Store", "/bin/pkg_installer"));
    icon_area.add(Icon::new("AI", "/bin/ai_assistant"));
    icon_area.add(Icon::new("Games", "/bin/games"));
    desktop_root.add(icon_area);

    // 3. Taskbar at the bottom
    // To push it to the bottom we'd need a more complex layout solver, 
    // for now we'll just add spacing.
    for _ in 0..15 { desktop_root.add(Label::new("")); }

    let mut taskbar = HBox::new();
    taskbar.add(Button::new(" START "));
    taskbar.add(Label::new(" | "));
    taskbar.add(Label::new(" Search... "));
    desktop_root.add(taskbar);

    if let Some(mut app) = UiApp::new("Smart Desktop", 1024, 768, Box::new(desktop_root)) {
        app.bg_color = 0x003366; // Professional Navy Blue background
        
        app.run(|root| {
            // Logic callback: Check if any icon was clicked
            // We drill down into the root -> icon_area (index 1)
            // This is a bit manual but demonstrates the architecture.
            // In a real app we'd use a message bus.
            
            // Note: Since we don't have downcasting easily, we'll just assume the structure
            // and use raw pointers or just log for now.
            // Let's actually add a helper to the Icon to handle the spawn.
        });
    }

    smartsdk::syscall::exit(0);
}

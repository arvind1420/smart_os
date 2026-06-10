#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use smartsdk::ui::{UiApp, VBox, HBox, Label, Button, Icon, Widget, Rect};
use smartsdk::io::print;
use smartsdk::syscall::{syscall2, syscall3, SYS_READDIR, SYS_SPAWN};
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

fn list_dir(path: &str) -> Vec<String> {
    let mut buf = [0u8; 4096];
    let res = syscall3(SYS_READDIR, path.as_ptr() as u64, path.len() as u64, buf.as_mut_ptr() as u64);
    if res == u64::MAX { return Vec::new(); }
    
    // readdir returns a SmartPack array of strings
    if let Ok(smartpack::Value::Array(arr)) = smartpack::decode(&buf[..res as usize]) {
        arr.into_iter().filter_map(|v| v.as_str().map(|s| String::from(s))).collect()
    } else {
        Vec::new()
    }
}

// ═══════════════════════════════════════════════════════════════
//  Main
// ═══════════════════════════════════════════════════════════════

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting File Manager...\n");

    let mut current_path = String::from("/bin");
    let mut files = list_dir(&current_path);

    let mut root = VBox::new();
    root.add(Label::new("--- SMART FILES ---"));
    root.add(Label::new(&current_path));
    
    let mut file_list = VBox::new();
    for file in &files {
        let full_path = if current_path == "/" {
            alloc::format!("/{}", file)
        } else {
            alloc::format!("{}/{}", current_path, file)
        };
        file_list.add(Icon::new(file, &full_path));
    }
    root.add(file_list);

    if let Some(mut app) = UiApp::new("File Manager", 400, 600, Box::new(root)) {
        app.bg_color = 0x222222;
        app.run(|_root| {
            // Logic for navigation could be added here
        });
    }

    smartsdk::syscall::exit(0);
}

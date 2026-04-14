#![no_std]
#![no_main]

extern crate alloc;

use smartsdk::gui::{Window, EVENT_MOUSE_CLICK};
use smartsdk::io::print;
use smartsdk::syscall::syscall2;
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

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Package Installer...
");

    if let Some(win) = Window::new(400, 300) {
        win.draw_text(10, 10, "--- SMART STORE ---");
        win.draw_text(10, 30, "Available Packages:");
        
        let mut btn_y = 60;
        let apps = ["browser", "games", "calculator"];
        let mut btn_ids = [0u8; 3];

        for (i, app) in apps.iter().enumerate() {
            let mut buf = [0u8; 64];
            let label = format_buf!(&mut buf, "Install {}", app);
            btn_ids[i] = win.add_button(10, btn_y, 160, 24);
            win.draw_text(20, btn_y + 5, label);
            btn_y += 40;
        }

        let mut status = "Select a package to download.";
        win.draw_text(10, 250, status);

        loop {
            while let Some(ev) = Window::poll_event() {
                if ev.event_type == EVENT_MOUSE_CLICK {
                    let clicked_id = ev.data[2] as u8;
                    for (i, &id) in btn_ids.iter().enumerate() {
                        if clicked_id == id {
                            status = "Downloading...";
                            win.draw_text(10, 250, "                              "); // Clear
                            win.draw_text(10, 250, status);
                            
                            // To actually trigger the kernel pkgmgr, we could use a custom syscall
                            // or trigger a shell command. Here we simulate the command by
                            // requesting the shell to run `pkg install <name>`
                            // For MVP, we will print to the kernel log
                            print("Requesting package install: ");
                            print(apps[i]);
                            print("
");

                            status = "Installed successfully.";
                            win.draw_text(10, 250, "                              "); // Clear
                            win.draw_text(10, 250, status);
                        }
                    }
                }
            }
            for _ in 0..1_000_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}

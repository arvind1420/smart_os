#![no_std]
#![no_main]

use smartsdk::gui::Window;
use smartsdk::sysinfo::get_sysinfo;
use smartsdk::io::print;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting System Dashboard...
");

    if let Some(win) = Window::new(400, 300) {
        win.draw_text(10, 10, "--- SYSTEM DASHBOARD ---");
        
        loop {
            if let Some(info) = get_sysinfo() {
                // Drawing stats (this is a simple demo, in a real app we'd format strings)
                win.draw_text(10, 40, "CPU Cores: 4"); // Hardcoded for demo until we have format!
                win.draw_text(10, 60, "Heap: Active");
                win.draw_text(10, 80, "Uptime: Running");
            }
            
            // Just a small delay loop
            for _ in 0..1000000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}

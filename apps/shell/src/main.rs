#![no_std]
#![no_main]

use smartsdk::gui::Window;
use smartsdk::io::print;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Smart Desktop Shell...\n");

    // This is a minimal implementation of the "Smart Desktop"
    // It creates a full-screen window to act as the desktop background/launcher.
    if let Some(desktop) = Window::new(1024, 768) {
        desktop.draw_text(400, 300, "SMART OS DESKTOP");
        desktop.draw_text(400, 320, "1. Launch Dashboard");
        desktop.draw_text(400, 340, "2. Launch Net Client");

        // Loop to display status
        loop {
            let count = desktop.hub_query("system_status");
            if count > 0 {
                desktop.draw_text(400, 380, "Dashboard: Active (Live)");
            }

            // Short delay
            for _ in 0..10_000_000 { core::hint::spin_loop(); }
            
            // Check for fork/exec condition or just keep running as a desktop
        }
    }

    smartsdk::syscall::exit(0);
}

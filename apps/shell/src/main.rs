#![no_std]
#![no_main]

use smartsdk::gui::Window;
use smartsdk::io::print;
use smartsdk::syscall::{syscall3, syscall1, SYS_FORK, SYS_EXEC, SYS_WAITPID};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Smart Desktop Shell...
");

    // This is a minimal implementation of the "Smart Desktop"
    // It creates a full-screen window to act as the desktop background/launcher.
    if let Some(desktop) = Window::new(1024, 768) {
        desktop.draw_text(400, 300, "SMART OS DESKTOP");
        desktop.draw_text(400, 320, "1. Launch Dashboard");
        desktop.draw_text(400, 340, "2. Launch Net Client");

        // In a real OS, we would use the SYS_READ or SYS_POLL to get keyboard/mouse events.
        // For this milestone, we'll simulate an automatic launch of the dashboard after a short delay
        // to prove the fork/exec from a native SDK app.
        
        for _ in 0..50_000_000 { core::hint::spin_loop(); }
        
        print("Shell: Forking dashboard...
");
        let child_pid = smartsdk::syscall::syscall1(SYS_FORK, 0);
        
        if child_pid == 0 {
            // Child
            let path = "/bin/dashboard\0";
            syscall3(SYS_EXEC, path.as_ptr() as u64, (path.len() - 1) as u64, 0);
            smartsdk::syscall::exit(1); // Exit if exec fails
        } else {
            // Parent: wait for child
            syscall1(SYS_WAITPID, child_pid);
        }
    }

    smartsdk::syscall::exit(0);
}

#![no_std]
#![no_main]

use smartsdk::gui::Window;
use smartsdk::sysinfo::get_sysinfo;
use smartsdk::io::print;
use smartsdk::format_buf;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting System Dashboard...\n");

    if let Some(win) = Window::new(400, 300) {
        win.draw_text(10, 10, "--- SYSTEM DASHBOARD ---");

        loop {
            if let Some(info) = get_sysinfo() {
                let mut buf1 = [0u8; 64];
                let mut buf2 = [0u8; 64];
                let mut buf3 = [0u8; 64];
                let mut buf4 = [0u8; 64];

                let cores_str = format_buf!(&mut buf1, "CPU Cores: {} active", info.cpu_count);
                let mem_str = format_buf!(&mut buf2, "Memory: {}K used / {}K free", info.heap_used / 1024, info.heap_free / 1024);
                let thread_str = format_buf!(&mut buf3, "Threads: {} running", info.thread_count);
                let uptime_str = format_buf!(&mut buf4, "Uptime: {} seconds", info.uptime);

                win.draw_text(10, 40, cores_str);
                win.draw_text(10, 60, mem_str);
                win.draw_text(10, 80, thread_str);
                win.draw_text(10, 100, uptime_str);
            }

            // Wait ~1 second before refreshing
            for _ in 0..10_000_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}


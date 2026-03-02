#![no_std]
#![no_main]

use smartsdk::gui::{Window, EVENT_MOUSE_CLICK};
use smartsdk::sysinfo::get_sysinfo;
use smartsdk::io::print;
use smartsdk::format_buf;
use smartsdk::kmod;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting System Dashboard...\n");

    if let Some(win) = Window::new(400, 300) {
        win.draw_text(10, 10, "--- SYSTEM DASHBOARD ---");
        
        let load_btn_id = win.add_button(10, 120, 120, 30);
        win.draw_text(20, 127, "Load Driver");

        let stress_btn_id = win.add_button(140, 120, 120, 30);
        win.draw_text(150, 127, "Stress Test");

        loop {
            // Check for events
            while let Some(ev) = Window::poll_event() {
                if ev.event_type == EVENT_MOUSE_CLICK {
                    let btn_id = ev.data[2] as u8;
                    if btn_id == load_btn_id {
                        print("Dashboard: Loading test driver...\n");
                        if kmod::load_module("/drivers/test_driver.sys") {
                            win.draw_text(10, 160, "Driver Loaded OK!");
                        } else {
                            win.draw_text(10, 160, "Driver Load Failed");
                        }
                    } else if btn_id == stress_btn_id {
                        print("Dashboard: Launching stress-test...\n");
                        let pid = smartsdk::syscall::syscall1(smartsdk::syscall::SYS_FORK, 0);
                        if pid == 0 {
                            let path = "/bin/stress-test\0";
                            smartsdk::syscall::syscall3(smartsdk::syscall::SYS_EXEC, path.as_ptr() as u64, (path.len()-1) as u64, 0);
                            smartsdk::syscall::exit(1);
                        }
                    }
                }
            }

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

                // Publish status to Hub
                win.hub_publish("system_status", info.uptime);
            }
            
            for _ in 0..5_000_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use smartsdk::gui::{Window, EVENT_MOUSE_CLICK};
use smartsdk::io::print;
use smartsdk::syscall::{syscall3, SYS_IPC_SEND, SYS_IPC_RECV, SYS_IPC_LOOKUP_PORT};
use smartpack::Value;
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
//  IPC Wrappers
// ═══════════════════════════════════════════════════════════════

fn pkgmgr_query(cmd: &str, name: Option<&str>) -> Result<Value, &'static str> {
    let mut map = Vec::new();
    map.push((Value::from("cmd"), Value::from(cmd)));
    if let Some(n) = name {
        map.push((Value::from("name"), Value::from(n)));
    }
    let msg = Value::Map(map);
    let payload = smartpack::encode(&msg).map_err(|_| "Encode error")?;

    // Send to pkgmgr.control
    let port_res = smartsdk::syscall::syscall2(SYS_IPC_LOOKUP_PORT, "pkgmgr.control".as_ptr() as u64, "pkgmgr.control".len() as u64);
    if port_res == u64::MAX { return Err("pkgmgr port not found"); }

    let _ = smartsdk::syscall::syscall3(SYS_IPC_SEND, port_res, payload.as_ptr() as u64, payload.len() as u64);

    // Wait for reply
    let mut buf = [0u8; 4096];
    let res = smartsdk::syscall::syscall2(SYS_IPC_RECV, buf.as_mut_ptr() as u64, buf.len() as u64);
    if res == u64::MAX { return Err("No reply from pkgmgr"); }

    let decoded = smartpack::decode(&buf[..res as usize]).map_err(|_| "Decode error")?;
    Ok(decoded)
}

// ═══════════════════════════════════════════════════════════════
//  Main
// ═══════════════════════════════════════════════════════════════

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Smart Store...
");

    if let Some(win) = Window::new(600, 450) {
        win.draw_text(10, 10, "--- SMART STORE v0.2 ---");
        
        let mut status = String::from("Loading packages...");
        win.draw_text(10, 400, &status);

        // Fetch package list via IPC
        let mut package_list = Vec::new();
        if let Ok(Value::Array(arr)) = pkgmgr_query("LIST", None) {
            for v in arr {
                if let Some(m) = v.as_map() {
                    let name = m.iter().find(|(k, _)| k.as_str() == Some("name")).map(|(_, v)| v.as_str()).flatten().unwrap_or("").to_string();
                    let desc = m.iter().find(|(k, _)| k.as_str() == Some("description")).map(|(_, v)| v.as_str()).flatten().unwrap_or("").to_string();
                    let inst = m.iter().find(|(k, _)| k.as_str() == Some("installed")).map(|(_, v)| v.as_bool()).flatten().unwrap_or(false);
                    package_list.push((name, desc, inst));
                }
            }
            status = String::from("Ready.");
        } else {
            status = String::from("Failed to load package list.");
        }

        // Render package list
        let mut btn_ids = Vec::new();
        let mut y = 60;
        for (name, desc, inst) in &package_list {
            let label = if *inst { "Installed" } else { "Install" };
            let btn_id = win.add_button(10, y, 100, 24);
            btn_ids.push(btn_id);
            
            win.draw_text(120, y + 5, name);
            let short_desc = if desc.len() > 50 { &desc[..50] } else { desc };
            win.draw_text(250, y + 5, short_desc);
            
            win.draw_text(20, y + 5, label);
            y += 40;
        }

        win.clear();
        win.draw_text(10, 10, "--- SMART STORE v0.2 ---");
        win.draw_text(10, 400, &status);
        
        // Re-render after clear
        let mut y = 60;
        for (i, (name, desc, inst)) in package_list.iter().enumerate() {
            let label = if *inst { "Installed" } else { "Install" };
            win.add_button(10, y, 100, 24);
            win.draw_text(20, y + 5, label);
            win.draw_text(120, y + 5, name);
            let short_desc = if desc.len() > 50 { &desc[..50] } else { desc };
            win.draw_text(250, y + 5, short_desc);
            y += 40;
        }

        loop {
            while let Some(ev) = Window::poll_event() {
                if ev.event_type == EVENT_MOUSE_CLICK {
                    let clicked_id = ev.data[2] as u8;
                    for (i, &id) in btn_ids.iter().enumerate() {
                        if clicked_id == id {
                            let (name, _, inst) = &package_list[i];
                            if *inst {
                                status = String::from("Package already installed.");
                            } else {
                                status = alloc::format!("Installing {}...", name);
                                win.draw_text(10, 400, "                                        ");
                                win.draw_text(10, 400, &status);
                                
                                match pkgmgr_query("INSTALL", Some(name)) {
                                    Ok(Value::String(res)) => {
                                        status = res;
                                        // Ideally we'd refresh the list here
                                    }
                                    _ => status = String::from("Installation failed."),
                                }
                            }
                            win.draw_text(10, 400, "                                        ");
                            win.draw_text(10, 400, &status);
                        }
                    }
                }
            }
            for _ in 0..1_000_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}

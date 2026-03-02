#![no_std]
#![no_main]

// In a real module, these would be linked against the kernel's KERNEL_SYMBOLS
unsafe extern "C" {
    fn serial_println(s: *const u8, len: usize);
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> i32 {
    module_init()
}

#[unsafe(no_mangle)]
pub extern "C" fn module_init() -> i32 {
    let msg = b"Hello from Dynamic Kernel Module!
";
    unsafe {
        serial_println(msg.as_ptr(), msg.len());
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn module_exit() {
    // Cleanup code
}

use core::panic::PanicInfo;
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

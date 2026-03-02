#![no_std]

pub mod syscall;
pub mod io;
pub mod net;
pub mod gui;
pub mod sysinfo;
pub mod fmt;
pub mod kmod;

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    syscall::exit(1);
}

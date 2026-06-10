#![no_std]

extern crate alloc;

pub mod syscall;
pub mod io;
pub mod net;
pub mod gui;
pub mod sysinfo;
pub mod fmt;
pub mod kmod;
pub mod ipc;
pub mod rpc;
pub mod db;
pub mod ui;
pub mod tensor;
pub mod license;

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    syscall::exit(1);
}

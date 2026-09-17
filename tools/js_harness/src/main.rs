//! Host-side test harness for the SmartOS browser JS engine.
//!
//! Pulls in the *actual* `kernel/src/net/js_*.rs` and `wpt_real.rs` sources
//! (via `#[path]`) so Tier 1 (real-WPT) iteration can run on the host with
//! `cargo run -p js_harness` instead of booting the kernel under QEMU.
//!
//! Only the handful of `crate::` calls these files make into kernel-only
//! code (serial logging, timer ticks, RTC clock) are stubbed below.

extern crate alloc;

#[macro_export]
macro_rules! serial_println {
    ($($arg:tt)*) => {{
        std::println!($($arg)*);
    }};
}

pub mod drivers {
    pub mod timer {
        pub fn ticks() -> u64 { 0 }
    }
    pub mod rtc {
        pub struct DateTime {
            pub year: u16,
            pub month: u8,
            pub day: u8,
            pub hour: u8,
            pub minute: u8,
            pub second: u8,
        }
        pub fn now() -> DateTime {
            DateTime { year: 2026, month: 1, day: 1, hour: 0, minute: 0, second: 0 }
        }
    }
}

#[path = "../../../kernel/src/net/js_ast.rs"]
pub mod js_ast;
#[path = "../../../kernel/src/net/js_lexer.rs"]
pub mod js_lexer;
#[path = "../../../kernel/src/net/js_parser.rs"]
pub mod js_parser;
#[path = "../../../kernel/src/net/js_jit.rs"]
pub mod js_jit;
#[path = "../../../kernel/src/net/js_interp.rs"]
pub mod js_interp;
#[path = "../../../kernel/src/net/wpt_real.rs"]
pub mod wpt_real;

fn main() {
    let report = wpt_real::run_real_wpt();
    println!(
        "=== Real WPT report: {}/{} ({}%) ===",
        report.passed, report.total, report.pass_rate_pct()
    );
    for f in &report.files {
        println!("\n-- {} ({}) --", f.file, f.source_url);
        for s in &f.subtests {
            if s.message.is_empty() {
                println!("  [{}] {}", s.status, s.name);
            } else {
                println!("  [{}] {} — {}", s.status, s.name, s.message);
            }
        }
    }
}

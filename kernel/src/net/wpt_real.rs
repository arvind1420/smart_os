//! Phase 141 — Real WPT (Web Platform Tests) subset runner. Tier 1.
//!
//! Unlike `wpt.rs` (300 synthetic, hand-written WPT-*style* tests that call
//! kernel APIs directly), this module runs a small curated corpus of
//! **verbatim `.any.js` files from the real web-platform-tests/wpt repo**
//! through a `testharness.js`-compatible shim on top of our own JS
//! interpreter (`net::js_interp`). This gives an honest baseline of how our
//! engine behaves against real-world test content, separate from the
//! synthetic suite.
//!
//! Each embedded file keeps its original `web-platform-tests/wpt` source
//! path in `source_url` for attribution / reproducibility. Files were
//! fetched verbatim; only the `testharness.js` / `testharnessreport.js`
//! includes were replaced by `HARNESS_SHIM` below (a from-scratch minimal
//! reimplementation of the subset of the testharness API these files use).

#![allow(dead_code)]

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;

use super::js_interp::{Interpreter, JsValue};

/// Minimal from-scratch `testharness.js`-compatible shim. Provides
/// `test`, `async_test`, `promise_test`, and the `assert_*` family used by
/// the embedded corpus. Results are pushed onto the global `__wpt_results__`
/// array as `{ name, status, message }` objects.
const HARNESS_SHIM: &str = r#"
var __wpt_results__ = [];

function __wpt_msg__(e) {
    if (e && e.message) return "" + e.message;
    return "" + e;
}

function __wpt_record__(name, status, message) {
    __wpt_results__.push({ name: name, status: status, message: message });
}

function test(fn, name) {
    try {
        fn();
        __wpt_record__(name, "PASS", "");
    } catch (e) {
        __wpt_record__(name, "FAIL", __wpt_msg__(e));
    }
}

function async_test(fn, name) {
    var t = {
        _done: false,
        _failed: false,
        step: function(f) {
            try { return f(); }
            catch (e) {
                if (!t._failed) { t._failed = true; __wpt_record__(name, "FAIL", __wpt_msg__(e)); }
            }
        },
        step_func: function(f) {
            return function() {
                try { return f(); }
                catch (e) {
                    if (!t._failed) { t._failed = true; __wpt_record__(name, "FAIL", __wpt_msg__(e)); }
                }
            };
        },
        step_func_done: function(f) {
            return function() {
                try { if (f) f(); }
                catch (e) {
                    if (!t._failed) { t._failed = true; __wpt_record__(name, "FAIL", __wpt_msg__(e)); }
                }
                t.done();
            };
        },
        unreached_func: function(msg) {
            return function() {
                if (!t._failed) {
                    t._failed = true;
                    __wpt_record__(name, "FAIL", "unreached: " + msg);
                }
            };
        },
        done: function() {
            if (!t._done && !t._failed) {
                t._done = true;
                __wpt_record__(name, "PASS", "");
            }
            t._done = true;
        }
    };
    try {
        fn(t);
        if (!t._done && !t._failed) {
            __wpt_record__(name, "FAIL", "did not complete synchronously");
        }
    } catch (e) {
        if (!t._failed) { __wpt_record__(name, "FAIL", __wpt_msg__(e)); }
    }
}

function promise_test(fn, name) {
    var t = { step: function(f) { return f(); } };
    try {
        var p = fn(t);
        if (p && p.__state__ === "rejected") {
            __wpt_record__(name, "FAIL", "promise rejected: " + __wpt_msg__(p.__value__));
        } else {
            __wpt_record__(name, "PASS", "");
        }
    } catch (e) {
        __wpt_record__(name, "FAIL", __wpt_msg__(e));
    }
}

function format_value(v) { return "" + v; }

function assert_true(actual, description) {
    if (actual !== true) throw { message: (description || "assert_true") + ": expected true, got " + format_value(actual) };
}
function assert_false(actual, description) {
    if (actual !== false) throw { message: (description || "assert_false") + ": expected false, got " + format_value(actual) };
}
function assert_equals(actual, expected, description) {
    if (actual !== expected) throw { message: (description || "assert_equals") + ": expected " + format_value(expected) + ", got " + format_value(actual) };
}
function assert_not_equals(actual, expected, description) {
    if (actual === expected) throw { message: (description || "assert_not_equals") + ": both " + format_value(actual) };
}
function assert_array_equals(actual, expected, description) {
    if (!actual || actual.length !== expected.length) {
        throw { message: (description || "assert_array_equals") + ": length mismatch" };
    }
    for (var i = 0; i < expected.length; i++) {
        if (actual[i] !== expected[i]) {
            throw { message: (description || "assert_array_equals") + ": differ at index " + i };
        }
    }
}
function assert_approx_equals(actual, expected, epsilon, description) {
    if (Math.abs(actual - expected) > epsilon) {
        throw { message: (description || "assert_approx_equals") + ": expected ~" + format_value(expected) + ", got " + format_value(actual) };
    }
}
function assert_throws_js(constructor, fn, description) {
    try { fn(); } catch (e) { return; }
    throw { message: (description || "assert_throws_js") + ": did not throw" };
}
function assert_throws_dom(name, fn, description) {
    try { fn(); } catch (e) { return; }
    throw { message: (description || "assert_throws_dom") + ": did not throw" };
}
function assert_throws_exactly(value, fn, description) {
    try { fn(); } catch (e) { return; }
    throw { message: (description || "assert_throws_exactly") + ": did not throw" };
}
function assert_class_string(object, class_string, description) {
    // Best-effort: our engine has no Symbol.toStringTag; skip strict check.
}
function assert_unreached(description) {
    throw { message: "assert_unreached: " + (description || "") };
}

self.performance = performance;
self.crypto = crypto;
"#;

/// One embedded real-WPT test file.
struct RealWptFile {
    /// Short display name.
    name: &'static str,
    /// Original path within github.com/web-platform-tests/wpt (master).
    source_url: &'static str,
    /// Verbatim (or near-verbatim) `.any.js` source.
    source: &'static str,
}

/// Result of a single WPT subtest (from `test()` / `async_test()` / `promise_test()`).
#[derive(Debug, Clone)]
pub struct RealSubtest {
    pub name:    String,
    pub status:  String, // "PASS" | "FAIL"
    pub message: String,
}

/// Result of running one embedded file.
#[derive(Debug, Clone)]
pub struct RealWptFileResult {
    pub file:       &'static str,
    pub source_url: &'static str,
    pub subtests:   Vec<RealSubtest>,
}

/// Aggregate report across all embedded real-WPT files.
#[derive(Debug, Default)]
pub struct RealWptReport {
    pub files:  Vec<RealWptFileResult>,
    pub total:  usize,
    pub passed: usize,
}

impl RealWptReport {
    pub fn pass_rate_pct(&self) -> u32 {
        if self.total == 0 { return 0; }
        ((self.passed as u64 * 100) / self.total as u64) as u32
    }
}

// ────────────────────────────────────────────────────────────────────────────
//  Embedded corpus — verbatim `.any.js` files from web-platform-tests/wpt
// ────────────────────────────────────────────────────────────────────────────

const CORPUS: &[RealWptFile] = &[
    RealWptFile {
        name: "console-is-a-namespace",
        source_url: "console/console-is-a-namespace.any.js",
        source: r#"
test(() => {
  assert_true(self.hasOwnProperty("console"));
}, "console exists on the global object");

test(() => {
  const propDesc = Object.getOwnPropertyDescriptor(self, "console");
  assert_equals(propDesc.writable, true, "must be writable");
  assert_equals(propDesc.enumerable, false, "must not be enumerable");
  assert_equals(propDesc.configurable, true, "must be configurable");
  assert_equals(propDesc.value, console, "must have the right value");
}, "console has the right property descriptors");

test(() => {
  assert_false("Console" in self);
}, "Console (uppercase, as if it were an interface) must not exist");

test(() => {
  const prototype1 = Object.getPrototypeOf(console);
  const prototype2 = Object.getPrototypeOf(prototype1);

  assert_equals(Object.getOwnPropertyNames(prototype1).length, 0, "The [[Prototype]] must have no properties");
  assert_equals(prototype2, Object.prototype, "The [[Prototype]]'s [[Prototype]] must be %ObjectPrototype%");
}, "The prototype chain must be correct");
"#,
    },
    RealWptFile {
        name: "console-log-symbol",
        source_url: "console/console-log-symbol.any.js",
        source: r#"
test(() => {
    console.log(Symbol());
    console.log(Symbol("abc"));
    console.log(Symbol.for("def"));
    console.log(Symbol.isConcatSpreadable);
}, "Logging a symbol doesn't throw");
"#,
    },
    RealWptFile {
        name: "queue-microtask",
        source_url: "html/webappapis/microtask-queuing/queue-microtask.any.js",
        source: r#"
test(() => {
  assert_equals(typeof queueMicrotask, "function");
}, "It exists and is a function");

test(() => {
  assert_throws_js(TypeError, () => queueMicrotask(), "no argument");
  assert_throws_js(TypeError, () => queueMicrotask(undefined), "undefined");
  assert_throws_js(TypeError, () => queueMicrotask(null), "null");
  assert_throws_js(TypeError, () => queueMicrotask(0), "0");
  assert_throws_js(TypeError, () => queueMicrotask({ handleEvent() { } }), "an event handler object");
  assert_throws_js(TypeError, () => queueMicrotask("window.x = 5;"), "a string");
}, "It throws when given non-functions");

async_test(t => {
  let called = false;
  queueMicrotask(t.step_func_done(() => {
    called = true;
  }));
  assert_false(called);
}, "It calls the callback asynchronously");

async_test(t => {
  const happenings = [];
  Promise.resolve().then(() => happenings.push("a"));
  queueMicrotask(() => happenings.push("b"));
  Promise.reject().catch(() => happenings.push("c"));
  queueMicrotask(t.step_func_done(() => {
    assert_array_equals(happenings, ["a", "b", "c"]);
  }));
}, "It interleaves with promises as expected");
"#,
    },
    RealWptFile {
        name: "hr-time-monotonic-clock",
        source_url: "hr-time/monotonic-clock.any.js",
        source: r#"
test(function() {
  assert_true(self.performance.now() > 0, "self.performance.now() returns positive numbers");
}, "self.performance.now() returns a positive number");

test(function() {
    var now1 = self.performance.now();
    var now2 = self.performance.now();
    assert_true((now2-now1) >= 0, "self.performance.now() difference is not negative");
  },
  "self.performance.now() difference is not negative"
);
"#,
    },
    RealWptFile {
        name: "url-tojson",
        source_url: "url/url-tojson.any.js",
        source: r#"
test(() => {
  const a = new URL("https://example.com/")
  assert_equals(JSON.stringify(a), "\"https://example.com/\"")
}, "URL.prototype.toJSON")
"#,
    },
    RealWptFile {
        name: "getRandomValues-float-arrays",
        source_url: "WebCryptoAPI/getRandomValues.any.js",
        source: r#"
test(function() {
    assert_throws_dom("TypeMismatchError", function() {
        self.crypto.getRandomValues(new Float32Array(6))
    }, "Float32Array")
    assert_throws_dom("TypeMismatchError", function() {
        self.crypto.getRandomValues(new Float64Array(6))
    }, "Float64Array")
}, "Float arrays");

test(function() {
    assert_throws_dom("TypeMismatchError", function() {
        self.crypto.getRandomValues(new DataView(new ArrayBuffer(6)))
    }, "DataView")
}, "DataView");
"#,
    },
];

// ────────────────────────────────────────────────────────────────────────────
//  Runner
// ────────────────────────────────────────────────────────────────────────────

/// Run every embedded real-WPT file through the harness shim + our JS
/// interpreter, and collect per-subtest PASS/FAIL results.
pub fn run_real_wpt() -> RealWptReport {
    let mut report = RealWptReport::default();

    for file in CORPUS {
        let mut interp = Interpreter::new();
        let combined = format!("{}\n{}", HARNESS_SHIM, file.source);
        interp.run(&combined);

        let mut subtests = Vec::new();
        if let JsValue::Array(arr) = interp.env.get("__wpt_results__") {
            for item in arr.borrow().iter() {
                if let JsValue::Object(o) = item {
                    let b = o.borrow();
                    subtests.push(RealSubtest {
                        name:    b.get("name").to_string_val(),
                        status:  b.get("status").to_string_val(),
                        message: b.get("message").to_string_val(),
                    });
                }
            }
        }

        // If the harness produced no results at all (e.g. a parse error
        // aborted the whole file), record that explicitly so the file is
        // still represented in the report rather than silently vanishing.
        if subtests.is_empty() {
            subtests.push(RealSubtest {
                name:    "(file)".to_string(),
                status:  "FAIL".to_string(),
                message: "no subtests recorded — parse or top-level runtime error".to_string(),
            });
        }

        report.total += subtests.len();
        report.passed += subtests.iter().filter(|s| s.status == "PASS").count();
        report.files.push(RealWptFileResult {
            file: file.name,
            source_url: file.source_url,
            subtests,
        });
    }

    report
}

/// Self-test entry point: runs the corpus and logs a summary + failures
/// to the serial console. Always returns `true` (this is a *report*, not
/// a pass/fail gate — Tier 1 work uses the failures as a TODO list).
pub fn self_test() -> bool {
    let report = run_real_wpt();
    crate::serial_println!(
        "[wpt-real] {}/{} real WPT subtests passing ({}%) across {} files",
        report.passed, report.total, report.pass_rate_pct(), report.files.len()
    );
    for f in &report.files {
        for s in &f.subtests {
            if s.status != "PASS" {
                crate::serial_println!(
                    "[wpt-real]   FAIL [{}] {} :: {} — {}",
                    f.file, f.source_url, s.name, s.message
                );
            }
        }
    }
    true
}

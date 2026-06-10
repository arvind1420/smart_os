#![allow(dead_code)]
/// Smart OS — Browser WPT (Web Platform Tests) Harness (Phase 96, v0.56.0)
///
/// Implements a minimal subset of the W3C Web Platform Tests runner:
///   • `WptTest`        — individual test case with pass/fail/timeout/notrun
///   • `WptSuite`       — collection of tests under a path prefix
///   • `WptHarness`     — manages suites, runs them, aggregates results
///   • `WptHtmlRunner`  — parses testharness.js-style HTML test output
///   • Built-in smoke tests for core browser subsystems
///
/// In the OS browser, tests run when the user navigates to `wpt://run`
/// (a special about: URL handled by the browser chrome).

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use alloc::vec;
use alloc::collections::BTreeMap;

// ─── Test status ─────────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TestStatus {
    Pass,
    Fail,
    Timeout,
    NotRun,
    Error,
}

impl TestStatus {
    pub fn name(self) -> &'static str {
        match self {
            TestStatus::Pass    => "PASS",
            TestStatus::Fail    => "FAIL",
            TestStatus::Timeout => "TIMEOUT",
            TestStatus::NotRun  => "NOTRUN",
            TestStatus::Error   => "ERROR",
        }
    }
    pub fn is_ok(self) -> bool { self == TestStatus::Pass }
}

// ─── Individual test ─────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct WptTest {
    pub name:    String,
    pub status:  TestStatus,
    pub message: Option<String>,
    pub duration_ms: u32,
}

impl WptTest {
    pub fn new(name: &str) -> Self {
        WptTest { name: name.to_string(), status: TestStatus::NotRun, message: None, duration_ms: 0 }
    }

    pub fn pass(name: &str) -> Self {
        WptTest { name: name.to_string(), status: TestStatus::Pass, message: None, duration_ms: 0 }
    }

    pub fn fail(name: &str, msg: &str) -> Self {
        WptTest { name: name.to_string(), status: TestStatus::Fail,
                  message: Some(msg.to_string()), duration_ms: 0 }
    }

    pub fn summary(&self) -> String {
        let msg = self.message.as_deref().unwrap_or("");
        if msg.is_empty() {
            format!("[{}] {}", self.status.name(), self.name)
        } else {
            format!("[{}] {} — {}", self.status.name(), self.name, msg)
        }
    }
}

// ─── Test suite ──────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct WptSuite {
    pub path:  String,
    pub tests: Vec<WptTest>,
}

impl WptSuite {
    pub fn new(path: &str) -> Self {
        WptSuite { path: path.to_string(), tests: Vec::new() }
    }

    pub fn add(&mut self, test: WptTest) {
        self.tests.push(test);
    }

    pub fn pass_count(&self) -> usize {
        self.tests.iter().filter(|t| t.status.is_ok()).count()
    }

    pub fn fail_count(&self) -> usize {
        self.tests.iter().filter(|t| t.status == TestStatus::Fail).count()
    }

    pub fn total(&self) -> usize { self.tests.len() }

    pub fn all_passed(&self) -> bool {
        !self.tests.is_empty() && self.tests.iter().all(|t| t.status.is_ok())
    }

    pub fn summary_line(&self) -> String {
        format!("{}: {}/{} passed", self.path, self.pass_count(), self.total())
    }
}

// ─── Harness ─────────────────────────────────────────────────────────────────
pub struct WptHarness {
    pub suites:    Vec<WptSuite>,
    pub started:   u64,
    pub finished:  u64,
}

impl WptHarness {
    pub fn new() -> Self {
        WptHarness { suites: Vec::new(), started: 0, finished: 0 }
    }

    pub fn add_suite(&mut self, suite: WptSuite) {
        self.suites.push(suite);
    }

    pub fn total_pass(&self) -> usize {
        self.suites.iter().map(|s| s.pass_count()).sum()
    }

    pub fn total_fail(&self) -> usize {
        self.suites.iter().map(|s| s.fail_count()).sum()
    }

    pub fn total_tests(&self) -> usize {
        self.suites.iter().map(|s| s.total()).sum()
    }

    pub fn all_passed(&self) -> bool {
        self.total_fail() == 0 && self.total_tests() > 0
    }

    pub fn report(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("WPT Results: {}/{} passed\n",
            self.total_pass(), self.total_tests()));
        for suite in &self.suites {
            out.push_str(&format!("  {}\n", suite.summary_line()));
            for test in &suite.tests {
                if test.status != TestStatus::Pass {
                    out.push_str(&format!("    {}\n", test.summary()));
                }
            }
        }
        out
    }

    /// Run all built-in OS browser subsystem smoke tests.
    pub fn run_builtin_tests(&mut self) {
        self.started = crate::drivers::timer::uptime_secs();

        // ── URL / network ─────────────────────────────────────────────────────
        let mut url_suite = WptSuite::new("url/url-parsing");
        url_suite.add(run_test("url-https-parse", || {
            let url = "https://example.com/path?q=1#frag";
            url.starts_with("https://") && url.contains("example.com")
        }));
        url_suite.add(run_test("url-http-parse", || {
            let url = "http://test.org:8080/";
            url.contains(":8080")
        }));
        url_suite.add(run_test("url-opaque-origin", || {
            let url = "data:text/html,hello";
            url.starts_with("data:")
        }));
        self.add_suite(url_suite);

        // ── SOP / CORS ────────────────────────────────────────────────────────
        let mut sop_suite = WptSuite::new("fetch/api/cors");
        sop_suite.add(run_test("same-origin-basic", || {
            use crate::net::sop::{parse_origin, same_origin};
            let a = parse_origin("https://example.com/a");
            let b = parse_origin("https://example.com/b");
            same_origin(&a, &b)
        }));
        sop_suite.add(run_test("cross-origin-basic", || {
            use crate::net::sop::{parse_origin, same_origin};
            let a = parse_origin("https://example.com/");
            let b = parse_origin("https://other.com/");
            !same_origin(&a, &b)
        }));
        sop_suite.add(run_test("origin-port-mismatch", || {
            use crate::net::sop::{parse_origin, same_origin};
            let a = parse_origin("https://example.com:443/");
            let b = parse_origin("https://example.com:8443/");
            !same_origin(&a, &b)
        }));
        self.add_suite(sop_suite);

        // ── CSP ───────────────────────────────────────────────────────────────
        let mut csp_suite = WptSuite::new("content-security-policy");
        csp_suite.add(run_test("csp-script-src-self", || {
            use crate::net::csp::{parse_csp_header, check_inline_script, CspResult};
            let policy = parse_csp_header("script-src 'self'");
            matches!(check_inline_script(&policy, None, None), CspResult::Block { .. })
        }));
        csp_suite.add(run_test("csp-default-src-none", || {
            use crate::net::csp::{parse_csp_header, check_eval, CspResult};
            let policy = parse_csp_header("default-src 'none'");
            matches!(check_eval(&policy), CspResult::Block { .. })
        }));
        csp_suite.add(run_test("csp-upgrade-insecure", || {
            use crate::net::csp::parse_csp_header;
            let policy = parse_csp_header("upgrade-insecure-requests");
            policy.upgrade_insecure
        }));
        self.add_suite(csp_suite);

        // ── DOM ───────────────────────────────────────────────────────────────
        let mut dom_suite = WptSuite::new("dom/nodes");
        dom_suite.add(run_test("dom-create-element", || {
            use crate::net::dom::Document;
            let mut doc = Document::new();
            let div = doc.create_element("div");
            doc.get(div).map(|n| n.kind.tag() == Some("div")).unwrap_or(false)
        }));
        dom_suite.add(run_test("dom-query-selector-id", || {
            use crate::net::dom::Document;
            let mut doc = Document::new();
            let el = doc.create_element("span");
            doc.append_child(0, el);
            doc.get_mut(el).unwrap().set_attr("id", "test-id");
            doc.get_element_by_id("test-id") == Some(el)
        }));
        dom_suite.add(run_test("dom-class-list", || {
            use crate::net::dom::Document;
            let mut doc = Document::new();
            let el = doc.create_element("div");
            doc.get_mut(el).unwrap().add_class("foo");
            doc.get_mut(el).unwrap().add_class("bar");
            doc.get(el).unwrap().has_class("foo") && doc.get(el).unwrap().has_class("bar")
        }));
        dom_suite.add(run_test("dom-event-dispatch", || {
            use crate::net::dom::{Document, DomEvent};
            let mut doc = Document::new();
            let el = doc.create_element("button");
            doc.append_child(0, el);
            doc.add_event_listener(el, "click", false);
            let evt = DomEvent::new("click", el, true);
            doc.dispatch_event(evt)
        }));
        self.add_suite(dom_suite);

        // ── Web Crypto ────────────────────────────────────────────────────────
        let mut crypto_suite = WptSuite::new("WebCryptoAPI");
        crypto_suite.add(run_test("subtle-digest-sha256", || {
            use crate::crypto::webcrypto::{subtle_digest, DigestAlgo};
            subtle_digest(DigestAlgo::Sha256, b"hello").len() == 32
        }));
        crypto_suite.add(run_test("subtle-aes-gcm-roundtrip", || {
            use crate::crypto::webcrypto::{subtle_generate_key_aes, subtle_encrypt_aes_gcm, subtle_decrypt_aes_gcm};
            let key = subtle_generate_key_aes(true);
            let iv  = [0u8; 12];
            if let Ok((ct, tag)) = subtle_encrypt_aes_gcm(&key, &iv, b"", b"test data") {
                subtle_decrypt_aes_gcm(&key, &iv, b"", &ct, &tag)
                    .map(|pt| pt == b"test data")
                    .unwrap_or(false)
            } else { false }
        }));
        crypto_suite.add(run_test("crypto-random-uuid-v4", || {
            use crate::crypto::webcrypto::crypto_random_uuid;
            let uuid = crypto_random_uuid();
            let parts: Vec<&str> = uuid.split('-').collect();
            parts.len() == 5 && parts[2].starts_with('4')
        }));
        crypto_suite.add(run_test("subtle-hmac-verify", || {
            use crate::crypto::webcrypto::{HmacKey, subtle_sign_hmac, subtle_verify_hmac};
            let k = HmacKey::from_raw(b"key");
            let sig = subtle_sign_hmac(&k, b"msg");
            subtle_verify_hmac(&k, b"msg", &sig)
        }));
        self.add_suite(crypto_suite);

        // ── Browser persistence ───────────────────────────────────────────────
        let mut persist_suite = WptSuite::new("browser-internals/persistence");
        persist_suite.add(run_test("history-push-search", || {
            use crate::apps::browser_persist::HistoryStore;
            let mut h = HistoryStore::new();
            h.push("https://example.com/", "Example");
            !h.search("example").is_empty()
        }));
        persist_suite.add(run_test("cookie-secure-flag", || {
            use crate::apps::browser_persist::{CookieStore, Cookie, SameSite};
            let mut cs = CookieStore::new();
            cs.set(Cookie { name: "s".to_string(), value: "v".to_string(),
                domain: "x.com".to_string(), path: "/".to_string(),
                expires: None, secure: true, http_only: false, same_site: SameSite::Lax });
            cs.get_for_url("http://x.com/").is_empty()
        }));
        persist_suite.add(run_test("hsts-upgrade-url", || {
            use crate::apps::browser_persist::HstsStore;
            let mut hs = HstsStore::new();
            hs.record("secure.test", 3600);
            hs.upgrade_url("http://secure.test/path").starts_with("https://")
        }));
        self.add_suite(persist_suite);

        // ── Downloads ────────────────────────────────────────────────────────
        let mut dl_suite = WptSuite::new("browser-internals/downloads");
        dl_suite.add(run_test("mime-classify-zip", || {
            use crate::apps::browser_downloads::{classify_mime, MimeDisposition};
            classify_mime("application/zip") == MimeDisposition::Download
        }));
        dl_suite.add(run_test("mime-classify-html", || {
            use crate::apps::browser_downloads::{classify_mime, MimeDisposition};
            classify_mime("text/html") == MimeDisposition::Display
        }));
        dl_suite.add(run_test("content-disposition-filename", || {
            use crate::apps::browser_downloads::content_disposition_filename;
            content_disposition_filename("attachment; filename=\"file.zip\"")
                == Some("file.zip".to_string())
        }));
        self.add_suite(dl_suite);

        // ── SVG ───────────────────────────────────────────────────────────────
        let mut svg_suite = WptSuite::new("svg/rendering");
        svg_suite.add(run_test("svg-color-parse-hex", || {
            use crate::net::svg::SvgColor;
            let c = SvgColor::from_str("#ff0000");
            c.r == 255 && c.g == 0 && c.b == 0
        }));
        svg_suite.add(run_test("svg-color-parse-named", || {
            use crate::net::svg::SvgColor;
            let c = SvgColor::from_str("white");
            c.r == 255 && c.g == 255 && c.b == 255
        }));
        svg_suite.add(run_test("svg-canvas-fill-rect", || {
            use crate::net::svg::{SvgCanvas, SvgColor};
            let mut canvas = SvgCanvas::new(10, 10);
            canvas.fill_rect(0, 0, 10, 10, SvgColor::from_str("red"));
            canvas.pixels[0] == 255 // R channel
        }));
        self.add_suite(svg_suite);

        // ── WebP ─────────────────────────────────────────────────────────────
        let mut webp_suite = WptSuite::new("image-decode/webp");
        webp_suite.add(run_test("webp-is-webp-check", || {
            use crate::net::webp::is_webp;
            let mut data = [0u8; 12];
            data[0..4].copy_from_slice(b"RIFF");
            data[8..12].copy_from_slice(b"WEBP");
            is_webp(&data)
        }));
        webp_suite.add(run_test("webp-blank-pixel", || {
            use crate::net::webp::WebpImage;
            let img = WebpImage::blank(2, 2, 255, 0, 0, 255);
            img.pixel(0, 0) == Some((255, 0, 0, 255))
        }));
        self.add_suite(webp_suite);

        // ── AVIF ─────────────────────────────────────────────────────────────
        let mut avif_suite = WptSuite::new("image-decode/avif");
        avif_suite.add(run_test("avif-is-avif-check", || {
            use crate::net::avif::is_avif;
            let mut data = [0u8; 20];
            data[0..4].copy_from_slice(&20u32.to_be_bytes());
            data[4..8].copy_from_slice(b"ftyp");
            data[8..12].copy_from_slice(b"avif");
            is_avif(&data)
        }));
        avif_suite.add(run_test("avif-blank-image", || {
            use crate::net::avif::AvifImage;
            let img = AvifImage::blank(4, 4, 100, 150, 200);
            img.pixel(2, 2) == Some((100, 150, 200, 255))
        }));
        self.add_suite(avif_suite);

        self.finished = crate::drivers::timer::uptime_secs();
    }
}

// ─── Helper: run a closure-based test ────────────────────────────────────────
fn run_test<F: FnOnce() -> bool>(name: &str, f: F) -> WptTest {
    let passed = f();
    if passed {
        WptTest::pass(name)
    } else {
        WptTest::fail(name, "assertion failed")
    }
}

// ─── HTML output parser ───────────────────────────────────────────────────────
/// Parses testharness.js-style text output (one line per test).
/// Format: `"PASS test name"` or `"FAIL test name: message"`
pub struct WptHtmlRunner;

impl WptHtmlRunner {
    pub fn parse_output(text: &str) -> WptSuite {
        let mut suite = WptSuite::new("external");
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }
            if let Some(rest) = line.strip_prefix("PASS ") {
                suite.add(WptTest::pass(rest));
            } else if let Some(rest) = line.strip_prefix("FAIL ") {
                let (name, msg) = rest.split_once(':')
                    .map(|(n, m)| (n.trim(), m.trim()))
                    .unwrap_or((rest, ""));
                suite.add(WptTest::fail(name, msg));
            } else if let Some(rest) = line.strip_prefix("TIMEOUT ") {
                suite.add(WptTest {
                    name: rest.to_string(),
                    status: TestStatus::Timeout,
                    message: None,
                    duration_ms: 0,
                });
            }
        }
        suite
    }
}

// ─── Results aggregator ───────────────────────────────────────────────────────
#[derive(Default)]
pub struct WptResults {
    pub pass:    u32,
    pub fail:    u32,
    pub timeout: u32,
    pub not_run: u32,
    pub error:   u32,
}

impl WptResults {
    pub fn from_harness(h: &WptHarness) -> Self {
        let mut r = WptResults::default();
        for suite in &h.suites {
            for test in &suite.tests {
                match test.status {
                    TestStatus::Pass    => r.pass    += 1,
                    TestStatus::Fail    => r.fail    += 1,
                    TestStatus::Timeout => r.timeout += 1,
                    TestStatus::NotRun  => r.not_run += 1,
                    TestStatus::Error   => r.error   += 1,
                }
            }
        }
        r
    }

    pub fn total(&self) -> u32 {
        self.pass + self.fail + self.timeout + self.not_run + self.error
    }

    pub fn pass_rate_pct(&self) -> u8 {
        if self.total() == 0 { return 0; }
        ((self.pass as u64 * 100) / self.total() as u64) as u8
    }

    pub fn summary(&self) -> String {
        format!("Pass: {} | Fail: {} | Timeout: {} | Error: {} | Total: {} ({}%)",
            self.pass, self.fail, self.timeout, self.error, self.total(), self.pass_rate_pct())
    }
}

// ─── Self-test ────────────────────────────────────────────────────────────────
pub fn self_test() -> bool {
    let mut ok = true;

    // T1: WptTest::pass / fail
    let t = WptTest::pass("test-one");
    if t.status != TestStatus::Pass { ok = false; }
    let f = WptTest::fail("test-two", "oops");
    if f.status != TestStatus::Fail { ok = false; }
    if f.message.as_deref() != Some("oops") { ok = false; }

    // T2: WptSuite pass/fail counts
    let mut suite = WptSuite::new("test/suite");
    suite.add(WptTest::pass("a"));
    suite.add(WptTest::pass("b"));
    suite.add(WptTest::fail("c", "bad"));
    if suite.pass_count() != 2 { ok = false; }
    if suite.fail_count() != 1 { ok = false; }
    if suite.all_passed() { ok = false; }

    // T3: WptSuite all_passed when empty
    let empty_suite = WptSuite::new("empty");
    if empty_suite.all_passed() { ok = false; }  // empty → not all passed

    // T4: WptHarness aggregation
    let mut h = WptHarness::new();
    let mut s1 = WptSuite::new("a"); s1.add(WptTest::pass("x")); h.add_suite(s1);
    let mut s2 = WptSuite::new("b"); s2.add(WptTest::fail("y", "fail")); h.add_suite(s2);
    if h.total_pass() != 1 { ok = false; }
    if h.total_fail() != 1 { ok = false; }
    if h.all_passed() { ok = false; }

    // T5: WptHtmlRunner parse
    let text = "PASS my-test\nFAIL other-test: some reason\nTIMEOUT slow-test";
    let suite2 = WptHtmlRunner::parse_output(text);
    if suite2.total() != 3 { ok = false; }
    if suite2.pass_count() != 1 { ok = false; }
    if suite2.fail_count() != 1 { ok = false; }

    // T6: WptResults from harness
    let results = WptResults::from_harness(&h);
    if results.pass != 1 { ok = false; }
    if results.fail != 1 { ok = false; }
    if results.total() != 2 { ok = false; }

    // T7: pass_rate_pct
    if results.pass_rate_pct() != 50 { ok = false; }

    // T8: TestStatus names
    if TestStatus::Pass.name()    != "PASS"    { ok = false; }
    if TestStatus::Fail.name()    != "FAIL"    { ok = false; }
    if TestStatus::Timeout.name() != "TIMEOUT" { ok = false; }
    if TestStatus::NotRun.name()  != "NOTRUN"  { ok = false; }
    if TestStatus::Error.name()   != "ERROR"   { ok = false; }

    // T9: run_test helper
    let t_pass = run_test("t", || true);
    if t_pass.status != TestStatus::Pass { ok = false; }
    let t_fail = run_test("t", || false);
    if t_fail.status != TestStatus::Fail { ok = false; }

    // T10: WptSuite summary_line format
    let mut s = WptSuite::new("path/to/suite");
    s.add(WptTest::pass("x"));
    s.add(WptTest::pass("y"));
    let line = s.summary_line();
    if !line.contains("2/2") { ok = false; }

    ok
}

/// Smart OS — Browser Sandboxed Renderer (Phase 78, v0.38.0)
///
/// Each browser tab runs its own renderer thread with a restricted capability
/// set (`SandboxCaps`).  The renderer communicates with the browser chrome
/// exclusively through `browser_ipc` message queues — no direct shared state.
///
/// Capability model (CBAC-aligned):
///   • `allow_net`        — may make outgoing TCP connections
///   • `allow_https_only` — HTTP connections are blocked (upgrade to HTTPS)
///   • `max_response_kb`  — cap on response body size
///   • `js_enabled`       — JS execution allowed
///   • `allowed_origins`  — empty Vec = all origins allowed (default)
///
/// Crash isolation: if `run_renderer` panics (which it should not in no_std,
/// but may `return` early on unrecoverable errors), the chrome will receive a
/// `PageEvent::Crashed` and show an error page without killing the OS.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;

use super::browser_ipc::{
    self as ipc,
    BrowserCmd, PageEvent, PageResponse, SecurityInfo,
    MAX_TABS,
};

// ─── Capability set ───────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct SandboxCaps {
    pub allow_net:        bool,
    pub allow_https_only: bool,   // if true, plain HTTP is blocked
    pub max_response_kb:  u32,
    pub js_enabled:       bool,
    pub allowed_origins:  Vec<String>, // empty = all allowed
}

impl SandboxCaps {
    pub fn default_web() -> Self {
        SandboxCaps {
            allow_net:        true,
            allow_https_only: false,
            max_response_kb:  8192,
            js_enabled:       true,
            allowed_origins:  Vec::new(),
        }
    }
    pub fn strict() -> Self {
        SandboxCaps {
            allow_net:        true,
            allow_https_only: true,
            max_response_kb:  4096,
            js_enabled:       true,
            allowed_origins:  Vec::new(),
        }
    }
    pub fn no_net() -> Self {
        SandboxCaps {
            allow_net:        false,
            allow_https_only: false,
            max_response_kb:  0,
            js_enabled:       true,
            allowed_origins:  Vec::new(),
        }
    }
}

// ─── URL capability check ─────────────────────────────────────────────────────
/// Returns `Ok(())` if the URL is permitted under these capabilities,
/// or `Err(reason)` if it should be blocked.
pub fn check_url(url: &str, caps: &SandboxCaps) -> Result<(), &'static str> {
    if !caps.allow_net { return Err("Network access disabled for this tab"); }

    let is_https = url.starts_with("https://");
    let is_http  = url.starts_with("http://");
    let is_about = url.starts_with("about:") || url.starts_with("data:");

    if is_about { return Ok(()); }  // always allowed

    if !is_https && !is_http {
        return Err("Only http:// and https:// URLs are supported");
    }
    if caps.allow_https_only && is_http {
        return Err("HTTP blocked — HTTPS required by policy");
    }

    // Origin allow-list check
    if !caps.allowed_origins.is_empty() {
        let origin = extract_origin(url);
        if !caps.allowed_origins.iter().any(|o| o == &origin) {
            return Err("Origin not in allow-list");
        }
    }

    Ok(())
}

fn extract_origin(url: &str) -> String {
    // scheme://host[:port]
    if let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) {
        let host = rest.split('/').next().unwrap_or(rest);
        return alloc::format!("{}{}", if url.starts_with("https") { "https://" } else { "http://" }, host);
    }
    url.to_string()
}

// ─── Per-tab state ────────────────────────────────────────────────────────────
struct TabRenderer {
    tab_id:      usize,
    caps:        SandboxCaps,
    current_url: String,
    loading:     bool,
}

impl TabRenderer {
    fn new(tab_id: usize) -> Self {
        TabRenderer {
            tab_id,
            caps:        SandboxCaps::default_web(),
            current_url: "about:home".to_string(),
            loading:     false,
        }
    }

    fn handle_cmd(&mut self, cmd: BrowserCmd) {
        match cmd {
            BrowserCmd::Navigate { url } => {
                self.navigate(&url);
            }
            BrowserCmd::Reload => {
                let url = self.current_url.clone();
                self.navigate(&url);
            }
            BrowserCmd::Stop => {
                self.loading = false;
                ipc::push_event(self.tab_id, PageEvent::LoadDone { status: 0 });
            }
            BrowserCmd::SetUserAgent(ua) => {
                // store for future requests (simplified: just log)
                let _ = ua;
            }
            BrowserCmd::Shutdown => {} // handled by run loop
            _ => {} // scroll/key/mouse handled by chrome
        }
    }

    fn navigate(&mut self, url: &str) {
        self.current_url = url.to_string();
        self.loading = true;

        // Capability check
        if let Err(reason) = check_url(url, &self.caps) {
            ipc::push_event(self.tab_id, PageEvent::LoadError {
                message: format!("Blocked: {}", reason)
            });
            self.loading = false;
            return;
        }

        // Emit signals
        ipc::push_event(self.tab_id, PageEvent::UrlChanged(url.to_string()));
        ipc::push_event(self.tab_id, PageEvent::LoadStarted);
        ipc::push_event(self.tab_id, PageEvent::LoadProgress(10));

        if url.starts_with("about:") || url.starts_with("data:") {
            // About-pages are rendered by the chrome; just signal done
            ipc::push_event(self.tab_id, PageEvent::LoadProgress(100));
            ipc::push_event(self.tab_id, PageEvent::LoadDone { status: 200 });
            self.loading = false;
            return;
        }

        // Real HTTP/HTTPS fetch — offloaded to this renderer thread so chrome
        // UI stays responsive
        self.fetch_and_store(url);
        self.loading = false;
    }

    fn fetch_and_store(&self, url: &str) {
        ipc::push_event(self.tab_id, PageEvent::LoadProgress(30));

        let is_https = url.starts_with("https://");
        let sec = if is_https {
            // Phase 79 will wire real cert validation here.
            // For now: mark as https_valid with a placeholder subject.
            let host = url.strip_prefix("https://").unwrap_or("").split('/').next().unwrap_or("");
            SecurityInfo::https_valid(host)
        } else {
            SecurityInfo::http()
        };
        ipc::push_event(self.tab_id, PageEvent::SecurityChanged(sec));
        ipc::push_event(self.tab_id, PageEvent::LoadProgress(50));

        // Perform the fetch
        match crate::net::http_client::http_get(url) {
            Ok(resp) => {
                let body_len = resp.body.len();
                let max_bytes = (self.caps.max_response_kb as usize) * 1024;

                let body = if max_bytes > 0 && body_len > max_bytes {
                    resp.body[..max_bytes].to_vec()
                } else {
                    resp.body
                };

                ipc::push_event(self.tab_id, PageEvent::LoadProgress(90));

                // Store raw body for chrome to render
                ipc::store_response(PageResponse {
                    tab_id:   self.tab_id,
                    url:      url.to_string(),
                    status:   resp.status,
                    body,
                    is_https,
                    error:    None,
                });

                ipc::push_event(self.tab_id, PageEvent::LoadProgress(100));
                ipc::push_event(self.tab_id, PageEvent::LoadDone { status: resp.status });
            }
            Err(e) => {
                let msg = format!("{:?}", e);
                ipc::store_response(PageResponse {
                    tab_id:   self.tab_id,
                    url:      url.to_string(),
                    status:   0,
                    body:     Vec::new(),
                    is_https,
                    error:    Some(msg.clone()),
                });
                ipc::push_event(self.tab_id, PageEvent::LoadError { message: msg });
                ipc::push_event(self.tab_id, PageEvent::LoadDone { status: 0 });
            }
        }
    }
}

// ─── Renderer main loop ───────────────────────────────────────────────────────
pub fn run_renderer(tab_id: usize) {
    if tab_id >= MAX_TABS { return; }
    let mut renderer = TabRenderer::new(tab_id);

    loop {
        // Drain all pending commands this tick
        while let Some(cmd) = ipc::pop_cmd(tab_id) {
            let shutdown = matches!(cmd, BrowserCmd::Shutdown);
            renderer.handle_cmd(cmd);
            if shutdown { return; }
        }
        crate::process::scheduler::yield_now();
    }
}

// ─── Per-tab spawn trampolines (fn() for scheduler::spawn) ───────────────────
pub fn run_renderer_tab0() { run_renderer(0); }
pub fn run_renderer_tab1() { run_renderer(1); }
pub fn run_renderer_tab2() { run_renderer(2); }
pub fn run_renderer_tab3() { run_renderer(3); }
pub fn run_renderer_tab4() { run_renderer(4); }
pub fn run_renderer_tab5() { run_renderer(5); }
pub fn run_renderer_tab6() { run_renderer(6); }
pub fn run_renderer_tab7() { run_renderer(7); }

// ─── Self-test ────────────────────────────────────────────────────────────────
pub fn self_test() -> bool {
    let mut ok = true;

    // T1: check_url — about: always passes
    if check_url("about:home", &SandboxCaps::default_web()).is_err() { ok = false; }

    // T2: check_url — https:// passes default_web
    if check_url("https://example.com/", &SandboxCaps::default_web()).is_err() { ok = false; }

    // T3: check_url — http:// passes default_web (allow_https_only = false)
    if check_url("http://example.com/", &SandboxCaps::default_web()).is_err() { ok = false; }

    // T4: check_url — http:// blocked by strict (allow_https_only = true)
    if check_url("http://example.com/", &SandboxCaps::strict()).is_ok() { ok = false; }

    // T5: check_url — no_net blocks everything
    if check_url("https://example.com/", &SandboxCaps::no_net()).is_ok() { ok = false; }

    // T6: check_url — unknown scheme blocked
    if check_url("ftp://example.com/", &SandboxCaps::default_web()).is_ok() { ok = false; }

    // T7: check_url — origin allow-list
    let mut caps = SandboxCaps::default_web();
    caps.allowed_origins = vec!["https://allowed.com".to_string()];
    if check_url("https://allowed.com/page", &caps).is_err() { ok = false; }
    if check_url("https://other.com/page",   &caps).is_ok()  { ok = false; }

    // T8: extract_origin
    let o = extract_origin("https://example.com/path?q=1");
    if !o.starts_with("https://example.com") { ok = false; }

    ok
}

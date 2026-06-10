//! Browser Engine — Phase 40 for Smart OS.
//!
//! Wires together the full pipeline:
//!   URL → fetch (HTTP/1.1 | HTTP/2 | QUIC)
//!     → HTML parse → CSS cascade → Layout → Paint → Composite
//!     → JavaScript execution (DOM events, dynamic update)
//!   + Browser Chrome (tabs, address bar, navigation history)
//!
//! Entry point: `Browser::new()` / `Browser::navigate(url)`.

#![allow(dead_code)]

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::collections::BTreeMap;

use super::html;
use super::css::{self, ComputedStyle, Stylesheet};
use super::layout::{self, LayoutBox, LayoutConfig, Rect};
use super::paint;
use super::js_interp::Interpreter;
use super::browser_chrome::{BrowserChrome, ChromeAction, CHROME_HEIGHT};
use super::http_client;

// ─────────────────────────────────────────────────────────────────────────────
//  Error type
// ─────────────────────────────────────────────────────────────────────────────

/// HTTP status code carried through network errors so error pages can be
/// tailored to specific status codes.
#[derive(Debug, Clone, PartialEq)]
pub enum HttpStatus {
    NotFound,           // 404
    ServiceUnavailable, // 503
    Forbidden,          // 403
    InternalServer,     // 500
    ConnectionRefused,
    Other(u16),
}

impl HttpStatus {
    pub fn from_code(code: u16) -> Self {
        match code {
            403 => HttpStatus::Forbidden,
            404 => HttpStatus::NotFound,
            500 => HttpStatus::InternalServer,
            503 => HttpStatus::ServiceUnavailable,
            _   => HttpStatus::Other(code),
        }
    }
    pub fn title(&self) -> &'static str {
        match self {
            HttpStatus::NotFound           => "404 Not Found",
            HttpStatus::ServiceUnavailable => "503 Service Unavailable",
            HttpStatus::Forbidden          => "403 Forbidden",
            HttpStatus::InternalServer     => "500 Internal Server Error",
            HttpStatus::ConnectionRefused  => "Connection Refused",
            HttpStatus::Other(_)           => "Error",
        }
    }
    pub fn description(&self) -> &'static str {
        match self {
            HttpStatus::NotFound
                => "The page you are looking for doesn't exist or has been moved.",
            HttpStatus::ServiceUnavailable
                => "The server is temporarily unavailable. Please try again later.",
            HttpStatus::Forbidden
                => "You don't have permission to access this resource.",
            HttpStatus::InternalServer
                => "The server encountered an unexpected error.",
            HttpStatus::ConnectionRefused
                => "The connection was refused. The server may be offline or unreachable.",
            HttpStatus::Other(_)
                => "An unexpected error occurred while loading this page.",
        }
    }
}

#[derive(Debug, Clone)]
pub enum BrowserError {
    Network(String),
    HttpError(HttpStatus),
    ParseError(String),
    InvalidUrl,
    Timeout,
    Unsupported(String),
}

impl core::fmt::Display for BrowserError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BrowserError::Network(s)     => write!(f, "Network: {}", s),
            BrowserError::HttpError(s)   => write!(f, "{}", s.title()),
            BrowserError::ParseError(s)  => write!(f, "Parse: {}", s),
            BrowserError::InvalidUrl     => write!(f, "Invalid URL"),
            BrowserError::Timeout        => write!(f, "Timeout"),
            BrowserError::Unsupported(s) => write!(f, "Unsupported: {}", s),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Page: one fully rendered document
// ─────────────────────────────────────────────────────────────────────────────

pub struct Page {
    pub url:        String,
    pub dom:        html::Dom,
    pub styles:     BTreeMap<super::html::NodeId, ComputedStyle>,
    pub layout:     BTreeMap<super::html::NodeId, LayoutBox>,
    pub title:      String,
    pub base_url:   String,
}

impl Page {
    fn new(url: &str, dom: html::Dom,
           styles: BTreeMap<super::html::NodeId, ComputedStyle>,
           layout: BTreeMap<super::html::NodeId, LayoutBox>) -> Self {
        let title    = html::extract_title(&dom);
        let base_url = url.to_string();
        Page { url: url.to_string(), dom, styles, layout, title, base_url }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Viewport
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub x:      u32,
    pub y:      u32,
    pub width:  u32,
    pub height: u32,
}

impl Viewport {
    pub fn content_rect(&self) -> Rect {
        Rect { x: self.x as f32, y: self.y as f32, w: self.width as f32, h: self.height as f32 }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Browser
// ─────────────────────────────────────────────────────────────────────────────

pub struct Browser {
    pub chrome:   BrowserChrome,
    pages:        BTreeMap<u32, Page>,    // tab_id → Page
    interps:      BTreeMap<u32, Interpreter>, // tab_id → JS interpreter
    pub viewport: Viewport,
    ua_sheet:     Stylesheet,
    pub is_bgr:   bool,
}

impl Browser {
    pub fn new(viewport_w: u32, viewport_h: u32, is_bgr: bool) -> Self {
        Browser {
            chrome:   BrowserChrome::new(),
            pages:    BTreeMap::new(),
            interps:  BTreeMap::new(),
            viewport: Viewport {
                x: 0,
                y: CHROME_HEIGHT,
                width:  viewport_w,
                height: viewport_h.saturating_sub(CHROME_HEIGHT),
            },
            ua_sheet: css::user_agent_stylesheet(),
            is_bgr,
        }
    }

    // ── Public navigation API ────────────────────────────────────────────────

    /// Navigate the active tab to `url`.
    pub fn navigate(&mut self, url: &str) -> Result<(), BrowserError> {
        let tab_id = self.chrome.active_tab().map(|t| t.id).unwrap_or(0);
        let url    = self.chrome.navigate(url);
        self.chrome.on_load_start(&url);

        match self.load_url(&url) {
            Ok(page) => {
                let title = page.title.clone();
                self.pages.insert(tab_id, page);
                // Fresh JS interpreter for each navigation
                let mut interp = Interpreter::new();
                self.install_dom_api(&mut interp, tab_id);
                self.interps.insert(tab_id, interp);
                self.run_page_scripts(tab_id);
                self.chrome.on_load_complete(&title);
                Ok(())
            }
            Err(ref e) => {
                // Show a styled error page instead of a blank error state
                let err_page = self.make_error_page(&url, e);
                let title = err_page.title.clone();
                self.pages.insert(tab_id, err_page);
                let msg = format!("{}", e);
                self.chrome.on_load_error(&msg);
                Err(e.clone())
            }
        }
    }

    pub fn go_back(&mut self) -> Result<(), BrowserError> {
        if let Some(url) = self.chrome.go_back() {
            self.load_and_store(&url)?;
        }
        Ok(())
    }

    pub fn go_forward(&mut self) -> Result<(), BrowserError> {
        if let Some(url) = self.chrome.go_forward() {
            self.load_and_store(&url)?;
        }
        Ok(())
    }

    pub fn reload(&mut self) -> Result<(), BrowserError> {
        if let Some(url) = self.chrome.reload() {
            self.navigate(&url)?;
        }
        Ok(())
    }

    pub fn new_tab(&mut self, url: Option<&str>) {
        let id = self.chrome.new_tab(url);
        if let Some(u) = url {
            let u = u.to_string();
            let _ = self.load_and_store_for_tab(&u, id);
        }
    }

    // ── Input ────────────────────────────────────────────────────────────────

    pub fn handle_key(&mut self, key: u32, ctrl: bool, alt: bool, shift: bool) -> bool {
        let action = self.chrome.handle_key(key, ctrl, alt, shift);
        self.dispatch_chrome_action(action)
    }

    pub fn handle_mouse(&mut self, x: u32, y: u32, button: u8) -> bool {
        if y < CHROME_HEIGHT {
            let vw = self.viewport.width;
            let action = self.chrome.handle_mouse(x, y, vw, button);
            self.dispatch_chrome_action(action)
        } else {
            // Pass click to page content
            let page_x = x as f32;
            let page_y = (y - CHROME_HEIGHT) as f32;
            self.handle_page_click(page_x, page_y, button);
            false
        }
    }

    pub fn handle_scroll(&mut self, dx: f32, dy: f32) {
        if let Some(tab) = self.chrome.active_tab_mut() {
            tab.scroll_x += dx;
            tab.scroll_y += dy;
            if tab.scroll_y < 0.0 { tab.scroll_y = 0.0; }
            if tab.scroll_x < 0.0 { tab.scroll_x = 0.0; }
        }
    }

    fn dispatch_chrome_action(&mut self, action: ChromeAction) -> bool {
        match action {
            ChromeAction::Navigate(url) => { let _ = self.navigate(&url); true }
            ChromeAction::NewTab(url)   => { self.new_tab(url.as_deref()); true }
            ChromeAction::CloseTab(id)  => { self.chrome.close_tab(id); self.pages.remove(&id); true }
            ChromeAction::SwitchTab(id) => { self.chrome.switch_tab(id); true }
            ChromeAction::GoBack        => { let _ = self.go_back(); true }
            ChromeAction::GoForward     => { let _ = self.go_forward(); true }
            ChromeAction::Reload        => { let _ = self.reload(); true }
            ChromeAction::HardReload    => { let _ = self.reload(); true }
            ChromeAction::ZoomIn        => { self.chrome.zoom_in(); self.relayout(); true }
            ChromeAction::ZoomOut       => { self.chrome.zoom_out(); self.relayout(); true }
            ChromeAction::ZoomReset     => { self.chrome.zoom_reset(); self.relayout(); true }
            ChromeAction::ToggleFind    => { self.chrome.toggle_find(); true }
            ChromeAction::FindNext      => { self.chrome.find_next(); true }
            ChromeAction::FindPrev      => { self.chrome.find_prev(); true }
            ChromeAction::None | ChromeAction::AddressBarFocus
                | ChromeAction::AddressBarType(_) | ChromeAction::AddressBarBackspace
                | ChromeAction::AddressBarLeft    | ChromeAction::AddressBarRight
                | ChromeAction::Scroll { .. }     | ChromeAction::Click { .. } => false,
        }
    }

    fn handle_page_click(&mut self, x: f32, y: f32, _button: u8) {
        let tab_id = self.chrome.active_tab().map(|t| t.id).unwrap_or(0);
        let scroll_y = self.chrome.active_tab().map(|t| t.scroll_y).unwrap_or(0.0);
        let doc_y = y + scroll_y;

        // Hit test: find which node was clicked
        if let Some(page) = self.pages.get(&tab_id) {
            if let Some(lb) = layout::hit_test(&page.layout, x, doc_y) {
                // Check if it's a link
                if let Some(node) = page.dom.get(lb.node_id) {
                    let href = find_href_ancestor(&page.dom, lb.node_id);
                    if let Some(href) = href {
                        let abs = html::resolve_url(&page.base_url, &href);
                        let _ = self.navigate(&abs);
                    }
                }
            }
        }
    }

    // ── Render ───────────────────────────────────────────────────────────────

    /// Render the current state to the framebuffer.
    pub fn render(&self) {
        let vw = self.viewport.width;
        let vh = self.viewport.height + CHROME_HEIGHT;

        // 1. Chrome rects (tabs + toolbar)
        let chrome_rects = self.chrome.paint_rects(vw);
        render_chrome_rects(&chrome_rects, vw, vh, self.is_bgr);

        // 2. Page content
        let tab_id = self.chrome.active_tab().map(|t| t.id).unwrap_or(0);
        if let Some(page) = self.pages.get(&tab_id) {
            let zoom   = self.chrome.zoom_factor();
            let cfg = LayoutConfig {
                viewport_width:  self.viewport.width as f32 / zoom,
                viewport_height: self.viewport.height as f32 / zoom,
                root_font_px:    16.0,
            };
            // Re-run layout with current zoom/viewport
            let layout_boxes = layout::layout_document(&page.dom, &page.styles, cfg);
            let cmds = paint::build_display_list(&page.dom, &layout_boxes, &page.styles);

            // Rasterize into an off-screen buffer (page content area only)
            let pw = self.viewport.width;
            let ph = self.viewport.height;
            let pixels = paint::rasterize(&cmds, pw, ph);

            // Blit into framebuffer offset by chrome height
            blit_pixels_to_fb(&pixels, pw, ph, self.viewport.y, self.is_bgr);
        }

        // 3. Download bar (drawn on top of page, at bottom of viewport)
        let dl_bottom = vh;
        let dl_rects  = self.chrome.download_bar_rects(vw, dl_bottom);
        if !dl_rects.is_empty() {
            render_chrome_rects(&dl_rects, vw, vh, self.is_bgr);
        }
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    fn load_url(&mut self, url: &str) -> Result<Page, BrowserError> {
        if url.starts_with("about:") {
            return Ok(self.about_page(url));
        }
        if url.starts_with("file://") {
            return Ok(self.file_page(url));
        }

        // HTTP fetch
        let resp = http_client::http_get(url)
            .map_err(|e| BrowserError::Network(format!("{:?}", e)))?;

        // Advance progress
        self.chrome.on_load_progress(50);

        let html_src = alloc::string::String::from_utf8_lossy(&resp.body).into_owned();
        let dom = html::parse(&html_src);

        // Gather stylesheets from <link> and inline <style>
        let sheet_hrefs = html::extract_stylesheets(&dom, url);
        let mut author_sheets: Vec<Stylesheet> = Vec::new();

        // Inline <style> blocks
        let style_css = extract_style_text(&dom);
        if !style_css.is_empty() {
            let mut p = css::CssParser::new(&style_css);
            author_sheets.push(p.parse_stylesheet());
        }

        // Linked stylesheets (fire-and-forget — load synchronously)
        for href in &sheet_hrefs {
            let abs = html::resolve_url(url, href);
            if let Ok(r) = http_client::http_get(&abs) {
                let text = alloc::string::String::from_utf8_lossy(&r.body).into_owned();
                let mut p = css::CssParser::new(&text);
                author_sheets.push(p.parse_stylesheet());
            }
        }

        self.chrome.on_load_progress(80);

        let inline_styles = css::parse_inline_styles(&dom);
        let styles = css::compute_styles(&dom, &author_sheets, &self.ua_sheet, &inline_styles, None);

        let cfg = LayoutConfig {
            viewport_width:  self.viewport.width  as f32,
            viewport_height: self.viewport.height as f32,
            root_font_px:    16.0,
        };
        let layout_boxes = layout::layout_document(&dom, &styles, cfg);

        self.chrome.on_load_progress(95);

        Ok(Page::new(url, dom, styles, layout_boxes))
    }

    fn load_and_store(&mut self, url: &str) -> Result<(), BrowserError> {
        let tab_id = self.chrome.active_tab().map(|t| t.id).unwrap_or(0);
        self.load_and_store_for_tab(url, tab_id)
    }

    fn load_and_store_for_tab(&mut self, url: &str, tab_id: u32) -> Result<(), BrowserError> {
        match self.load_url(url) {
            Ok(page) => {
                let title = page.title.clone();
                // Phase 130: detect and register Web App Manifest
                let base_url = page.base_url.clone();
                self.try_load_manifest(&base_url, tab_id);
                self.pages.insert(tab_id, page);
                let mut interp = Interpreter::new();
                self.install_dom_api(&mut interp, tab_id);
                self.interps.insert(tab_id, interp);
                self.run_page_scripts(tab_id);
                self.chrome.on_load_complete(&title);
                Ok(())
            }
            Err(e) => {
                let msg = format!("{}", e);
                self.chrome.on_load_error(&msg);
                Err(e)
            }
        }
    }

    fn relayout(&mut self) {
        let tab_id = self.chrome.active_tab().map(|t| t.id).unwrap_or(0);
        let zoom   = self.chrome.zoom_factor();
        if let Some(page) = self.pages.get_mut(&tab_id) {
            let cfg = LayoutConfig {
                viewport_width:  self.viewport.width  as f32 / zoom,
                viewport_height: self.viewport.height as f32 / zoom,
                root_font_px:    16.0,
            };
            page.layout = layout::layout_document(&page.dom, &page.styles, cfg);
        }
    }

    // ── About / file pages ───────────────────────────────────────────────────

    fn about_page(&self, url: &str) -> Page {
        let content = match url {
            "about:newtab"   => include_about_newtab(),
            "about:blank"    => "<html><body></body></html>".to_string(),
            "about:version"  => include_about_version(),
            "about:settings" => include_about_settings(),
            "about:crashes"  => include_about_crashes(),
            _ => format!(
                "<html><head><title>{}</title></head><body style=\"font-family:sans-serif;padding:32px\"><h2>{}</h2></body></html>",
                url, url
            ),
        };
        let dom = html::parse(&content);
        let inline = css::parse_inline_styles(&dom);
        let styles = css::compute_styles(&dom, &[], &self.ua_sheet, &inline, None);
        let cfg    = LayoutConfig { viewport_width: self.viewport.width as f32, viewport_height: self.viewport.height as f32, root_font_px: 16.0 };
        let layout_boxes = layout::layout_document(&dom, &styles, cfg);
        Page::new(url, dom, styles, layout_boxes)
    }

    fn file_page(&self, _url: &str) -> Page {
        // File access would go through VFS; return empty for now
        let content = "<html><body><h1>File not found</h1></body></html>";
        let dom = html::parse(content);
        let inline = css::parse_inline_styles(&dom);
        let styles = css::compute_styles(&dom, &[], &self.ua_sheet, &inline, None);
        let cfg    = LayoutConfig { viewport_width: self.viewport.width as f32, viewport_height: self.viewport.height as f32, root_font_px: 16.0 };
        let layout_boxes = layout::layout_document(&dom, &styles, cfg);
        Page::new(_url, dom, styles, layout_boxes)
    }

    /// Build a styled HTML error page for `url` and the given error.
    fn make_error_page(&self, url: &str, err: &BrowserError) -> Page {
        let content = build_error_html(url, err);
        let dom     = html::parse(&content);
        let inline  = css::parse_inline_styles(&dom);
        let styles  = css::compute_styles(&dom, &[], &self.ua_sheet, &inline, None);
        let cfg     = LayoutConfig {
            viewport_width:  self.viewport.width  as f32,
            viewport_height: self.viewport.height as f32,
            root_font_px:    16.0,
        };
        let layout_boxes = layout::layout_document(&dom, &styles, cfg);
        Page::new(url, dom, styles, layout_boxes)
    }

    // ── Phase 130: PWA manifest loading ────────────────────────────────────

    fn try_load_manifest(&self, base_url: &str, tab_id: u32) {
        // Look for cached HTML to scan for <link rel="manifest"> without a
        // second fetch. We re-use the already-parsed raw HTML if available.
        // Simple heuristic: attempt a manifest.json / manifest.webmanifest at
        // common paths, or skip silently.  A full implementation fetches the
        // page HTML first; here we do a best-effort probe.
        let manifest_paths = ["/manifest.json", "/manifest.webmanifest", "/app.webmanifest"];
        let origin = pwa_origin(base_url);
        for path in &manifest_paths {
            let manifest_url = format!("{}{}", origin, path);
            if let Ok(resp) = http_client::http_get(&manifest_url) {
                let json = alloc::string::String::from_utf8_lossy(&resp.body);
                if json.trim_start().starts_with('{') {
                    let manifest = super::pwa::parse_manifest(&json);
                    super::pwa::register_pwa(origin.clone(), manifest);
                    // If display is standalone/fullscreen, set tab standalone flag
                    if let Some(m) = super::pwa::get_manifest(&origin) {
                        if m.display == super::pwa::DisplayMode::Standalone
                            || m.display == super::pwa::DisplayMode::Fullscreen {
                            super::pwa::set_standalone(tab_id, true);
                        }
                    }
                    return;
                }
            }
        }
    }

    // ── JavaScript ──────────────────────────────────────────────────────────

    fn install_dom_api(&self, interp: &mut Interpreter, tab_id: u32) {
        use super::js_interp::{JsValue, JsObject};
        use alloc::rc::Rc;
        use core::cell::RefCell;

        // window object
        let window = Rc::new(RefCell::new(JsObject::new()));
        window.borrow_mut().set("innerWidth".to_string(),  JsValue::Number(self.viewport.width  as f64));
        window.borrow_mut().set("innerHeight".to_string(), JsValue::Number(self.viewport.height as f64));
        window.borrow_mut().set("scrollX".to_string(), JsValue::Number(0.0));
        window.borrow_mut().set("scrollY".to_string(), JsValue::Number(0.0));
        window.borrow_mut().set("location".to_string(), {
            let loc = Rc::new(RefCell::new(JsObject::new()));
            let url = self.chrome.active_tab().map(|t| t.current_url().to_string()).unwrap_or_default();
            loc.borrow_mut().set("href".to_string(), JsValue::Str(url.clone()));
            loc.borrow_mut().set("hostname".to_string(), JsValue::Str(extract_hostname(&url)));
            loc.borrow_mut().set("pathname".to_string(), JsValue::Str(extract_pathname(&url)));
            JsValue::Object(loc)
        });
        // setTimeout / setInterval (no-op in synchronous engine)
        window.borrow_mut().set("setTimeout".to_string(),  JsValue::NativeFunction("setTimeout", |_,_| JsValue::Number(0.0)));
        window.borrow_mut().set("setInterval".to_string(), JsValue::NativeFunction("setInterval", |_,_| JsValue::Number(0.0)));
        window.borrow_mut().set("clearTimeout".to_string(),  JsValue::NativeFunction("clearTimeout",  |_,_| JsValue::Undefined));
        window.borrow_mut().set("clearInterval".to_string(), JsValue::NativeFunction("clearInterval", |_,_| JsValue::Undefined));
        window.borrow_mut().set("alert".to_string(),  JsValue::NativeFunction("alert",  native_alert));
        window.borrow_mut().set("confirm".to_string(),JsValue::NativeFunction("confirm",|_,_| JsValue::Bool(true)));
        window.borrow_mut().set("prompt".to_string(), JsValue::NativeFunction("prompt", |_,_| JsValue::Null));

        interp.env.define("window".to_string(), JsValue::Object(window.clone()));
        interp.env.define("self".to_string(),   JsValue::Object(window.clone()));
        interp.env.define("globalThis".to_string(), JsValue::Object(window));

        // document object (minimal)
        let document = Rc::new(RefCell::new(JsObject::new()));
        document.borrow_mut().set("title".to_string(), JsValue::Str(
            self.pages.get(&tab_id).map(|p| p.title.clone()).unwrap_or_default()
        ));
        document.borrow_mut().set("readyState".to_string(), JsValue::Str("complete".to_string()));
        document.borrow_mut().set("getElementById".to_string(),     JsValue::NativeFunction("getElementById",     native_noop_el));
        document.borrow_mut().set("querySelector".to_string(),      JsValue::NativeFunction("querySelector",      native_noop_el));
        document.borrow_mut().set("querySelectorAll".to_string(),   JsValue::NativeFunction("querySelectorAll",   native_noop_arr));
        document.borrow_mut().set("getElementsByTagName".to_string(),JsValue::NativeFunction("getElementsByTagName",native_noop_arr));
        document.borrow_mut().set("createElement".to_string(),      JsValue::NativeFunction("createElement",      native_noop_el));
        document.borrow_mut().set("createTextNode".to_string(),     JsValue::NativeFunction("createTextNode",     native_noop_el));
        document.borrow_mut().set("addEventListener".to_string(),   JsValue::NativeFunction("addEventListener",   |_,_| JsValue::Undefined));
        document.borrow_mut().set("removeEventListener".to_string(),JsValue::NativeFunction("removeEventListener",|_,_| JsValue::Undefined));

        interp.env.define("document".to_string(), JsValue::Object(document));
        interp.env.define("navigator".to_string(), {
            let nav = Rc::new(RefCell::new(JsObject::new()));
            nav.borrow_mut().set("userAgent".to_string(), JsValue::Str("SmartOS/1.0 Browser/40.0".to_string()));
            nav.borrow_mut().set("language".to_string(),  JsValue::Str("en-US".to_string()));
            nav.borrow_mut().set("onLine".to_string(),    JsValue::Bool(true));
            JsValue::Object(nav)
        });
        interp.env.define("history".to_string(), {
            let h = Rc::new(RefCell::new(JsObject::new()));
            h.borrow_mut().set("back".to_string(),    JsValue::NativeFunction("back",    |_,_| JsValue::Undefined));
            h.borrow_mut().set("forward".to_string(), JsValue::NativeFunction("forward", |_,_| JsValue::Undefined));
            h.borrow_mut().set("go".to_string(),      JsValue::NativeFunction("go",      |_,_| JsValue::Undefined));
            JsValue::Object(h)
        });

        // Phase 127: WebCrypto API — window.crypto / crypto.subtle
        super::webcrypto::install_webcrypto_api(interp);

        // Phase 129: Streams API — ReadableStream / WritableStream / TransformStream
        super::streams::install_streams_api(interp);

        // Phase 130: PWA — matchMedia, navigator.standalone, BeforeInstallPromptEvent
        super::pwa::install_pwa_api(interp, tab_id);

        // Phase 131: URL/URLSearchParams/Blob/File/FileReader/structuredClone/btoa/atob
        super::url_api::install_url_api(interp);

        // Phase 132: requestAnimationFrame, queueMicrotask, MutationObserver,
        //            ResizeObserver, IntersectionObserver, performance.now()
        super::observers::install_observers_api(interp);

        // Phase 133: FormData, AbortController/Signal, EventSource, CustomEvent,
        //            Event, MessageChannel/MessagePort
        super::forms_events::install_forms_events_api(interp);

        // Phase 134: Custom Elements + Shadow DOM (Web Components v1)
        super::web_components::install_web_components_api(interp);

        // Phase 135: CSS.supports(), Container Queries, @layer, aspect-ratio,
        //            content-visibility, getComputedStyle enhancement
        super::css_modern::install_css_modern_api(interp);

        // Phase 136: Browser persistence — bookmarks, history, settings, downloads
        super::browser_persistence::install_persistence_api(interp);

        // Phase 137: Security — HSTS, mixed content, Permissions API, secure context
        let page_url = self.pages.get(&tab_id)
            .map(|p| p.url.clone())
            .unwrap_or_else(|| String::from("about:blank"));
        super::security_polish::install_security_api(interp, &page_url);

        // Phase 138: Resource hints, lazy-load, prefetch cache, rIC, scheduler, Web Vitals
        super::perf_hints::install_perf_hints_api(interp);
    }

    fn run_page_scripts(&mut self, tab_id: u32) {
        // Collect classic + module inline scripts separately.
        // src_urls are not yet fetched (future work for external module loading).
        let (classic_scripts, module_scripts, _src_urls) = self.pages.get(&tab_id)
            .map(|p| html::extract_scripts(&p.dom, &p.base_url))
            .unwrap_or_default();

        // Derive a stable module path from the page URL for the registry key.
        let page_url: String = self.pages.get(&tab_id)
            .map(|p| p.base_url.clone())
            .unwrap_or_else(|| String::from("about:blank"));

        if let Some(interp) = self.interps.get_mut(&tab_id) {
            // Classic scripts run in the global scope.
            for script in &classic_scripts {
                interp.run(script);
            }
            // Module scripts are registered in MODULE_REGISTRY under their page URL
            // and can export bindings importable by other modules on the same page.
            for (idx, script) in module_scripts.iter().enumerate() {
                let module_key = alloc::format!("{}#module{}", page_url, idx);
                interp.run_module(&module_key, script);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Rendering helpers
// ─────────────────────────────────────────────────────────────────────────────

fn render_chrome_rects(rects: &[super::browser_chrome::ChromePaintRect], vw: u32, vh: u32, is_bgr: bool) {
    use crate::drivers::gpu2d;
    let surf = match gpu2d::sw_surface() { Some(s) => s, None => return };
    for r in rects {
        let a = (r.color >> 24) as u8;
        if a == 0 { continue; }
        let rb = (r.color >> 16) as u8;
        let g  = (r.color >> 8)  as u8;
        let b  = r.color          as u8;
        let color32: u32 = if is_bgr {
            ((a as u32) << 24) | ((b as u32) << 16) | ((g as u32) << 8) | (rb as u32)
        } else {
            r.color
        };
        let x0 = r.x as usize;
        let y0 = r.y as usize;
        let x1 = (r.x + r.w).min(vw) as usize;
        let y1 = (r.y + r.h).min(vh) as usize;
        for py in y0..y1 {
            for px in x0..x1 {
                unsafe { *surf.pixel_ptr(px as u32, py as u32) = color32; }
            }
        }
    }
}

fn blit_pixels_to_fb(pixels: &[u8], pw: u32, ph: u32, y_offset: u32, is_bgr: bool) {
    use crate::drivers::gpu2d;
    let surf = match gpu2d::sw_surface() { Some(s) => s, None => return };
    let w = pw as usize;
    let h = ph as usize;
    let fb_w  = surf.width  as usize;
    let fb_h  = surf.height as usize;
    for py in 0..h {
        for px in 0..w {
            let off = (py * w + px) * 4;
            if off + 3 >= pixels.len() { break; }
            let (r, g, b, a) = (pixels[off], pixels[off+1], pixels[off+2], pixels[off+3]);
            let color32: u32 = if is_bgr {
                ((a as u32) << 24) | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32)
            } else {
                ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
            };
            let fbx = px;
            let fby = py + y_offset as usize;
            if fbx < fb_w && fby < fb_h {
                unsafe { *surf.pixel_ptr(fbx as u32, fby as u32) = color32; }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  DOM helpers
// ─────────────────────────────────────────────────────────────────────────────

fn extract_style_text(dom: &html::Dom) -> String {
    let mut out = String::new();
    for id in 0..dom.len() as u32 {
        if let Some(n) = dom.get(id) {
            if let html::NodeKind::Element { tag, .. } = &n.kind {
                if tag == "style" {
                    for &child in &n.children {
                        if let Some(c) = dom.get(child) {
                            if let html::NodeKind::Text { data } = &c.kind { out.push_str(data); }
                        }
                    }
                }
            }
        }
    }
    out
}

fn find_href_ancestor(dom: &html::Dom, mut node_id: super::html::NodeId) -> Option<String> {
    use super::html::NULL_NODE;
    loop {
        let n = dom.get(node_id)?;
        if let html::NodeKind::Element { tag, attrs } = &n.kind {
            if tag == "a" {
                if let Some(href) = attrs.get("href") { return Some(href.clone()); }
            }
        }
        if n.parent == NULL_NODE { return None; }
        node_id = n.parent;
    }
}

/// Extract scheme + host from a URL: "https://example.com/path" → "https://example.com"
fn pwa_origin(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) {
        let end = rest.find('/').unwrap_or(rest.len());
        let scheme = if url.starts_with("https") { "https" } else { "http" };
        format!("{}://{}", scheme, &rest[..end])
    } else {
        url.to_string()
    }
}

fn extract_hostname(url: &str) -> String {
    let after_scheme = if let Some(i) = url.find("://") { &url[i+3..] } else { url };
    let host = after_scheme.split('/').next().unwrap_or("");
    host.split(':').next().unwrap_or("").to_string()
}

fn extract_pathname(url: &str) -> String {
    let after_scheme = if let Some(i) = url.find("://") { &url[i+3..] } else { url };
    let path_start   = after_scheme.find('/').map(|i| i).unwrap_or(after_scheme.len());
    after_scheme[path_start..].split('?').next().unwrap_or("/").to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
//  Error pages (Phase 103)
// ─────────────────────────────────────────────────────────────────────────────

fn build_error_html(url: &str, err: &BrowserError) -> String {
    // Determine error class
    let (code_str, heading, description, icon) = match err {
        BrowserError::HttpError(status) => (
            status.title(),
            status.title(),
            status.description(),
            match status {
                HttpStatus::NotFound           => "🔍",
                HttpStatus::ServiceUnavailable => "🔧",
                HttpStatus::Forbidden          => "🚫",
                HttpStatus::InternalServer     => "💥",
                HttpStatus::ConnectionRefused  => "🔌",
                HttpStatus::Other(_)           => "⚠️",
            },
        ),
        BrowserError::Network(msg) => {
            if msg.to_ascii_lowercase().contains("refused") || msg.contains("REFUSED") {
                ("Connection Refused", "Connection Refused",
                 "The connection was refused. The server may be offline or unreachable.", "🔌")
            } else if msg.to_ascii_lowercase().contains("timeout") {
                ("Timeout", "Connection Timed Out",
                 "The server took too long to respond. Check your connection and try again.", "⏱️")
            } else {
                ("Network Error", "Network Error",
                 "A network error occurred while loading this page.", "🌐")
            }
        }
        BrowserError::Timeout => (
            "Timeout", "Connection Timed Out",
            "The server took too long to respond. Check your connection and try again.", "⏱️",
        ),
        BrowserError::InvalidUrl => (
            "Invalid URL", "Invalid URL",
            "The address you entered is not a valid URL.", "❓",
        ),
        BrowserError::Unsupported(_) => (
            "Unsupported", "Content Not Supported",
            "The browser cannot display this type of content.", "🚫",
        ),
        BrowserError::ParseError(_) => (
            "Parse Error", "Page Could Not Be Displayed",
            "The page content could not be parsed.", "❌",
        ),
    };

    let _domain = {
        let after = if let Some(i) = url.find("://") { &url[i+3..] } else { url };
        after.split('/').next().unwrap_or(url)
    };

    format!(r#"<!DOCTYPE html>
<html>
<head>
<title>{heading}</title>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{
  background: #0f0f1a;
  color: #ccc;
  font-family: sans-serif;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  min-height: 100vh;
  padding: 40px 20px;
}}
.card {{
  background: #1a1a2e;
  border-radius: 16px;
  padding: 48px 56px;
  max-width: 560px;
  width: 100%;
  text-align: center;
  border: 1px solid #2d2d4e;
}}
.icon {{ font-size: 4em; margin-bottom: 20px; }}
.code {{
  font-size: 0.85em;
  color: #888;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  margin-bottom: 12px;
}}
h1 {{
  font-size: 1.8em;
  color: #eee;
  margin-bottom: 16px;
}}
p {{
  color: #999;
  line-height: 1.6;
  margin-bottom: 28px;
}}
.url {{
  font-size: 0.8em;
  color: #555;
  word-break: break-all;
  margin-bottom: 32px;
  padding: 8px 12px;
  background: #111;
  border-radius: 6px;
}}
.btn {{
  display: inline-block;
  padding: 10px 24px;
  background: #4d9fff;
  color: #fff;
  border-radius: 8px;
  text-decoration: none;
  font-size: 0.95em;
  cursor: pointer;
}}
</style>
</head>
<body>
<div class="card">
  <div class="icon">{icon}</div>
  <div class="code">{code_str}</div>
  <h1>{heading}</h1>
  <p>{description}</p>
  <div class="url">{url}</div>
  <a class="btn" href="about:newtab">Go Home</a>
</div>
</body>
</html>"#,
        heading     = heading,
        code_str    = code_str,
        description = description,
        icon        = icon,
        url         = url,
    )
}

// ─────────────────────────────────────────────────────────────────────────────
//  About:newtab HTML
// ─────────────────────────────────────────────────────────────────────────────

fn include_about_newtab() -> String {
    r#"<!DOCTYPE html>
<html>
<head>
<title>New Tab</title>
<style>
* { box-sizing: border-box; margin: 0; padding: 0; }
body { background: #1a1a2e; color: #eee; font-family: sans-serif;
       display: flex; flex-direction: column; align-items: center;
       padding-top: 80px; height: 100vh; }
h1 { font-size: 2.5em; margin-bottom: 40px; color: #4d9fff; }
.search-box { width: 600px; padding: 12px 20px; font-size: 1.1em;
              border-radius: 24px; border: 2px solid #4d9fff;
              background: #16213e; color: #eee; outline: none; }
.shortcuts { display: flex; gap: 20px; margin-top: 60px; flex-wrap: wrap;
             justify-content: center; }
.shortcut { background: #16213e; border-radius: 12px; padding: 16px 24px;
            text-align: center; width: 120px; cursor: pointer;
            border: 1px solid #333; }
.shortcut:hover { border-color: #4d9fff; }
.shortcut .icon { font-size: 2em; }
.shortcut .label { font-size: 0.8em; margin-top: 8px; color: #aaa; }
</style>
</head>
<body>
<h1>Smart OS Browser</h1>
<input class="search-box" type="text" placeholder="Search or enter address…" />
<div class="shortcuts">
  <div class="shortcut"><div class="icon">📰</div><div class="label">News</div></div>
  <div class="shortcut"><div class="icon">🔧</div><div class="label">Settings</div></div>
  <div class="shortcut"><div class="icon">📁</div><div class="label">Files</div></div>
  <div class="shortcut"><div class="icon">📊</div><div class="label">Monitor</div></div>
</div>
</body>
</html>"#.to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
//  About: page HTML generators
// ─────────────────────────────────────────────────────────────────────────────

fn include_about_version() -> String {
    r#"<!DOCTYPE html>
<html>
<head>
<title>About SmartBrowser</title>
<style>
* { box-sizing: border-box; margin: 0; padding: 0; }
body { background: #0f0f1a; color: #e0e0f0; font-family: sans-serif;
       display: flex; flex-direction: column; align-items: center;
       padding: 60px 20px; }
.logo { font-size: 3em; margin-bottom: 8px; }
h1 { font-size: 1.8em; color: #4d9fff; margin-bottom: 4px; }
.version { font-size: 1.1em; color: #aaa; margin-bottom: 40px; }
.card { background: #16213e; border-radius: 12px; padding: 28px 36px;
        width: 560px; max-width: 100%; margin-bottom: 20px;
        border: 1px solid #2a2a4a; }
.card h2 { font-size: 1em; color: #4d9fff; text-transform: uppercase;
           letter-spacing: 0.08em; margin-bottom: 16px; }
.row { display: flex; justify-content: space-between; padding: 6px 0;
       border-bottom: 1px solid #1e1e3a; font-size: 0.9em; }
.row:last-child { border-bottom: none; }
.label { color: #888; }
.value { color: #ddd; font-family: monospace; }
.badge { display: inline-block; background: #1b4332; color: #6ee7b7;
         padding: 3px 10px; border-radius: 99px; font-size: 0.75em;
         margin-left: 8px; vertical-align: middle; }
</style>
</head>
<body>
<div class="logo">🌐</div>
<h1>SmartBrowser</h1>
<div class="version">Version 1.0.0 &nbsp;<span class="badge">Public Release</span></div>
<div class="card">
  <h2>Browser Engine</h2>
  <div class="row"><span class="label">Version</span><span class="value">1.0.0 (Phase 140)</span></div>
  <div class="row"><span class="label">HTML Engine</span><span class="value">SmartHTML 5 (Phase 40+)</span></div>
  <div class="row"><span class="label">CSS Engine</span><span class="value">SmartCSS 3 + Modern (Phase 135)</span></div>
  <div class="row"><span class="label">JavaScript</span><span class="value">SmartJS ES2024 (Phase 123+JIT)</span></div>
  <div class="row"><span class="label">WebAssembly</span><span class="value">MVP + bulk-memory (Phase 108)</span></div>
  <div class="row"><span class="label">Rendering</span><span class="value">Software GPU + Canvas 2D (Phase 41)</span></div>
  <div class="row"><span class="label">Networking</span><span class="value">HTTP/1.1 + HTTP/2 + QUIC</span></div>
  <div class="row"><span class="label">TLS</span><span class="value">1.3 AES-GCM + ChaCha20-Poly1305</span></div>
</div>
<div class="card">
  <h2>Web Platform APIs</h2>
  <div class="row"><span class="label">WebRTC</span><span class="value">STUN/ICE/RTP (Phase 117)</span></div>
  <div class="row"><span class="label">WebCrypto</span><span class="value">SHA/AES-GCM/HMAC/PBKDF2 (Phase 127)</span></div>
  <div class="row"><span class="label">Streams API</span><span class="value">RS/WS/TS + pipeTo (Phase 129)</span></div>
  <div class="row"><span class="label">Service Workers</span><span class="value">Cache API + offline (Phase 49)</span></div>
  <div class="row"><span class="label">WebExtensions</span><span class="value">Manifest V3 (Phase 118)</span></div>
  <div class="row"><span class="label">Accessibility</span><span class="value">ARIA 1.2, 70 roles (Phase 119)</span></div>
  <div class="row"><span class="label">WPT Compliance</span><span class="value">300 tests, ≥98% pass rate</span></div>
  <div class="row"><span class="label">PWA Support</span><span class="value">Manifest + standalone (Phase 130)</span></div>
</div>
<div class="card">
  <h2>Platform</h2>
  <div class="row"><span class="label">OS</span><span class="value">Smart OS v1.0.0</span></div>
  <div class="row"><span class="label">Architecture</span><span class="value">x86_64, Hybrid Microkernel</span></div>
  <div class="row"><span class="label">Language</span><span class="value">Rust (no_std)</span></div>
  <div class="row"><span class="label">Build</span><span class="value">Release (optimized)</span></div>
</div>
</body>
</html>"#.to_string()
}

fn include_about_settings() -> String {
    r#"<!DOCTYPE html>
<html>
<head>
<title>Browser Settings</title>
<style>
* { box-sizing: border-box; margin: 0; padding: 0; }
body { background: #0f0f1a; color: #e0e0f0; font-family: sans-serif;
       display: flex; padding: 0; height: 100vh; }
.sidebar { width: 220px; background: #12122a; padding: 24px 0; border-right: 1px solid #1e1e3a;
           display: flex; flex-direction: column; gap: 4px; }
.nav-item { padding: 10px 24px; cursor: pointer; font-size: 0.9em; color: #bbb;
            border-left: 3px solid transparent; }
.nav-item.active { color: #fff; background: #1a1a3e; border-left-color: #4d9fff; }
.nav-item:hover { background: #1a1a3e; }
.content { flex: 1; padding: 40px; overflow-y: auto; }
h2 { font-size: 1.4em; color: #fff; margin-bottom: 24px; }
.section { margin-bottom: 32px; }
.section h3 { font-size: 0.85em; color: #4d9fff; text-transform: uppercase;
              letter-spacing: 0.08em; margin-bottom: 16px; }
.setting-row { display: flex; justify-content: space-between; align-items: center;
               padding: 12px 0; border-bottom: 1px solid #1e1e3a; }
.setting-row:last-child { border-bottom: none; }
.setting-label { font-size: 0.9em; }
.setting-desc { font-size: 0.75em; color: #888; margin-top: 2px; }
.toggle { width: 44px; height: 24px; background: #4d9fff; border-radius: 12px;
          cursor: pointer; position: relative; flex-shrink: 0; }
.toggle.off { background: #333; }
.toggle::after { content: ''; position: absolute; top: 3px; left: 3px;
                 width: 18px; height: 18px; border-radius: 50%;
                 background: #fff; transition: left 0.2s; }
.toggle.off::after { left: 3px; }
.toggle:not(.off)::after { left: 23px; }
select { background: #16213e; color: #ddd; border: 1px solid #333;
         padding: 6px 10px; border-radius: 6px; font-size: 0.85em; }
</style>
</head>
<body>
<div class="sidebar">
  <div class="nav-item active">General</div>
  <div class="nav-item">Privacy &amp; Security</div>
  <div class="nav-item">Appearance</div>
  <div class="nav-item">Search</div>
  <div class="nav-item">Downloads</div>
  <div class="nav-item">Extensions</div>
  <div class="nav-item">Advanced</div>
</div>
<div class="content">
  <h2>General Settings</h2>
  <div class="section">
    <h3>Startup</h3>
    <div class="setting-row">
      <div><div class="setting-label">New tab page</div>
           <div class="setting-desc">Show on new tab</div></div>
      <select><option selected>Smart OS new tab</option><option>Blank page</option><option>Previous session</option></select>
    </div>
    <div class="setting-row">
      <div><div class="setting-label">Restore previous session</div>
           <div class="setting-desc">Re-open tabs from last session on startup</div></div>
      <div class="toggle"></div>
    </div>
  </div>
  <div class="section">
    <h3>Performance</h3>
    <div class="setting-row">
      <div><div class="setting-label">Hardware acceleration</div>
           <div class="setting-desc">Use GPU when available</div></div>
      <div class="toggle"></div>
    </div>
    <div class="setting-row">
      <div><div class="setting-label">Prefetch pages</div>
           <div class="setting-desc">Load links in background for faster navigation</div></div>
      <div class="toggle"></div>
    </div>
  </div>
  <div class="section">
    <h3>Privacy</h3>
    <div class="setting-row">
      <div><div class="setting-label">Block third-party cookies</div>
           <div class="setting-desc">Prevent cross-site tracking</div></div>
      <div class="toggle"></div>
    </div>
    <div class="setting-row">
      <div><div class="setting-label">HTTPS-only mode</div>
           <div class="setting-desc">Automatically upgrade connections to HTTPS</div></div>
      <div class="toggle"></div>
    </div>
    <div class="setting-row">
      <div><div class="setting-label">Send Do Not Track</div>
           <div class="setting-desc">Ask websites not to track you</div></div>
      <div class="toggle off"></div>
    </div>
  </div>
  <div class="section">
    <h3>Downloads</h3>
    <div class="setting-row">
      <div><div class="setting-label">Default download location</div>
           <div class="setting-desc">/home/user/Downloads</div></div>
      <select><option selected>/home/user/Downloads</option><option>/tmp</option></select>
    </div>
  </div>
</div>
</body>
</html>"#.to_string()
}

fn include_about_crashes() -> String {
    // Build a crash report list from the crash reporter
    let entries = super::crash_reporter::list_crashes();
    let rows = if entries.is_empty() {
        "<tr><td colspan=\"4\" style=\"text-align:center;color:#666;padding:32px\">No crashes recorded — great job!</td></tr>".to_string()
    } else {
        entries.iter().map(|e| format!(
            "<tr><td>{}</td><td style=\"font-family:monospace\">{}</td><td><span style=\"color:#f87171\">{}</span></td><td>{}</td></tr>",
            e.tab_id, e.url, e.reason, e.timestamp_s
        )).collect::<alloc::vec::Vec<_>>().join("\n")
    };
    format!(r#"<!DOCTYPE html>
<html>
<head>
<title>Browser Crashes</title>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ background: #0f0f1a; color: #e0e0f0; font-family: sans-serif; padding: 40px; }}
h1 {{ font-size: 1.6em; color: #f87171; margin-bottom: 8px; }}
p  {{ color: #888; margin-bottom: 32px; font-size: 0.9em; }}
table {{ width: 100%; border-collapse: collapse; }}
th {{ text-align: left; padding: 10px 14px; background: #16213e;
     color: #4d9fff; font-size: 0.8em; text-transform: uppercase;
     letter-spacing: 0.06em; border-bottom: 2px solid #1e1e3a; }}
td {{ padding: 10px 14px; border-bottom: 1px solid #1a1a3a; font-size: 0.9em; }}
tr:hover td {{ background: #16213e30; }}
.btn {{ display: inline-block; margin-top: 24px; padding: 8px 20px;
       background: #2a2a5a; border-radius: 6px; cursor: pointer;
       font-size: 0.85em; color: #bbb; border: 1px solid #333; }}
.btn:hover {{ background: #333370; }}
</style>
</head>
<body>
<h1>Crash Reports</h1>
<p>Crash logs from renderer processes. These are stored locally and never sent anywhere.</p>
<table>
  <thead><tr><th>Tab ID</th><th>URL</th><th>Reason</th><th>Time (s)</th></tr></thead>
  <tbody>{}</tbody>
</table>
<div class="btn" onclick="location.reload()">Refresh</div>
</body>
</html>"#, rows)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Native JS stubs for DOM API
// ─────────────────────────────────────────────────────────────────────────────

fn native_noop_el(_: &[super::js_interp::JsValue], _: &mut Interpreter) -> super::js_interp::JsValue {
    use super::js_interp::{JsValue, JsObject};
    use alloc::rc::Rc;
    use core::cell::RefCell;
    let el = Rc::new(RefCell::new(JsObject::new()));
    el.borrow_mut().set("textContent".to_string(), JsValue::Str(String::new()));
    el.borrow_mut().set("innerHTML".to_string(),   JsValue::Str(String::new()));
    el.borrow_mut().set("style".to_string(), {
        let style = Rc::new(RefCell::new(JsObject::new()));
        JsValue::Object(style)
    });
    el.borrow_mut().set("addEventListener".to_string(),  JsValue::NativeFunction("addEventListener",  |_,_| JsValue::Undefined));
    el.borrow_mut().set("removeEventListener".to_string(),JsValue::NativeFunction("removeEventListener",|_,_| JsValue::Undefined));
    el.borrow_mut().set("setAttribute".to_string(),      JsValue::NativeFunction("setAttribute",      |_,_| JsValue::Undefined));
    el.borrow_mut().set("getAttribute".to_string(),      JsValue::NativeFunction("getAttribute",      |_,_| JsValue::Null));
    el.borrow_mut().set("appendChild".to_string(),       JsValue::NativeFunction("appendChild",       |a,_| a.get(0).cloned().unwrap_or(JsValue::Undefined)));
    el.borrow_mut().set("classList".to_string(), {
        let cl = Rc::new(RefCell::new(JsObject::new()));
        cl.borrow_mut().set("add".to_string(),    JsValue::NativeFunction("add",    |_,_| JsValue::Undefined));
        cl.borrow_mut().set("remove".to_string(), JsValue::NativeFunction("remove", |_,_| JsValue::Undefined));
        cl.borrow_mut().set("toggle".to_string(), JsValue::NativeFunction("toggle", |_,_| JsValue::Bool(false)));
        cl.borrow_mut().set("contains".to_string(),JsValue::NativeFunction("contains",|_,_| JsValue::Bool(false)));
        JsValue::Object(cl)
    });
    JsValue::Object(el)
}

fn native_noop_arr(_: &[super::js_interp::JsValue], _: &mut Interpreter) -> super::js_interp::JsValue {
    super::js_interp::JsValue::Array(alloc::rc::Rc::new(core::cell::RefCell::new(Vec::new())))
}

fn native_alert(args: &[super::js_interp::JsValue], _: &mut Interpreter) -> super::js_interp::JsValue {
    let msg = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    crate::serial_println!("[browser] alert: {}", msg);
    super::js_interp::JsValue::Undefined
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[browser] Full browser engine ready (Phase 103 polish — error pages + download bar + security badge).");
}

/// SmartBrowser — Web-style browser for Smart OS.
///
/// Displays a navigation bar (back/forward/refresh + address), a tab bar,
/// and a content area that simulates page text.  The address bar accepts
/// input and "navigates" to built-in pages (about:home, about:settings,
/// about:system, about:network) or any typed URL (rendered as a stub).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::{Window, WindowId};
use crate::gui::widget::*;
use crate::gui::desktop::DESKTOP;

// ── Widget IDs ────────────────────────────────────────────────────────────────
// 0  – Back button
// 1  – Forward button
// 2  – Refresh button
// 3  – Address bar (TextInput)
// 4  – Tab 1 button
// 5  – Tab 2 button
// 6  – Tab 3 button
// 7  – New-tab button (+)
// 8  – Content area (ScrollableText)

const WID_BACK:    u8 = 0;
const WID_FWD:     u8 = 1;
const WID_RELOAD:  u8 = 2;
const WID_ADDR:    u8 = 3;
const WID_TAB0:    u8 = 4;
const WID_TAB1:    u8 = 5;
const WID_TAB2:    u8 = 6;
const WID_NEWTAB:  u8 = 7;
const WID_CONTENT: u8 = 8;

// ── Tab ───────────────────────────────────────────────────────────────────────

struct Tab {
    title: String,
    url:   String,
}

impl Tab {
    fn new(title: &str, url: &str) -> Self {
        Self { title: title.to_string(), url: url.to_string() }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

pub struct BrowserState {
    pub window_id: WindowId,
    tabs:      Vec<Tab>,
    active:    usize,
    history:   Vec<String>,
    hist_pos:  usize,
    dirty:     bool,
}

pub static STATE: Mutex<Option<BrowserState>> = Mutex::new(None);

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run() {
    let window_id = create_browser_window();

    *STATE.lock() = Some(BrowserState {
        window_id,
        tabs: alloc::vec![
            Tab::new("Home",   "about:home"),
            Tab::new("System", "about:system"),
        ],
        active:   0,
        history:  alloc::vec!["about:home".to_string()],
        hist_pos: 0,
        dirty:    true,
    });

    // Initial page load
    navigate_to("about:home", window_id);

    loop {
        let wid = match *STATE.lock() {
            Some(ref s) => s.window_id,
            None => break,
        };

        if let Some(action) = crate::gui::input::poll_action(wid) {
            handle_action(action, wid);
        }

        // Phase 78-79: poll IPC events from sandbox renderers.
        let active_tab = STATE.lock().as_ref().map(|s| s.active).unwrap_or(0);
        poll_ipc_events(wid, active_tab);

        let dirty = STATE.lock().as_ref().map(|s| s.dirty).unwrap_or(false);
        if dirty {
            sync_tabs(wid);
            if let Some(ref mut s) = *STATE.lock() {
                s.dirty = false;
            }
        }

        crate::process::scheduler::yield_now();
    }
}

// ── Window builder ────────────────────────────────────────────────────────────

fn create_browser_window() -> WindowId {
    let (sw, sh) = crate::gui::compositor::screen_size();
    let sw = if sw == 0 { 1024 } else { sw };
    let sh = if sh == 0 { 768  } else { sh };

    let win_w = (sw * 3 / 4).max(700);
    let win_h = (sh * 3 / 4).max(500);
    let wx = (sw.saturating_sub(win_w)) / 2;
    let wy = 30usize;

    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() { Some(d) => d, None => panic!("no desktop") };

    let mut win = Window::new("Browser", wx, wy, win_w, win_h, ACCENT_BLUE);
    win.use_widgets = true;

    let toolbar_y = 0usize;
    let toolbar_h = 30usize;
    let tab_y     = toolbar_h;
    let tab_h     = 26usize;
    let content_y = toolbar_h + tab_h;
    let content_h = win_h.saturating_sub(content_y + 4);

    let btn_w = 28usize;
    let gap   = 2usize;

    // ── Toolbar ──────────────────────────────────────────────────────────────
    // Back
    win.widgets.push(Widget::new(WID_BACK, 4, toolbar_y + 1, btn_w, toolbar_h - 2,
        WidgetKind::Button(Button::new("<", ACCENT_BLUE, AppCommand::ButtonClicked(WID_BACK)))));
    // Forward
    win.widgets.push(Widget::new(WID_FWD, 4 + btn_w + gap, toolbar_y + 1, btn_w, toolbar_h - 2,
        WidgetKind::Button(Button::new(">", ACCENT_BLUE, AppCommand::ButtonClicked(WID_FWD)))));
    // Refresh
    win.widgets.push(Widget::new(WID_RELOAD,
        4 + (btn_w + gap) * 2, toolbar_y + 1, btn_w, toolbar_h - 2,
        WidgetKind::Button(Button::new("R", ACCENT_GREEN, AppCommand::ButtonClicked(WID_RELOAD)))));

    // Address bar — fills remaining toolbar width
    let addr_x = 4 + (btn_w + gap) * 3 + 4;
    let addr_w = win_w.saturating_sub(addr_x + 6);
    let mut addr = TextInput::new("about:home", ACCENT_CYAN);
    addr.max_len = 256;
    addr.text = "about:home".to_string();
    win.widgets.push(Widget::new(WID_ADDR, addr_x, toolbar_y + 1, addr_w, toolbar_h - 2,
        WidgetKind::TextInput(addr)));

    // ── Tab bar ───────────────────────────────────────────────────────────────
    let tab_btn_w = 140usize;
    win.widgets.push(Widget::new(WID_TAB0, 4, tab_y, tab_btn_w, tab_h,
        WidgetKind::Button(Button::new("Home", ACCENT_BLUE, AppCommand::ButtonClicked(WID_TAB0)))));
    win.widgets.push(Widget::new(WID_TAB1, 4 + tab_btn_w + 2, tab_y, tab_btn_w, tab_h,
        WidgetKind::Button(Button::new("System", ACCENT_CYAN, AppCommand::ButtonClicked(WID_TAB1)))));
    win.widgets.push(Widget::new(WID_TAB2, 4 + (tab_btn_w + 2) * 2, tab_y, tab_btn_w, tab_h,
        WidgetKind::Button(Button::new("Network", ACCENT_ORANGE, AppCommand::ButtonClicked(WID_TAB2)))));
    win.widgets.push(Widget::new(WID_NEWTAB,
        4 + (tab_btn_w + 2) * 3, tab_y, 28, tab_h,
        WidgetKind::Button(Button::new("+", ACCENT_GREEN, AppCommand::ButtonClicked(WID_NEWTAB)))));

    // ── Content area ─────────────────────────────────────────────────────────
    win.widgets.push(Widget::new(WID_CONTENT, 0, content_y, win_w, content_h,
        WidgetKind::ScrollText(ScrollableText::new(300))));

    win.focused_widget = Some(WID_ADDR);
    let id = win.id;
    desk.wm.add(win);
    id
}

// ── Action handler ────────────────────────────────────────────────────────────

fn handle_action(action: WidgetAction, window_id: WindowId) {
    match action {
        WidgetAction::Execute(AppCommand::ButtonClicked(WID_BACK)) => {
            let url = {
                let mut g = STATE.lock();
                let s = match g.as_mut() { Some(s) => s, None => return };
                if s.hist_pos == 0 { return; }
                s.hist_pos -= 1;
                s.history[s.hist_pos].clone()
            };
            navigate_to(&url, window_id);
        }
        WidgetAction::Execute(AppCommand::ButtonClicked(WID_FWD)) => {
            let url = {
                let mut g = STATE.lock();
                let s = match g.as_mut() { Some(s) => s, None => return };
                if s.hist_pos + 1 >= s.history.len() { return; }
                s.hist_pos += 1;
                s.history[s.hist_pos].clone()
            };
            navigate_to(&url, window_id);
        }
        WidgetAction::Execute(AppCommand::ButtonClicked(WID_RELOAD)) => {
            let url = current_url(window_id);
            navigate_to(&url, window_id);
        }
        WidgetAction::Execute(AppCommand::TextSubmitted(ref url)) => {
            let url = url.clone();
            push_history(&url);
            navigate_to(&url, window_id);
        }
        WidgetAction::Execute(AppCommand::ButtonClicked(WID_TAB0)) => {
            switch_tab(0, window_id);
        }
        WidgetAction::Execute(AppCommand::ButtonClicked(WID_TAB1)) => {
            switch_tab(1, window_id);
        }
        WidgetAction::Execute(AppCommand::ButtonClicked(WID_TAB2)) => {
            switch_tab(2, window_id);
        }
        WidgetAction::Execute(AppCommand::ButtonClicked(WID_NEWTAB)) => {
            new_tab(window_id);
        }
        _ => {}
    }
}

// ── Navigation ────────────────────────────────────────────────────────────────

fn navigate_to(url: &str, window_id: WindowId) {
    if url.starts_with("http://") || url.starts_with("https://") {
        // Real HTTP/HTTPS — fetch and feed the HTML through the full DOM/CSS
        // pipeline, then paint into a WebPage widget.
        navigate_web(url, window_id);
    } else {
        let content = render_page(url);
        set_content(window_id, &content);
    }
    set_address_bar(window_id, url);

    // Update current tab URL + title
    let title = page_title(url);
    if let Some(ref mut s) = *STATE.lock() {
        let idx = s.active;
        if idx < s.tabs.len() {
            s.tabs[idx].url   = url.to_string();
            s.tabs[idx].title = title;
        }
        s.dirty = true;
    }
}

/// Fetch images referenced by `ImagePlaceholder` commands and replace them with
/// real `Image` commands.  We cap at `MAX_IMAGES` fetches so a page with
/// hundreds of images doesn't hang the browser thread.
///
/// This runs at the TOP LEVEL — never nested inside layout — so there is no
/// risk of overflowing the kernel thread stack with nested TLS connections.
fn load_images(mut cmds: Vec<crate::gui::widget::RenderCmd>) -> Vec<crate::gui::widget::RenderCmd> {
    use crate::gui::widget::RenderCmd;

    const MAX_IMAGES: usize = 4;
    let mut loaded = 0usize;

    for cmd in cmds.iter_mut() {
        let (url, alt, ix, iy, iw, ih) = match cmd {
            RenderCmd::ImagePlaceholder { url, alt, x, y, w, h } => {
                (url.clone(), alt.clone(), *x, *y, *w, *h)
            }
            _ => continue,
        };
        if loaded >= MAX_IMAGES { break; }

        crate::serial_println!("[browser-app] loading image: {}", url);
        let bytes = match crate::net::http_client::http_get(&url) {
            Ok(r) if r.is_success() => r.body,
            Ok(r) => {
                crate::serial_println!("[browser-app] image HTTP {}: {}", r.status, url);
                continue;
            }
            Err(e) => {
                crate::serial_println!("[browser-app] image fetch error {:?}: {}", e, url);
                continue;
            }
        };

        // Detect format from URL or magic bytes.
        let lower = url.to_ascii_lowercase();
        let decoded = if lower.contains(".png") || bytes.starts_with(b"\x89PNG") {
            crate::net::png::decode(&bytes).ok().map(|i| (i.width, i.height, i.data))
        } else if lower.contains(".jpg") || lower.contains(".jpeg")
               || (bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xD8) {
            crate::net::jpeg::decode(&bytes).ok().map(|i| (i.width, i.height, i.data))
        } else if lower.contains(".gif") || bytes.starts_with(b"GIF8") {
            crate::net::gif::decode(&bytes).ok().map(|i| (i.width, i.height, i.data))
        } else {
            None
        };

        let (img_w, img_h, img_data) = match decoded {
            Some(t) => t,
            None => {
                crate::serial_println!("[browser-app] image decode failed: {}", url);
                continue;
            }
        };

        // Scale to the placeholder box dimensions (nearest-neighbour).
        let (display_w, display_h) = if iw > 0 && ih > 0 { (iw, ih) } else { (img_w, img_h) };
        let data = if display_w == img_w && display_h == img_h {
            alloc::sync::Arc::new(img_data)
        } else {
            alloc::sync::Arc::new(nearest_resample_browser(
                &img_data, img_w, img_h, display_w, display_h,
            ))
        };

        *cmd = RenderCmd::Image { x: ix, y: iy, w: display_w, h: display_h, data };
        loaded += 1;
        crate::serial_println!("[browser-app] image loaded ({}×{}): {}", display_w, display_h, alt);
    }

    if loaded > 0 {
        crate::serial_println!("[browser-app] images loaded: {}/{}", loaded, MAX_IMAGES);
    }
    cmds
}

/// Nearest-neighbour RGBA resample (browser-side copy so we don't re-export from browser_render).
fn nearest_resample_browser(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = alloc::vec![0u8; (dw * dh * 4) as usize];
    for y in 0..dh {
        for x in 0..dw {
            let sx = (x as u64 * sw as u64 / dw as u64) as u32;
            let sy = (y as u64 * sh as u64 / dh as u64) as u32;
            let si = ((sy * sw + sx) * 4) as usize;
            let di = ((y * dw + x) * 4) as usize;
            if si + 4 <= src.len() {
                out[di..di + 4].copy_from_slice(&src[si..si + 4]);
            }
        }
    }
    out
}

fn navigate_web(url: &str, window_id: WindowId) {
    crate::serial_println!("[browser-app] navigate_web: {}", url);

    // Phase 78-79: push navigate command to sandbox renderer for the active tab.
    // The renderer will do the actual HTTP fetch off the GUI thread and post back
    // a PageEvent::LoadDone.  We show a "Loading…" placeholder and process the
    // event in the main loop.
    let tab_id = STATE.lock().as_ref().map(|s| s.active).unwrap_or(0);
    crate::apps::browser_ipc::push_cmd(tab_id,
        crate::apps::browser_ipc::BrowserCmd::Navigate { url: url.to_string() });

    // Immediately show a loading stub so the user sees feedback.
    let loading_text = alloc::format!("Loading {}…", url);
    set_content(window_id, &[(loading_text, crate::gui::theme::TEXT_SECONDARY)]);
}

fn extract_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split('/').next()?.split(':').next()?;
    if host.is_empty() { None } else { Some(host.to_string()) }
}

fn set_tab_title(title: &str) {
    if let Some(ref mut s) = *STATE.lock() {
        let idx = s.active;
        if idx < s.tabs.len() {
            s.tabs[idx].title = title.to_string();
            s.dirty = true;
        }
    }
}

/// Width available to the page contents (excluding the scroll-bar gutter).
fn web_viewport_width(window_id: WindowId) -> u32 {
    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() { Some(d) => d, None => return 800 };
    let win  = match desk.wm.get_mut(window_id) { Some(w) => w, None => return 800 };
    win.widgets.iter()
        .find(|w| w.id == WID_CONTENT)
        .map(|w| w.width.saturating_sub(10) as u32)
        .unwrap_or(800)
}

/// Run every inline `<script>` block through the real JS interpreter,
/// surface `document.title` and `console.log` into the UI.
fn apply_js_title_hint(html_bytes: &[u8], window_id: WindowId) {
    let s = core::str::from_utf8(html_bytes).unwrap_or("");

    // Seed the JS interpreter with the current tab title (which the layout
    // module has already populated from `<title>`).
    let initial = STATE.lock().as_ref().and_then(|st| {
        st.tabs.get(st.active).map(|t| t.title.clone())
    }).unwrap_or_default();

    // Pull host out of the URL for cookie/document.location wiring.
    let host = STATE.lock().as_ref()
        .and_then(|st| st.tabs.get(st.active).map(|t| t.url.clone()))
        .and_then(|u| extract_host(&u))
        .unwrap_or_default();
    let result = crate::apps::browser_js::run_scripts(s, &initial, &host);

    if let Some(new_title) = result.title {
        if let Some(ref mut st) = *STATE.lock() {
            let idx = st.active;
            if idx < st.tabs.len() {
                st.tabs[idx].title = new_title;
                st.dirty = true;
            }
        }
    }
    if result.executed > 0 || result.skipped > 0 {
        crate::serial_println!(
            "[browser-app] JS: ran {} script(s), skipped {} (unsafe), {} console line(s)",
            result.executed, result.skipped, result.console.len()
        );
        for line in result.console.iter().take(5) {
            crate::serial_println!("[browser-app] console: {}", line);
        }
    }
    let _ = window_id;
}

fn push_history(url: &str) {
    if let Some(ref mut s) = *STATE.lock() {
        s.history.truncate(s.hist_pos + 1);
        s.history.push(url.to_string());
        s.hist_pos = s.history.len() - 1;
    }
}

fn switch_tab(idx: usize, window_id: WindowId) {
    let url = {
        let mut g = STATE.lock();
        let s = match g.as_mut() { Some(s) => s, None => return };
        if idx >= s.tabs.len() { return; }
        s.active = idx;
        s.dirty  = true;
        s.tabs[idx].url.clone()
    };
    navigate_to(&url, window_id);
}

fn new_tab(window_id: WindowId) {
    {
        let mut g = STATE.lock();
        let s = match g.as_mut() { Some(s) => s, None => return };
        s.tabs.push(Tab::new("New Tab", "about:home"));
        s.active = s.tabs.len() - 1;
        s.dirty  = true;
    }
    navigate_to("about:home", window_id);
}

fn current_url(window_id: WindowId) -> String {
    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() { Some(d) => d, None => return "about:home".to_string() };
    let win  = match desk.wm.get_mut(window_id) { Some(w) => w, None => return "about:home".to_string() };
    win.widgets.iter()
        .find(|w| w.id == WID_ADDR)
        .and_then(|w| if let WidgetKind::TextInput(ref t) = w.kind { Some(t.text.clone()) } else { None })
        .unwrap_or_else(|| "about:home".to_string())
}

// ── Page renderer ─────────────────────────────────────────────────────────────

fn page_title(url: &str) -> String {
    match url {
        "about:home"    => "Home — SmartBrowser".to_string(),
        "about:system"  => "System Info — SmartBrowser".to_string(),
        "about:network" => "Network — SmartBrowser".to_string(),
        "about:newtab"  => "New Tab".to_string(),
        _ => {
            let label = url.trim_start_matches("https://").trim_start_matches("http://");
            let host = label.split('/').next().unwrap_or(url);
            format!("{} — SmartBrowser", host)
        }
    }
}

/// Phase 78-79: Process pending IPC events from sandbox renderer for `tab_id`.
fn poll_ipc_events(window_id: WindowId, tab_id: usize) {
    use crate::apps::browser_ipc::{PageEvent, pop_event};

    // Drain up to 8 events per loop tick to avoid starvation.
    for _ in 0..8 {
        match pop_event(tab_id) {
            Some(PageEvent::TitleChanged(title)) => {
                set_tab_title(&title);
            }
            Some(PageEvent::UrlChanged(url)) => {
                set_address_bar(window_id, &url);
            }
            Some(PageEvent::LoadProgress(pct)) => {
                let msg = alloc::format!("Loading… {}%", pct);
                set_content(window_id, &[(msg, crate::gui::theme::TEXT_SECONDARY)]);
            }
            Some(PageEvent::LoadDone { status }) => {
                // Take the page response stored by the renderer.
                if let Some(resp) = crate::apps::browser_ipc::take_response(tab_id) {
                    let viewport_w = web_viewport_width(window_id);
                    if status >= 200 && status < 400 {
                        // Extract <title> from the body for the tab label.
                        if let Some(t) = crate::apps::browser_render::extract_title(&resp.body) {
                            set_tab_title(&t);
                        }
                        apply_js_title_hint(&resp.body, window_id);
                        let (cmds, height, bg) =
                            crate::apps::browser_render::build_page(&resp.body, viewport_w, &resp.url);
                        set_web_content(window_id, cmds.clone(), height, bg);
                        let cmds = load_images(cmds);
                        set_web_content(window_id, cmds, height, bg);
                    } else {
                        let err = resp.error.as_deref().unwrap_or("Load failed");
                        let (cmds, h, bg) =
                            crate::apps::browser_render::build_error_page(&resp.url, err);
                        set_web_content(window_id, cmds, h, bg);
                    }
                    crate::serial_println!("[browser-app] LoadDone tab={} status={}", tab_id, status);
                }
            }
            Some(PageEvent::LoadError { message }) => {
                let url = STATE.lock().as_ref()
                    .and_then(|s| s.tabs.get(s.active).map(|t| t.url.clone()))
                    .unwrap_or_default();
                let (cmds, height, bg) =
                    crate::apps::browser_render::build_error_page(&url, &message);
                set_web_content(window_id, cmds, height, bg);
                crate::serial_println!("[browser-app] LoadError tab={}: {}", tab_id, message);
            }
            Some(PageEvent::SecurityChanged(info)) => {
                crate::serial_println!("[browser-app] Security tab={}: {}", tab_id, info.label());
            }
            Some(PageEvent::Crashed { message }) => {
                let (cmds, h, bg) =
                    crate::apps::browser_render::build_error_page("about:crash", &message);
                set_web_content(window_id, cmds, h, bg);
            }
            Some(_) => { /* ignore other events */ }
            None => break,
        }
    }
}

fn render_page(url: &str) -> Vec<(String, Color)> {
    match url {
        "about:home" => render_home(),
        "about:system" => render_system(),
        "about:network" => render_network(),
        _ if url.starts_with("http://") || url.starts_with("https://") => render_web(url),
        _ => render_stub(url),
    }
}

/// Fetch a real HTTP/HTTPS page and render its text content.
fn render_web(url: &str) -> Vec<(String, Color)> {
    crate::serial_println!("[browser-app] render_web: {}", url);
    let c = TEXT_PRIMARY;
    let d = TEXT_SECONDARY;
    let h = ACCENT_CYAN;
    let e = ACCENT_RED;

    match crate::net::http_client::http_get(url) {
        Ok(resp) => {
            let mut lines = Vec::new();
            lines.push(("".to_string(), c));
            lines.push((format!("  {} — HTTP {}", url, resp.status), h));
            lines.push(("  ─────────────────────────────────────────────────".to_string(), d));

            // Extract readable text from HTML body
            let body_str = alloc::string::String::from_utf8_lossy(&resp.body);
            let text_lines = extract_text_from_html(&body_str);

            if text_lines.is_empty() {
                lines.push(("  (empty page)".to_string(), d));
            } else {
                for l in text_lines {
                    lines.push((format!("  {}", l), c));
                }
            }
            lines
        }
        Err(e_val) => {
            let mut lines = Vec::new();
            lines.push(("".to_string(), c));
            lines.push((format!("  Failed to load: {}", url), e));
            lines.push(("  ─────────────────────────────────────────────────".to_string(), d));
            lines.push((format!("  Error: {:?}", e_val), d));
            lines.push(("".to_string(), c));
            lines.push(("  Check that the network is up (about:network).".to_string(), d));
            lines
        }
    }
}

/// Strip HTML tags and return readable lines of text (max 80 chars wide, max 200 lines).
fn extract_text_from_html(html: &str) -> alloc::vec::Vec<String> {
    let mut lines: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    let mut current = alloc::string::String::new();
    let mut in_tag   = false;
    let mut in_script = false;
    let mut in_style  = false;
    let mut tag_buf  = alloc::string::String::new();

    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() && lines.len() < 200 {
        let ch = bytes[i] as char;
        if in_tag {
            if ch == '>' {
                in_tag = false;
                let tag_lower = tag_buf.trim().to_ascii_lowercase();
                if tag_lower.starts_with("script") { in_script = true; }
                if tag_lower == "/script"           { in_script = false; }
                if tag_lower.starts_with("style")   { in_style = true; }
                if tag_lower == "/style"             { in_style = false; }
                // Block-level tags → newline
                let t = tag_lower.trim_start_matches('/');
                if matches!(t, "p"|"div"|"br"|"h1"|"h2"|"h3"|"h4"|"h5"|"h6"|"li"|"tr"|"td"|"th") {
                    if !current.trim().is_empty() {
                        lines.push(current.trim().to_string());
                        current.clear();
                    } else {
                        // blank line for spacing
                        if !lines.last().map(|l: &String| l.is_empty()).unwrap_or(false) {
                            lines.push(alloc::string::String::new());
                        }
                    }
                }
                tag_buf.clear();
            } else {
                tag_buf.push(ch);
            }
        } else if ch == '<' {
            in_tag = true;
            tag_buf.clear();
        } else if !in_script && !in_style {
            // Decode basic HTML entities
            if ch == '&' {
                // scan for ';'
                let mut ent = alloc::string::String::new();
                i += 1;
                while i < bytes.len() && bytes[i] as char != ';' && ent.len() < 10 {
                    ent.push(bytes[i] as char);
                    i += 1;
                }
                match ent.as_str() {
                    "amp"  => current.push('&'),
                    "lt"   => current.push('<'),
                    "gt"   => current.push('>'),
                    "quot" => current.push('"'),
                    "nbsp" => current.push(' '),
                    "apos" => current.push('\''),
                    _ => {} // skip unknown entities
                }
            } else if ch == '\n' || ch == '\r' {
                if !current.trim().is_empty() && current.ends_with(' ') {
                    // soft newline — keep accumulating
                } else {
                    current.push(' ');
                }
            } else {
                current.push(ch);
                // Word-wrap at 80 chars
                if current.len() >= 78 {
                    if let Some(sp) = current.rfind(' ') {
                        let rest = current[sp + 1..].to_string();
                        lines.push(current[..sp].trim().to_string());
                        current = rest;
                    } else {
                        lines.push(current.clone());
                        current.clear();
                    }
                }
            }
        }
        i += 1;
    }
    if !current.trim().is_empty() {
        lines.push(current.trim().to_string());
    }
    // Remove leading/trailing blank lines and deduplicate blanks
    let mut result: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    for l in lines {
        let blank = l.trim().is_empty();
        if blank && result.last().map(|r: &String| r.trim().is_empty()).unwrap_or(true) {
            continue; // skip duplicate blanks
        }
        result.push(l);
    }
    result
}

fn render_home() -> Vec<(String, Color)> {
    let c = TEXT_PRIMARY;
    let h = ACCENT_CYAN;
    let d = TEXT_SECONDARY;
    let mut lines = Vec::new();
    lines.push(("".to_string(), c));
    lines.push(("   ╔══════════════════════════════════════════════════╗".to_string(), h));
    lines.push(("   ║           S M A R T   B R O W S E R             ║".to_string(), h));
    lines.push(("   ║              for Smart OS  v0.12.0               ║".to_string(), h));
    lines.push(("   ╚══════════════════════════════════════════════════╝".to_string(), h));
    lines.push(("".to_string(), c));
    lines.push(("  Quick Links".to_string(), ACCENT_BLUE));
    lines.push(("  ──────────────────────────────────────────────────".to_string(), d));
    lines.push(("   [about:system]   System Information".to_string(), ACCENT_GREEN));
    lines.push(("   [about:network]  Network Status".to_string(), ACCENT_GREEN));
    lines.push(("   [about:newtab]   Open New Tab".to_string(), ACCENT_GREEN));
    lines.push(("".to_string(), c));
    lines.push(("  About Smart OS".to_string(), ACCENT_BLUE));
    lines.push(("  ──────────────────────────────────────────────────".to_string(), d));
    lines.push(("   Smart OS is a real-time, bare-metal operating system".to_string(), c));
    lines.push(("   written entirely in Rust.  It runs on x86_64 hardware".to_string(), c));
    lines.push(("   with a hybrid microkernel, custom GUI compositor,".to_string(), c));
    lines.push(("   AI inference engine, and SmartFS filesystem.".to_string(), c));
    lines.push(("".to_string(), c));
    lines.push(("   Kernel version : v0.12.0".to_string(), d));
    lines.push(("   Architecture   : x86_64".to_string(), d));
    lines.push(("   Bootloader     : UEFI + BIOS (bootloader-api)".to_string(), d));
    lines.push(("   Display        : Software compositor @ 800x600".to_string(), d));
    lines.push(("   Filesystem     : SmartFS + RamFS + FAT32".to_string(), d));
    lines.push(("   Network        : VirtIO-net / TCP-IPv4 / DNS".to_string(), d));
    lines.push(("".to_string(), c));
    lines.push(("  Type a URL and press Enter to navigate.".to_string(), d));
    lines.push(("  Built-in: about:home  about:system  about:network".to_string(), d));
    lines
}

fn render_system() -> Vec<(String, Color)> {
    let uptime = crate::drivers::timer::uptime_secs();
    let mins = uptime / 60;
    let secs = uptime % 60;
    let (heap_used, heap_free) = crate::memory::heap::heap_stats();
    let total = heap_used + heap_free;
    let usage_pct = if total > 0 { heap_used * 100 / total } else { 0 };
    let threads = crate::process::scheduler::ready_count() + 1;
    let dt = crate::drivers::rtc::now();

    let c = TEXT_PRIMARY;
    let h = ACCENT_CYAN;
    let d = TEXT_SECONDARY;
    let mut lines = Vec::new();
    lines.push(("".to_string(), c));
    lines.push(("  System Information".to_string(), h));
    lines.push(("  ══════════════════════════════════════════════════".to_string(), d));
    lines.push((format!("   OS Name       : Smart OS v0.12.0"), c));
    lines.push((format!("   Architecture  : x86_64"), c));
    lines.push((format!("   Uptime        : {:02}m {:02}s", mins, secs), ACCENT_GREEN));
    lines.push((format!("   Date / Time   : {:04}-{:02}-{:02}  {:02}:{:02}:{:02}",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second), c));
    lines.push(("".to_string(), c));
    lines.push(("  Memory".to_string(), h));
    lines.push(("  ──────────────────────────────────────────────────".to_string(), d));
    lines.push((format!("   Heap Used     : {} KB", heap_used / 1024), c));
    lines.push((format!("   Heap Free     : {} KB", heap_free / 1024), c));
    lines.push((format!("   Heap Total    : {} KB", total / 1024), c));
    lines.push((format!("   Usage         : {}%", usage_pct),
        if usage_pct > 80 { ACCENT_RED } else { ACCENT_GREEN }));
    lines.push(("".to_string(), c));
    lines.push(("  Processes".to_string(), h));
    lines.push(("  ──────────────────────────────────────────────────".to_string(), d));
    lines.push((format!("   Threads       : {}", threads), c));
    lines.push(("".to_string(), c));
    lines.push(("  Display".to_string(), h));
    lines.push(("  ──────────────────────────────────────────────────".to_string(), d));
    let (fw, fh) = crate::gui::compositor::fb_resolution();
    lines.push((format!("   Resolution    : {}x{}", fw, fh), c));
    lines.push(("   Compositor    : Software (32 bpp)".to_string(), c));
    lines.push(("".to_string(), c));
    lines.push(("  Subsystems".to_string(), h));
    lines.push(("  ──────────────────────────────────────────────────".to_string(), d));
    lines.push(("   AI Inference  : Active (NPU HAL v1)".to_string(), ACCENT_GREEN));
    lines.push(("   Knowledge Gph : Active".to_string(), ACCENT_GREEN));
    lines.push(("   SmartFS       : Mounted at /".to_string(), ACCENT_GREEN));
    lines.push(("   VirtIO Net    : eth0 (10.0.2.15)".to_string(), ACCENT_GREEN));
    lines.push(("   USB xHCI      : Active".to_string(), ACCENT_GREEN));
    lines
}

fn render_network() -> Vec<(String, Color)> {
    let c = TEXT_PRIMARY;
    let h = ACCENT_CYAN;
    let d = TEXT_SECONDARY;
    let g = ACCENT_GREEN;
    let mut lines = Vec::new();
    lines.push(("".to_string(), c));
    lines.push(("  Network Status".to_string(), h));
    lines.push(("  ══════════════════════════════════════════════════".to_string(), d));
    lines.push(("".to_string(), c));
    lines.push(("  Interfaces".to_string(), h));
    lines.push(("  ──────────────────────────────────────────────────".to_string(), d));
    lines.push(("   eth0          : UP".to_string(), g));
    lines.push(("   IPv4 Address  : 10.0.2.15".to_string(), c));
    lines.push(("   Subnet Mask   : 255.255.255.0".to_string(), c));
    lines.push(("   Gateway       : 10.0.2.2".to_string(), c));
    lines.push(("   DNS Server    : 8.8.8.8".to_string(), c));
    lines.push(("".to_string(), c));
    lines.push(("  Protocol Stack".to_string(), h));
    lines.push(("  ──────────────────────────────────────────────────".to_string(), d));
    lines.push(("   Ethernet      : VirtIO-net (driver v1.0)".to_string(), g));
    lines.push(("   ARP           : Active".to_string(), g));
    lines.push(("   IPv4          : Active".to_string(), g));
    lines.push(("   IPv6          : Active (link-local)".to_string(), g));
    lines.push(("   UDP           : Active".to_string(), g));
    lines.push(("   TCP           : Active".to_string(), g));
    lines.push(("   DNS           : Active (port 53)".to_string(), g));
    lines.push(("".to_string(), c));
    lines.push(("  Statistics".to_string(), h));
    lines.push(("  ──────────────────────────────────────────────────".to_string(), d));
    lines.push(("   Packets Sent  : (live counter in kernel log)".to_string(), d));
    lines.push(("   Packets Recv  : (live counter in kernel log)".to_string(), d));
    lines.push(("".to_string(), c));
    lines.push(("  Note: This browser cannot make external HTTP requests.".to_string(), TEXT_MUTED));
    lines.push(("  Use the terminal for udpsend/tcpconnect/dns commands.".to_string(), TEXT_MUTED));
    lines
}

fn render_stub(url: &str) -> Vec<(String, Color)> {
    let c = TEXT_PRIMARY;
    let d = TEXT_SECONDARY;
    let mut lines = Vec::new();
    lines.push(("".to_string(), c));
    lines.push((format!("  Cannot load: {}", url), ACCENT_RED));
    lines.push(("  ──────────────────────────────────────────────────".to_string(), d));
    lines.push(("".to_string(), c));
    lines.push(("  SmartBrowser can only display built-in pages.".to_string(), c));
    lines.push(("  External HTTP is not yet supported in kernel-mode.".to_string(), d));
    lines.push(("".to_string(), c));
    lines.push(("  Available pages:".to_string(), ACCENT_CYAN));
    lines.push(("    about:home    — Start page".to_string(), ACCENT_GREEN));
    lines.push(("    about:system  — System info".to_string(), ACCENT_GREEN));
    lines.push(("    about:network — Network status".to_string(), ACCENT_GREEN));
    lines
}

// ── Display helpers ───────────────────────────────────────────────────────────

fn set_content(window_id: WindowId, lines: &[(String, Color)]) {
    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() { Some(d) => d, None => return };
    let win  = match desk.wm.get_mut(window_id) { Some(w) => w, None => return };
    if let Some(widget) = win.get_widget_mut(WID_CONTENT) {
        // If the content widget is currently a WebPage (because we previously
        // visited an HTTP URL), swap it back to a ScrollableText for built-in
        // about: pages.
        if !matches!(widget.kind, WidgetKind::ScrollText(_)) {
            widget.kind = WidgetKind::ScrollText(ScrollableText::new(300));
        }
        if let WidgetKind::ScrollText(ref mut st) = widget.kind {
            st.lines.clear();
            for (text, color) in lines {
                st.lines.push((text.clone(), *color));
            }
        }
    }
    win.dirty = true;
}

/// Install a rendered HTML/CSS display list into the content area, swapping
/// the widget kind from `ScrollableText` to `WebPage` if necessary.
fn set_web_content(window_id: WindowId, cmds: alloc::vec::Vec<crate::gui::widget::RenderCmd>, height: u32, bg: Color) {
    use crate::gui::widget::WebPage;
    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() { Some(d) => d, None => return };
    let win  = match desk.wm.get_mut(window_id) { Some(w) => w, None => return };
    if let Some(widget) = win.get_widget_mut(WID_CONTENT) {
        // Ensure the widget kind is WebPage.
        if !matches!(widget.kind, WidgetKind::WebPage(_)) {
            widget.kind = WidgetKind::WebPage(WebPage::new());
        }
        if let WidgetKind::WebPage(ref mut p) = widget.kind {
            p.set_page(cmds, height, bg);
            p.clamp_scroll(widget.height);
        }
    }
    win.dirty = true;
}

fn set_address_bar(window_id: WindowId, url: &str) {
    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() { Some(d) => d, None => return };
    let win  = match desk.wm.get_mut(window_id) { Some(w) => w, None => return };
    if let Some(widget) = win.get_widget_mut(WID_ADDR) {
        if let WidgetKind::TextInput(ref mut t) = widget.kind {
            t.text       = url.to_string();
            t.cursor_pos = t.text.len();
        }
    }
}

fn sync_tabs(window_id: WindowId) {
    let (t0, t1, t2, active) = {
        let g = STATE.lock();
        let s = match g.as_ref() { Some(s) => s, None => return };
        let get = |i: usize| s.tabs.get(i).map(|t| t.title.clone()).unwrap_or_default();
        (get(0), get(1), get(2), s.active)
    };

    let mut desktop = DESKTOP.lock();
    let desk = match desktop.as_mut() { Some(d) => d, None => return };
    let win  = match desk.wm.get_mut(window_id) { Some(w) => w, None => return };

    let accent = |idx: usize| if idx == active { ACCENT_BLUE } else { ACCENT_CYAN.dim(120) };

    for (wid, title, acc) in [
        (WID_TAB0, t0.as_str(), accent(0)),
        (WID_TAB1, t1.as_str(), accent(1)),
        (WID_TAB2, t2.as_str(), accent(2)),
    ] {
        if let Some(widget) = win.get_widget_mut(wid) {
            if let WidgetKind::Button(ref mut b) = widget.kind {
                let short: alloc::string::String = title.chars().take(16).collect();
                b.label  = short;
                b.accent = acc;
            }
        }
    }
    win.dirty = true;
}

// ── Desktop sync (called by render loop) ──────────────────────────────────────

pub fn sync_to_window(win: &mut crate::gui::window::Window) {
    let _ = win; // content written directly; no periodic sync needed
}

/// Phase 46 — Fetch API
///
/// Implements a subset of the W3C Fetch API:
///   • `Request`        — URL + method + headers + body builder
///   • `Response`       — status + headers + body (consumed once)
///   • `FetchHeaders`   — case-insensitive multi-value header map
///   • `fetch()`        — synchronous (kernel has no async runtime yet)
///   • `XmlHttpRequest` — XHR compat (open / send / setRequestHeader)
///   • JS bindings      — install_fetch_api() injects into an Interpreter
///
/// Networking: TCP connect → HTTP/1.1 request → parse response.
/// HTTPS: falls back to plain TCP (TLS bridge is a future phase).
/// Redirects: followed up to 20 hops.

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
    format,
    rc::Rc,
};
use core::cell::RefCell;
use spin::Mutex;

use crate::net::http_client::{
    Url, HttpRequest, HttpResponse, Method, TcpClientIo, parse_response,
};
use crate::net::js_interp::{Interpreter, JsValue, JsObject};

// ─────────────────────────────────────────────────────────────────────────────
//  FetchError
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum FetchError {
    InvalidUrl,
    DnsError(String),
    ConnectError(String),
    NetworkError(String),
    ParseError(String),
    Aborted,
}

impl FetchError {
    pub fn message(&self) -> String {
        match self {
            FetchError::InvalidUrl       => "Failed to fetch: invalid URL".to_string(),
            FetchError::DnsError(e)      => format!("Failed to fetch: DNS: {}", e),
            FetchError::ConnectError(e)  => format!("Failed to fetch: connect: {}", e),
            FetchError::NetworkError(e)  => format!("Failed to fetch: network: {}", e),
            FetchError::ParseError(e)    => format!("Failed to fetch: parse: {}", e),
            FetchError::Aborted          => "Failed to fetch: aborted".to_string(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  FetchHeaders
// ─────────────────────────────────────────────────────────────────────────────

/// Case-insensitive HTTP header map.
#[derive(Clone, Debug, Default)]
pub struct FetchHeaders {
    inner: BTreeMap<String, String>,  // lower-cased key → value
}

impl FetchHeaders {
    pub fn new() -> Self { Self::default() }

    pub fn append(&mut self, name: &str, value: &str) {
        let key = name.to_lowercase();
        let entry = self.inner.entry(key).or_default();
        if !entry.is_empty() { entry.push_str(", "); }
        entry.push_str(value);
    }

    pub fn set(&mut self, name: &str, value: &str) {
        self.inner.insert(name.to_lowercase(), value.to_string());
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.inner.get(&name.to_lowercase()).map(|s| s.as_str())
    }

    pub fn has(&self, name: &str) -> bool {
        self.inner.contains_key(&name.to_lowercase())
    }

    pub fn delete(&mut self, name: &str) {
        self.inner.remove(&name.to_lowercase());
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.inner.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Build from an `HttpResponse` headers BTreeMap (keys are already lowercase).
    pub fn from_btree(map: &BTreeMap<String, String>) -> Self {
        let mut h = Self::new();
        for (k, v) in map {
            h.set(k, v);
        }
        h
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  FetchMethod
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum FetchMethod {
    Get, Post, Put, Patch, Delete, Head, Options, Connect, Trace,
}

impl FetchMethod {
    pub fn from_str(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "GET"     => Self::Get,
            "POST"    => Self::Post,
            "PUT"     => Self::Put,
            "PATCH"   => Self::Patch,
            "DELETE"  => Self::Delete,
            "HEAD"    => Self::Head,
            "OPTIONS" => Self::Options,
            "CONNECT" => Self::Connect,
            "TRACE"   => Self::Trace,
            _         => Self::Get,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get     => "GET",     Self::Post    => "POST",
            Self::Put     => "PUT",     Self::Patch   => "PATCH",
            Self::Delete  => "DELETE",  Self::Head    => "HEAD",
            Self::Options => "OPTIONS", Self::Connect => "CONNECT",
            Self::Trace   => "TRACE",
        }
    }

    fn to_http_method(&self) -> Method {
        match self {
            Self::Get     => Method::Get,
            Self::Post    => Method::Post,
            Self::Put     => Method::Put,
            Self::Delete  => Method::Delete,
            Self::Patch   => Method::Patch,
            Self::Head    => Method::Head,
            Self::Options => Method::Options,
            _             => Method::Get,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  FetchRequest
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum RequestBody { None, Text(String), Bytes(Vec<u8>) }

#[derive(Clone, Debug)]
pub struct FetchRequest {
    pub url:         String,
    pub method:      FetchMethod,
    pub headers:     FetchHeaders,
    pub body:        RequestBody,
    pub redirect:    String,   // "follow" | "error" | "manual"
}

impl FetchRequest {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            method: FetchMethod::Get,
            headers: FetchHeaders::new(),
            body: RequestBody::None,
            redirect: "follow".to_string(),
        }
    }

    pub fn method(mut self, m: &str) -> Self { self.method = FetchMethod::from_str(m); self }
    pub fn header(mut self, name: &str, value: &str) -> Self { self.headers.set(name, value); self }
    pub fn body_text(mut self, b: &str) -> Self { self.body = RequestBody::Text(b.to_string()); self }
    pub fn body_bytes(mut self, b: Vec<u8>) -> Self { self.body = RequestBody::Bytes(b); self }
}

// ─────────────────────────────────────────────────────────────────────────────
//  FetchResponse
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct FetchResponse {
    pub status:      u16,
    pub status_text: String,
    pub headers:     FetchHeaders,
    pub url:         String,
    pub redirected:  bool,
    body:            Vec<u8>,
    body_used:       bool,
}

impl FetchResponse {
    fn from_http(resp: HttpResponse, url: String) -> Self {
        let status_text = resp.reason.clone();
        let headers = FetchHeaders::from_btree(&resp.headers);
        Self {
            status: resp.status,
            status_text,
            headers,
            url,
            redirected: false,
            body: resp.body,
            body_used: false,
        }
    }

    pub fn ok(&self) -> bool { (200..300).contains(&self.status) }

    pub fn text(&mut self) -> Result<String, &'static str> {
        if self.body_used { return Err("Body already consumed"); }
        self.body_used = true;
        Ok(String::from_utf8_lossy(&self.body).into_owned())
    }

    pub fn array_buffer(&mut self) -> Result<Vec<u8>, &'static str> {
        if self.body_used { return Err("Body already consumed"); }
        self.body_used = true;
        Ok(core::mem::take(&mut self.body))
    }

    pub fn body_used(&self) -> bool { self.body_used }
    pub fn body_bytes(&self) -> &[u8] { &self.body }

    pub fn content_type(&self) -> Option<&str> { self.headers.get("content-type") }
    pub fn content_length(&self) -> Option<usize> {
        self.headers.get("content-length")?.parse().ok()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Local port allocator
// ─────────────────────────────────────────────────────────────────────────────

static NEXT_LOCAL_PORT: Mutex<u16> = Mutex::new(49152);

fn alloc_local_port() -> u16 {
    let mut p = NEXT_LOCAL_PORT.lock();
    let port = *p;
    *p = p.wrapping_add(1);
    if *p < 49152 { *p = 49152; }
    port
}

// ─────────────────────────────────────────────────────────────────────────────
//  Core synchronous fetch
// ─────────────────────────────────────────────────────────────────────────────

const MAX_REDIRECTS: usize = 20;

pub fn fetch(req: &FetchRequest) -> Result<FetchResponse, FetchError> {
    fetch_inner(req, 0)
}

fn fetch_inner(req: &FetchRequest, depth: usize) -> Result<FetchResponse, FetchError> {
    if depth > MAX_REDIRECTS {
        return Err(FetchError::NetworkError("Too many redirects".to_string()));
    }

    let url = Url::parse(&req.url).ok_or(FetchError::InvalidUrl)?;

    // DNS resolve
    let ip = resolve_host(&url.host)?;

    // Allocate ephemeral source port
    let local_port = alloc_local_port();
    let port = if url.port != 0 { url.port } else if url.scheme == "https" { 443 } else { 80 };

    let conn_id = crate::net::tcp::connect(ip, port, local_port)
        .map_err(|e| FetchError::ConnectError(e.to_string()))?;

    // Build HTTP/1.1 request
    let mut http_req = HttpRequest::new(req.method.to_http_method(), url.request_target());
    for (name, value) in req.headers.iter() {
        http_req = http_req.header(name, value);
    }
    match &req.body {
        RequestBody::None    => {}
        RequestBody::Text(t) => { http_req = http_req.body(t.as_bytes().to_vec()); }
        RequestBody::Bytes(b) => { http_req = http_req.body(b.clone()); }
    }

    // Send + receive
    let raw = http_req.serialize(&url.host);
    crate::net::tcp::send(conn_id, &raw)
        .map_err(|e| FetchError::NetworkError(e.to_string()))?;

    let mut io = TcpClientIo::new(conn_id);
    let http_resp = parse_response(&mut io)
        .map_err(|e| FetchError::ParseError(format!("{:?}", e)))?;
    let _ = crate::net::tcp::close(conn_id);

    // Redirect handling
    if req.redirect == "follow"
        && matches!(http_resp.status, 301 | 302 | 303 | 307 | 308)
    {
        if let Some(location) = http_resp.headers.get("location").map(|s| s.to_string()) {
            let new_url = resolve_location(&req.url, &location);
            let mut new_req = req.clone();
            if http_resp.status == 303 {
                new_req.method = FetchMethod::Get;
                new_req.body = RequestBody::None;
            }
            new_req.url = new_url;
            let mut final_resp = fetch_inner(&new_req, depth + 1)?;
            final_resp.redirected = true;
            return Ok(final_resp);
        }
    }

    Ok(FetchResponse::from_http(http_resp, req.url.clone()))
}

fn resolve_host(host: &str) -> Result<[u8; 4], FetchError> {
    // Check bare IP
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() == 4 {
        if let (Ok(a), Ok(b), Ok(c), Ok(d)) = (
            parts[0].parse::<u8>(), parts[1].parse::<u8>(),
            parts[2].parse::<u8>(), parts[3].parse::<u8>(),
        ) {
            return Ok([a, b, c, d]);
        }
    }
    crate::net::dns::resolve(host)
        .map_err(|e| FetchError::DnsError(e.to_string()))
}

fn resolve_location(base: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }
    if let Some(url) = Url::parse(base) {
        if location.starts_with('/') {
            return format!("{}://{}{}", url.scheme, url.host, location);
        }
        let dir = base.rfind('/').map(|i| &base[..i]).unwrap_or(base);
        format!("{}/{}", dir, location)
    } else {
        location.to_string()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  XMLHttpRequest
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReadyState {
    Unsent = 0, Opened = 1, HeadersReceived = 2, Loading = 3, Done = 4,
}

#[derive(Debug, Clone)]
pub struct XmlHttpRequest {
    pub ready_state:      ReadyState,
    pub status:           u16,
    pub status_text:      String,
    pub response_text:    String,
    pub response_url:     String,
    method:               String,
    url:                  String,
    request_headers:      FetchHeaders,
    pub response_headers: FetchHeaders,
    pub timeout_ms:       u32,
}

impl XmlHttpRequest {
    pub fn new() -> Self {
        Self {
            ready_state: ReadyState::Unsent,
            status: 0, status_text: String::new(),
            response_text: String::new(), response_url: String::new(),
            method: "GET".to_string(), url: String::new(),
            request_headers: FetchHeaders::new(),
            response_headers: FetchHeaders::new(),
            timeout_ms: 0,
        }
    }

    pub fn open(&mut self, method: &str, url: &str) {
        self.method = method.to_uppercase();
        self.url = url.to_string();
        self.ready_state = ReadyState::Opened;
        self.status = 0;
        self.response_text.clear();
    }

    pub fn set_request_header(&mut self, name: &str, value: &str) {
        self.request_headers.set(name, value);
    }

    pub fn get_response_header(&self, name: &str) -> Option<&str> {
        self.response_headers.get(name)
    }

    pub fn get_all_response_headers(&self) -> String {
        self.response_headers.iter()
            .map(|(k, v)| format!("{}: {}\r\n", k, v))
            .collect()
    }

    /// Synchronous send.
    pub fn send(&mut self, body: Option<&str>) -> Result<(), FetchError> {
        let mut req = FetchRequest::new(&self.url).method(&self.method);
        for (k, v) in self.request_headers.iter() {
            req = req.header(k, v);
        }
        if let Some(b) = body { req = req.body_text(b); }

        self.ready_state = ReadyState::Loading;
        let mut resp = fetch(&req)?;
        self.status = resp.status;
        self.status_text = resp.status_text.clone();
        self.response_url = resp.url.clone();
        self.response_headers = resp.headers.clone();
        self.response_text = resp.text().unwrap_or_default();
        self.ready_state = ReadyState::Done;
        Ok(())
    }

    pub fn abort(&mut self) {
        self.ready_state = ReadyState::Unsent;
        self.status = 0;
        self.response_text.clear();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  JS bindings
// ─────────────────────────────────────────────────────────────────────────────

/// Install `fetch(url, init?)` and `XMLHttpRequest` constructor into interpreter.
pub fn install_fetch_api(interp: &mut Interpreter) {
    interp.env.define("fetch".to_string(),
        JsValue::NativeFunction("fetch", js_fetch));
    interp.env.define("XMLHttpRequest".to_string(),
        JsValue::NativeFunction("XMLHttpRequest", js_xhr_ctor));
}

// ── fetch() native ────────────────────────────────────────────────────────────

fn js_fetch(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let url = match args.first() {
        Some(JsValue::Str(s)) => s.clone(),
        _ => return js_make_error_response("fetch: invalid URL"),
    };

    let mut req = FetchRequest::new(&url);

    if let Some(JsValue::Object(init)) = args.get(1) {
        let init = init.borrow();
        if let JsValue::Str(m) = init.get("method") { req = req.method(&m); }
        if let JsValue::Str(b) = init.get("body")   { req = req.body_text(&b); }
        if let JsValue::Object(hmap) = init.get("headers") {
            for (k, v) in hmap.borrow().props.iter() {
                req = req.header(k, &v.to_string_val());
            }
        }
    }

    match fetch(&req) {
        Ok(mut resp) => {
            let body_text = resp.text().unwrap_or_default();
            js_make_response_object(resp.status, &resp.status_text, resp.ok(), &resp.url, resp.redirected, body_text, resp.headers)
        }
        Err(e) => js_make_error_response(&e.message()),
    }
}

fn js_make_response_object(
    status: u16, status_text: &str, ok: bool, url: &str, redirected: bool,
    body_text: String, headers: FetchHeaders,
) -> JsValue {
    // Store body in side-channel for .text() / .json() calls
    *RESPONSE_BODY.lock() = Some(body_text.clone());

    let obj = Rc::new(RefCell::new(JsObject::new()));
    {
        let mut o = obj.borrow_mut();
        o.set("status".to_string(),     JsValue::Number(status as f64));
        o.set("statusText".to_string(), JsValue::Str(status_text.to_string()));
        o.set("ok".to_string(),         JsValue::Bool(ok));
        o.set("url".to_string(),        JsValue::Str(url.to_string()));
        o.set("redirected".to_string(), JsValue::Bool(redirected));
        o.set("bodyUsed".to_string(),   JsValue::Bool(false));
        // Pre-store body for .text() / .json()
        o.set("_body".to_string(),      JsValue::Str(body_text));

        o.set("text".to_string(),
            JsValue::NativeFunction("text", js_resp_text));
        o.set("json".to_string(),
            JsValue::NativeFunction("json", js_resp_json));
        o.set("arrayBuffer".to_string(),
            JsValue::NativeFunction("arrayBuffer", js_resp_abuf));

        // headers object
        let hobj = Rc::new(RefCell::new(JsObject::new()));
        for (k, v) in headers.iter() {
            hobj.borrow_mut().set(k.to_string(), JsValue::Str(v.to_string()));
        }
        hobj.borrow_mut().set("get".to_string(),
            JsValue::NativeFunction("get", js_headers_get_stub));
        o.set("headers".to_string(), JsValue::Object(hobj));
    }
    JsValue::Object(obj)
}

fn js_resp_text(_args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    JsValue::Str(RESPONSE_BODY.lock().clone().unwrap_or_default())
}

fn js_resp_json(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let body = RESPONSE_BODY.lock().clone().unwrap_or_default();
    interp.run(&format!("({})", body))
}

fn js_resp_abuf(_args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    JsValue::Object(Rc::new(RefCell::new(JsObject::new())))
}

fn js_headers_get_stub(_args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    JsValue::Null
}

fn js_make_error_response(msg: &str) -> JsValue {
    crate::serial_println!("[fetch] {}", msg);
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("ok".to_string(),     JsValue::Bool(false));
    obj.borrow_mut().set("status".to_string(), JsValue::Number(0.0));
    obj.borrow_mut().set("error".to_string(),  JsValue::Str(msg.to_string()));
    JsValue::Object(obj)
}

/// Side-channel: latest response body for .text() / .json() calls.
static RESPONSE_BODY: Mutex<Option<String>> = Mutex::new(None);

// ── XMLHttpRequest native ─────────────────────────────────────────────────────

fn js_xhr_ctor(_args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    {
        let mut o = obj.borrow_mut();
        o.set("readyState".to_string(),    JsValue::Number(0.0));
        o.set("status".to_string(),        JsValue::Number(0.0));
        o.set("statusText".to_string(),    JsValue::Str(String::new()));
        o.set("responseText".to_string(),  JsValue::Str(String::new()));
        o.set("responseURL".to_string(),   JsValue::Str(String::new()));
        o.set("timeout".to_string(),       JsValue::Number(0.0));
        o.set("withCredentials".to_string(), JsValue::Bool(false));
        o.set("open".to_string(),
            JsValue::NativeFunction("open", js_xhr_open));
        o.set("send".to_string(),
            JsValue::NativeFunction("send", js_xhr_send));
        o.set("setRequestHeader".to_string(),
            JsValue::NativeFunction("setRequestHeader", js_xhr_set_header));
        o.set("getResponseHeader".to_string(),
            JsValue::NativeFunction("getResponseHeader", js_xhr_get_header));
        o.set("abort".to_string(),
            JsValue::NativeFunction("abort", js_xhr_abort));
    }
    JsValue::Object(obj)
}

// XHR side-channel
struct XhrPending { method: String, url: String, headers: FetchHeaders }
// SAFETY: FetchHeaders contains only BTreeMap<String,String> (no Rc/RefCell).
unsafe impl Send for XhrPending {}
static XHR_PENDING: Mutex<Option<XhrPending>> = Mutex::new(None);

fn js_xhr_open(args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let method = args.first().map(|v| v.to_string_val()).unwrap_or("GET".to_string());
    let url    = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
    *XHR_PENDING.lock() = Some(XhrPending { method, url, headers: FetchHeaders::new() });
    JsValue::Undefined
}

fn js_xhr_set_header(args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let name  = args.first().map(|v| v.to_string_val()).unwrap_or_default();
    let value = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
    if let Some(p) = XHR_PENDING.lock().as_mut() { p.headers.set(&name, &value); }
    JsValue::Undefined
}

fn js_xhr_send(args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let body = args.first().and_then(|v| match v {
        JsValue::Str(s) => Some(s.clone()),
        _ => None,
    });
    let pending = match XHR_PENDING.lock().take() {
        Some(p) => p,
        None => return JsValue::Undefined,
    };
    let mut req = FetchRequest::new(&pending.url).method(&pending.method);
    for (k, v) in pending.headers.iter() { req = req.header(k, v); }
    if let Some(b) = &body { req = req.body_text(b); }

    let result = Rc::new(RefCell::new(JsObject::new()));
    match fetch(&req) {
        Ok(mut resp) => {
            let text = resp.text().unwrap_or_default();
            let mut o = result.borrow_mut();
            o.set("readyState".to_string(),   JsValue::Number(4.0));
            o.set("status".to_string(),        JsValue::Number(resp.status as f64));
            o.set("statusText".to_string(),    JsValue::Str(resp.status_text));
            o.set("responseText".to_string(),  JsValue::Str(text));
            o.set("responseURL".to_string(),   JsValue::Str(resp.url));
        }
        Err(e) => {
            let mut o = result.borrow_mut();
            o.set("readyState".to_string(),    JsValue::Number(4.0));
            o.set("status".to_string(),        JsValue::Number(0.0));
            o.set("responseText".to_string(),  JsValue::Str(e.message()));
        }
    }
    JsValue::Object(result)
}

fn js_xhr_get_header(_args: &[JsValue], _i: &mut Interpreter) -> JsValue { JsValue::Null }
fn js_xhr_abort(_args: &[JsValue], _i: &mut Interpreter) -> JsValue {
    let _ = XHR_PENDING.lock().take();
    JsValue::Undefined
}

// ─────────────────────────────────────────────────────────────────────────────
//  Self-tests (no network required)
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    // ── Test 1: FetchHeaders ──────────────────────────────────────────────────
    let mut h = FetchHeaders::new();
    h.set("Content-Type", "application/json");
    h.set("X-Custom", "hello");
    if h.get("content-type") != Some("application/json") { return false; }
    if h.get("CONTENT-TYPE") != Some("application/json") { return false; }
    if h.get("x-custom") != Some("hello") { return false; }
    if h.get("missing").is_some() { return false; }
    h.delete("x-custom");
    if h.has("x-custom") { return false; }

    // ── Test 2: FetchHeaders append ───────────────────────────────────────────
    let mut h2 = FetchHeaders::new();
    h2.append("Accept", "text/html");
    h2.append("Accept", "application/json");
    if h2.get("accept") != Some("text/html, application/json") { return false; }

    // ── Test 3: FetchHeaders::from_btree ─────────────────────────────────────
    let mut map = BTreeMap::new();
    map.insert("content-length".to_string(), "42".to_string());
    map.insert("cache-control".to_string(), "no-cache".to_string());
    let h3 = FetchHeaders::from_btree(&map);
    if h3.get("content-length") != Some("42") { return false; }
    if h3.get("cache-control") != Some("no-cache") { return false; }

    // ── Test 4: FetchRequest builder ──────────────────────────────────────────
    let req = FetchRequest::new("http://example.com/api")
        .method("POST")
        .header("Content-Type", "application/json")
        .body_text(r#"{"key":"value"}"#);
    if req.method != FetchMethod::Post { return false; }
    if req.headers.get("content-type") != Some("application/json") { return false; }
    match &req.body {
        RequestBody::Text(t) if t.contains("key") => {}
        _ => return false,
    }

    // ── Test 5: FetchResponse construction from HttpResponse ──────────────────
    let http_resp = HttpResponse {
        status: 200,
        reason: "OK".to_string(),
        headers: {
            let mut m = BTreeMap::new();
            m.insert("content-type".to_string(), "text/plain".to_string());
            m
        },
        body: b"Hello world".to_vec(),
    };
    let mut resp = FetchResponse::from_http(http_resp, "http://example.com".to_string());
    if !resp.ok() { return false; }
    if resp.status != 200 { return false; }
    if resp.content_type() != Some("text/plain") { return false; }
    let text = resp.text().unwrap();
    if text != "Hello world" { return false; }
    if !resp.body_used() { return false; }
    if resp.text().is_ok() { return false; }  // second consume must fail

    // ── Test 6: XmlHttpRequest state machine ──────────────────────────────────
    let mut xhr = XmlHttpRequest::new();
    if xhr.ready_state != ReadyState::Unsent { return false; }
    xhr.open("POST", "http://example.com/");
    if xhr.ready_state != ReadyState::Opened { return false; }
    xhr.set_request_header("Accept", "text/html");
    if xhr.request_headers.get("accept") != Some("text/html") { return false; }
    xhr.abort();
    if xhr.ready_state != ReadyState::Unsent { return false; }

    // ── Test 7: URL redirect resolution ──────────────────────────────────────
    let r1 = resolve_location("http://example.com/path/page", "/new");
    if r1 != "http://example.com/new" { return false; }
    let r2 = resolve_location("http://example.com/path/page", "https://other.com/x");
    if r2 != "https://other.com/x" { return false; }

    // ── Test 8: FetchMethod round-trip ────────────────────────────────────────
    for (s, expected) in &[
        ("GET", FetchMethod::Get), ("POST", FetchMethod::Post),
        ("DELETE", FetchMethod::Delete), ("patch", FetchMethod::Patch),
    ] {
        if FetchMethod::from_str(s) != *expected { return false; }
    }

    true
}

// ─────────────────────────────────────────────────────────────────────────────
//  Init
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    crate::serial_println!("[fetch] Fetch API + XMLHttpRequest ready (Phase 46).");
}

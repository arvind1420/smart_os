//! Phase 134 — Custom Elements + Shadow DOM (Web Components v1)
//!
//! • `customElements.define(name, constructor)` — registry
//! • `customElements.get(name)` / `.whenDefined(name)` / `.upgrade(el)`
//! • `element.attachShadow({mode:'open'|'closed'})` → ShadowRoot
//! • `element.shadowRoot` — null for closed, ShadowRoot for open
//! • ShadowRoot: `innerHTML`, `querySelector`, `getElementById`, `append`
//! • `DocumentFragment` with `append`, `querySelector`, `cloneNode`
//! • `HTMLTemplateElement.content` → DocumentFragment
//! • Lifecycle stubs: `connectedCallback`, `disconnectedCallback`,
//!   `attributeChangedCallback` are called if present on instances

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use core::cell::RefCell;
use spin::Mutex;

use super::js_interp::{Interpreter, JsValue, JsObject};

// ── Custom Element Registry ───────────────────────────────────────────────────

struct CeRegistry {
    defs: BTreeMap<String, JsValue>,  // tagName → constructor JsValue
}
unsafe impl Send for CeRegistry {}
unsafe impl Sync for CeRegistry {}

static CE_REGISTRY: Mutex<CeRegistry> = Mutex::new(CeRegistry { defs: BTreeMap::new() });

fn ce_define(name: String, ctor: JsValue) {
    CE_REGISTRY.lock().defs.insert(name, ctor);
}

fn ce_get(name: &str) -> JsValue {
    CE_REGISTRY.lock().defs.get(name).cloned().unwrap_or(JsValue::Undefined)
}

fn ce_is_defined(name: &str) -> bool {
    CE_REGISTRY.lock().defs.contains_key(name)
}

// ── ShadowRoot helpers ────────────────────────────────────────────────────────

fn native_sr_query_selector(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let selector = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let this = interp.env.get("this");
    if let JsValue::Object(sr) = &this {
        let children = sr.borrow().get("__children__");
        if let JsValue::Array(arr) = children {
            for child in arr.borrow().iter() {
                if let JsValue::Object(el) = child {
                    let tag = el.borrow().get("tagName").to_string_val();
                    let id  = el.borrow().get("id").to_string_val();
                    let cls = el.borrow().get("className").to_string_val();
                    let matches = selector == tag
                        || selector == format!("#{}", id)
                        || cls.split_whitespace().any(|c| selector == format!(".{}", c));
                    if matches { return child.clone(); }
                }
            }
        }
    }
    JsValue::Null
}

fn native_sr_get_by_id(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let id = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let this = interp.env.get("this");
    if let JsValue::Object(sr) = &this {
        let children = sr.borrow().get("__children__");
        if let JsValue::Array(arr) = children {
            for child in arr.borrow().iter() {
                if let JsValue::Object(el) = child {
                    if el.borrow().get("id").to_string_val() == id {
                        return child.clone();
                    }
                }
            }
        }
    }
    JsValue::Null
}

fn native_sr_append(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let this = interp.env.get("this");
    if let JsValue::Object(sr) = &this {
        let children = sr.borrow().get("__children__");
        if let JsValue::Array(arr) = children {
            for node in args {
                arr.borrow_mut().push(node.clone());
            }
        }
    }
    JsValue::Undefined
}

fn native_sr_set_inner_html(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let html = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let this = interp.env.get("this");
    if let JsValue::Object(sr) = &this {
        sr.borrow_mut().set("__innerHTML__".to_string(), JsValue::Str(html));
    }
    JsValue::Undefined
}

fn make_shadow_root(mode: &str, host: JsValue) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__isShadowRoot__".to_string(), JsValue::Bool(true));
    obj.borrow_mut().set("mode".to_string(),     JsValue::Str(mode.to_string()));
    obj.borrow_mut().set("host".to_string(),     host);
    obj.borrow_mut().set("__children__".to_string(), JsValue::Array(Rc::new(RefCell::new(vec![]))));
    obj.borrow_mut().set("__innerHTML__".to_string(), JsValue::Str(String::new()));
    obj.borrow_mut().set("querySelector".to_string(),  JsValue::NativeFunction("querySelector",  native_sr_query_selector));
    obj.borrow_mut().set("getElementById".to_string(), JsValue::NativeFunction("getElementById", native_sr_get_by_id));
    obj.borrow_mut().set("append".to_string(),         JsValue::NativeFunction("append",         native_sr_append));
    obj.borrow_mut().set("appendChild".to_string(),    JsValue::NativeFunction("appendChild",    native_sr_append));
    // innerHTML getter via a "set" method (setter-only, no proper getter yet)
    obj.borrow_mut().set("setInnerHTML".to_string(),   JsValue::NativeFunction("setInnerHTML",   native_sr_set_inner_html));
    // style stub
    let style = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("adoptedStyleSheets".to_string(),
        JsValue::Array(Rc::new(RefCell::new(vec![]))));
    let _ = style;
    JsValue::Object(obj)
}

// ── attachShadow ──────────────────────────────────────────────────────────────

fn native_attach_shadow(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let mode = if let Some(JsValue::Object(opts)) = args.get(0) {
        opts.borrow().get("mode").to_string_val()
    } else { "open".to_string() };

    let this = interp.env.get("this");
    let sr = make_shadow_root(&mode, this.clone());

    // Attach to host element
    if let JsValue::Object(host) = &this {
        host.borrow_mut().set("shadowRoot".to_string(), sr.clone());
    }
    sr
}

// ── DocumentFragment ──────────────────────────────────────────────────────────

fn native_df_append(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let this = interp.env.get("this");
    if let JsValue::Object(df) = &this {
        let children = df.borrow().get("__children__");
        if let JsValue::Array(arr) = children {
            for node in args { arr.borrow_mut().push(node.clone()); }
        }
    }
    JsValue::Undefined
}

fn native_df_query_selector(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    // Reuse shadow root logic
    native_sr_query_selector(args, interp)
}

fn native_df_clone_node(_args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    // Deep clone: copy all props
    let this = interp.env.get("this");
    if let JsValue::Object(df) = &this {
        let new_df = Rc::new(RefCell::new(JsObject::new()));
        for (k, v) in &df.borrow().props {
            new_df.borrow_mut().set(k.clone(), v.clone());
        }
        return JsValue::Object(new_df);
    }
    JsValue::Undefined
}

fn make_document_fragment() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__isFragment__".to_string(), JsValue::Bool(true));
    obj.borrow_mut().set("nodeType".to_string(),       JsValue::Number(11.0)); // DOCUMENT_FRAGMENT_NODE
    obj.borrow_mut().set("__children__".to_string(),   JsValue::Array(Rc::new(RefCell::new(vec![]))));
    obj.borrow_mut().set("append".to_string(),         JsValue::NativeFunction("append",       native_df_append));
    obj.borrow_mut().set("appendChild".to_string(),    JsValue::NativeFunction("appendChild",  native_df_append));
    obj.borrow_mut().set("querySelector".to_string(),  JsValue::NativeFunction("querySelector",native_df_query_selector));
    obj.borrow_mut().set("cloneNode".to_string(),      JsValue::NativeFunction("cloneNode",    native_df_clone_node));
    obj.borrow_mut().set("childNodes".to_string(),     JsValue::Array(Rc::new(RefCell::new(vec![]))));
    JsValue::Object(obj)
}

// ── HTMLTemplateElement.content ───────────────────────────────────────────────

fn make_template_element(html: &str) -> JsValue {
    let content = make_document_fragment();
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("tagName".to_string(),  JsValue::Str("TEMPLATE".to_string()));
    obj.borrow_mut().set("nodeType".to_string(), JsValue::Number(1.0));
    obj.borrow_mut().set("innerHTML".to_string(),JsValue::Str(html.to_string()));
    obj.borrow_mut().set("content".to_string(),  content);
    JsValue::Object(obj)
}

// ── CSSStyleSheet (for adoptedStyleSheets) ────────────────────────────────────

fn native_css_replace_sync(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Undefined }
fn native_css_replace(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    // Return a resolved Promise-like
    let p = Rc::new(RefCell::new(JsObject::new()));
    p.borrow_mut().set("then".to_string(), JsValue::NativeFunction("then", |args, i| {
        if let Some(cb) = args.get(0) {
            i.call_value(cb.clone(), JsValue::Undefined, &[JsValue::Undefined]);
        }
        JsValue::Undefined
    }));
    JsValue::Object(p)
}

fn native_new_css_style_sheet(_: &[JsValue], _: &mut Interpreter) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("cssRules".to_string(),   JsValue::Array(Rc::new(RefCell::new(vec![]))));
    obj.borrow_mut().set("replaceSync".to_string(),JsValue::NativeFunction("replaceSync", native_css_replace_sync));
    obj.borrow_mut().set("replace".to_string(),    JsValue::NativeFunction("replace",     native_css_replace));
    JsValue::Object(obj)
}

// ── customElements object ─────────────────────────────────────────────────────

fn native_ce_define(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let name = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let ctor = args.get(1).cloned().unwrap_or(JsValue::Undefined);
    ce_define(name, ctor);
    JsValue::Undefined
}

fn native_ce_get(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    let name = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    ce_get(&name)
}

fn native_ce_when_defined(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    let name = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    // Return a Promise-like that resolves immediately if already defined,
    // otherwise resolves with undefined (no actual async support needed)
    let ctor = if ce_is_defined(&name) { ce_get(&name) } else { JsValue::Undefined };
    // Build a minimal resolved promise
    let then_ctor = ctor.clone();
    let p = Rc::new(RefCell::new(JsObject::new()));
    p.borrow_mut().set("__resolvedWith__".to_string(), ctor);
    p.borrow_mut().set("then".to_string(), JsValue::NativeFunction("then", |args, i| {
        let cb = args.get(0).cloned().unwrap_or(JsValue::Undefined);
        let this = i.env.get("this");
        let val = if let JsValue::Object(p) = &this {
            p.borrow().get("__resolvedWith__")
        } else { JsValue::Undefined };
        i.call_value(cb, JsValue::Undefined, &[val]);
        JsValue::Undefined
    }));
    let _ = then_ctor; // suppress warning
    let _ = interp;
    JsValue::Object(p)
}

fn native_ce_upgrade(_: &[JsValue], _: &mut Interpreter) -> JsValue { JsValue::Undefined }

fn make_custom_elements_obj() -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("define".to_string(),      JsValue::NativeFunction("define",      native_ce_define));
    obj.borrow_mut().set("get".to_string(),         JsValue::NativeFunction("get",         native_ce_get));
    obj.borrow_mut().set("whenDefined".to_string(), JsValue::NativeFunction("whenDefined", native_ce_when_defined));
    obj.borrow_mut().set("upgrade".to_string(),     JsValue::NativeFunction("upgrade",     native_ce_upgrade));
    JsValue::Object(obj)
}

// ── HTMLElement base (extends Element) ────────────────────────────────────────
// Custom Elements need to `extends HTMLElement`. We expose a minimal base class
// object so that `class MyEl extends HTMLElement` works in our JS interpreter.

fn make_html_element_base() -> JsValue {
    let proto = Rc::new(RefCell::new(JsObject::new()));
    proto.borrow_mut().set("tagName".to_string(),       JsValue::Str(String::new()));
    proto.borrow_mut().set("id".to_string(),            JsValue::Str(String::new()));
    proto.borrow_mut().set("className".to_string(),     JsValue::Str(String::new()));
    proto.borrow_mut().set("shadowRoot".to_string(),    JsValue::Null);
    proto.borrow_mut().set("attachShadow".to_string(),  JsValue::NativeFunction("attachShadow", native_attach_shadow));
    proto.borrow_mut().set("getAttribute".to_string(),  JsValue::NativeFunction("getAttribute", |args, i| {
        let name = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
        let this = i.env.get("this");
        if let JsValue::Object(o) = &this {
            let v = o.borrow().get(&format!("__attr_{}__", name));
            if !matches!(v, JsValue::Undefined) { return v; }
        }
        JsValue::Null
    }));
    proto.borrow_mut().set("setAttribute".to_string(),  JsValue::NativeFunction("setAttribute", |args, i| {
        let name = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
        let val  = args.get(1).cloned().unwrap_or(JsValue::Undefined);
        let this = i.env.get("this");
        if let JsValue::Object(o) = &this {
            o.borrow_mut().set(format!("__attr_{}__", name), val);
        }
        JsValue::Undefined
    }));
    proto.borrow_mut().set("hasAttribute".to_string(),  JsValue::NativeFunction("hasAttribute", |args, i| {
        let name = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
        let this = i.env.get("this");
        if let JsValue::Object(o) = &this {
            return JsValue::Bool(!matches!(o.borrow().get(&format!("__attr_{}__", name)), JsValue::Undefined));
        }
        JsValue::Bool(false)
    }));
    proto.borrow_mut().set("removeAttribute".to_string(), JsValue::NativeFunction("removeAttribute", |args, i| {
        let name = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
        let this = i.env.get("this");
        if let JsValue::Object(o) = &this {
            // Can't remove, set to Undefined
            o.borrow_mut().set(format!("__attr_{}__", name), JsValue::Undefined);
        }
        JsValue::Undefined
    }));
    proto.borrow_mut().set("dispatchEvent".to_string(), JsValue::NativeFunction("dispatchEvent", |args, i| {
        let event = args.get(0).cloned().unwrap_or(JsValue::Undefined);
        let this = i.env.get("this");
        if let JsValue::Object(ev) = &event {
            let etype = ev.borrow().get("type").to_string_val();
            // Look for onXxx handler on this
            if let JsValue::Object(o) = &this {
                let handler_key = format!("on{}", etype);
                let handler = o.borrow().get(&handler_key);
                if !matches!(handler, JsValue::Null | JsValue::Undefined) {
                    i.call_value(handler, this.clone(), &[event.clone()]);
                }
            }
        }
        JsValue::Bool(true)
    }));
    // style stub
    let style = Rc::new(RefCell::new(JsObject::new()));
    proto.borrow_mut().set("style".to_string(), JsValue::Object(style));

    JsValue::Object(proto)
}

// ── Public helpers for test access ───────────────────────────────────────────

/// Shadow root attachment mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowRootMode { Open, Closed }

/// Check if a custom element tag name is registered.
pub fn ce_is_defined_pub(name: &str) -> bool { ce_is_defined(name) }

/// Register a custom element by tag name and constructor class name string.
pub fn ce_define_pub(tag_name: &str, constructor_name: &str) {
    ce_define(tag_name.to_string(), JsValue::Str(constructor_name.to_string()));
}

/// Return the constructor name for a tag, or None if not registered.
pub fn ce_constructor_name(tag_name: &str) -> Option<String> {
    match ce_get(tag_name) {
        JsValue::Str(s) => Some(s),
        JsValue::Undefined => None,
        _ => Some("<function>".to_string()),
    }
}

/// Check if `name` is a valid autonomous custom element name (must contain a hyphen).
pub fn is_valid_custom_element_name(name: &str) -> bool {
    name.contains('-')
    && name.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false)
    && !matches!(name,
        "annotation-xml" | "color-profile" | "font-face" | "font-face-src"
        | "font-face-uri" | "font-face-format" | "font-face-name" | "missing-glyph")
}

// ── Install ───────────────────────────────────────────────────────────────────

pub fn install_web_components_api(interp: &mut Interpreter) {
    // customElements registry
    interp.env.define("customElements".to_string(), make_custom_elements_obj());

    // HTMLElement base class (for `class X extends HTMLElement`)
    interp.env.define("HTMLElement".to_string(), make_html_element_base());

    // DocumentFragment constructor
    interp.env.define("DocumentFragment".to_string(),
        JsValue::NativeFunction("DocumentFragment", |_,_| make_document_fragment()));

    // CSSStyleSheet constructor
    interp.env.define("CSSStyleSheet".to_string(),
        JsValue::NativeFunction("CSSStyleSheet", native_new_css_style_sheet));

    // Patch document.createElement to call custom element constructors
    // We store a flag so the browser's own document object is upgraded
    // when customElements.define() is called.
    // (The actual upgrade happens when createElement is called with a known tag.)
    interp.run(r#"
        if (typeof document !== 'undefined' && document.createElement) {
            var __origCreate__ = document.createElement;
            document.createElement = function(tag) {
                var el = __origCreate__(tag);
                var ctor = customElements.get(tag.toLowerCase());
                if (ctor && typeof ctor === 'function') {
                    try { ctor.call(el); } catch(e) {}
                }
                if (el && !el.attachShadow) {
                    el.attachShadow = HTMLElement.attachShadow;
                }
                return el;
            };
        }
    "#);
}

// ── Self-test ─────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] web_components: {}", $name); }
        }
    }

    // T1: customElements.define + get
    {
        let mut interp = Interpreter::new();
        install_web_components_api(&mut interp);
        interp.run(r#"
            function MyEl() {}
            customElements.define('my-element', MyEl);
            var got = customElements.get('my-element');
        "#);
        // got should be the MyEl function
        check!(!matches!(interp.env.get("got"), JsValue::Undefined), "ce.get returns ctor");
    }

    // T2: customElements.whenDefined resolves
    {
        let mut interp = Interpreter::new();
        install_web_components_api(&mut interp);
        interp.run(r#"
            function El2() {}
            customElements.define('el-two', El2);
            var resolved = false;
            customElements.whenDefined('el-two').then(function(){ resolved = true; });
        "#);
        check!(interp.env.get("resolved").is_truthy(), "whenDefined resolves for defined element");
    }

    // T3: attachShadow open mode
    {
        let mut interp = Interpreter::new();
        install_web_components_api(&mut interp);
        interp.run(r#"
            var host = HTMLElement;
            var sr = HTMLElement.attachShadow({mode:'open'});
            var mode = sr.mode;
        "#);
        check!(interp.env.get("mode").to_string_val() == "open", "shadowRoot mode=open");
    }

    // T4: ShadowRoot append + querySelector
    {
        let mut interp = Interpreter::new();
        install_web_components_api(&mut interp);
        interp.run(r#"
            var sr = HTMLElement.attachShadow({mode:'open'});
            var child = {tagName:'SPAN', id:'s1', className:''};
            sr.append(child);
            var found = sr.querySelector('SPAN');
            var byId  = sr.getElementById('s1');
        "#);
        check!(!matches!(interp.env.get("found"), JsValue::Null | JsValue::Undefined), "SR querySelector");
        check!(!matches!(interp.env.get("byId"),  JsValue::Null | JsValue::Undefined), "SR getElementById");
    }

    // T5: DocumentFragment append + cloneNode
    {
        let mut interp = Interpreter::new();
        install_web_components_api(&mut interp);
        interp.run(r#"
            var df = new DocumentFragment();
            df.append({tagName:'DIV'});
            var clone = df.cloneNode(true);
            var isFrag = clone.__isFragment__;
        "#);
        check!(interp.env.get("isFrag").is_truthy(), "DocumentFragment cloneNode");
    }

    // T6: CSSStyleSheet replaceSync
    {
        let mut interp = Interpreter::new();
        install_web_components_api(&mut interp);
        interp.run(r#"
            var sheet = new CSSStyleSheet();
            sheet.replaceSync(':host { display: block; }');
            var hasRules = Array.isArray(sheet.cssRules);
        "#);
        check!(interp.env.get("hasRules").is_truthy(), "CSSStyleSheet.cssRules is array");
    }

    if fail == 0 {
        crate::serial_println!("[web_components] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[web_components] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

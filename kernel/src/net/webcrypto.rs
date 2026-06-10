//! WebCrypto API — W3C Web Cryptography API Level 1.
//!
//! Implements `window.crypto.subtle` and `window.crypto.getRandomValues`.
//! Operations are performed synchronously internally; Promises are returned
//! as pre-resolved values (our JS engine's `await nonPromise → nonPromise`
//! semantics make this transparent to callers).
//!
//! Supported algorithms
//! ─────────────────────
//!   Digest    : SHA-256 / SHA-384 / SHA-512
//!   Symmetric : AES-GCM (128-bit key)
//!   MAC       : HMAC-SHA-256
//!   KDF       : PBKDF2-SHA-256, HKDF-SHA-256
//!   Random    : getRandomValues, randomUUID

extern crate alloc;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::rc::Rc;
use core::cell::RefCell;
use spin::Mutex;

use super::js_interp::{Interpreter, JsValue, JsObject};

// ── Crypto Key Store ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct CryptoKeyEntry {
    algorithm:   String,  // "AES-GCM", "HMAC", "PBKDF2", "HKDF"
    hash:        String,  // "SHA-256" etc. — empty for AES-GCM
    extractable: bool,
    data:        Vec<u8>, // raw key bytes
}

struct KeyStore {
    next_id: u32,
    keys:    BTreeMap<u32, CryptoKeyEntry>,
}

static KEY_STORE: Mutex<KeyStore> = Mutex::new(KeyStore {
    next_id: 1,
    keys:    BTreeMap::new(),
});
unsafe impl Send for KeyStore {}
unsafe impl Sync for KeyStore {}

fn store_key(entry: CryptoKeyEntry) -> u32 {
    let mut ks = KEY_STORE.lock();
    let id = ks.next_id;
    ks.next_id += 1;
    ks.keys.insert(id, entry);
    id
}

fn lookup_key(id: u32) -> Option<CryptoKeyEntry> {
    KEY_STORE.lock().keys.get(&id).cloned()
}

// ── JS value helpers ──────────────────────────────────────────────────────────

/// Extract bytes from a JsValue::Array (of numbers) or indexed Object.
fn js_to_bytes(val: &JsValue) -> Vec<u8> {
    match val {
        JsValue::Array(a) => a.borrow().iter()
            .map(|v| v.to_number() as u8)
            .collect(),
        JsValue::Object(o) => {
            let o = o.borrow();
            let len_v = o.get("length");
            if matches!(len_v, JsValue::Undefined | JsValue::Null) {
                return Vec::new();
            }
            let len = len_v.to_number() as usize;
            (0..len).map(|i: usize| {
                let key = alloc::format!("{}", i);
                o.get(&key).to_number() as u8
            }).collect()
        }
        JsValue::Str(s) => s.as_bytes().to_vec(),
        _ => Vec::new(),
    }
}

/// Wrap a byte slice in a JsValue::Array of Numbers.
fn bytes_to_js(bytes: &[u8]) -> JsValue {
    let arr: Vec<JsValue> = bytes.iter()
        .map(|&b| JsValue::Number(b as f64))
        .collect();
    JsValue::Array(Rc::new(RefCell::new(arr)))
}

/// Get the algorithm name from a string or `{name: ...}` object argument.
fn algo_name(val: &JsValue) -> String {
    match val {
        JsValue::Str(s) => s.to_ascii_uppercase(),
        JsValue::Object(o) => {
            let o = o.borrow();
            match o.get("name") {
                JsValue::Str(s) => s.to_ascii_uppercase(),
                _ => String::new(),
            }
        }
        _ => String::new(),
    }
}

/// Extract field bytes from a `{iv: Uint8Array, ...}` algorithm descriptor.
fn algo_field_bytes(val: &JsValue, field: &str) -> Vec<u8> {
    match val {
        JsValue::Object(o) => {
            let o = o.borrow();
            js_to_bytes(&o.get(field))
        }
        _ => Vec::new(),
    }
}

/// Extract field as u32 from algorithm descriptor.
fn algo_field_u32(val: &JsValue, field: &str, default: u32) -> u32 {
    match val {
        JsValue::Object(o) => {
            let o = o.borrow();
            let v = o.get(field);
            if matches!(v, JsValue::Undefined | JsValue::Null) {
                default
            } else {
                v.to_number() as u32
            }
        }
        _ => default,
    }
}

/// Build a CryptoKey JS object wrapping a KEY_STORE id.
fn make_crypto_key(id: u32, alg_name: &str, key_type: &str, extractable: bool) -> JsValue {
    let obj = Rc::new(RefCell::new(JsObject::new()));
    obj.borrow_mut().set("__cryptoKeyId__".into(),
        JsValue::Number(id as f64));
    obj.borrow_mut().set("type".into(),
        JsValue::Str(key_type.into()));
    obj.borrow_mut().set("extractable".into(),
        JsValue::Bool(extractable));
    obj.borrow_mut().set("algorithm".into(), {
        let a = Rc::new(RefCell::new(JsObject::new()));
        a.borrow_mut().set("name".into(), JsValue::Str(alg_name.into()));
        JsValue::Object(a)
    });
    JsValue::Object(obj)
}

/// Extract KEY_STORE id from a CryptoKey JsValue.
fn key_id(val: &JsValue) -> Option<u32> {
    if let JsValue::Object(o) = val {
        let o = o.borrow();
        let v = o.get("__cryptoKeyId__");
        if !matches!(v, JsValue::Undefined | JsValue::Null) {
            return Some(v.to_number() as u32);
        }
    }
    None
}

// ── Hash / HMAC ───────────────────────────────────────────────────────────────

fn digest_bytes(alg: &str, data: &[u8]) -> Option<Vec<u8>> {
    use sha2::Digest;
    match alg {
        "SHA-256" => Some(sha2::Sha256::digest(data).to_vec()),
        "SHA-384" => Some(sha2::Sha384::digest(data).to_vec()),
        "SHA-512" => Some(sha2::Sha512::digest(data).to_vec()),
        _ => None,
    }
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::Mac;
    let mut mac = <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(key)
        .unwrap_or_else(|_| <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(
            &[0u8; 32]).unwrap());
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

// ── PBKDF2-SHA-256 ────────────────────────────────────────────────────────────

fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, out_len: usize) -> Vec<u8> {
    let hlen = 32usize;
    let blocks = (out_len + hlen - 1) / hlen;
    let mut out = Vec::with_capacity(blocks * hlen);
    for block_idx in 1u32..=blocks as u32 {
        let mut data = Vec::with_capacity(salt.len() + 4);
        data.extend_from_slice(salt);
        data.extend_from_slice(&block_idx.to_be_bytes());
        let mut u = hmac_sha256(password, &data);
        let mut t = u.clone();
        for _ in 1..iterations {
            u = hmac_sha256(password, &u);
            for j in 0..hlen { t[j] ^= u[j]; }
        }
        out.extend_from_slice(&t);
    }
    out.truncate(out_len);
    out
}

// ── HKDF-SHA-256 ─────────────────────────────────────────────────────────────

fn hkdf_sha256_expand(prk: &[u8], info: &[u8], out_len: usize) -> Vec<u8> {
    // RFC 5869 HKDF-Expand
    let hlen = 32usize;
    let blocks = (out_len + hlen - 1) / hlen;
    let mut out = Vec::with_capacity(blocks * hlen);
    let mut t: Vec<u8> = Vec::new();
    for i in 1u8..=(blocks as u8) {
        let mut data = t.clone();
        data.extend_from_slice(info);
        data.push(i);
        t = hmac_sha256(prk, &data);
        out.extend_from_slice(&t);
    }
    out.truncate(out_len);
    out
}

// ── RDRAND ────────────────────────────────────────────────────────────────────

fn fill_random(buf: &mut [u8]) {
    let mut pos = 0;
    while pos < buf.len() {
        let rnd = crate::net::tls::random_bytes_32();
        let take = (buf.len() - pos).min(32);
        buf[pos..pos + take].copy_from_slice(&rnd[..take]);
        pos += take;
    }
}

fn random_uuid() -> String {
    let mut b = [0u8; 16];
    fill_random(&mut b);
    // RFC 4122 version 4
    b[6] = (b[6] & 0x0F) | 0x40;
    b[8] = (b[8] & 0x3F) | 0x80;
    alloc::format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7],
        b[8],b[9],b[10],b[11],b[12],b[13],b[14],b[15]
    )
}

// ── Native function implementations ──────────────────────────────────────────

// crypto.subtle.digest(algorithm, data)
fn native_digest(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    if args.len() < 2 { return JsValue::Undefined; }
    let alg  = algo_name(&args[0]);
    let data = js_to_bytes(&args[1]);
    match digest_bytes(&alg, &data) {
        Some(hash) => bytes_to_js(&hash),
        None => JsValue::Undefined,
    }
}

// crypto.subtle.generateKey({name, length?}, extractable, usages)
fn native_generate_key(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    if args.len() < 3 { return JsValue::Undefined; }
    let alg  = algo_name(&args[0]);
    let extractable = args[1].is_truthy();
    let key_len_bits = algo_field_u32(&args[0], "length", 128) as usize;
    let key_len = key_len_bits / 8;

    let mut key_data = vec![0u8; key_len.max(16)];
    fill_random(&mut key_data);

    let id = store_key(CryptoKeyEntry {
        algorithm: alg.clone(),
        hash: String::new(),
        extractable,
        data: key_data,
    });
    make_crypto_key(id, &alg, "secret", extractable)
}

// crypto.subtle.importKey(format, keyData, algorithm, extractable, usages)
fn native_import_key(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    if args.len() < 4 { return JsValue::Undefined; }
    let _format    = match &args[0] { JsValue::Str(s) => s.clone(), _ => return JsValue::Undefined };
    let key_data   = js_to_bytes(&args[1]);
    let alg        = algo_name(&args[2]);
    let hash_name  = match &args[2] {
        JsValue::Object(o) => {
            let o = o.borrow();
            match o.get("hash") {
                JsValue::Str(s) => s.to_ascii_uppercase(),
                JsValue::Object(ho) => {
                    let ho = ho.borrow();
                    match ho.get("name") {
                        JsValue::Str(s) => s.to_ascii_uppercase(),
                        _ => String::new(),
                    }
                }
                _ => String::new(),
            }
        }
        _ => String::new(),
    };
    let extractable = if args.len() > 3 { args[3].is_truthy() } else { false };
    let id = store_key(CryptoKeyEntry {
        algorithm: alg.clone(),
        hash: hash_name,
        extractable,
        data: key_data,
    });
    make_crypto_key(id, &alg, "secret", extractable)
}

// crypto.subtle.exportKey(format, key)
fn native_export_key(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    if args.len() < 2 { return JsValue::Undefined; }
    let format = match &args[0] { JsValue::Str(s) => s.clone(), _ => return JsValue::Undefined };
    let id = match key_id(&args[1]) { Some(id) => id, None => return JsValue::Undefined };
    let entry = match lookup_key(id) { Some(e) => e, None => return JsValue::Undefined };
    if !entry.extractable { return JsValue::Undefined; }
    if format == "RAW" || format == "raw" {
        bytes_to_js(&entry.data)
    } else {
        JsValue::Undefined
    }
}

// crypto.subtle.encrypt({name, iv, additionalData?}, key, data)
fn native_encrypt(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    if args.len() < 3 { return JsValue::Undefined; }
    let alg   = algo_name(&args[0]);
    let id    = match key_id(&args[1]) { Some(id) => id, None => return JsValue::Undefined };
    let entry = match lookup_key(id) { Some(e) => e, None => return JsValue::Undefined };
    let data  = js_to_bytes(&args[2]);

    match alg.as_str() {
        "AES-GCM" => {
            let iv_bytes  = algo_field_bytes(&args[0], "iv");
            let aad_bytes = algo_field_bytes(&args[0], "additionalData");
            if iv_bytes.len() < 12 || entry.data.len() < 16 { return JsValue::Undefined; }
            let mut iv = [0u8; 12];
            iv.copy_from_slice(&iv_bytes[..12]);
            let mut key16 = [0u8; 16];
            key16.copy_from_slice(&entry.data[..16]);
            let ct = crate::net::tls::aes128_gcm_encrypt(&key16, &iv, &aad_bytes, &data);
            bytes_to_js(&ct)
        }
        _ => JsValue::Undefined,
    }
}

// crypto.subtle.decrypt({name, iv, additionalData?}, key, data)
fn native_decrypt(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    if args.len() < 3 { return JsValue::Undefined; }
    let alg   = algo_name(&args[0]);
    let id    = match key_id(&args[1]) { Some(id) => id, None => return JsValue::Undefined };
    let entry = match lookup_key(id) { Some(e) => e, None => return JsValue::Undefined };
    let data  = js_to_bytes(&args[2]);

    match alg.as_str() {
        "AES-GCM" => {
            let iv_bytes  = algo_field_bytes(&args[0], "iv");
            let aad_bytes = algo_field_bytes(&args[0], "additionalData");
            if iv_bytes.len() < 12 || entry.data.len() < 16 { return JsValue::Undefined; }
            let mut iv = [0u8; 12];
            iv.copy_from_slice(&iv_bytes[..12]);
            let mut key16 = [0u8; 16];
            key16.copy_from_slice(&entry.data[..16]);
            match crate::net::tls::aes128_gcm_decrypt(&key16, &iv, &aad_bytes, &data) {
                Ok(pt) => bytes_to_js(&pt),
                Err(_) => JsValue::Undefined,
            }
        }
        _ => JsValue::Undefined,
    }
}

// crypto.subtle.sign({name, hash?}, key, data)
fn native_sign(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    if args.len() < 3 { return JsValue::Undefined; }
    let alg   = algo_name(&args[0]);
    let id    = match key_id(&args[1]) { Some(id) => id, None => return JsValue::Undefined };
    let entry = match lookup_key(id) { Some(e) => e, None => return JsValue::Undefined };
    let data  = js_to_bytes(&args[2]);

    match alg.as_str() {
        "HMAC" => {
            let mac = hmac_sha256(&entry.data, &data);
            bytes_to_js(&mac)
        }
        _ => JsValue::Undefined,
    }
}

// crypto.subtle.verify({name, hash?}, key, signature, data)
fn native_verify(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    if args.len() < 4 { return JsValue::Undefined; }
    let alg   = algo_name(&args[0]);
    let id    = match key_id(&args[1]) { Some(id) => id, None => return JsValue::Undefined };
    let entry = match lookup_key(id) { Some(e) => e, None => return JsValue::Undefined };
    let sig   = js_to_bytes(&args[2]);
    let data  = js_to_bytes(&args[3]);

    match alg.as_str() {
        "HMAC" => {
            let expected = hmac_sha256(&entry.data, &data);
            // Constant-time comparison
            if expected.len() != sig.len() { return JsValue::Bool(false); }
            let mut diff = 0u8;
            for (a, b) in expected.iter().zip(sig.iter()) { diff |= a ^ b; }
            JsValue::Bool(diff == 0)
        }
        _ => JsValue::Bool(false),
    }
}

// crypto.subtle.deriveBits({name, salt, iterations, hash}, key, lengthBits)
fn native_derive_bits(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    if args.len() < 3 { return JsValue::Undefined; }
    let alg    = algo_name(&args[0]);
    let id     = match key_id(&args[1]) { Some(id) => id, None => return JsValue::Undefined };
    let entry  = match lookup_key(id) { Some(e) => e, None => return JsValue::Undefined };
    let length = args[2].to_number() as usize / 8; // bits → bytes

    match alg.as_str() {
        "PBKDF2" => {
            let salt  = algo_field_bytes(&args[0], "salt");
            let iters = algo_field_u32(&args[0], "iterations", 100_000);
            let out = pbkdf2_sha256(&entry.data, &salt, iters, length);
            bytes_to_js(&out)
        }
        "HKDF" => {
            let salt  = algo_field_bytes(&args[0], "salt");
            let info  = algo_field_bytes(&args[0], "info");
            // HKDF-Extract: PRK = HMAC(salt, ikm)
            let prk = hmac_sha256(&salt, &entry.data);
            let out = hkdf_sha256_expand(&prk, &info, length);
            bytes_to_js(&out)
        }
        _ => JsValue::Undefined,
    }
}

// crypto.subtle.deriveKey(algorithm, baseKey, derivedKeyAlg, extractable, usages)
fn native_derive_key(args: &[JsValue], interp: &mut Interpreter) -> JsValue {
    if args.len() < 5 { return JsValue::Undefined; }
    // Derive the bits first
    let target_len = algo_field_u32(&args[2], "length", 128) as usize;
    let bits_args = [args[0].clone(), args[1].clone(), JsValue::Number((target_len) as f64)];
    let bits_val = native_derive_bits(&bits_args, interp);
    let key_bytes = js_to_bytes(&bits_val);
    if key_bytes.is_empty() { return JsValue::Undefined; }

    let derived_alg = algo_name(&args[2]);
    let extractable = args[3].is_truthy();
    let id = store_key(CryptoKeyEntry {
        algorithm: derived_alg.clone(),
        hash:      String::new(),
        extractable,
        data:      key_bytes,
    });
    make_crypto_key(id, &derived_alg, "secret", extractable)
}

// crypto.getRandomValues(typedArray)
fn native_get_random_values(args: &[JsValue], _: &mut Interpreter) -> JsValue {
    if args.is_empty() { return JsValue::Undefined; }
    match &args[0] {
        JsValue::Array(arr) => {
            let mut arr = arr.borrow_mut();
            let len = arr.len();
            let mut buf = vec![0u8; len];
            fill_random(&mut buf);
            for (i, b) in buf.iter().enumerate() {
                arr[i] = JsValue::Number(*b as f64);
            }
            drop(arr);
            args[0].clone()
        }
        _ => args[0].clone(),
    }
}

// crypto.randomUUID()
fn native_random_uuid(_args: &[JsValue], _: &mut Interpreter) -> JsValue {
    JsValue::Str(random_uuid())
}

// ── Install ───────────────────────────────────────────────────────────────────

/// Install `crypto` global object on the interpreter.
/// Call from `browser.rs::install_dom_api()`.
pub fn install_webcrypto_api(interp: &mut Interpreter) {
    // Build crypto.subtle object
    let subtle = Rc::new(RefCell::new(JsObject::new()));
    subtle.borrow_mut().set("digest".into(),      JsValue::NativeFunction("digest",      native_digest));
    subtle.borrow_mut().set("generateKey".into(), JsValue::NativeFunction("generateKey", native_generate_key));
    subtle.borrow_mut().set("importKey".into(),   JsValue::NativeFunction("importKey",   native_import_key));
    subtle.borrow_mut().set("exportKey".into(),   JsValue::NativeFunction("exportKey",   native_export_key));
    subtle.borrow_mut().set("encrypt".into(),     JsValue::NativeFunction("encrypt",     native_encrypt));
    subtle.borrow_mut().set("decrypt".into(),     JsValue::NativeFunction("decrypt",     native_decrypt));
    subtle.borrow_mut().set("sign".into(),        JsValue::NativeFunction("sign",        native_sign));
    subtle.borrow_mut().set("verify".into(),      JsValue::NativeFunction("verify",      native_verify));
    subtle.borrow_mut().set("deriveBits".into(),  JsValue::NativeFunction("deriveBits",  native_derive_bits));
    subtle.borrow_mut().set("deriveKey".into(),   JsValue::NativeFunction("deriveKey",   native_derive_key));

    // Build crypto object
    let crypto = Rc::new(RefCell::new(JsObject::new()));
    crypto.borrow_mut().set("subtle".into(),           JsValue::Object(subtle));
    crypto.borrow_mut().set("getRandomValues".into(),  JsValue::NativeFunction("getRandomValues",  native_get_random_values));
    crypto.borrow_mut().set("randomUUID".into(),       JsValue::NativeFunction("randomUUID",       native_random_uuid));

    let crypto_val = JsValue::Object(crypto);
    interp.env.define("crypto".into(), crypto_val.clone());
    // Also define as self.crypto / window.crypto
    interp.env.define("__crypto__".into(), crypto_val);
}

// ── Self-test ─────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] webcrypto: {}", $name); }
        }
    }

    // T1: SHA-256 known vector (empty string)
    {
        use sha2::Digest;
        let hash = sha2::Sha256::digest(b"").to_vec();
        let expected: [u8; 32] = [
            0xe3,0xb0,0xc4,0x42,0x98,0xfc,0x1c,0x14,0x9a,0xfb,0xf4,0xc8,0x99,0x6f,0xb9,0x24,
            0x27,0xae,0x41,0xe4,0x64,0x9b,0x93,0x4c,0xa4,0x95,0x99,0x1b,0x78,0x52,0xb8,0x55,
        ];
        check!(hash.as_slice() == &expected, "SHA-256 empty-string vector");
    }

    // T2: SHA-256 "abc"
    {
        let h = digest_bytes("SHA-256", b"abc").unwrap();
        check!(h[0] == 0xba && h[1] == 0x78 && h[2] == 0x16, "SHA-256 'abc' first bytes");
    }

    // T3: HMAC-SHA-256 known vector (RFC 4231 test case 1)
    {
        let key  = [0x0bu8; 20];
        let data = b"Hi There";
        let mac = hmac_sha256(&key, data);
        // Expected: b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7
        check!(mac[0] == 0xb0 && mac[1] == 0x34 && mac[2] == 0x4c, "HMAC-SHA-256 RFC 4231 T1");
    }

    // T4: PBKDF2 round-trip (just check output length and determinism)
    {
        let pwd  = b"password";
        let salt = b"saltsalt";
        let out1 = pbkdf2_sha256(pwd, salt, 100, 32);
        let out2 = pbkdf2_sha256(pwd, salt, 100, 32);
        check!(out1.len() == 32, "PBKDF2 output length");
        check!(out1 == out2, "PBKDF2 determinism");
        check!(out1.iter().any(|&b| b != 0), "PBKDF2 non-zero output");
    }

    // T5: AES-128-GCM encrypt/decrypt round-trip
    {
        let key16 = [0x42u8; 16];
        let iv    = [0x11u8; 12];
        let aad   = b"aad";
        let plain = b"hello WebCrypto!";
        let ct    = crate::net::tls::aes128_gcm_encrypt(&key16, &iv, aad, plain);
        let pt    = crate::net::tls::aes128_gcm_decrypt(&key16, &iv, aad, &ct);
        check!(pt == Ok(plain.to_vec()), "AES-128-GCM round-trip");
    }

    // T6: HKDF derive bits determinism
    {
        let ikm  = b"input key material";
        let salt = b"some salt value";
        let info = b"context info";
        let prk  = hmac_sha256(salt, ikm);
        let out1 = hkdf_sha256_expand(&prk, info, 32);
        let out2 = hkdf_sha256_expand(&prk, info, 32);
        check!(out1.len() == 32 && out1 == out2, "HKDF-SHA-256 determinism");
    }

    // T7: randomUUID format check (8-4-4-4-12 hex)
    {
        let uuid = random_uuid();
        check!(uuid.len() == 36, "UUID length");
        check!(uuid.chars().nth(8) == Some('-'), "UUID dash at pos 8");
        check!(uuid.chars().nth(13) == Some('-'), "UUID dash at pos 13");
    }

    // T8: js_to_bytes / bytes_to_js round-trip
    {
        let original = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let js_val = bytes_to_js(&original);
        let recovered = js_to_bytes(&js_val);
        check!(recovered == original, "bytes_to_js/js_to_bytes round-trip");
    }

    if fail == 0 {
        crate::serial_println!("[webcrypto] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[webcrypto] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

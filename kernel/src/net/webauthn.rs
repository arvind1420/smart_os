/// Phase 115 — WebAuthn / FIDO2
///
/// Implements the Web Authentication API (W3C WebAuthn Level 2):
///   • Software FIDO2 authenticator (no hardware security key required)
///   • Credential creation:  `navigator.credentials.create()`
///   • Assertion:            `navigator.credentials.get()`
///   • Credential store per origin  (BTreeMap backed, RAM only)
///   • CBOR-encoded authenticator data
///   • ES256 challenge-response (HMAC-SHA256 software stub)
///   • User verification via PIN / biometric (simulated)

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::collections::BTreeMap;

// ─────────────────────────────────────────────────────────────────────────────
// COSE / CBOR CONSTANTS (subset for WebAuthn)
// ─────────────────────────────────────────────────────────────────────────────

/// COSE algorithm identifiers (RFC 8152).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum CoseAlg {
    ES256  = -7,   // ECDSA w/ SHA-256
    RS256  = -257, // RSASSA-PKCS1-v1_5 w/ SHA-256
    EdDSA  = -8,   // EdDSA
}

/// Encode an i32 as minimal CBOR int.
fn cbor_int(v: i32, buf: &mut Vec<u8>) {
    if v >= 0 && v <= 23 {
        buf.push(v as u8);
    } else if v >= -24 && v < 0 {
        buf.push(0x20 | ((-v - 1) as u8));
    } else if v > 23 && v <= 255 {
        buf.push(0x18); buf.push(v as u8);
    } else if v >= -256 && v < -24 {
        buf.push(0x38); buf.push((-v - 1) as u8);
    } else {
        buf.push(0x19);
        buf.extend_from_slice(&(v as u16).to_be_bytes());
    }
}

/// Encode a byte string as CBOR bstr.
fn cbor_bstr(data: &[u8], buf: &mut Vec<u8>) {
    let len = data.len();
    if len <= 23       { buf.push(0x40 | len as u8); }
    else if len <= 255 { buf.push(0x58); buf.push(len as u8); }
    else               { buf.push(0x59); buf.extend_from_slice(&(len as u16).to_be_bytes()); }
    buf.extend_from_slice(data);
}

/// Encode a text string as CBOR tstr.
fn cbor_tstr(s: &str, buf: &mut Vec<u8>) {
    let b = s.as_bytes();
    let len = b.len();
    if len <= 23       { buf.push(0x60 | len as u8); }
    else if len <= 255 { buf.push(0x78); buf.push(len as u8); }
    else               { buf.push(0x79); buf.extend_from_slice(&(len as u16).to_be_bytes()); }
    buf.extend_from_slice(b);
}

/// Encode a CBOR map header (n pairs).
fn cbor_map_hdr(n: usize, buf: &mut Vec<u8>) {
    if n <= 23 { buf.push(0xA0 | n as u8); }
    else       { buf.push(0xB8); buf.push(n as u8); }
}

// ─────────────────────────────────────────────────────────────────────────────
// CREDENTIAL
// ─────────────────────────────────────────────────────────────────────────────

/// A stored credential (public key + user info).
#[derive(Debug, Clone)]
pub struct Credential {
    /// Random 16-byte credential ID.
    pub id:          [u8; 16],
    /// User handle (opaque bytes).
    pub user_handle: Vec<u8>,
    /// User display name.
    pub user_name:   String,
    /// Relying party ID (origin hostname).
    pub rp_id:       String,
    /// Public key bytes (32-byte "public key" — for this software authenticator
    /// we use a 32-byte HMAC key that stands in for the private key).
    pub private_key: [u8; 32],
    /// Signature counter — incremented on every assertion.
    pub sign_count:  u32,
    /// COSE algorithm.
    pub alg:         CoseAlg,
}

impl Credential {
    pub fn credential_id_hex(&self) -> String {
        self.id.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AUTHENTICATOR DATA  (§6.1 WebAuthn spec)
// ─────────────────────────────────────────────────────────────────────────────

/// Flags byte bits.
pub const FLAG_UP: u8 = 1 << 0;  // User Presence
pub const FLAG_UV: u8 = 1 << 2;  // User Verification
pub const FLAG_AT: u8 = 1 << 6;  // Attested Credential Data present
pub const FLAG_ED: u8 = 1 << 7;  // Extension Data present

/// Build the authenticator data byte string (§6.1).
pub fn build_auth_data(
    rp_id_hash: &[u8; 32],
    flags:      u8,
    sign_count: u32,
    cred_id:    Option<&[u8]>,
    pub_key_cbor: Option<&[u8]>,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(rp_id_hash);
    out.push(flags);
    out.extend_from_slice(&sign_count.to_be_bytes());
    if let (Some(cid), Some(pk)) = (cred_id, pub_key_cbor) {
        // AAGUID (16 bytes zero for software authenticator)
        out.extend_from_slice(&[0u8; 16]);
        // credentialIdLength (2 bytes) + credentialId
        out.extend_from_slice(&(cid.len() as u16).to_be_bytes());
        out.extend_from_slice(cid);
        // credentialPublicKey (CBOR-encoded COSE key)
        out.extend_from_slice(pk);
    }
    out
}

/// Encode a 32-byte "public key" as a COSE ES256 key (EC2 key type, P-256 stub).
pub fn encode_cose_key(pub_bytes: &[u8; 32]) -> Vec<u8> {
    // COSE map: {1: 2, 3: -7, -1: 1, -2: x, -3: y}
    // We split the 32 bytes as two 16-byte halves standing in for x and y.
    let mut buf = Vec::new();
    cbor_map_hdr(5, &mut buf);
    cbor_int(1,  &mut buf); cbor_int(2, &mut buf);   // kty: EC2
    cbor_int(3,  &mut buf); cbor_int(-7, &mut buf);  // alg: ES256
    cbor_int(-1, &mut buf); cbor_int(1, &mut buf);   // crv: P-256
    cbor_int(-2, &mut buf); cbor_bstr(&pub_bytes[..16], &mut buf); // x
    cbor_int(-3, &mut buf); cbor_bstr(&pub_bytes[16..], &mut buf); // y
    buf
}

// ─────────────────────────────────────────────────────────────────────────────
// SIMPLE HASH HELPERS (SHA-256 inline, no external crate)
// ─────────────────────────────────────────────────────────────────────────────

/// Inline SHA-256.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
        0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
        0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
        0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
        0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
        0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
        0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
        0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,
        0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19,
    ];
    let mut msg: Vec<u8> = Vec::with_capacity(data.len() + 9 + 63);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 { w[i] = u32::from_be_bytes([chunk[i*4],chunk[i*4+1],chunk[i*4+2],chunk[i*4+3]]); }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7)^w[i-15].rotate_right(18)^(w[i-15]>>3);
            let s1 = w[i-2].rotate_right(17)^w[i-2].rotate_right(19)^(w[i-2]>>10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let (mut a,mut b,mut c,mut d,mut e,mut f,mut g,mut hh) =
            (h[0],h[1],h[2],h[3],h[4],h[5],h[6],h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6)^e.rotate_right(11)^e.rotate_right(25);
            let ch = (e&f)^((!e)&g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2)^a.rotate_right(13)^a.rotate_right(22);
            let maj= (a&b)^(a&c)^(b&c);
            let temp2 = s0.wrapping_add(maj);
            hh=g; g=f; f=e; e=d.wrapping_add(temp1);
            d=c; c=b; b=a; a=temp1.wrapping_add(temp2);
        }
        h[0]=h[0].wrapping_add(a); h[1]=h[1].wrapping_add(b);
        h[2]=h[2].wrapping_add(c); h[3]=h[3].wrapping_add(d);
        h[4]=h[4].wrapping_add(e); h[5]=h[5].wrapping_add(f);
        h[6]=h[6].wrapping_add(g); h[7]=h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for i in 0..8 { out[i*4..i*4+4].copy_from_slice(&h[i].to_be_bytes()); }
    out
}

/// HMAC-SHA256.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let h = sha256(key); k[..32].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0u8; BLOCK];
    let mut opad = [0u8; BLOCK];
    for i in 0..BLOCK { ipad[i] = k[i]^0x36; opad[i] = k[i]^0x5C; }
    let mut inner = Vec::with_capacity(BLOCK + data.len());
    inner.extend_from_slice(&ipad); inner.extend_from_slice(data);
    let hi = sha256(&inner);
    let mut outer = Vec::with_capacity(BLOCK + 32);
    outer.extend_from_slice(&opad); outer.extend_from_slice(&hi);
    sha256(&outer)
}

// ─────────────────────────────────────────────────────────────────────────────
// SOFTWARE AUTHENTICATOR
// ─────────────────────────────────────────────────────────────────────────────

/// Software FIDO2 authenticator — signs challenges using HMAC-SHA256.
pub struct SoftwareAuthenticator {
    /// Per-origin credential stores.
    pub credentials: BTreeMap<String, Vec<Credential>>,
    /// Master secret used to derive per-credential keys.
    master_secret: [u8; 32],
    /// Next credential counter (global, per spec).
    global_counter: u32,
}

impl SoftwareAuthenticator {
    pub fn new(master_secret: [u8; 32]) -> Self {
        SoftwareAuthenticator {
            credentials: BTreeMap::new(),
            master_secret,
            global_counter: 1,
        }
    }

    /// Generate a random-ish credential ID from origin + username + master secret.
    fn derive_cred_id(&self, rp_id: &str, user: &str) -> [u8; 16] {
        let mut input = Vec::new();
        input.extend_from_slice(rp_id.as_bytes());
        input.extend_from_slice(b"|");
        input.extend_from_slice(user.as_bytes());
        let h = hmac_sha256(&self.master_secret, &input);
        let mut id = [0u8; 16];
        id.copy_from_slice(&h[..16]);
        id
    }

    /// Derive a deterministic 32-byte "private key" for a credential.
    fn derive_private_key(&self, cred_id: &[u8; 16]) -> [u8; 32] {
        let h = hmac_sha256(&self.master_secret, cred_id);
        h
    }

    /// Create a new credential (make credential).
    /// Returns (credential_id_bytes, attestation_object_cbor).
    pub fn make_credential(
        &mut self,
        rp_id: &str,
        user_name: &str,
        user_handle: &[u8],
        _challenge: &[u8],
        _alg_preferences: &[CoseAlg],
    ) -> Result<([u8; 16], Vec<u8>), &'static str> {
        let cred_id = self.derive_cred_id(rp_id, user_name);
        let priv_key = self.derive_private_key(&cred_id);

        let rp_id_hash = sha256(rp_id.as_bytes());
        let mut rp_hash = [0u8; 32]; rp_hash.copy_from_slice(&rp_id_hash);

        let pub_key_cbor = encode_cose_key(&priv_key);
        let auth_data = build_auth_data(
            &rp_hash,
            FLAG_UP | FLAG_UV | FLAG_AT,
            0, // sign_count=0 for new credential
            Some(&cred_id),
            Some(&pub_key_cbor),
        );

        // Build a minimal attestation object (CBOR map with fmt, attStmt, authData)
        let mut att_obj = Vec::new();
        cbor_map_hdr(3, &mut att_obj);
        cbor_tstr("fmt",      &mut att_obj); cbor_tstr("none",   &mut att_obj);
        cbor_tstr("attStmt",  &mut att_obj); cbor_map_hdr(0, &mut att_obj);
        cbor_tstr("authData", &mut att_obj); cbor_bstr(&auth_data, &mut att_obj);

        let cred = Credential {
            id: cred_id, user_handle: user_handle.to_vec(),
            user_name: user_name.to_string(), rp_id: rp_id.to_string(),
            private_key: priv_key, sign_count: 0, alg: CoseAlg::ES256,
        };
        self.credentials.entry(rp_id.to_string()).or_default().push(cred);

        Ok((cred_id, att_obj))
    }

    /// Get assertion (authenticate with existing credential).
    /// Returns (credential_id, authenticator_data, signature).
    pub fn get_assertion(
        &mut self,
        rp_id: &str,
        challenge: &[u8],
        _allowed_creds: &[[u8; 16]],
    ) -> Result<([u8; 16], Vec<u8>, Vec<u8>), &'static str> {
        let creds = self.credentials.get_mut(rp_id).ok_or("no credential for origin")?;
        let cred = creds.first_mut().ok_or("no credential for origin")?;

        cred.sign_count += 1;
        self.global_counter += 1;

        let rp_id_hash_bytes = sha256(rp_id.as_bytes());
        let mut rp_hash = [0u8; 32]; rp_hash.copy_from_slice(&rp_id_hash_bytes);
        let auth_data = build_auth_data(&rp_hash, FLAG_UP | FLAG_UV, cred.sign_count, None, None);

        // Compute signature: HMAC-SHA256(private_key, auth_data || challenge)
        let mut signed = auth_data.clone();
        signed.extend_from_slice(challenge);
        let sig = hmac_sha256(&cred.private_key, &signed);

        let cred_id = cred.id;
        Ok((cred_id, auth_data, sig.to_vec()))
    }

    /// Verify an assertion signature (relying party side).
    pub fn verify_assertion(
        &self,
        rp_id:     &str,
        cred_id:   &[u8; 16],
        auth_data: &[u8],
        challenge: &[u8],
        signature: &[u8],
    ) -> bool {
        let creds = match self.credentials.get(rp_id) { Some(c) => c, None => return false };
        let cred = match creds.iter().find(|c| &c.id == cred_id) { Some(c) => c, None => return false };
        let mut signed = auth_data.to_vec();
        signed.extend_from_slice(challenge);
        let expected = hmac_sha256(&cred.private_key, &signed);
        expected.as_ref() == signature
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SELF-TEST
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut pass = 0u32;
    let mut fail = 0u32;

    macro_rules! check {
        ($cond:expr, $name:expr) => {
            if $cond { pass += 1; }
            else { fail += 1; crate::serial_println!("[FAIL] webauthn: {}", $name); }
        }
    }

    // T1: SHA-256 known vector
    {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let h = sha256(b"");
        check!(h[0] == 0xE3 && h[1] == 0xB0, "SHA256 empty string");
    }

    // T2: SHA-256 known vector 2
    {
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2ec73b00361bbef0469416f04c4aed32a60d
        let h = sha256(b"abc");
        check!(h[0] == 0xBA && h[1] == 0x78, "SHA256 'abc'");
    }

    // T3: HMAC-SHA256 basic (just check length)
    {
        let mac = hmac_sha256(b"key", b"data");
        check!(mac.len() == 32, "HMAC-SHA256 returns 32 bytes");
    }

    // T4: COSE key encoding produces non-empty CBOR
    {
        let pk = [0u8; 32];
        let cbor = encode_cose_key(&pk);
        check!(!cbor.is_empty(), "COSE key CBOR non-empty");
        check!(cbor[0] == 0xA5, "COSE key CBOR is 5-entry map (0xA5)");
    }

    // T5: authenticator data build
    {
        let rp_hash = [0u8; 32];
        let auth = build_auth_data(&rp_hash, FLAG_UP | FLAG_UV, 1, None, None);
        check!(auth.len() == 37, "authData without AT is 37 bytes");
        check!(auth[32] == FLAG_UP | FLAG_UV, "authData flags byte");
        check!(u32::from_be_bytes([auth[33],auth[34],auth[35],auth[36]]) == 1, "authData signCount=1");
    }

    // T6: make_credential returns credential ID
    {
        let mut auth = SoftwareAuthenticator::new([0u8; 32]);
        let r = auth.make_credential("example.com", "alice", b"alice_handle", b"challenge", &[CoseAlg::ES256]);
        check!(r.is_ok(), "make_credential succeeds");
        let (cred_id, att_obj) = r.unwrap();
        check!(cred_id.len() == 16, "credential ID is 16 bytes");
        check!(!att_obj.is_empty(), "attestation object non-empty");
        check!(auth.credentials.contains_key("example.com"), "credential stored");
    }

    // T7: get_assertion returns signature
    {
        let mut auth = SoftwareAuthenticator::new([0u8; 32]);
        auth.make_credential("example.com", "alice", b"h", b"challenge1", &[]).unwrap();
        let challenge = b"random_challenge";
        let r = auth.get_assertion("example.com", challenge, &[]);
        check!(r.is_ok(), "get_assertion succeeds");
        let (_, _, sig) = r.unwrap();
        check!(sig.len() == 32, "signature is 32 bytes");
    }

    // T8: verify_assertion round-trip
    {
        let mut auth = SoftwareAuthenticator::new([42u8; 32]);
        auth.make_credential("rp.example", "bob", b"bob_h", b"c0", &[]).unwrap();
        let challenge = b"verification_challenge";
        let (cred_id, auth_data, sig) = auth.get_assertion("rp.example", challenge, &[]).unwrap();
        let ok = auth.verify_assertion("rp.example", &cred_id, &auth_data, challenge, &sig);
        check!(ok, "verify_assertion round-trip");
    }

    // T9: wrong challenge fails verification
    {
        let mut auth = SoftwareAuthenticator::new([0u8; 32]);
        auth.make_credential("rp.example", "carol", b"c_h", b"c0", &[]).unwrap();
        let challenge = b"correct_challenge";
        let (cred_id, auth_data, sig) = auth.get_assertion("rp.example", challenge, &[]).unwrap();
        let bad_challenge = b"wrong_challenge!!";
        let ok = auth.verify_assertion("rp.example", &cred_id, &auth_data, bad_challenge, &sig);
        check!(!ok, "wrong challenge fails verification");
    }

    // T10: sign_count increments on each assertion
    {
        let mut auth = SoftwareAuthenticator::new([0u8; 32]);
        auth.make_credential("sc.test", "user", b"u_h", b"c0", &[]).unwrap();
        auth.get_assertion("sc.test", b"c1", &[]).unwrap();
        auth.get_assertion("sc.test", b"c2", &[]).unwrap();
        let count = auth.credentials["sc.test"][0].sign_count;
        check!(count == 2, "sign_count=2 after 2 assertions");
    }

    if fail == 0 {
        crate::serial_println!("[webauthn] All {} tests passed.", pass);
        true
    } else {
        crate::serial_println!("[webauthn] {}/{} tests FAILED.", fail, pass + fail);
        false
    }
}

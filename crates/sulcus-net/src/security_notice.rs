//! The signed security notice list: lowest safe app version, revoked engines,
//! helpers and keys, and short messages, published as one static file and
//! passed between peers as is. Anyone may relay a notice, so nothing here
//! trusts where it came from: only the signature and the sequence count.
//!
//! Wire format (UTF-8 JSON):
//! `{"v":1,"body":"<base64 of the body JSON>","key":"<hex Ed25519 public key>","sig":"<base64 signature>"}`
//! The signature covers `SIGNING_DOMAIN` followed by the decoded body bytes,
//! so the body never has to be re-serialized the same way to be checked, and
//! a signature made for anything else with the same key cannot pass as one.
//!
//! Each notice is complete, not a change list: the newest accepted one replaces
//! the last. Key trust is the exception: successor keys and revoked keys pile
//! up in `TrustState`, so an old notice can never undo a revocation.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use rustls::{crypto::ring::default_provider, SignatureScheme};

/// Hex Ed25519 public keys the app trusts out of the box. Empty until the
/// offline signing key exists: with no key, every notice is rejected.
const BUILT_IN_KEYS: &[&str] = &[];

pub const SIGNING_DOMAIN: &[u8] = b"SulcusAI security notice v1\n";

/// A notice is a few kilobytes; the cap keeps a hostile peer from making
/// this PC decode and hold something huge.
pub const MAX_NOTICE_BYTES: usize = 256 * 1024;

/// PC clocks drift and time zones get set wrong, so a little slack, but not
/// enough for a notice dated next month to block every real one until then.
pub const MAX_FUTURE_SECS: i64 = 24 * 60 * 60;

const MAX_REVOKED: usize = 2000;
const MAX_KEYS: usize = 16;
const MAX_MESSAGES: usize = 50;
const MAX_TEXT_CHARS: usize = 2000;
const MAX_ID_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeError {
    TooLarge,
    Malformed(String),
    UnsupportedFormat(u64),
    Unsigned,
    UnknownKey,
    BadSignature,
    /// Same sequence as the one already held: harmless when a peer passes it again.
    AlreadyHave,
    Older,
    FromFuture,
    InvalidContent(String),
    /// The notice would revoke every trusted key, leaving nothing to accept the next one.
    NoKeysLeft,
}

impl std::fmt::Display for NoticeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "The security notice is too large."),
            Self::Malformed(e) => write!(f, "The security notice could not be read: {e}"),
            Self::UnsupportedFormat(v) => write!(f, "The security notice uses format {v}, which this version can't read."),
            Self::Unsigned => write!(f, "The security notice is not signed."),
            Self::UnknownKey => write!(f, "The security notice is signed with a key this app doesn't trust."),
            Self::BadSignature => write!(f, "The security notice's signature doesn't match."),
            Self::AlreadyHave => write!(f, "This security notice is already in use."),
            Self::Older => write!(f, "The security notice is older than the one already in use."),
            Self::FromFuture => write!(f, "The security notice is dated in the future."),
            Self::InvalidContent(e) => write!(f, "The security notice has invalid content: {e}"),
            Self::NoKeysLeft => write!(f, "The security notice would leave no trusted key."),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    v: u64,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    sig: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub sequence: u64,
    /// Unix seconds.
    pub issued: i64,
    pub lowest_safe_version: String,
    #[serde(default)]
    pub revoked: Vec<Revoked>,
    #[serde(default)]
    pub revoked_keys: Vec<String>,
    /// Keys to trust from now on, named by a key already trusted, so the
    /// signing key can be replaced without a new app release.
    #[serde(default)]
    pub next_keys: Vec<String>,
    #[serde(default)]
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revoked {
    /// "engine" or "helper"; kept as text so a new kind doesn't make older
    /// apps reject the whole notice.
    pub kind: String,
    pub id: String,
    pub sha256: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
    /// A level added after this version: shown as a warning rather than
    /// rejecting the notice.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub severity: Severity,
    pub text: String,
    /// Shown only to app versions below this one, so a message about a fixed
    /// problem stops appearing once the fix is installed.
    #[serde(default)]
    pub below_version: Option<String>,
}

/// What this PC keeps between notices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustState {
    #[serde(default)]
    pub last_sequence: u64,
    #[serde(default)]
    pub last_issued: i64,
    #[serde(default)]
    pub added_keys: Vec<String>,
    #[serde(default)]
    pub revoked_keys: Vec<String>,
    /// The accepted notice exactly as received, to hand on to peers and to
    /// read again after a restart.
    #[serde(default)]
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub kind: String,
    pub id: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokedHit {
    pub kind: String,
    pub id: String,
    pub sha256: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actions {
    pub update_needed: bool,
    pub lowest_safe_version: String,
    pub revoked: Vec<RevokedHit>,
    pub messages: Vec<Message>,
}

pub type PublicKey = [u8; 32];

pub fn built_in_keys() -> Vec<PublicKey> {
    BUILT_IN_KEYS.iter().filter_map(|k| parse_key(k).ok()).collect()
}

fn parse_key(hex_key: &str) -> Result<PublicKey, NoticeError> {
    let bytes = hex::decode(hex_key.trim()).map_err(|_| NoticeError::InvalidContent("a key is not hex".into()))?;
    bytes.try_into().map_err(|_| NoticeError::InvalidContent("a key is not 32 bytes".into()))
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn ed25519_verify(key: &PublicKey, domain: &[u8], message: &[u8], sig: &[u8]) -> bool {
    if sig.len() != 64 {
        return false;
    }
    let provider = default_provider();
    let Some((_, algs)) = provider.signature_verification_algorithms.mapping.iter().find(|(s, _)| *s == SignatureScheme::ED25519) else {
        return false;
    };
    let mut signed = Vec::with_capacity(domain.len() + message.len());
    signed.extend_from_slice(domain);
    signed.extend_from_slice(message);
    algs.iter().any(|a| a.verify_signature(key, &signed, sig).is_ok())
}

/// Checks the envelope and signature against `keys` and returns the body
/// bytes. Nothing in the body is parsed before the signature holds.
fn verified_body(raw: &[u8], keys: &[PublicKey]) -> Result<Vec<u8>, NoticeError> {
    verified_body_in(raw, keys, SIGNING_DOMAIN)
}

/// The same envelope and keys, signed under another purpose's prefix (a
/// release manifest), so a notice can't pass for a manifest or back.
pub(crate) fn verified_body_in(raw: &[u8], keys: &[PublicKey], domain: &[u8]) -> Result<Vec<u8>, NoticeError> {
    if raw.len() > MAX_NOTICE_BYTES {
        return Err(NoticeError::TooLarge);
    }
    let env: Envelope = serde_json::from_slice(raw).map_err(|e| NoticeError::Malformed(e.to_string()))?;
    if env.v != 1 {
        return Err(NoticeError::UnsupportedFormat(env.v));
    }
    let (Some(key), Some(sig)) = (env.key.filter(|k| !k.is_empty()), env.sig.filter(|s| !s.is_empty())) else {
        return Err(NoticeError::Unsigned);
    };
    let key = parse_key(&key).map_err(|_| NoticeError::UnknownKey)?;
    if !keys.contains(&key) {
        return Err(NoticeError::UnknownKey);
    }
    let body = B64.decode(env.body.unwrap_or_default().as_bytes()).map_err(|_| NoticeError::Malformed("the body is not base64".into()))?;
    let sig = B64.decode(sig.as_bytes()).map_err(|_| NoticeError::BadSignature)?;
    if !ed25519_verify(&key, domain, &body, &sig) {
        return Err(NoticeError::BadSignature);
    }
    Ok(body)
}

fn parse_body(body: &[u8]) -> Result<Notice, NoticeError> {
    let mut n: Notice = serde_json::from_slice(body).map_err(|e| NoticeError::Malformed(e.to_string()))?;
    let bad = |m: &str| Err(NoticeError::InvalidContent(m.into()));
    if n.sequence == 0 {
        return bad("the sequence starts at 1");
    }
    if Version::parse(&n.lowest_safe_version).is_none() {
        return bad("the lowest safe version is not a version");
    }
    if n.revoked.len() > MAX_REVOKED || n.messages.len() > MAX_MESSAGES || n.revoked_keys.len() > MAX_KEYS || n.next_keys.len() > MAX_KEYS {
        return bad("too many entries");
    }
    for r in &mut n.revoked {
        if !is_sha256(&r.sha256) {
            return bad("a revoked file's hash is not SHA-256");
        }
        r.sha256.make_ascii_lowercase();
        if r.id.trim().is_empty() || r.id.chars().count() > MAX_ID_CHARS || r.kind.chars().count() > MAX_ID_CHARS || r.reason.chars().count() > MAX_TEXT_CHARS {
            return bad("a revoked entry's name or reason is empty or too long");
        }
    }
    for k in n.revoked_keys.iter_mut().chain(n.next_keys.iter_mut()) {
        parse_key(k)?;
        *k = k.trim().to_ascii_lowercase();
    }
    for m in &n.messages {
        if m.text.trim().is_empty() || m.text.chars().count() > MAX_TEXT_CHARS {
            return bad("a message is empty or too long");
        }
        if m.below_version.as_deref().is_some_and(|v| Version::parse(v).is_none()) {
            return bad("a message's version is not a version");
        }
    }
    Ok(n)
}

pub fn trusted_keys(built_in: &[PublicKey], state: &TrustState) -> Vec<PublicKey> {
    let revoked: Vec<PublicKey> = state.revoked_keys.iter().filter_map(|k| parse_key(k).ok()).collect();
    built_in
        .iter()
        .copied()
        .chain(state.added_keys.iter().filter_map(|k| parse_key(k).ok()))
        .filter(|k| !revoked.contains(k))
        .collect()
}

/// Checks a received notice and, if it is newer and properly signed, returns
/// it with the state to store. The old state stays valid on any error.
// The page and the fetch use the key-injectable forms; peer passing calls these.
#[allow(dead_code)]
pub fn accept(raw: &[u8], state: &TrustState, now: i64) -> Result<(Notice, TrustState), NoticeError> {
    accept_with(raw, state, &built_in_keys(), now)
}

pub fn accept_with(raw: &[u8], state: &TrustState, built_in: &[PublicKey], now: i64) -> Result<(Notice, TrustState), NoticeError> {
    let body = verified_body(raw, &trusted_keys(built_in, state))?;
    let notice = parse_body(&body)?;
    if notice.issued > now.saturating_add(MAX_FUTURE_SECS) {
        return Err(NoticeError::FromFuture);
    }
    match notice.sequence.cmp(&state.last_sequence) {
        Ordering::Equal => return Err(NoticeError::AlreadyHave),
        Ordering::Less => return Err(NoticeError::Older),
        Ordering::Greater => {}
    }
    if notice.issued < state.last_issued {
        return Err(NoticeError::Older);
    }

    let mut next = state.clone();
    for k in &notice.revoked_keys {
        if !next.revoked_keys.contains(k) {
            next.revoked_keys.push(k.clone());
        }
    }
    for k in &notice.next_keys {
        let already = next.added_keys.contains(k) || parse_key(k).is_ok_and(|p| built_in.contains(&p));
        if !already {
            next.added_keys.push(k.clone());
        }
    }
    if trusted_keys(built_in, &next).is_empty() {
        return Err(NoticeError::NoKeysLeft);
    }
    next.last_sequence = notice.sequence;
    next.last_issued = notice.issued;
    next.notice = Some(String::from_utf8_lossy(raw).into_owned());
    Ok((notice, next))
}

/// The notice already accepted, read back from the stored state. Checked
/// against every key ever trusted, revoked or not, because the notice that
/// retires a key is often signed by that key.
#[allow(dead_code)]
pub fn current(state: &TrustState) -> Option<Notice> {
    current_with(state, &built_in_keys())
}

pub fn current_with(state: &TrustState, built_in: &[PublicKey]) -> Option<Notice> {
    let raw = state.notice.as_ref()?;
    let keys: Vec<PublicKey> = built_in.iter().copied().chain(state.added_keys.iter().filter_map(|k| parse_key(k).ok())).collect();
    let notice = parse_body(&verified_body(raw.as_bytes(), &keys).ok()?).ok()?;
    (notice.sequence == state.last_sequence).then_some(notice)
}

/// What this PC must do about a notice: whether the app needs updating,
/// which installed engines and helpers are revoked, which messages to show.
pub fn assess(notice: &Notice, app_version: &str, installed: &[Installed]) -> Actions {
    let app = Version::parse(app_version);
    // An unreadable app version counts as below everything: an update is
    // the safe answer when it can't be compared.
    let below = |v: &str| match (&app, Version::parse(v)) {
        (Some(a), Some(b)) => a < &b,
        _ => true,
    };
    let revoked = installed
        .iter()
        .filter_map(|i| {
            let r = notice.revoked.iter().find(|r| r.sha256.eq_ignore_ascii_case(i.sha256.trim()))?;
            Some(RevokedHit { kind: i.kind.clone(), id: i.id.clone(), sha256: r.sha256.clone(), reason: r.reason.clone() })
        })
        .collect();
    let mut messages: Vec<Message> = notice
        .messages
        .iter()
        .filter(|m| m.below_version.as_deref().is_none_or(below))
        .cloned()
        .map(|mut m| {
            if m.severity == Severity::Other {
                m.severity = Severity::Warning;
            }
            m
        })
        .collect();
    messages.sort_by(|a, b| b.severity.cmp(&a.severity));
    Actions { update_needed: below(&notice.lowest_safe_version), lowest_safe_version: notice.lowest_safe_version.clone(), revoked, messages }
}

/// Semantic version enough for app versions: numbers, then an optional
/// pre-release, which sorts below the release it leads to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    nums: [u64; 3],
    pre: Vec<String>,
}

impl Version {
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('v');
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        let mut nums = [0u64; 3];
        let mut parts = core.split('.');
        for n in &mut nums {
            let p = parts.next()?;
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            *n = p.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            Some(p) => {
                let ids: Vec<String> = p.split('.').map(str::to_string).collect();
                if ids.iter().any(|i| i.is_empty()) {
                    return None;
                }
                ids
            }
            None => Vec::new(),
        };
        Some(Self { nums, pre })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.nums.cmp(&other.nums).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&other.pre) {
                    let ord = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(x), Ok(y)) => x.cmp(&y),
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        (Err(_), Err(_)) => a.cmp(b),
                    };
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        })
    }
}

#[cfg(any(test, feature = "dev-sign"))]
pub mod dev_sign {
    //! Test-only signing. Release notices are signed offline with the real
    //! key; nothing that can sign ships in the app.
    use super::*;
    use rustls::{crypto::ring::sign::any_eddsa_type, pki_types::PrivatePkcs8KeyDer};

    /// PKCS#8 v1 wrapping of a raw Ed25519 seed.
    fn pkcs8(seed: &[u8; 32]) -> Vec<u8> {
        let mut der = vec![0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];
        der.extend_from_slice(seed);
        der
    }

    pub struct DevKey(std::sync::Arc<dyn rustls::sign::SigningKey>);

    impl DevKey {
        pub fn from_seed(seed: [u8; 32]) -> Self {
            let der = pkcs8(&seed);
            Self(any_eddsa_type(&PrivatePkcs8KeyDer::from(der.as_slice())).expect("test seed"))
        }

        pub fn public(&self) -> PublicKey {
            let spki = self.0.public_key().expect("public key");
            let bytes = spki.as_ref();
            bytes[bytes.len() - 32..].try_into().unwrap()
        }

        pub fn public_hex(&self) -> String {
            hex::encode(self.public())
        }

        pub fn sign_exact(&self, bytes: &[u8]) -> Vec<u8> {
            self.0.choose_scheme(&[SignatureScheme::ED25519]).expect("ed25519").sign(bytes).expect("sign")
        }

        pub fn sign_raw(&self, message: &[u8]) -> Vec<u8> {
            let mut signed = SIGNING_DOMAIN.to_vec();
            signed.extend_from_slice(message);
            self.sign_exact(&signed)
        }

        pub fn sign_body(&self, body: &[u8]) -> String {
            serde_json::json!({ "v": 1, "body": B64.encode(body), "key": self.public_hex(), "sig": B64.encode(self.sign_raw(body)) }).to_string()
        }

        pub fn sign(&self, notice: &Notice) -> String {
            self.sign_body(&serde_json::to_vec(notice).unwrap())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::dev_sign::DevKey;
    use super::*;

    const NOW: i64 = 1_793_000_000;
    const HASH_A: &str = "8ce71f060c08cc8b89115eab0d63209d80537936706544d95c3b936fd0f63095";
    const HASH_B: &str = "5f21f3e6d86b18f4a3177d4fb24de452f84d093f62afbe231e0d13194ca7132c";

    fn key(n: u8) -> DevKey {
        DevKey::from_seed([n; 32])
    }

    fn notice(seq: u64) -> Notice {
        Notice {
            sequence: seq,
            issued: NOW - 60,
            lowest_safe_version: "0.9.49".into(),
            revoked: vec![Revoked { kind: "engine".into(), id: "llama.cpp vulkan-x64".into(), sha256: HASH_A.to_uppercase(), reason: "unsafe model loading".into() }],
            revoked_keys: vec![],
            next_keys: vec![],
            messages: vec![
                Message { severity: Severity::Info, text: "All good.".into(), below_version: None },
                Message { severity: Severity::Critical, text: "Update now.".into(), below_version: Some("0.9.49".into()) },
            ],
        }
    }

    fn accept_one(raw: &str, state: &TrustState, k: &DevKey) -> Result<(Notice, TrustState), NoticeError> {
        accept_with(raw.as_bytes(), state, &[k.public()], NOW)
    }

    #[test]
    fn good_notice_accepted_and_stored() {
        let k = key(1);
        let raw = k.sign(&notice(5));
        let (n, state) = accept_one(&raw, &TrustState::default(), &k).unwrap();
        assert_eq!(n.sequence, 5);
        assert_eq!(n.revoked[0].sha256, HASH_A);
        assert_eq!(state.last_sequence, 5);
        assert_eq!(state.notice.as_deref(), Some(raw.as_str()));
        assert_eq!(current_with(&state, &[k.public()]), Some(n));
    }

    #[test]
    fn stored_state_round_trips() {
        let k = key(1);
        let (_, state) = accept_one(&k.sign(&notice(1)), &TrustState::default(), &k).unwrap();
        let back: TrustState = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(back, state);
        assert!(current_with(&back, &[k.public()]).is_some());
    }

    #[test]
    fn bad_signature_rejected() {
        let k = key(1);
        let mut env: serde_json::Value = serde_json::from_str(&k.sign(&notice(1))).unwrap();
        let mut sig = B64.decode(env["sig"].as_str().unwrap()).unwrap();
        sig[10] ^= 1;
        env["sig"] = B64.encode(&sig).into();
        assert_eq!(accept_one(&env.to_string(), &TrustState::default(), &k), Err(NoticeError::BadSignature));
        env["sig"] = B64.encode(&sig[..63]).into();
        assert_eq!(accept_one(&env.to_string(), &TrustState::default(), &k), Err(NoticeError::BadSignature));
        env["sig"] = "not base64!".into();
        assert_eq!(accept_one(&env.to_string(), &TrustState::default(), &k), Err(NoticeError::BadSignature));
    }

    #[test]
    fn wrong_key_rejected() {
        let trusted = key(1);
        let other = key(2);
        let raw = other.sign(&notice(1));
        assert_eq!(accept_one(&raw, &TrustState::default(), &trusted), Err(NoticeError::UnknownKey));
        // Naming the trusted key while signing with another one is a bad signature, not a pass.
        let mut env: serde_json::Value = serde_json::from_str(&raw).unwrap();
        env["key"] = trusted.public_hex().into();
        assert_eq!(accept_one(&env.to_string(), &TrustState::default(), &trusted), Err(NoticeError::BadSignature));
    }

    #[test]
    fn no_built_in_key_rejects_everything() {
        let raw = key(1).sign(&notice(1));
        assert_eq!(accept_with(raw.as_bytes(), &TrustState::default(), &[], NOW), Err(NoticeError::UnknownKey));
    }

    #[test]
    fn unsigned_rejected() {
        let k = key(1);
        let body = B64.encode(serde_json::to_vec(&notice(1)).unwrap());
        for raw in [
            serde_json::json!({ "v": 1, "body": body }),
            serde_json::json!({ "v": 1, "body": body, "key": k.public_hex() }),
            serde_json::json!({ "v": 1, "body": body, "key": "", "sig": "" }),
        ] {
            assert_eq!(accept_one(&raw.to_string(), &TrustState::default(), &k), Err(NoticeError::Unsigned));
        }
    }

    #[test]
    fn tampered_body_rejected() {
        let k = key(1);
        let mut env: serde_json::Value = serde_json::from_str(&k.sign(&notice(1))).unwrap();
        let mut n = notice(1);
        n.lowest_safe_version = "0.1.0".into();
        env["body"] = B64.encode(serde_json::to_vec(&n).unwrap()).into();
        assert_eq!(accept_one(&env.to_string(), &TrustState::default(), &k), Err(NoticeError::BadSignature));
    }

    #[test]
    fn signature_from_another_purpose_rejected() {
        let k = key(1);
        let body = serde_json::to_vec(&notice(1)).unwrap();
        let raw = serde_json::json!({ "v": 1, "body": B64.encode(&body), "key": k.public_hex(), "sig": B64.encode(k.sign_exact(&body)) });
        assert_eq!(accept_one(&raw.to_string(), &TrustState::default(), &k), Err(NoticeError::BadSignature));
    }

    #[test]
    fn garbled_input_never_panics() {
        let k = key(1);
        let good = k.sign(&notice(1));
        let state = TrustState::default();
        for cut in 0..good.len() {
            assert!(accept_one(&good[..cut], &state, &k).is_err());
        }
        let mut bytes = good.clone().into_bytes();
        for i in (0..bytes.len()).step_by(3) {
            let saved = bytes[i];
            bytes[i] = saved.wrapping_add(1);
            assert!(accept_with(&bytes, &state, &[k.public()], NOW).is_err());
            bytes[i] = saved;
        }
        for raw in ["", "null", "[]", "{}", "{\"v\":\"1\"}", "{\"v\":2,\"body\":\"\",\"key\":\"\",\"sig\":\"\"}", "\u{0}\u{ff}", "{\"v\":1,\"body\":7,\"key\":[],\"sig\":{}}"] {
            assert!(accept_one(raw, &state, &k).is_err(), "{raw}");
        }
        assert!(matches!(accept_with(&[0xff, 0xfe, 0x00], &state, &[k.public()], NOW), Err(NoticeError::Malformed(_))));
        assert_eq!(accept_with(&vec![b' '; MAX_NOTICE_BYTES + 1], &state, &[k.public()], NOW), Err(NoticeError::TooLarge));
        assert_eq!(accept_one(&serde_json::json!({ "v": 2 }).to_string(), &state, &k), Err(NoticeError::UnsupportedFormat(2)));
        // Signed but not a notice, or a notice that breaks the rules.
        assert!(matches!(accept_one(&k.sign_body(b"\xff\x00garbage"), &state, &k), Err(NoticeError::Malformed(_))));
        assert!(matches!(accept_one(&k.sign_body(b"{\"sequence\":1}"), &state, &k), Err(NoticeError::Malformed(_))));
        let mut n = notice(1);
        n.revoked[0].sha256 = "abc".into();
        assert!(matches!(accept_one(&k.sign(&n), &state, &k), Err(NoticeError::InvalidContent(_))));
        let mut n = notice(1);
        n.lowest_safe_version = "soon".into();
        assert!(matches!(accept_one(&k.sign(&n), &state, &k), Err(NoticeError::InvalidContent(_))));
        let mut n = notice(1);
        n.next_keys = vec!["00ff".into()];
        assert!(matches!(accept_one(&k.sign(&n), &state, &k), Err(NoticeError::InvalidContent(_))));
        assert!(matches!(accept_one(&k.sign(&notice(0)), &state, &k), Err(NoticeError::InvalidContent(_))));
    }

    #[test]
    fn replayed_and_older_rejected() {
        let k = key(1);
        let (_, state) = accept_one(&k.sign(&notice(7)), &TrustState::default(), &k).unwrap();
        assert_eq!(accept_one(&k.sign(&notice(7)), &state, &k), Err(NoticeError::AlreadyHave));
        assert_eq!(accept_one(&k.sign(&notice(6)), &state, &k), Err(NoticeError::Older));
        let mut earlier = notice(8);
        earlier.issued = state.last_issued - 1;
        assert_eq!(accept_one(&k.sign(&earlier), &state, &k), Err(NoticeError::Older));
        assert!(accept_one(&k.sign(&notice(8)), &state, &k).is_ok());
    }

    #[test]
    fn far_future_rejected() {
        let k = key(1);
        let mut n = notice(1);
        n.issued = NOW + MAX_FUTURE_SECS + 1;
        assert_eq!(accept_one(&k.sign(&n), &TrustState::default(), &k), Err(NoticeError::FromFuture));
        n.issued = NOW + MAX_FUTURE_SECS - 1;
        assert!(accept_one(&k.sign(&n), &TrustState::default(), &k).is_ok());
        n.issued = i64::MAX;
        assert_eq!(accept_with(k.sign(&n).as_bytes(), &TrustState::default(), &[k.public()], i64::MAX - 5).map(|r| r.0.sequence), Ok(1));
    }

    #[test]
    fn successor_key_handover() {
        let old = key(1);
        let new = key(2);
        let built_in = [old.public()];

        let mut handover = notice(1);
        handover.next_keys = vec![new.public_hex().to_uppercase()];
        handover.revoked_keys = vec![old.public_hex()];
        let (_, state) = accept_with(old.sign(&handover).as_bytes(), &TrustState::default(), &built_in, NOW).unwrap();
        assert_eq!(state.added_keys, vec![new.public_hex()]);
        assert!(current_with(&state, &built_in).is_some(), "the retiring key's own notice stays readable");

        let after = accept_with(new.sign(&notice(2)).as_bytes(), &state, &built_in, NOW).unwrap().1;
        assert_eq!(after.last_sequence, 2);
        assert_eq!(accept_with(old.sign(&notice(3)).as_bytes(), &after, &built_in, NOW), Err(NoticeError::UnknownKey));
    }

    #[test]
    fn untrusted_key_cannot_name_successors() {
        let trusted = key(1);
        let attacker = key(3);
        let mut n = notice(1);
        n.next_keys = vec![attacker.public_hex()];
        assert_eq!(accept_one(&attacker.sign(&n), &TrustState::default(), &trusted), Err(NoticeError::UnknownKey));
    }

    #[test]
    fn revoked_key_stays_revoked() {
        let a = key(1);
        let b = key(2);
        let built_in = [a.public(), b.public()];
        let mut n = notice(1);
        n.revoked_keys = vec![a.public_hex()];
        let (_, state) = accept_with(b.sign(&n).as_bytes(), &TrustState::default(), &built_in, NOW).unwrap();
        assert_eq!(accept_with(a.sign(&notice(2)).as_bytes(), &state, &built_in, NOW), Err(NoticeError::UnknownKey));
        // A later notice that names the revoked key again as a successor doesn't bring it back.
        let mut back = notice(2);
        back.next_keys = vec![a.public_hex()];
        let (_, state) = accept_with(b.sign(&back).as_bytes(), &state, &built_in, NOW).unwrap();
        assert_eq!(accept_with(a.sign(&notice(3)).as_bytes(), &state, &built_in, NOW), Err(NoticeError::UnknownKey));
    }

    #[test]
    fn cannot_revoke_every_key() {
        let k = key(1);
        let mut n = notice(1);
        n.revoked_keys = vec![k.public_hex()];
        assert_eq!(accept_one(&k.sign(&n), &TrustState::default(), &k), Err(NoticeError::NoKeysLeft));
    }

    #[test]
    fn revoked_hashes_found() {
        let n = notice(1);
        let installed = [
            Installed { kind: "engine".into(), id: "llama.cpp".into(), sha256: HASH_A.into() },
            Installed { kind: "helper".into(), id: "ffmpeg".into(), sha256: HASH_B.into() },
        ];
        let a = assess(&n, "0.9.49", &installed);
        assert_eq!(a.revoked.len(), 1);
        assert_eq!(a.revoked[0].id, "llama.cpp");
        assert_eq!(a.revoked[0].reason, "unsafe model loading");
        assert!(assess(&n, "0.9.49", &installed[1..]).revoked.is_empty());
    }

    #[test]
    fn lowest_safe_version_and_messages() {
        let n = notice(1);
        let old = assess(&n, "0.9.48", &[]);
        assert!(old.update_needed);
        assert_eq!(old.messages[0].severity, Severity::Critical, "most severe first");
        assert_eq!(old.messages.len(), 2);
        let current = assess(&n, "0.9.49", &[]);
        assert!(!current.update_needed);
        assert_eq!(current.messages.len(), 1, "the fixed problem's message is gone");
        assert!(!assess(&n, "1.0.0", &[]).update_needed);
        assert!(assess(&n, "0.9.49-beta.2", &[]).update_needed);
        assert!(assess(&n, "not a version", &[]).update_needed);
    }

    #[test]
    fn unknown_severity_shown_as_warning() {
        let k = key(1);
        let body = serde_json::json!({
            "sequence": 1, "issued": NOW, "lowest_safe_version": "0.1.0",
            "messages": [{ "severity": "emergency", "text": "Something new." }],
            "added_later": true
        });
        let (n, _) = accept_one(&k.sign_body(body.to_string().as_bytes()), &TrustState::default(), &k).unwrap();
        assert_eq!(assess(&n, "1.0.0", &[]).messages[0].severity, Severity::Warning);
    }

    #[test]
    fn version_order() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("0.9.10") > v("0.9.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("1.0.0") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.10") > v("1.0.0-rc.9"));
        assert!(v("1.0.0-rc") > v("1.0.0-1"));
        assert!(v("1.0.0-rc.1") > v("1.0.0-rc"));
        assert_eq!(v("v1.2.3+build5"), v("1.2.3"));
        for bad in ["", "1", "1.2", "1.2.3.4", "1..3", "a.b.c", "1.2.3-", "1.2.3-a..b", "-1.2.3", "99999999999999999999.0.0"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    /// Made with `openssl pkeyutl -sign -rawin` and a throwaway key (deleted
    /// afterwards), the way release notices are meant to be signed offline.
    #[test]
    fn openssl_signed_notice_accepted() {
        let raw = r#"{"v":1,"body":"eyJzZXF1ZW5jZSI6MywiaXNzdWVkIjoxNzkzMDAwMDAwLCJsb3dlc3Rfc2FmZV92ZXJzaW9uIjoiMC45LjUwIiwibWVzc2FnZXMiOlt7InNldmVyaXR5IjoiY3JpdGljYWwiLCJ0ZXh0IjoiVXBkYXRlLiJ9XX0=","key":"db1138af9da827e2a1e841ecc7c1ea1daf2b21e4661f8294dd5f6aa2bf475846","sig":"JAA54kKb4NN+gj34XEL/w0FUinJ7KE9UwvtqS+VZL7TiSOIpjPpZUYdqcLGnhvSJucHzAfe8Wq1Vu/bwy9ZmBQ=="}"#;
        let key = parse_key("db1138af9da827e2a1e841ecc7c1ea1daf2b21e4661f8294dd5f6aa2bf475846").unwrap();
        let (n, _) = accept_with(raw.as_bytes(), &TrustState::default(), &[key], NOW).unwrap();
        assert_eq!(n.sequence, 3);
        assert!(assess(&n, "0.9.49", &[]).update_needed);
    }

    #[test]
    fn built_in_keys_are_valid() {
        assert_eq!(built_in_keys().len(), BUILT_IN_KEYS.len());
    }
}

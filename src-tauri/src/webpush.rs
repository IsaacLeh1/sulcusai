// SPDX-License-Identifier: AGPL-3.0-only
//! Notifications on a paired phone, sent straight from this PC (Web Push).
//!
//! The message is encrypted for one phone's browser (RFC 8291, aes128gcm) and
//! posted to the push service that browser named when it subscribed (Apple,
//! Google or Mozilla), signed with this PC's own VAPID key (RFC 8292). That
//! service only relays the sealed bytes: it sees that a notification went to
//! that phone and how big it was, not what it says.
//!
//! The elliptic-curve part (P-256 key pairs, ECDH, ES256) sits behind
//! [`Curve`], and the HTTP post behind [`Transport`], so everything else here
//! runs, and is tested, without either.

use std::future::Future;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::crypto::Cipher;

/// Record size announced in the body header. Push services accept bodies up
/// to 4096 bytes, so one record is all there is.
const RECORD_SIZE: u32 = 4096;
/// An uncompressed P-256 point (0x04 || x || y).
const POINT_LEN: usize = 65;
/// What a message may carry: the record, minus the header (salt, record size,
/// key id length and the sender's point), the delimiter and the GCM tag.
pub const MAX_PAYLOAD: usize = RECORD_SIZE as usize - (16 + 4 + 1 + POINT_LEN) - 1 - 16;
/// RFC 8292 caps a token at 24 hours; half of that leaves room for clock drift.
const JWT_LIFETIME_SECS: i64 = 12 * 60 * 60;

/// P-256 operations, supplied by a curve library.
pub trait Curve {
    /// A fresh key pair: the private scalar and the uncompressed public point.
    fn generate(&self) -> Result<(Zeroizing<[u8; 32]>, [u8; POINT_LEN]), String>;
    fn public_of(&self, private: &[u8; 32]) -> Result<[u8; POINT_LEN], String>;
    /// The x coordinate of the shared point.
    fn ecdh(&self, private: &[u8; 32], peer_public: &[u8]) -> Result<Zeroizing<[u8; 32]>, String>;
    /// ES256: ECDSA over the SHA-256 of `message`, as r || s.
    fn sign(&self, private: &[u8; 32], message: &[u8]) -> Result<[u8; 64], String>;
}

pub struct PushRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub struct PushResponse {
    pub status: u16,
}

/// The one place bytes leave this PC. Tests swap it for a recorder.
pub trait Transport {
    fn post(&self, req: PushRequest) -> impl Future<Output = Result<PushResponse, String>> + Send;
}

/// The app's HTTP client, built by the caller for the user's connection level.
pub struct HttpTransport(pub reqwest::Client);

impl Transport for HttpTransport {
    async fn post(&self, req: PushRequest) -> Result<PushResponse, String> {
        let mut r = self.0.post(&req.url).body(req.body).timeout(std::time::Duration::from_secs(20));
        for (k, v) in &req.headers {
            r = r.header(k.as_str(), v.as_str());
        }
        let resp = r.send().await.map_err(|e| e.to_string())?;
        Ok(PushResponse { status: resp.status().as_u16() })
    }
}

fn unb64(s: &str) -> Result<Vec<u8>, String> {
    B64URL.decode(s.trim().trim_end_matches('=')).map_err(|_| "Not valid base64url.".to_string())
}

/// This PC's identity toward the push services.
pub struct Vapid {
    private: Zeroizing<[u8; 32]>,
    public: [u8; POINT_LEN],
    /// A `mailto:` or `https:` contact the push service can reach.
    subject: String,
}

impl Vapid {
    pub fn generate<C: Curve>(curve: &C, subject: &str) -> Result<Vapid, String> {
        let (private, public) = curve.generate()?;
        Ok(Vapid { private, public, subject: subject.to_string() })
    }

    /// What the phone's browser needs to subscribe (`applicationServerKey`).
    pub fn public_b64(&self) -> String {
        B64URL.encode(self.public)
    }

    /// The compact JWT of RFC 8292 for one push service (`audience` is its origin).
    pub fn jwt<C: Curve>(&self, curve: &C, audience: &str, now_secs: i64) -> Result<String, String> {
        let header = B64URL.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = B64URL.encode(json!({ "aud": audience, "exp": now_secs + JWT_LIFETIME_SECS, "sub": self.subject }).to_string());
        let signing_input = format!("{header}.{claims}");
        let sig = curve.sign(&self.private, signing_input.as_bytes())?;
        Ok(format!("{signing_input}.{}", B64URL.encode(sig)))
    }

    pub fn authorization<C: Curve>(&self, curve: &C, audience: &str, now_secs: i64) -> Result<String, String> {
        Ok(format!("vapid t={}, k={}", self.jwt(curve, audience, now_secs)?, self.public_b64()))
    }
}

/// The key as saved: the public half in the clear, the private half sealed
/// with the app's data key like other stored secrets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredVapid {
    public: String,
    sealed_private: String,
}

/// A phone browser's push subscription, as `PushSubscription.toJSON()` gives it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSubscription {
    pub endpoint: String,
    #[serde(default)]
    pub expiration_time: Option<f64>,
    pub keys: BrowserKeys,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BrowserKeys {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    pub phone: String,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub added: i64,
    /// Milliseconds since 1970, when the browser said the subscription ends.
    #[serde(default)]
    pub expires: Option<i64>,
}

/// Hosts of the browser vendors' push services. A paired phone chooses the
/// endpoint, so anything else (a private address, say) is refused rather than
/// posted to from this PC.
const PUSH_HOSTS: &[&str] = &["googleapis.com", "push.services.mozilla.com", "push.apple.com", "notify.windows.com"];

fn endpoint_allowed(endpoint: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| "The push address isn't valid.".to_string())?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let known = PUSH_HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}")));
    if url.scheme() != "https" || !known || url.username() != "" || url.password().is_some() {
        return Err("That isn't a push service address SulcusAI sends to.".into());
    }
    Ok(url)
}

impl Subscription {
    fn keys(&self) -> Result<([u8; POINT_LEN], [u8; 16]), String> {
        let point: [u8; POINT_LEN] = unb64(&self.p256dh)?.try_into().map_err(|_| "The phone's push key has the wrong size.".to_string())?;
        if point[0] != 0x04 {
            return Err("The phone's push key isn't an uncompressed point.".into());
        }
        let auth: [u8; 16] = unb64(&self.auth)?.try_into().map_err(|_| "The phone's push secret has the wrong size.".to_string())?;
        Ok((point, auth))
    }
}

/// Everything kept for push: this PC's key and each phone's subscriptions.
/// Saved as one JSON text by the caller.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PushStore {
    pub vapid: Option<StoredVapid>,
    pub subs: Vec<Subscription>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Delivered,
    /// The service says the subscription no longer exists (404 or 410).
    Gone,
    /// Anything else, with a reason; the subscription is kept.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Delivery {
    pub endpoint: String,
    pub outcome: Outcome,
}

impl PushStore {
    pub fn from_json(text: &str) -> PushStore {
        serde_json::from_str(text).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The key pair, made on first use and the same ever after.
    pub fn vapid<C: Curve>(&mut self, curve: &C, cipher: &Cipher, subject: &str) -> Result<Vapid, String> {
        if let Some(stored) = &self.vapid {
            let private: [u8; 32] = unb64(&cipher.decrypt(&stored.sealed_private)?)?.try_into().map_err(|_| "The stored push key is damaged.".to_string())?;
            let public = curve.public_of(&private)?;
            if B64URL.encode(public) != stored.public {
                return Err("The stored push key is damaged.".into());
            }
            return Ok(Vapid { private: Zeroizing::new(private), public, subject: subject.to_string() });
        }
        let v = Vapid::generate(curve, subject)?;
        let sealed_private = cipher.encrypt(&B64URL.encode(*v.private));
        self.vapid = Some(StoredVapid { public: v.public_b64(), sealed_private });
        Ok(v)
    }

    /// One subscription per endpoint: subscribing again replaces it.
    pub fn subscribe(&mut self, phone: &str, sub: &BrowserSubscription, now_ms: i64) -> Result<(), String> {
        endpoint_allowed(&sub.endpoint)?;
        let s = Subscription {
            phone: phone.to_string(),
            endpoint: sub.endpoint.clone(),
            p256dh: sub.keys.p256dh.clone(),
            auth: sub.keys.auth.clone(),
            added: now_ms,
            expires: sub.expiration_time.map(|t| t as i64),
        };
        s.keys()?;
        self.subs.retain(|x| x.endpoint != s.endpoint);
        self.subs.push(s);
        Ok(())
    }

    pub fn unsubscribe(&mut self, phone: &str, endpoint: &str) {
        self.subs.retain(|x| !(x.phone == phone && x.endpoint == endpoint));
    }

    /// For a phone that was unpaired.
    pub fn forget_phone(&mut self, phone: &str) {
        self.subs.retain(|x| x.phone != phone);
    }

    /// Who to notify: one phone or all. Subscriptions past their end are
    /// dropped on the way.
    pub fn targets(&mut self, phone: Option<&str>, now_ms: i64) -> Vec<Subscription> {
        self.subs.retain(|x| x.expires.is_none_or(|e| e > now_ms));
        self.subs.iter().filter(|x| phone.is_none_or(|p| x.phone == p)).cloned().collect()
    }

    /// Drops what the push services reported gone. Returns how many.
    pub fn record(&mut self, deliveries: &[Delivery]) -> usize {
        let before = self.subs.len();
        self.subs.retain(|s| !deliveries.iter().any(|d| d.outcome == Outcome::Gone && d.endpoint == s.endpoint));
        before - self.subs.len()
    }
}

/// What the phone's service worker shows.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// A page of the app to open when the notification is tapped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// A newer notification with the same tag replaces the older one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

impl Notification {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

fn content_keys(ecdh: &[u8; 32], auth: &[u8; 16], ua_public: &[u8; POINT_LEN], as_public: &[u8; POINT_LEN], salt: &[u8; 16]) -> Result<([u8; 16], [u8; 12]), String> {
    let bad = |_| "Key derivation failed.".to_string();
    let mut key_info = Vec::with_capacity(14 + 2 * POINT_LEN);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);
    let mut ikm = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(auth), ecdh).expand(&key_info, ikm.as_mut()).map_err(bad)?;
    let prk = Hkdf::<Sha256>::new(Some(salt), ikm.as_ref());
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek).map_err(bad)?;
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce).map_err(bad)?;
    Ok((cek, nonce))
}

/// The request body for one subscription, with the sender's key and the salt
/// given (the published test vector fixes both).
pub fn encrypt_with<C: Curve>(curve: &C, as_private: &[u8; 32], salt: &[u8; 16], sub: &Subscription, plain: &[u8]) -> Result<Vec<u8>, String> {
    if plain.len() > MAX_PAYLOAD {
        return Err(format!("A push message can carry at most {MAX_PAYLOAD} bytes."));
    }
    let (ua_public, auth) = sub.keys()?;
    let as_public = curve.public_of(as_private)?;
    let secret = curve.ecdh(as_private, &ua_public)?;
    let (cek, nonce) = content_keys(&secret, &auth, &ua_public, &as_public, salt)?;
    let mut record = Zeroizing::new(Vec::with_capacity(plain.len() + 1));
    record.extend_from_slice(plain);
    // Marks the last (and only) record.
    record.push(0x02);
    let sealed = Aes128Gcm::new_from_slice(&cek)
        .map_err(|_| "Encryption failed.".to_string())?
        .encrypt(Nonce::from_slice(&nonce), record.as_slice())
        .map_err(|_| "Encryption failed.".to_string())?;
    let mut body = Vec::with_capacity(16 + 4 + 1 + POINT_LEN + sealed.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(POINT_LEN as u8);
    body.extend_from_slice(&as_public);
    body.extend_from_slice(&sealed);
    Ok(body)
}

/// A fresh sender key and salt for every message.
pub fn encrypt<C: Curve>(curve: &C, sub: &Subscription, plain: &[u8]) -> Result<Vec<u8>, String> {
    let (as_private, _) = curve.generate()?;
    let salt: [u8; 16] = crate::sync::random_bytes();
    encrypt_with(curve, &as_private, &salt, sub, plain)
}

pub struct SendOptions {
    /// How long the push service holds the message for a phone that is off.
    pub ttl_secs: u32,
    /// "very-low", "low", "normal" or "high".
    pub urgency: &'static str,
}

impl Default for SendOptions {
    fn default() -> Self {
        SendOptions { ttl_secs: 24 * 60 * 60, urgency: "normal" }
    }
}

pub fn build_request<C: Curve>(curve: &C, vapid: &Vapid, sub: &Subscription, plain: &[u8], opts: &SendOptions, now_secs: i64) -> Result<PushRequest, String> {
    let url = endpoint_allowed(&sub.endpoint)?;
    let audience = url.origin().ascii_serialization();
    let body = encrypt(curve, sub, plain)?;
    Ok(PushRequest {
        url: sub.endpoint.clone(),
        headers: vec![
            ("Authorization".into(), vapid.authorization(curve, &audience, now_secs)?),
            ("Content-Encoding".into(), "aes128gcm".into()),
            ("Content-Type".into(), "application/octet-stream".into()),
            ("TTL".into(), opts.ttl_secs.to_string()),
            ("Urgency".into(), opts.urgency.into()),
        ],
        body,
    })
}

pub async fn send<C: Curve, T: Transport>(curve: &C, transport: &T, vapid: &Vapid, sub: &Subscription, plain: &[u8], opts: &SendOptions, now_secs: i64) -> Outcome {
    let req = match build_request(curve, vapid, sub, plain, opts, now_secs) {
        Ok(r) => r,
        Err(e) => return Outcome::Failed(e),
    };
    match transport.post(req).await {
        Ok(r) if (200..300).contains(&r.status) => Outcome::Delivered,
        Ok(r) if r.status == 404 || r.status == 410 => Outcome::Gone,
        Ok(r) => Outcome::Failed(format!("The push service answered {}.", r.status)),
        Err(e) => Outcome::Failed(e),
    }
}

/// Sends to each subscription in turn (a few phones at most).
pub async fn deliver<C: Curve, T: Transport>(curve: &C, transport: &T, vapid: &Vapid, subs: &[Subscription], plain: &[u8], opts: &SendOptions, now_secs: i64) -> Vec<Delivery> {
    let mut out = Vec::with_capacity(subs.len());
    for s in subs {
        out.push(Delivery { endpoint: s.endpoint.clone(), outcome: send(curve, transport, vapid, s, plain, opts, now_secs).await });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;
    use std::sync::Mutex;

    // RFC 8291, section 5 and appendix A.
    const PLAIN: &str = "When I grow up, I want to be a watermelon";
    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const UA_PUBLIC: &str = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const AS_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    const AS_PUBLIC: &str = "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const ECDH_SECRET: &str = "kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs";
    const BODY: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";

    fn arr<const N: usize>(b64: &str) -> [u8; N] {
        unb64(b64).unwrap().try_into().unwrap()
    }

    /// Not elliptic-curve maths: a public point carries its private scalar, so
    /// the shared secret is symmetric and different on every pair. The one
    /// pair the RFC publishes answers with the RFC's secret, which is what
    /// lets the vector check every other step of the scheme.
    struct Toy;

    impl Curve for Toy {
        fn generate(&self) -> Result<(Zeroizing<[u8; 32]>, [u8; POINT_LEN]), String> {
            let private = Zeroizing::new(crate::sync::random_bytes::<32>());
            let public = self.public_of(&private)?;
            Ok((private, public))
        }

        fn public_of(&self, private: &[u8; 32]) -> Result<[u8; POINT_LEN], String> {
            if *private == arr::<32>(AS_PRIVATE) {
                return Ok(arr(AS_PUBLIC));
            }
            let mut p = [0u8; POINT_LEN];
            p[0] = 0x04;
            p[1..33].copy_from_slice(private);
            p[33..].copy_from_slice(private);
            Ok(p)
        }

        fn ecdh(&self, private: &[u8; 32], peer_public: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
            if *private == arr::<32>(AS_PRIVATE) && peer_public == arr::<POINT_LEN>(UA_PUBLIC) {
                return Ok(Zeroizing::new(arr(ECDH_SECRET)));
            }
            let (a, b) = (private.as_slice(), &peer_public[1..33]);
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            Ok(Zeroizing::new(Sha256::new().chain_update(lo).chain_update(hi).finalize().into()))
        }

        fn sign(&self, private: &[u8; 32], message: &[u8]) -> Result<[u8; 64], String> {
            let h: [u8; 32] = Sha256::new().chain_update(private).chain_update(message).finalize().into();
            let mut s = [0u8; 64];
            s[..32].copy_from_slice(&h);
            s[32..].copy_from_slice(&h);
            Ok(s)
        }
    }

    /// The phone's side, to read back what was sealed (RFC 8291, section 3.4).
    fn open(curve: &Toy, ua_private: &[u8; 32], auth: &[u8; 16], body: &[u8]) -> Vec<u8> {
        let salt: [u8; 16] = body[..16].try_into().unwrap();
        assert_eq!(u32::from_be_bytes(body[16..20].try_into().unwrap()), RECORD_SIZE);
        assert_eq!(body[20] as usize, POINT_LEN);
        let as_public: [u8; POINT_LEN] = body[21..21 + POINT_LEN].try_into().unwrap();
        let ua_public = curve.public_of(ua_private).unwrap();
        let secret = curve.ecdh(ua_private, &as_public).unwrap();
        let (cek, nonce) = content_keys(&secret, auth, &ua_public, &as_public, &salt).unwrap();
        let mut plain = Aes128Gcm::new_from_slice(&cek).unwrap().decrypt(Nonce::from_slice(&nonce), &body[21 + POINT_LEN..]).unwrap();
        assert_eq!(plain.pop(), Some(0x02));
        plain
    }

    fn browser_sub(endpoint: &str, ua_public: &str, auth: &str) -> BrowserSubscription {
        serde_json::from_value(json!({ "endpoint": endpoint, "expirationTime": null, "keys": { "p256dh": ua_public, "auth": auth } })).unwrap()
    }

    const FCM: &str = "https://fcm.googleapis.com/fcm/send/abc123";

    fn sub_with(phone: &str, endpoint: &str) -> Subscription {
        let mut s = PushStore::default();
        s.subscribe(phone, &browser_sub(endpoint, UA_PUBLIC, AUTH), 1_000).unwrap();
        s.subs.remove(0)
    }

    #[test]
    fn rfc8291_published_vector() {
        let sub = sub_with("p1", FCM);
        let body = encrypt_with(&Toy, &arr::<32>(AS_PRIVATE), &arr::<16>(SALT), &sub, PLAIN.as_bytes()).unwrap();
        assert_eq!(B64URL.encode(&body), BODY);
    }

    #[test]
    fn sealed_message_opens_on_the_phone() {
        let ua_private = Toy.generate().unwrap();
        let sub = {
            let mut s = PushStore::default();
            s.subscribe("p1", &browser_sub(FCM, &B64URL.encode(ua_private.1), &B64URL.encode([7u8; 16])), 0).unwrap();
            s.subs.remove(0)
        };
        let body = encrypt(&Toy, &sub, b"{\"title\":\"Done\"}").unwrap();
        assert_eq!(open(&Toy, &ua_private.0, &[7u8; 16], &body), b"{\"title\":\"Done\"}");
        // A fresh salt and sender key each time.
        assert_ne!(body, encrypt(&Toy, &sub, b"{\"title\":\"Done\"}").unwrap());
    }

    #[test]
    fn longest_message_fits_one_record_and_one_more_byte_does_not() {
        let sub = sub_with("p1", FCM);
        let ok = encrypt(&Toy, &sub, &vec![b'x'; MAX_PAYLOAD]).unwrap();
        assert_eq!(ok.len(), RECORD_SIZE as usize);
        assert!(encrypt(&Toy, &sub, &vec![b'x'; MAX_PAYLOAD + 1]).is_err());
    }

    #[test]
    fn vapid_jwt_has_the_rfc8292_shape_and_signs_header_dot_claims() {
        let v = Vapid::generate(&Toy, "mailto:push@example.com").unwrap();
        let jwt = v.jwt(&Toy, "https://fcm.googleapis.com", 1_700_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value = serde_json::from_slice(&unb64(parts[0]).unwrap()).unwrap();
        assert_eq!(header, json!({ "typ": "JWT", "alg": "ES256" }));
        let claims: serde_json::Value = serde_json::from_slice(&unb64(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["sub"], "mailto:push@example.com");
        let exp = claims["exp"].as_i64().unwrap();
        assert!(exp > 1_700_000_000 && exp - 1_700_000_000 <= 24 * 60 * 60);
        let sig = unb64(parts[2]).unwrap();
        assert_eq!(sig.len(), 64);
        assert_eq!(sig, Toy.sign(&v.private, format!("{}.{}", parts[0], parts[1]).as_bytes()).unwrap());
        assert_eq!(unb64(&v.public_b64()).unwrap().len(), POINT_LEN);
    }

    #[test]
    fn request_carries_the_headers_push_services_require() {
        let v = Vapid::generate(&Toy, "mailto:push@example.com").unwrap();
        let sub = sub_with("p1", "https://updates.push.services.mozilla.com/wpush/v2/xyz");
        let req = build_request(&Toy, &v, &sub, b"hi", &SendOptions::default(), 1_700_000_000).unwrap();
        let h = |n: &str| req.headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(h("Content-Encoding"), "aes128gcm");
        assert_eq!(h("TTL"), "86400");
        let auth = h("Authorization");
        assert!(auth.starts_with("vapid t="));
        assert!(auth.ends_with(&format!(", k={}", v.public_b64())));
        let jwt = auth.trim_start_matches("vapid t=").split(", k=").next().unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&unb64(jwt.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://updates.push.services.mozilla.com");
    }

    #[test]
    fn subscriptions_are_added_replaced_and_dropped() {
        let mut s = PushStore::default();
        s.subscribe("p1", &browser_sub(FCM, UA_PUBLIC, AUTH), 10).unwrap();
        s.subscribe("p1", &browser_sub(FCM, UA_PUBLIC, AUTH), 20).unwrap();
        s.subscribe("p2", &browser_sub("https://web.push.apple.com/Qabc", UA_PUBLIC, AUTH), 30).unwrap();
        assert_eq!(s.subs.len(), 2);
        assert_eq!(s.subs[0].added, 20);
        assert_eq!(s.targets(Some("p2"), 40).len(), 1);
        assert_eq!(s.targets(None, 40).len(), 2);
        s.unsubscribe("p1", FCM);
        assert_eq!(s.subs.len(), 1);
        s.forget_phone("p2");
        assert!(s.subs.is_empty());
    }

    #[test]
    fn bad_subscriptions_are_refused() {
        let mut s = PushStore::default();
        for endpoint in ["http://fcm.googleapis.com/x", "https://192.168.1.5/x", "https://localhost/x", "https://evil.example.com/x", "https://notgoogleapis.com/x", "not a url"] {
            assert!(s.subscribe("p1", &browser_sub(endpoint, UA_PUBLIC, AUTH), 0).is_err(), "{endpoint}");
        }
        assert!(s.subscribe("p1", &browser_sub(FCM, "AAAA", AUTH), 0).is_err());
        assert!(s.subscribe("p1", &browser_sub(FCM, UA_PUBLIC, "AAAA"), 0).is_err());
        assert!(s.subs.is_empty());
    }

    #[test]
    fn expired_subscriptions_are_pruned() {
        let mut s = PushStore::default();
        let mut b = browser_sub(FCM, UA_PUBLIC, AUTH);
        b.expiration_time = Some(5_000.0);
        s.subscribe("p1", &b, 0).unwrap();
        assert_eq!(s.targets(None, 4_999).len(), 1);
        assert!(s.targets(None, 5_000).is_empty());
        assert!(s.subs.is_empty());
    }

    #[test]
    fn vapid_key_is_made_once_and_sealed() {
        let cipher = Cipher::for_test();
        let mut s = PushStore::default();
        let first = s.vapid(&Toy, &cipher, "mailto:a@example.com").unwrap();
        let saved = s.to_json();
        // Checked on the sealed field alone: the stand-in curve's public point contains the scalar.
        let sealed = &s.vapid.as_ref().unwrap().sealed_private;
        assert!(sealed.starts_with("enc1:") && !sealed.contains(&B64URL.encode(*first.private)));
        let mut again = PushStore::from_json(&saved);
        let second = again.vapid(&Toy, &cipher, "mailto:a@example.com").unwrap();
        assert_eq!(first.public_b64(), second.public_b64());
        assert_eq!(*first.private, *second.private);
        assert!(PushStore::from_json(&saved).vapid(&Toy, &Cipher::for_test(), "mailto:a@example.com").is_err());
    }

    /// Records what would have been posted; answers with the status given.
    struct Recorder {
        status: Box<dyn Fn(&str) -> Result<u16, String> + Send + Sync>,
        seen: Mutex<Vec<String>>,
    }

    impl Transport for Recorder {
        async fn post(&self, req: PushRequest) -> Result<PushResponse, String> {
            self.seen.lock().unwrap().push(req.url.clone());
            (self.status)(&req.url).map(|status| PushResponse { status })
        }
    }

    #[tokio::test]
    async fn dead_subscriptions_are_dropped_and_the_rest_kept() {
        let cipher = Cipher::for_test();
        let mut store = PushStore::default();
        let v = store.vapid(&Toy, &cipher, "mailto:a@example.com").unwrap();
        for (phone, endpoint) in [
            ("p1", "https://fcm.googleapis.com/ok"),
            ("p2", "https://fcm.googleapis.com/gone404"),
            ("p3", "https://fcm.googleapis.com/gone410"),
            ("p4", "https://fcm.googleapis.com/busy"),
            ("p5", "https://fcm.googleapis.com/offline"),
        ] {
            store.subscribe(phone, &browser_sub(endpoint, UA_PUBLIC, AUTH), 0).unwrap();
        }
        let t = Recorder {
            status: Box::new(|url| match url.rsplit('/').next().unwrap() {
                "gone404" => Ok(404),
                "gone410" => Ok(410),
                "busy" => Ok(503),
                "offline" => Err("no route".into()),
                _ => Ok(201),
            }),
            seen: Mutex::new(Vec::new()),
        };
        let targets = store.targets(None, 1);
        let results = deliver(&Toy, &t, &v, &targets, b"{}", &SendOptions::default(), 1_700_000_000).await;
        assert_eq!(t.seen.lock().unwrap().len(), 5);
        assert_eq!(results[0].outcome, Outcome::Delivered);
        assert_eq!(results[1].outcome, Outcome::Gone);
        assert_eq!(results[2].outcome, Outcome::Gone);
        assert!(matches!(results[3].outcome, Outcome::Failed(_)));
        assert!(matches!(results[4].outcome, Outcome::Failed(_)));
        assert_eq!(store.record(&results), 2);
        let left: Vec<&str> = store.subs.iter().map(|s| s.phone.as_str()).collect();
        assert_eq!(left, ["p1", "p4", "p5"]);
    }

    #[test]
    fn notification_json_leaves_out_what_is_unset() {
        let n = Notification { title: "Done".into(), body: "Your picture is ready".into(), url: None, tag: Some("pic".into()) };
        let v: serde_json::Value = serde_json::from_slice(&n.to_bytes()).unwrap();
        assert_eq!(v, json!({ "title": "Done", "body": "Your picture is ready", "tag": "pic" }));
    }
}

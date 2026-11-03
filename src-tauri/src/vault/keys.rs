//! The vault's own keys. Nothing here touches SulcusAI's data key, so a
//! stolen app key (or an unlocked app) never opens the vault.
//!
//! A random vault key encrypts the entries. It is stored twice, wrapped by a
//! key from the master password and by a key from the recovery key, so either
//! one opens the vault and the master password can change without
//! re-encrypting anything.

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use zeroize::{Zeroize, Zeroizing};

use super::VaultError;

pub(crate) type Key = Zeroizing<[u8; 32]>;

const NONCE_LEN: usize = 12;
const SALT_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KdfCost {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

// A master password is chosen by a person, so each guess is made as costly as
// a desktop can bear at unlock: 256 MiB per guess rules out cheap GPU farms.
pub(crate) const PASSWORD_COST: KdfCost = KdfCost { m_kib: 256 * 1024, t: 3, p: 4 };
// The recovery key is 150 random bits; guessing it is hopeless at any cost.
pub(crate) const RECOVERY_COST: KdfCost = KdfCost { m_kib: 64 * 1024, t: 2, p: 1 };

#[cfg(test)]
pub(crate) const TEST_COST: KdfCost = KdfCost { m_kib: 64, t: 1, p: 1 };

impl KdfCost {
    /// A damaged or hostile file must not make an unlock eat all memory or
    /// run for hours. The lower end is left to Argon2 itself: lowering the
    /// cost in the file gains an attacker nothing, since the wrapped key was
    /// sealed under the original cost.
    fn check(&self) -> Result<(), VaultError> {
        if self.m_kib > 4 * 1024 * 1024 || self.t == 0 || self.t > 32 || self.p == 0 || self.p > 16 {
            return Err(VaultError::Damaged("key settings out of range".into()));
        }
        Ok(())
    }
}

fn derive(secret: &[u8], salt: &[u8], cost: &KdfCost) -> Result<Key, VaultError> {
    cost.check()?;
    let params = Params::new(cost.m_kib, cost.t, cost.p, Some(32)).map_err(|e| VaultError::Damaged(e.to_string()))?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(secret, salt, out.as_mut())
        .map_err(|e| VaultError::Damaged(e.to_string()))?;
    Ok(out)
}

pub(crate) fn random_key() -> Key {
    let mut k = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(k.as_mut());
    k
}

pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

/// AES-256-GCM with a fresh random nonce. The AAD ties each blob to its
/// vault and purpose, so blobs can't be swapped between files or fields.
pub(crate) fn seal(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let nonce: [u8; NONCE_LEN] = random_bytes();
    let ct = Aes256Gcm::new(key.into())
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
        .expect("AES-GCM encryption");
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

pub(crate) fn open(key: &[u8; 32], aad: &[u8], blob: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if blob.len() < NONCE_LEN + 16 {
        return None;
    }
    let (nonce, ct) = blob.split_at(NONCE_LEN);
    Aes256Gcm::new(key.into()).decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad }).ok().map(Zeroizing::new)
}

/// The same password typed on another keyboard or OS can arrive composed or
/// decomposed; NFC makes both the same bytes.
pub(crate) fn normalize_password(password: &str) -> Zeroizing<String> {
    Zeroizing::new(password.nfc().collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Wrapped {
    salt: String,
    cost: KdfCost,
    blob: String,
}

pub(crate) fn wrap(secret: &[u8], key: &[u8; 32], cost: KdfCost, aad: &[u8]) -> Result<Wrapped, VaultError> {
    let salt: [u8; SALT_LEN] = random_bytes();
    let kek = derive(secret, &salt, &cost)?;
    Ok(Wrapped { salt: B64.encode(salt), cost, blob: B64.encode(seal(&kek, aad, key)) })
}

/// `Ok(None)` means the secret was wrong; `Err` means the file is damaged.
pub(crate) fn unwrap(secret: &[u8], w: &Wrapped, aad: &[u8]) -> Result<Option<Key>, VaultError> {
    let salt = B64.decode(&w.salt).map_err(|_| VaultError::Damaged("bad salt".into()))?;
    let blob = B64.decode(&w.blob).map_err(|_| VaultError::Damaged("bad wrapped key".into()))?;
    if salt.len() < 8 {
        return Err(VaultError::Damaged("bad salt".into()));
    }
    let kek = derive(secret, &salt, &w.cost)?;
    let Some(raw) = open(&kek, aad, &blob) else { return Ok(None) };
    let arr: [u8; 32] = raw.as_slice().try_into().map_err(|_| VaultError::Damaged("bad key length".into()))?;
    Ok(Some(Zeroizing::new(arr)))
}

/// A key straight from a password, for exports that carry their own salt.
pub(crate) fn password_key(password: &str, salt: &[u8], cost: &KdfCost) -> Result<Key, VaultError> {
    let pw = normalize_password(password);
    derive(pw.as_bytes(), salt, cost)
}

pub(crate) fn salt_b64() -> (String, [u8; SALT_LEN]) {
    let s: [u8; SALT_LEN] = random_bytes();
    (B64.encode(s), s)
}

pub(crate) fn b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

pub(crate) fn unb64(text: &str) -> Result<Vec<u8>, VaultError> {
    B64.decode(text.trim()).map_err(|_| VaultError::Damaged("bad base64".into()))
}

// Crockford base32: no I, L, O or U, so a printed key reads back without
// confusing 1/I/L or 0/O.
const RECOVERY_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const RECOVERY_CHARS: usize = 30;

/// Six groups of five, 150 random bits.
pub(crate) fn new_recovery_key() -> Zeroizing<String> {
    let mut bytes: Zeroizing<[u8; RECOVERY_CHARS]> = Zeroizing::new(random_bytes());
    // 256 is a multiple of 32, so the low five bits are uniform.
    let mut out = Zeroizing::new(String::with_capacity(RECOVERY_CHARS + 6));
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && i % 5 == 0 {
            out.push('-');
        }
        out.push(RECOVERY_ALPHABET[(*b & 31) as usize] as char);
    }
    bytes.zeroize();
    out
}

/// Accepts the key as typed: any case, spaces or dashes, and the letters
/// people misread (O for 0, I and L for 1).
pub(crate) fn normalize_recovery_key(typed: &str) -> Zeroizing<String> {
    Zeroizing::new(
        typed
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            })
            .collect(),
    )
}

/// Wrong guesses through the app wait longer and longer. It lives in the
/// vault file, so restarting SulcusAI doesn't reset it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Lockout {
    #[serde(default)]
    pub failed: u32,
    #[serde(default)]
    pub until_ms: i64,
}

// A few typos cost nothing; after that the wait doubles from one second.
const FREE_TRIES: u32 = 3;
pub(crate) const MAX_WAIT_MS: i64 = 15 * 60 * 1000;

pub(crate) fn wait_after(failed: u32) -> i64 {
    if failed < FREE_TRIES {
        return 0;
    }
    let doublings = (failed - FREE_TRIES).min(20);
    (1000i64 << doublings).min(MAX_WAIT_MS)
}

impl Lockout {
    /// Clamped, so a clock set back or an edited file can't lock the vault
    /// for longer than the longest real wait.
    pub fn wait_ms(&self, now_ms: i64) -> i64 {
        self.until_ms.saturating_sub(now_ms).clamp(0, MAX_WAIT_MS)
    }

    pub fn failure(&mut self, now_ms: i64) {
        self.failed = self.failed.saturating_add(1);
        self.until_ms = now_ms.saturating_add(wait_after(self.failed));
    }

    pub fn success(&mut self) {
        *self = Lockout::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_round_trips_and_refuses_wrong_secret_or_aad() {
        let key = random_key();
        let w = wrap(b"correct horse", &key, TEST_COST, b"aad-1").unwrap();
        assert_eq!(*unwrap(b"correct horse", &w, b"aad-1").unwrap().unwrap(), *key);
        assert!(unwrap(b"correct hors", &w, b"aad-1").unwrap().is_none());
        assert!(unwrap(b"correct horse", &w, b"aad-2").unwrap().is_none());
    }

    #[test]
    fn hostile_costs_are_refused_before_hashing() {
        let key = random_key();
        let mut w = wrap(b"pw", &key, TEST_COST, b"a").unwrap();
        w.cost.m_kib = u32::MAX;
        assert!(matches!(unwrap(b"pw", &w, b"a"), Err(VaultError::Damaged(_))));
        w.cost = KdfCost { m_kib: 64, t: 0, p: 1 };
        assert!(matches!(unwrap(b"pw", &w, b"a"), Err(VaultError::Damaged(_))));
    }

    #[test]
    fn tampered_blob_fails_closed() {
        let key = random_key();
        let blob = seal(&key, b"x", b"secret");
        for i in 0..blob.len() {
            let mut b = blob.clone();
            b[i] ^= 1;
            assert!(open(&key, b"x", &b).is_none());
        }
        assert!(open(&key, b"x", &blob[..10]).is_none());
        assert_eq!(open(&key, b"x", &blob).unwrap().as_slice(), b"secret");
    }

    #[test]
    fn recovery_key_shape_and_typing() {
        let k = new_recovery_key();
        assert_eq!(k.len(), 35);
        assert_eq!(k.split('-').count(), 6);
        assert!(k.chars().all(|c| c == '-' || RECOVERY_ALPHABET.contains(&(c as u8))));
        let typed = k.to_lowercase().replace('-', " ").replace('0', "o").replace('1', "l");
        assert_eq!(*normalize_recovery_key(&typed), k.replace('-', ""));
        assert_ne!(*new_recovery_key(), *k);
    }

    #[test]
    fn composed_and_decomposed_passwords_match() {
        assert_eq!(*normalize_password("caf\u{e9}"), *normalize_password("cafe\u{301}"));
    }

    #[test]
    fn lockout_waits_grow_and_are_capped() {
        let mut l = Lockout::default();
        let now = 1_000_000;
        for _ in 0..FREE_TRIES - 1 {
            l.failure(now);
            assert_eq!(l.wait_ms(now), 0);
        }
        let mut last = 0;
        for _ in 0..30 {
            l.failure(now);
            let w = l.wait_ms(now);
            assert!(w >= last && w <= MAX_WAIT_MS);
            last = w;
        }
        assert_eq!(last, MAX_WAIT_MS);
        assert_eq!(l.wait_ms(now + MAX_WAIT_MS), 0);
        l.until_ms = i64::MAX;
        assert_eq!(l.wait_ms(now), MAX_WAIT_MS);
        l.success();
        assert_eq!(l, Lockout::default());
    }

    #[test]
    fn default_password_cost_is_accepted_by_argon2() {
        Params::new(PASSWORD_COST.m_kib, PASSWORD_COST.t, PASSWORD_COST.p, Some(32)).unwrap();
        Params::new(RECOVERY_COST.m_kib, RECOVERY_COST.t, RECOVERY_COST.p, Some(32)).unwrap();
        PASSWORD_COST.check().unwrap();
    }
}

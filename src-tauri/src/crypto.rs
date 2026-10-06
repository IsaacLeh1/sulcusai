// SPDX-License-Identifier: AGPL-3.0-only
//! Encryption at rest.
//!
//! One random 256-bit data key encrypts the sensitive columns (chat titles,
//! messages, the profile) with AES-256-GCM. The data key is only ever stored
//! wrapped, in `keys.json`:
//! - App lock off: wrapped by Windows DPAPI, so only this Windows account can
//!   open it and the app unlocks by itself.
//! - App lock on: wrapped by a key derived from the PIN (Argon2id) and then by
//!   DPAPI, so guessing the PIN needs this Windows account too. A one-time
//!   recovery code holds a second copy. Windows Hello, if turned on, keeps a
//!   DPAPI copy that the app only opens after Hello says yes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const PREFIX: &str = "enc1:";
const NONCE_LEN: usize = 12;
pub const MIN_PIN_LEN: usize = 4;
/// Crockford base32 without I, L, O, U, so codes are easy to read aloud.
const CODE_ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

type Key = Zeroizing<[u8; 32]>;

/// Encrypts and decrypts stored text with the data key.
#[derive(Clone)]
pub struct Cipher(Arc<Key>);

impl Cipher {
    pub fn encrypt(&self, plain: &str) -> String {
        format!("{PREFIX}{}", B64.encode(seal(&self.0, plain.as_bytes())))
    }

    /// Text saved before encryption existed passes through unchanged.
    pub fn decrypt(&self, stored: &str) -> Result<String, String> {
        let Some(b64) = stored.strip_prefix(PREFIX) else {
            return Ok(stored.to_string());
        };
        let blob = B64.decode(b64).map_err(|_| "Stored data is damaged.".to_string())?;
        let plain = open(&self.0, &blob).ok_or("Stored data couldn't be decrypted.")?;
        String::from_utf8(plain).map_err(|_| "Stored data is damaged.".to_string())
    }

    /// For lists: an undecryptable value shows as a placeholder instead of
    /// failing the whole list.
    pub fn decrypt_or(&self, stored: &str, fallback: &str) -> String {
        self.decrypt(stored).unwrap_or_else(|_| fallback.to_string())
    }

    pub fn is_encrypted(stored: &str) -> bool {
        stored.starts_with(PREFIX)
    }
}

fn seal(key: &[u8; 32], plain: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new(key.into());
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let ct = cipher.encrypt(Nonce::from_slice(&nonce), plain).expect("AES-GCM encryption");
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

fn open(key: &[u8; 32], blob: &[u8]) -> Option<Vec<u8>> {
    if blob.len() < NONCE_LEN {
        return None;
    }
    let (nonce, ct) = blob.split_at(NONCE_LEN);
    Aes256Gcm::new(key.into()).decrypt(Nonce::from_slice(nonce), ct).ok()
}

/// Seals data so only this Windows account can open it (DPAPI on Windows).
pub trait Protector: Send + Sync {
    fn protect(&self, data: &[u8]) -> Result<Vec<u8>, String>;
    fn unprotect(&self, data: &[u8]) -> Result<Vec<u8>, String>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Wrapped {
    salt: String,
    blob: String,
    m_kib: u32,
    t: u32,
    p: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct KeyFile {
    version: u32,
    #[serde(default)]
    dpapi: Option<String>,
    #[serde(default)]
    pin: Option<Wrapped>,
    #[serde(default)]
    recovery: Option<Wrapped>,
    #[serde(default)]
    hello: Option<String>,
}

struct KdfCost {
    m_kib: u32,
    t: u32,
    p: u32,
}

// PINs can be short, so make each guess expensive.
const PIN_COST: KdfCost = KdfCost { m_kib: 64 * 1024, t: 3, p: 1 };
// Recovery codes carry 100 random bits; a lighter cost is plenty.
const CODE_COST: KdfCost = KdfCost { m_kib: 19 * 1024, t: 2, p: 1 };

fn derive(secret: &str, salt: &[u8], m_kib: u32, t: u32, p: u32) -> Result<Key, String> {
    let params = Params::new(m_kib, t, p, Some(32)).map_err(|e| e.to_string())?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(secret.as_bytes(), salt, out.as_mut())
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn wrap(secret: &str, data_key: &[u8; 32], cost: &KdfCost) -> Result<Wrapped, String> {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    let kek = derive(secret, &salt, cost.m_kib, cost.t, cost.p)?;
    Ok(Wrapped {
        salt: B64.encode(salt),
        blob: B64.encode(seal(&kek, data_key)),
        m_kib: cost.m_kib,
        t: cost.t,
        p: cost.p,
    })
}

fn unwrap(secret: &str, w: &Wrapped) -> Option<Key> {
    let salt = B64.decode(&w.salt).ok()?;
    let blob = B64.decode(&w.blob).ok()?;
    let kek = derive(secret, &salt, w.m_kib, w.t, w.p).ok()?;
    to_key(open(&kek, &blob)?)
}

fn to_key(bytes: Vec<u8>) -> Option<Key> {
    let bytes = Zeroizing::new(bytes);
    let arr: [u8; 32] = bytes.as_slice().try_into().ok()?;
    Some(Zeroizing::new(arr))
}

/// Normalizes a typed recovery code: case, spaces, dashes and look-alikes.
fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect()
}

fn new_recovery_code() -> String {
    let mut bytes = [0u8; 20];
    OsRng.fill_bytes(&mut bytes);
    let chars: String = bytes.iter().map(|b| CODE_ALPHABET[(*b % 32) as usize] as char).collect();
    chars.as_bytes().chunks(5).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("-")
}

pub struct Vault {
    path: PathBuf,
    file: KeyFile,
    key: Option<Cipher>,
    protector: Box<dyn Protector>,
    failed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnlockWith {
    Pin,
    RecoveryCode,
}

impl Vault {
    /// Opens `keys.json`, creating a new data key on first run.
    pub fn open(path: &Path, protector: Box<dyn Protector>) -> Result<Vault, String> {
        let mut vault = Vault { path: path.to_path_buf(), file: KeyFile::default(), key: None, protector, failed: 0 };
        if path.exists() {
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            vault.file = serde_json::from_str(&text).map_err(|e| format!("keys.json is damaged: {e}"))?;
            if let Some(d) = vault.file.dpapi.clone() {
                let raw = vault.protector.unprotect(&B64.decode(d).map_err(|e| e.to_string())?)?;
                vault.key = Some(Cipher(Arc::new(to_key(raw).ok_or("keys.json is damaged")?)));
            }
        } else {
            let mut dk = Zeroizing::new([0u8; 32]);
            OsRng.fill_bytes(dk.as_mut());
            vault.file = KeyFile { version: 1, dpapi: Some(B64.encode(vault.protector.protect(dk.as_ref())?)), ..Default::default() };
            vault.key = Some(Cipher(Arc::new(dk)));
            vault.save()?;
        }
        Ok(vault)
    }

    fn save(&self) -> Result<(), String> {
        let tmp = self.path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(&self.file).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn cipher(&self) -> Option<Cipher> {
        self.key.clone()
    }

    pub fn locked(&self) -> bool {
        self.key.is_none()
    }

    pub fn lock_enabled(&self) -> bool {
        self.file.pin.is_some()
    }

    pub fn hello_enabled(&self) -> bool {
        self.file.hello.is_some()
    }

    /// Forgets the data key. Does nothing when app lock is off.
    pub fn lock(&mut self) {
        if self.lock_enabled() {
            self.key = None;
        }
    }

    fn data_key(&self) -> Result<Key, String> {
        let c = self.key.as_ref().ok_or("SulcusAI is locked.")?;
        Ok(Zeroizing::new(**c.0))
    }

    fn wrap_pin(&self, pin: &str, dk: &[u8; 32]) -> Result<Wrapped, String> {
        let mut w = wrap(pin, dk, &PIN_COST)?;
        // Second layer: the PIN alone isn't enough without this Windows account.
        let inner = B64.decode(&w.blob).map_err(|e| e.to_string())?;
        w.blob = B64.encode(self.protector.protect(&inner)?);
        Ok(w)
    }

    fn unwrap_pin(&self, pin: &str) -> Option<Key> {
        let w = self.file.pin.as_ref()?;
        let outer = B64.decode(&w.blob).ok()?;
        let inner = self.protector.unprotect(&outer).ok()?;
        unwrap(pin, &Wrapped { blob: B64.encode(inner), ..w.clone() })
    }

    fn check_pin_rules(pin: &str) -> Result<(), String> {
        if pin.chars().count() < MIN_PIN_LEN {
            return Err(format!("Use at least {MIN_PIN_LEN} characters."));
        }
        Ok(())
    }

    /// Turns on app lock. Returns the recovery code to show the user once.
    pub fn enable_lock(&mut self, pin: &str) -> Result<String, String> {
        Self::check_pin_rules(pin)?;
        if self.lock_enabled() {
            return Err("App lock is already on.".into());
        }
        let dk = self.data_key()?;
        let code = new_recovery_code();
        self.file.pin = Some(self.wrap_pin(pin, &dk)?);
        self.file.recovery = Some(wrap(&normalize_code(&code), &dk, &CODE_COST)?);
        self.file.dpapi = None;
        self.save()?;
        Ok(code)
    }

    pub fn change_pin(&mut self, old: &str, new: &str) -> Result<(), String> {
        Self::check_pin_rules(new)?;
        let dk = self.unwrap_pin(old).ok_or("That PIN isn't right.")?;
        self.file.pin = Some(self.wrap_pin(new, &dk)?);
        self.save()
    }

    /// After unlocking with the recovery code: set a new PIN and replace the
    /// used code. Returns the new recovery code.
    pub fn reset_pin(&mut self, new: &str) -> Result<String, String> {
        Self::check_pin_rules(new)?;
        let dk = self.data_key()?;
        let code = new_recovery_code();
        self.file.pin = Some(self.wrap_pin(new, &dk)?);
        self.file.recovery = Some(wrap(&normalize_code(&code), &dk, &CODE_COST)?);
        self.save()?;
        Ok(code)
    }

    pub fn disable_lock(&mut self, pin: &str) -> Result<(), String> {
        let dk = self.unwrap_pin(pin).ok_or("That PIN isn't right.")?;
        self.file.dpapi = Some(B64.encode(self.protector.protect(dk.as_ref())?));
        self.file.pin = None;
        self.file.recovery = None;
        self.file.hello = None;
        self.save()?;
        self.key = Some(Cipher(Arc::new(dk)));
        Ok(())
    }

    /// Waiting time before the next attempt, growing after repeated mistakes.
    pub fn penalty(&self) -> Duration {
        match self.failed {
            0..=2 => Duration::ZERO,
            n => Duration::from_secs((1u64 << (n - 2).min(5)).min(30)),
        }
    }

    pub fn unlock(&mut self, how: UnlockWith, secret: &str) -> Result<(), String> {
        let key = match how {
            UnlockWith::Pin => self.unwrap_pin(secret),
            UnlockWith::RecoveryCode => self.file.recovery.as_ref().and_then(|w| unwrap(&normalize_code(secret), w)),
        };
        match key {
            Some(k) => {
                self.failed = 0;
                self.key = Some(Cipher(Arc::new(k)));
                Ok(())
            }
            None => {
                self.failed += 1;
                Err(match how {
                    UnlockWith::Pin => "That PIN isn't right.",
                    UnlockWith::RecoveryCode => "That recovery code isn't right.",
                }
                .into())
            }
        }
    }

    /// Stores (or removes) the Windows Hello copy of the data key.
    pub fn set_hello(&mut self, on: bool) -> Result<(), String> {
        if !self.lock_enabled() {
            return Err("Turn on app lock first.".into());
        }
        self.file.hello = if on {
            let dk = self.data_key()?;
            Some(B64.encode(self.protector.protect(dk.as_ref())?))
        } else {
            None
        };
        self.save()
    }

    /// Call only after Windows Hello has verified the user.
    pub fn unlock_after_hello(&mut self) -> Result<(), String> {
        let blob = self.file.hello.as_ref().ok_or("Windows Hello isn't set up.")?;
        let raw = self.protector.unprotect(&B64.decode(blob).map_err(|e| e.to_string())?)?;
        self.key = Some(Cipher(Arc::new(to_key(raw).ok_or("keys.json is damaged")?)));
        self.failed = 0;
        Ok(())
    }
}

#[cfg(windows)]
pub mod dpapi {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    /// Ties the sealed key to this app as well as to the Windows account.
    const ENTROPY: &[u8] = b"SulcusAI data key v1";

    pub struct Dpapi;

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY: DPAPI allocated `out` with LocalAlloc; we copy it, then free it.
        unsafe {
            let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            let _ = LocalFree(HLOCAL(out.pbData as _));
            v
        }
    }

    impl super::Protector for Dpapi {
        fn protect(&self, data: &[u8]) -> Result<Vec<u8>, String> {
            let mut out = CRYPT_INTEGER_BLOB::default();
            // SAFETY: input blobs point at live slices for the duration of the call.
            unsafe {
                CryptProtectData(&blob(data), PCWSTR::null(), Some(&blob(ENTROPY)), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            }
            .map_err(|e| format!("Windows couldn't protect the key: {e}"))?;
            Ok(take(out))
        }

        fn unprotect(&self, data: &[u8]) -> Result<Vec<u8>, String> {
            let mut out = CRYPT_INTEGER_BLOB::default();
            // SAFETY: as above.
            unsafe {
                CryptUnprotectData(&blob(data), None, Some(&blob(ENTROPY)), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            }
            .map_err(|_| "Windows couldn't open the key. Was it created by another Windows account?".to_string())?;
            Ok(take(out))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-in for DPAPI in tests: reversible, and detects foreign data.
    struct FakeProtector;
    impl Protector for FakeProtector {
        fn protect(&self, data: &[u8]) -> Result<Vec<u8>, String> {
            let mut v = b"FAKE".to_vec();
            v.extend(data.iter().map(|b| b ^ 0x5A));
            Ok(v)
        }
        fn unprotect(&self, data: &[u8]) -> Result<Vec<u8>, String> {
            let body = data.strip_prefix(b"FAKE").ok_or("not ours")?;
            Ok(body.iter().map(|b| b ^ 0x5A).collect())
        }
    }

    fn temp_keys(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sulcusai-test-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("keys.json")
    }

    fn vault(path: &Path) -> Vault {
        Vault::open(path, Box::new(FakeProtector)).unwrap()
    }

    #[test]
    fn fields_round_trip_and_legacy_text_passes_through() {
        let v = vault(&temp_keys("rt"));
        let c = v.cipher().unwrap();
        let enc = c.encrypt("hello é 👋");
        assert!(Cipher::is_encrypted(&enc));
        assert!(!enc.contains("hello"));
        assert_eq!(c.decrypt(&enc).unwrap(), "hello é 👋");
        assert_eq!(c.decrypt("old plain text").unwrap(), "old plain text");
        // Same text encrypts differently each time (random nonce).
        assert_ne!(c.encrypt("x"), c.encrypt("x"));
    }

    #[test]
    fn tampered_data_is_rejected() {
        let v = vault(&temp_keys("tamper"));
        let c = v.cipher().unwrap();
        let enc = c.encrypt("secret");
        let mut raw = B64.decode(enc.strip_prefix(PREFIX).unwrap()).unwrap();
        *raw.last_mut().unwrap() ^= 1;
        assert!(c.decrypt(&format!("{PREFIX}{}", B64.encode(raw))).is_err());
    }

    #[test]
    fn without_lock_the_vault_reopens_unlocked_with_the_same_key() {
        let path = temp_keys("reopen");
        let enc = vault(&path).cipher().unwrap().encrypt("persisted");
        let again = vault(&path);
        assert!(!again.locked());
        assert_eq!(again.cipher().unwrap().decrypt(&enc).unwrap(), "persisted");
    }

    #[test]
    fn app_lock_pin_recovery_and_disable() {
        let path = temp_keys("lock");
        let mut v = vault(&path);
        let enc = v.cipher().unwrap().encrypt("chat");
        assert!(v.enable_lock("12").is_err(), "too short");
        let code = v.enable_lock("2468").unwrap();
        assert_eq!(code.len(), 23);

        // Reopening starts locked; the data key is no longer stored for DPAPI alone.
        let mut v = vault(&path);
        assert!(v.locked() && v.lock_enabled());
        assert!(v.unlock(UnlockWith::Pin, "0000").is_err());
        v.unlock(UnlockWith::Pin, "2468").unwrap();
        assert_eq!(v.cipher().unwrap().decrypt(&enc).unwrap(), "chat");

        // Recovery code works in any case and with look-alike characters.
        v.lock();
        assert!(v.locked());
        let typed = code.to_lowercase().replace('0', "o").replace('-', " ");
        v.unlock(UnlockWith::RecoveryCode, &typed).unwrap();
        let new_code = v.reset_pin("1357").unwrap();
        assert_ne!(new_code, code);
        v.lock();
        assert!(v.unlock(UnlockWith::RecoveryCode, &code).is_err(), "old code is retired");
        v.unlock(UnlockWith::Pin, "1357").unwrap();

        v.change_pin("1357", "abcd").unwrap();
        assert!(v.disable_lock("1357").is_err());
        v.disable_lock("abcd").unwrap();
        let v = vault(&path);
        assert!(!v.locked() && !v.lock_enabled());
        assert_eq!(v.cipher().unwrap().decrypt(&enc).unwrap(), "chat");
    }

    #[test]
    fn hello_copy_unlocks_and_is_dropped_with_the_lock() {
        let path = temp_keys("hello");
        let mut v = vault(&path);
        assert!(v.set_hello(true).is_err(), "needs app lock first");
        v.enable_lock("2468").unwrap();
        v.set_hello(true).unwrap();
        let mut v = vault(&path);
        assert!(v.locked() && v.hello_enabled());
        v.unlock_after_hello().unwrap();
        assert!(!v.locked());
        v.disable_lock("2468").unwrap();
        assert!(!v.hello_enabled());
    }

    #[test]
    fn repeated_mistakes_add_a_growing_wait() {
        let path = temp_keys("penalty");
        let mut v = vault(&path);
        v.enable_lock("2468").unwrap();
        v.lock();
        for _ in 0..3 {
            let _ = v.unlock(UnlockWith::Pin, "9999");
        }
        assert!(v.penalty() > Duration::ZERO);
        v.unlock(UnlockWith::Pin, "2468").unwrap();
        assert_eq!(v.penalty(), Duration::ZERO);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_round_trip() {
        let p = dpapi::Dpapi;
        let sealed = p.protect(b"data key").unwrap();
        assert_ne!(sealed, b"data key");
        assert_eq!(p.unprotect(&sealed).unwrap(), b"data key");
    }
}

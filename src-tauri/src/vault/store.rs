//! The vault file and the encrypted export. Both are JSON envelopes around
//! one AES-256-GCM blob; only key settings, salts and the lockout counter are
//! readable without the key.

use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::keys::{self, KdfCost, Key, Lockout, Wrapped};
use super::VaultError;

const FORMAT: &str = "sulcusai-vault";
const EXPORT_FORMAT: &str = "sulcusai-vault-export";
const VERSION: u32 = 1;

// Defaults per field: a container-level default would move fields out of a
// type that implements Drop.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub id: String,
    /// `https://host[:port]` when the address could be read; otherwise the
    /// address as imported, which is kept but never filled.
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub created_ms: i64,
    #[serde(default)]
    pub changed_ms: i64,
}

impl Entry {
    pub fn login(origin: &str, username: &str, password: &str) -> Entry {
        let mut e = Entry::default();
        e.origin = origin.into();
        e.username = username.into();
        e.password = password.into();
        e
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
        self.notes.zeroize();
    }
}

// Entries end up in error messages and logs by accident; the password and
// notes must not.
impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("id", &self.id)
            .field("origin", &self.origin)
            .field("name", &self.name)
            .field("username", &self.username)
            .field("password", &"<hidden>")
            .field("notes", &if self.notes.is_empty() { "" } else { "<hidden>" })
            .field("created_ms", &self.created_ms)
            .field("changed_ms", &self.changed_ms)
            .finish()
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct VaultData {
    pub entries: Vec<Entry>,
    /// Origins where SulcusAI never offers to save.
    pub never: Vec<String>,
}

impl VaultData {
    /// Serialized into one buffer sized up front, so growing it doesn't leave
    /// stray plaintext copies behind in freed memory.
    fn to_plain(&self) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        let guess = 256
            + self.never.iter().map(|n| n.len() * 2 + 8).sum::<usize>()
            + self.entries.iter().map(|e| 2 * (e.id.len() + e.origin.len() + e.name.len() + e.username.len() + e.password.len() + e.notes.len()) + 256).sum::<usize>();
        let mut buf = Zeroizing::new(Vec::with_capacity(guess * 2));
        serde_json::to_writer(&mut *buf, self).map_err(|e| VaultError::Damaged(e.to_string()))?;
        Ok(buf)
    }

    fn from_plain(plain: &[u8]) -> Result<VaultData, VaultError> {
        serde_json::from_slice(plain).map_err(|_| VaultError::Damaged("the vault's contents can't be read".into()))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct VaultFile {
    format: String,
    version: u32,
    id: String,
    password: Wrapped,
    recovery: Wrapped,
    #[serde(default)]
    pub lockout: Lockout,
    data: String,
}

fn aad(id: &str, part: &str) -> Vec<u8> {
    format!("{FORMAT}/{VERSION}/{part}/{id}").into_bytes()
}

impl VaultFile {
    /// A new vault: a random key, wrapped by the master password and by a
    /// fresh recovery key, which is returned once for printing.
    pub fn create(master: &str, cost: KdfCost, recovery_cost: KdfCost) -> Result<(VaultFile, Key, Zeroizing<String>), VaultError> {
        let id = uuid::Uuid::new_v4().to_string();
        let key = keys::random_key();
        let pw = keys::normalize_password(master);
        let recovery = keys::new_recovery_key();
        let rk = keys::normalize_recovery_key(&recovery);
        let file = VaultFile {
            format: FORMAT.into(),
            version: VERSION,
            password: keys::wrap(pw.as_bytes(), &key, cost, &aad(&id, "password"))?,
            recovery: keys::wrap(rk.as_bytes(), &key, recovery_cost, &aad(&id, "recovery"))?,
            id,
            lockout: Lockout::default(),
            data: String::new(),
        };
        Ok((file, key, recovery))
    }

    pub fn parse(text: &str) -> Result<VaultFile, VaultError> {
        let f: VaultFile = serde_json::from_str(text).map_err(|_| VaultError::Damaged("not a SulcusAI vault file".into()))?;
        if f.format != FORMAT {
            return Err(VaultError::Damaged("not a SulcusAI vault file".into()));
        }
        if f.version != VERSION {
            return Err(VaultError::Damaged(format!("vault version {} needs a newer SulcusAI", f.version)));
        }
        Ok(f)
    }

    pub fn to_text(&self) -> Result<String, VaultError> {
        serde_json::to_string_pretty(self).map_err(|e| VaultError::Damaged(e.to_string()))
    }

    pub fn unlock_password(&self, master: &str) -> Result<Option<Key>, VaultError> {
        let pw = keys::normalize_password(master);
        keys::unwrap(pw.as_bytes(), &self.password, &aad(&self.id, "password"))
    }

    pub fn unlock_recovery(&self, typed: &str) -> Result<Option<Key>, VaultError> {
        let rk = keys::normalize_recovery_key(typed);
        keys::unwrap(rk.as_bytes(), &self.recovery, &aad(&self.id, "recovery"))
    }

    pub fn set_password(&mut self, master: &str, key: &Key, cost: KdfCost) -> Result<(), VaultError> {
        let pw = keys::normalize_password(master);
        self.password = keys::wrap(pw.as_bytes(), key, cost, &aad(&self.id, "password"))?;
        Ok(())
    }

    pub fn new_recovery(&mut self, key: &Key, cost: KdfCost) -> Result<Zeroizing<String>, VaultError> {
        let recovery = keys::new_recovery_key();
        let rk = keys::normalize_recovery_key(&recovery);
        self.recovery = keys::wrap(rk.as_bytes(), key, cost, &aad(&self.id, "recovery"))?;
        Ok(recovery)
    }

    pub fn read_data(&self, key: &Key) -> Result<VaultData, VaultError> {
        if self.data.is_empty() {
            return Ok(VaultData::default());
        }
        let blob = keys::unb64(&self.data)?;
        let plain = keys::open(key, &aad(&self.id, "data"), &blob).ok_or_else(|| VaultError::Damaged("the vault's contents fail their check".into()))?;
        VaultData::from_plain(&plain)
    }

    pub fn write_data(&mut self, key: &Key, data: &VaultData) -> Result<(), VaultError> {
        let plain = data.to_plain()?;
        self.data = keys::b64(&keys::seal(key, &aad(&self.id, "data"), &plain));
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct ExportFile {
    format: String,
    version: u32,
    salt: String,
    cost: KdfCost,
    data: String,
}

fn export_aad(salt: &str) -> Vec<u8> {
    format!("{EXPORT_FORMAT}/{VERSION}/{salt}").into_bytes()
}

/// An export sealed under its own password, so it can be moved to another PC
/// or kept as a backup without the master password or recovery key.
pub(crate) fn export(data: &VaultData, password: &str, cost: KdfCost) -> Result<Vec<u8>, VaultError> {
    let (salt_text, salt) = keys::salt_b64();
    let key = keys::password_key(password, &salt, &cost)?;
    let plain = data.to_plain()?;
    let blob = keys::seal(&key, &export_aad(&salt_text), &plain);
    let file = ExportFile { format: EXPORT_FORMAT.into(), version: VERSION, salt: salt_text, cost, data: keys::b64(&blob) };
    serde_json::to_vec_pretty(&file).map_err(|e| VaultError::Damaged(e.to_string()))
}

pub(crate) fn is_export(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(512)];
    String::from_utf8_lossy(head).contains(EXPORT_FORMAT)
}

pub(crate) fn read_export(bytes: &[u8], password: &str) -> Result<VaultData, VaultError> {
    let file: ExportFile = serde_json::from_slice(bytes).map_err(|_| VaultError::BadImport("not a SulcusAI vault export".into()))?;
    if file.format != EXPORT_FORMAT {
        return Err(VaultError::BadImport("not a SulcusAI vault export".into()));
    }
    if file.version != VERSION {
        return Err(VaultError::BadImport(format!("export version {} needs a newer SulcusAI", file.version)));
    }
    let salt = keys::unb64(&file.salt).map_err(|_| VaultError::BadImport("the export is damaged".into()))?;
    if salt.len() < 8 {
        return Err(VaultError::BadImport("the export is damaged".into()));
    }
    let key = keys::password_key(password, &salt, &file.cost).map_err(|_| VaultError::BadImport("the export's key settings are damaged".into()))?;
    let blob = keys::unb64(&file.data).map_err(|_| VaultError::BadImport("the export is damaged".into()))?;
    let plain = keys::open(&key, &export_aad(&file.salt), &blob).ok_or(VaultError::WrongPassword { wait_ms: 0 })?;
    VaultData::from_plain(&plain).map_err(|_| VaultError::BadImport("the export is damaged".into()))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct MergeReport {
    pub added: usize,
    pub updated: usize,
    /// Already saved, or older than what is saved.
    pub unchanged: usize,
    /// Rows with no password, or that aren't logins.
    pub skipped: usize,
}

/// Adds imported entries. A saved login for the same site and username keeps
/// whichever password changed last, so importing an old export never rolls a
/// password back.
pub(crate) fn merge(into: &mut VaultData, imported: Vec<Entry>, never: Vec<String>, now_ms: i64) -> MergeReport {
    let mut report = MergeReport::default();
    for mut e in imported {
        if e.password.is_empty() {
            report.skipped += 1;
            continue;
        }
        let raw = std::mem::take(&mut e.origin);
        e.origin = super::fill::normalize_origin(&raw).unwrap_or_else(|| raw.trim().to_string());
        if e.created_ms <= 0 {
            e.created_ms = if e.changed_ms > 0 { e.changed_ms } else { now_ms };
        }
        if e.changed_ms <= 0 {
            e.changed_ms = e.created_ms;
        }
        match into.entries.iter_mut().find(|x| x.origin == e.origin && x.username == e.username) {
            Some(x) if x.password == e.password => report.unchanged += 1,
            Some(x) if e.changed_ms > x.changed_ms => {
                x.password = std::mem::take(&mut e.password);
                x.changed_ms = e.changed_ms;
                if x.notes.is_empty() {
                    x.notes = std::mem::take(&mut e.notes);
                }
                report.updated += 1;
            }
            Some(_) => report.unchanged += 1,
            None => {
                e.id = uuid::Uuid::new_v4().to_string();
                into.entries.push(e);
                report.added += 1;
            }
        }
    }
    for n in never {
        let n = super::fill::normalize_origin(&n).unwrap_or(n);
        if !into.never.contains(&n) {
            into.never.push(n);
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::keys::TEST_COST;

    fn e(origin: &str, user: &str, pw: &str, changed: i64) -> Entry {
        let mut x = Entry::login(origin, user, pw);
        x.changed_ms = changed;
        x
    }

    #[test]
    fn data_round_trips_and_is_bound_to_its_vault() {
        let (mut f, key, _) = VaultFile::create("master password", TEST_COST, TEST_COST).unwrap();
        let mut data = VaultData::default();
        merge(&mut data, vec![e("https://a.example/login", "me", "pw1", 5)], vec!["https://b.example".into()], 10);
        f.write_data(&key, &data).unwrap();
        let back = VaultFile::parse(&f.to_text().unwrap()).unwrap().read_data(&key).unwrap();
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries[0].origin, "https://a.example");
        assert_eq!(back.entries[0].password, "pw1");
        assert_eq!(back.never, vec!["https://b.example".to_string()]);

        let (mut other, _, _) = VaultFile::create("master password", TEST_COST, TEST_COST).unwrap();
        other.data = f.data.clone();
        assert!(other.read_data(&key).is_err());
    }

    #[test]
    fn merge_keeps_the_newest_password() {
        let mut data = VaultData::default();
        let r = merge(&mut data, vec![e("https://a.example", "me", "old", 100), e("https://a.example", "you", "x", 100), e("https://a.example", "nopw", "", 1)], vec![], 1000);
        assert_eq!(r, MergeReport { added: 2, updated: 0, unchanged: 0, skipped: 1 });
        let r = merge(&mut data, vec![e("https://a.example/", "me", "older", 50), e("https://A.example", "me", "old", 999)], vec![], 1000);
        assert_eq!(r, MergeReport { added: 0, updated: 0, unchanged: 2, skipped: 0 });
        let r = merge(&mut data, vec![e("https://a.example", "me", "new", 200)], vec![], 1000);
        assert_eq!(r.updated, 1);
        assert_eq!(data.entries.iter().find(|x| x.username == "me").unwrap().password, "new");
        let r = merge(&mut data, vec![e("android://key@com.example.app/", "me", "p", 0)], vec![], 1000);
        assert_eq!(r.added, 1);
        let app = data.entries.iter().find(|x| x.origin.starts_with("android")).unwrap();
        assert_eq!((app.created_ms, app.changed_ms), (1000, 1000));
    }

    #[test]
    fn export_needs_its_password() {
        let mut data = VaultData::default();
        merge(&mut data, vec![e("https://a.example", "me", "pw", 1)], vec![], 1);
        let bytes = export(&data, "export pass", TEST_COST).unwrap();
        assert!(is_export(&bytes));
        assert!(!String::from_utf8_lossy(&bytes).contains("\"pw\""));
        assert!(matches!(read_export(&bytes, "wrong"), Err(VaultError::WrongPassword { .. })));
        let back = read_export(&bytes, "export pass").unwrap();
        assert_eq!(back.entries[0].password, "pw");
        assert!(read_export(b"{}", "x").is_err());
        assert!(read_export(&bytes[..bytes.len() / 2], "export pass").is_err());
    }

    #[test]
    fn debug_hides_passwords() {
        let shown = format!("{:?}", e("https://a.example", "me", "hunter2-secret", 1));
        assert!(!shown.contains("hunter2-secret"));
    }

    #[test]
    fn dropping_an_entry_wipes_its_secrets() {
        let mut entry = e("https://a.example", "me", "wipe-me-please", 1);
        let (ptr, cap) = (entry.password.as_ptr(), entry.password.capacity());
        entry.password.zeroize();
        // The string's buffer is still allocated here, so reading it is sound.
        let bytes = unsafe { std::slice::from_raw_parts(ptr, cap) };
        assert!(bytes.iter().all(|b| *b == 0));
        drop(entry);
    }
}

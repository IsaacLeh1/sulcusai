//! The password vault: saved sign-ins under a key of their own, opened by a
//! master password (or the recovery key printed at setup), never by
//! SulcusAI's data key.
//!
//! Everything sits behind one switch that starts off. While it is off, or the
//! vault is locked, every call refuses. The vault makes no network
//! connections of any kind; nothing here may ever import a network crate.

// Nothing outside the tests calls the vault until its switch and commands are
// wired in; drop this then.
#![cfg_attr(not(test), allow(dead_code))]

pub mod checks;
pub mod fill;
pub mod import;
mod import_formats;
mod keys;
pub mod store;

#[cfg(test)]
mod import_tests;
#[cfg(test)]
mod tests;

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

pub use checks::{CheckReport, GenOptions, Strength};
pub use fill::Refusal;
pub use import::Source;
pub use store::{Entry, MergeReport};

use keys::{KdfCost, Key};
use store::{VaultData, VaultFile};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultError {
    Off,
    NotSetUp,
    AlreadySetUp,
    Locked,
    WrongPassword { wait_ms: i64 },
    /// Too many wrong passwords; try again after this long.
    Wait { wait_ms: i64 },
    WeakMaster(String),
    NotFound,
    Refused(Refusal),
    Damaged(String),
    BadImport(String),
    Io(String),
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secs = |ms: &i64| (ms + 999) / 1000;
        match self {
            VaultError::Off => f.write_str("The password vault is turned off."),
            VaultError::NotSetUp => f.write_str("The password vault hasn't been set up yet."),
            VaultError::AlreadySetUp => f.write_str("The password vault is already set up."),
            VaultError::Locked => f.write_str("The password vault is locked."),
            VaultError::WrongPassword { wait_ms: 0 } => f.write_str("That password isn't right."),
            VaultError::WrongPassword { wait_ms } => write!(f, "That password isn't right. Try again in {} seconds.", secs(wait_ms)),
            VaultError::Wait { wait_ms } => write!(f, "Too many wrong tries. Try again in {} seconds.", secs(wait_ms)),
            VaultError::WeakMaster(why) => f.write_str(why),
            VaultError::NotFound => f.write_str("That saved password no longer exists."),
            VaultError::Refused(r) => write!(f, "{r}"),
            VaultError::Damaged(why) => write!(f, "The vault file is damaged ({why})."),
            VaultError::BadImport(why) => write!(f, "Can't import this file: {why}."),
            VaultError::Io(why) => write!(f, "The vault file can't be saved ({why})."),
        }
    }
}

impl std::error::Error for VaultError {}

impl From<VaultError> for String {
    fn from(e: VaultError) -> String {
        e.to_string()
    }
}

pub const MIN_MASTER_CHARS: usize = 10;

/// Refuses master and export passwords that would fall to a guessing run
/// despite the slow key derivation.
pub fn check_master(password: &str) -> Result<(), VaultError> {
    if password.chars().count() < MIN_MASTER_CHARS {
        return Err(VaultError::WeakMaster(format!("Use at least {MIN_MASTER_CHARS} characters.")));
    }
    if checks::is_common(password) || checks::strength(password, &[]) < Strength::Fair {
        return Err(VaultError::WeakMaster("That password is too easy to guess. A few unrelated words make a strong one.".into()));
    }
    Ok(())
}

/// What lists and pickers show: everything but the password and notes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EntrySummary {
    pub id: String,
    pub origin: String,
    pub name: String,
    pub username: String,
    pub created_ms: i64,
    pub changed_ms: i64,
    /// False for entries saved for http or a non-web address: kept, but
    /// never filled.
    pub fillable: bool,
}

impl From<&Entry> for EntrySummary {
    fn from(e: &Entry) -> Self {
        EntrySummary {
            id: e.id.clone(),
            origin: e.origin.clone(),
            name: e.name.clone(),
            username: e.username.clone(),
            created_ms: e.created_ms,
            changed_ms: e.changed_ms,
            fillable: fill::parse_origin(&e.origin).is_some_and(|o| o.scheme == "https"),
        }
    }
}

#[derive(Clone, Default, Deserialize)]
pub struct EntryInput {
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
}

impl Drop for EntryInput {
    fn drop(&mut self) {
        self.password.zeroize();
        self.notes.zeroize();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Saved {
    Added,
    Updated,
    Unchanged,
}

struct Open {
    file: VaultFile,
    key: Key,
    data: VaultData,
}

impl Drop for Open {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

pub struct PasswordVault {
    path: PathBuf,
    enabled: bool,
    open: Option<Open>,
    cost: KdfCost,
    recovery_cost: KdfCost,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl PasswordVault {
    /// The vault kept at `path` (its own file, apart from SulcusAI's
    /// database), switched off until `set_enabled(true)`.
    pub fn new(path: &Path) -> PasswordVault {
        PasswordVault { path: path.to_path_buf(), enabled: false, open: None, cost: keys::PASSWORD_COST, recovery_cost: keys::RECOVERY_COST }
    }

    #[cfg(test)]
    pub(crate) fn for_test(path: &Path) -> PasswordVault {
        PasswordVault { cost: keys::TEST_COST, recovery_cost: keys::TEST_COST, ..PasswordVault::new(path) }
    }

    /// Turning the switch off locks the vault at once.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.lock();
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_set_up(&self) -> bool {
        self.path.exists()
    }

    pub fn is_open(&self) -> bool {
        self.enabled && self.open.is_some()
    }

    /// Drops the key and every decrypted entry; their memory is wiped on the
    /// way out (`Open` and `Entry` zeroize on drop).
    pub fn lock(&mut self) {
        self.open = None;
    }

    fn on(&self) -> Result<(), VaultError> {
        if self.enabled {
            Ok(())
        } else {
            Err(VaultError::Off)
        }
    }

    fn opened(&self) -> Result<&Open, VaultError> {
        self.on()?;
        self.open.as_ref().ok_or(VaultError::Locked)
    }

    fn opened_mut(&mut self) -> Result<&mut Open, VaultError> {
        self.on()?;
        self.open.as_mut().ok_or(VaultError::Locked)
    }

    fn write(&self, file: &VaultFile) -> Result<(), VaultError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| VaultError::Io(e.to_string()))?;
        }
        crate::storage::write_safely(&self.path, file.to_text()?).map_err(|e| VaultError::Io(e.to_string()))
    }

    fn save(&mut self) -> Result<(), VaultError> {
        let open = self.open.as_mut().ok_or(VaultError::Locked)?;
        open.file.write_data(&open.key, &open.data)?;
        let file = open.file.clone();
        self.write(&file)
    }

    fn read_file(&self) -> Result<VaultFile, VaultError> {
        if !self.path.exists() {
            return Err(VaultError::NotSetUp);
        }
        let text = std::fs::read_to_string(&self.path).map_err(|e| VaultError::Io(e.to_string()))?;
        VaultFile::parse(&text)
    }

    /// Creates the vault and opens it. Returns the recovery key, which is
    /// shown once for printing and never stored in readable form.
    pub fn setup(&mut self, master: &str) -> Result<Zeroizing<String>, VaultError> {
        self.on()?;
        if self.is_set_up() {
            return Err(VaultError::AlreadySetUp);
        }
        check_master(master)?;
        let (file, key, recovery) = VaultFile::create(master, self.cost, self.recovery_cost)?;
        self.open = Some(Open { file, key, data: VaultData::default() });
        if let Err(e) = self.save() {
            self.open = None;
            return Err(e);
        }
        Ok(recovery)
    }

    pub fn unlock(&mut self, master: &str) -> Result<(), VaultError> {
        self.unlock_at(master, now_ms())
    }

    pub(crate) fn unlock_at(&mut self, master: &str, now: i64) -> Result<(), VaultError> {
        self.try_open(now, |f| f.unlock_password(master)).map(|_| ())
    }

    /// Opens with the printed recovery key and sets a new master password,
    /// for when the old one is forgotten.
    pub fn recover(&mut self, recovery_key: &str, new_master: &str) -> Result<(), VaultError> {
        self.recover_at(recovery_key, new_master, now_ms())
    }

    pub(crate) fn recover_at(&mut self, recovery_key: &str, new_master: &str, now: i64) -> Result<(), VaultError> {
        self.on()?;
        check_master(new_master)?;
        self.try_open(now, |f| f.unlock_recovery(recovery_key))?;
        let cost = self.cost;
        let open = self.opened_mut()?;
        open.file.set_password(new_master, &open.key, cost)?;
        self.save()
    }

    /// Wrong guesses count against one shared lockout, whichever secret was
    /// tried, so the recovery key is no way around the waits.
    fn try_open(&mut self, now: i64, unwrap: impl FnOnce(&VaultFile) -> Result<Option<Key>, VaultError>) -> Result<(), VaultError> {
        self.on()?;
        let mut file = self.read_file()?;
        let wait_ms = file.lockout.wait_ms(now);
        if wait_ms > 0 {
            return Err(VaultError::Wait { wait_ms });
        }
        match unwrap(&file)? {
            Some(key) => {
                let data = file.read_data(&key)?;
                let had_failures = file.lockout.failed > 0;
                file.lockout.success();
                self.open = Some(Open { file, key, data });
                if had_failures {
                    self.save()?;
                }
                Ok(())
            }
            None => {
                file.lockout.failure(now);
                self.write(&file)?;
                Err(VaultError::WrongPassword { wait_ms: file.lockout.wait_ms(now) })
            }
        }
    }

    /// Asks for the current password again, so someone at an unlocked PC
    /// can't take the vault over.
    pub fn change_master(&mut self, current: &str, new_master: &str) -> Result<(), VaultError> {
        check_master(new_master)?;
        self.confirm(current)?;
        let cost = self.cost;
        let open = self.opened_mut()?;
        open.file.set_password(new_master, &open.key, cost)?;
        self.save()
    }

    /// Replaces the recovery key; the old one stops working.
    pub fn new_recovery_key(&mut self, master: &str) -> Result<Zeroizing<String>, VaultError> {
        self.confirm(master)?;
        let cost = self.recovery_cost;
        let open = self.opened_mut()?;
        let recovery = open.file.new_recovery(&open.key, cost)?;
        self.save()?;
        Ok(recovery)
    }

    fn confirm(&self, master: &str) -> Result<(), VaultError> {
        let open = self.opened()?;
        match open.file.unlock_password(master)? {
            Some(_) => Ok(()),
            None => Err(VaultError::WrongPassword { wait_ms: 0 }),
        }
    }

    pub fn list(&self) -> Result<Vec<EntrySummary>, VaultError> {
        Ok(self.opened()?.data.entries.iter().map(EntrySummary::from).collect())
    }

    fn entry(&self, id: &str) -> Result<&Entry, VaultError> {
        self.opened()?.data.entries.iter().find(|e| e.id == id).ok_or(VaultError::NotFound)
    }

    /// For the vault's own window, where the person asked to see it.
    pub fn reveal(&self, id: &str) -> Result<(Zeroizing<String>, Zeroizing<String>), VaultError> {
        let e = self.entry(id)?;
        Ok((Zeroizing::new(e.password.clone()), Zeroizing::new(e.notes.clone())))
    }

    /// Sign-ins that may be filled into a frame at `frame_url` on a page at
    /// `top_url`, checked with the exact-origin rule.
    pub fn logins_for(&self, frame_url: &str, top_url: &str) -> Result<Vec<EntrySummary>, VaultError> {
        Ok(self.opened()?.data.entries.iter().filter(|e| fill::may_fill(&e.origin, frame_url, top_url).is_ok()).map(EntrySummary::from).collect())
    }

    /// The password to fill, only if the fill rule allows it here.
    pub fn password_for_fill(&self, id: &str, frame_url: &str, top_url: &str) -> Result<Zeroizing<String>, VaultError> {
        let e = self.entry(id)?;
        fill::may_fill(&e.origin, frame_url, top_url).map_err(VaultError::Refused)?;
        Ok(Zeroizing::new(e.password.clone()))
    }

    /// Saves a sign-in, updating the one for the same site and username if
    /// there is one.
    pub fn save_login(&mut self, input: &EntryInput) -> Result<(String, Saved), VaultError> {
        self.save_login_at(input, now_ms())
    }

    pub(crate) fn save_login_at(&mut self, input: &EntryInput, now: i64) -> Result<(String, Saved), VaultError> {
        if input.password.is_empty() {
            return Err(VaultError::BadImport("there's no password to save".into()));
        }
        let origin = fill::normalize_origin(&input.origin).unwrap_or_else(|| input.origin.trim().to_string());
        let open = self.opened_mut()?;
        let username = input.username.trim();
        let (id, saved) = match open.data.entries.iter_mut().find(|e| e.origin == origin && e.username == username) {
            Some(e) if e.password == input.password => return Ok((e.id.clone(), Saved::Unchanged)),
            Some(e) => {
                e.password = input.password.clone();
                e.changed_ms = now;
                if !input.notes.is_empty() {
                    e.notes = input.notes.clone();
                }
                (e.id.clone(), Saved::Updated)
            }
            None => {
                let mut e = Entry::login(&origin, username, &input.password);
                e.id = uuid::Uuid::new_v4().to_string();
                e.name = input.name.trim().to_string();
                e.notes = input.notes.clone();
                e.created_ms = now;
                e.changed_ms = now;
                let id = e.id.clone();
                open.data.entries.push(e);
                (id, Saved::Added)
            }
        };
        self.save()?;
        Ok((id, saved))
    }

    /// Edits an entry from the vault window. An empty password keeps the old
    /// one.
    pub fn update(&mut self, id: &str, input: &EntryInput) -> Result<(), VaultError> {
        let now = now_ms();
        let open = self.opened_mut()?;
        let e = open.data.entries.iter_mut().find(|e| e.id == id).ok_or(VaultError::NotFound)?;
        e.origin = fill::normalize_origin(&input.origin).unwrap_or_else(|| input.origin.trim().to_string());
        e.name = input.name.trim().to_string();
        e.username = input.username.trim().to_string();
        e.notes = input.notes.clone();
        if !input.password.is_empty() && input.password != e.password {
            e.password = input.password.clone();
            e.changed_ms = now;
        }
        self.save()
    }

    pub fn remove(&mut self, id: &str) -> Result<(), VaultError> {
        let open = self.opened_mut()?;
        let before = open.data.entries.len();
        open.data.entries.retain(|e| e.id != id);
        if open.data.entries.len() == before {
            return Err(VaultError::NotFound);
        }
        self.save()
    }

    /// Whether to ask "Save password?" on this page: never on http, on
    /// sites marked Never, or while the vault is off or locked.
    pub fn should_offer_save(&self, page_url: &str) -> bool {
        let Ok(open) = self.opened() else { return false };
        let Some(o) = fill::parse_origin(page_url) else { return false };
        o.scheme == "https" && !open.data.never.contains(&o.to_string())
    }

    pub fn never_for(&mut self, page_url: &str) -> Result<(), VaultError> {
        let origin = fill::normalize_origin(page_url).ok_or(VaultError::Refused(Refusal::BadAddress))?;
        let open = self.opened_mut()?;
        if !open.data.never.contains(&origin) {
            open.data.never.push(origin);
        }
        self.save()
    }

    pub fn allow_again(&mut self, origin: &str) -> Result<(), VaultError> {
        let origin = fill::normalize_origin(origin).unwrap_or_else(|| origin.to_string());
        self.opened_mut()?.data.never.retain(|n| *n != origin);
        self.save()
    }

    pub fn never_list(&self) -> Result<Vec<String>, VaultError> {
        Ok(self.opened()?.data.never.clone())
    }

    /// An export sealed under a password chosen for it, held to the same
    /// bar as the master password.
    pub fn export(&self, export_password: &str) -> Result<Vec<u8>, VaultError> {
        check_master(export_password)?;
        store::export(&self.opened()?.data, export_password, self.cost)
    }

    pub fn import_export(&mut self, bytes: &[u8], export_password: &str) -> Result<MergeReport, VaultError> {
        self.opened()?;
        let mut theirs = store::read_export(bytes, export_password)?;
        let entries = std::mem::take(&mut theirs.entries);
        let never = std::mem::take(&mut theirs.never);
        let open = self.opened_mut()?;
        let report = store::merge(&mut open.data, entries, never, now_ms());
        self.save()?;
        Ok(report)
    }

    /// Imports another password manager's export file.
    pub fn import_file(&mut self, bytes: &[u8]) -> Result<(Option<Source>, MergeReport), VaultError> {
        self.opened()?;
        let mut imported = import::import_any(bytes)?;
        let entries = std::mem::take(&mut imported.entries);
        let open = self.opened_mut()?;
        let mut report = store::merge(&mut open.data, entries, Vec::new(), now_ms());
        report.skipped += imported.skipped;
        self.save()?;
        Ok((imported.source, report))
    }

    pub fn check(&self) -> Result<CheckReport, VaultError> {
        Ok(checks::check(&self.opened()?.data.entries, now_ms()))
    }

    pub fn generate(&self, opts: &GenOptions) -> Result<Zeroizing<String>, VaultError> {
        self.on()?;
        Ok(checks::generate(opts))
    }
}

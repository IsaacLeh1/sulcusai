// SPDX-License-Identifier: AGPL-3.0-only
//! The password vault's commands for its settings page. Thin: every rule
//! (switch, lock, lockout, fill) lives in `vault/`. Passwords reach the window
//! only through `vault_reveal` and `vault_generate`; lists and reports carry
//! none. Nothing here is a tool, a memory or a search source, and the phone
//! may not call any of it (`phone/remote.rs` refuses the `vault_` prefix).

use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use serde::{Deserialize, Serialize, Serializer};
use zeroize::{Zeroize, Zeroizing};

use crate::vault::import::MAX_IMPORT_BYTES;
use crate::vault::{CheckReport, EntryInput, EntrySummary, GenOptions, MergeReport, PasswordVault, Saved, Source, VaultError};
use crate::{db, AppState, AppStateRef};

const SWITCH_KEY: &str = "password_vault_switch";

/// The vault is one per app; a static keeps `AppState` untouched.
static VAULT: OnceLock<Mutex<PasswordVault>> = OnceLock::new();

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Switch {
    on: bool,
    /// When the responsibility notice was accepted; 0 if never.
    accepted_ms: i64,
}

/// A secret that comes in from the window and is wiped when the call ends.
#[derive(Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// A secret that goes out to the window and is wiped once it has been sent.
pub struct SecretOut(Zeroizing<String>);

impl Serialize for SecretOut {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

#[derive(Serialize)]
pub struct Revealed {
    password: SecretOut,
    notes: SecretOut,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Status {
    pub enabled: bool,
    pub set_up: bool,
    pub unlocked: bool,
    pub notice_accepted: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Saving {
    pub id: String,
    pub result: Saved,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Imported {
    /// Which program's export it was, when that could be told.
    pub source: Option<Source>,
    pub report: MergeReport,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportFormat {
    /// Another password manager's export (Chrome, Edge, Firefox, KeePass,
    /// Bitwarden, 1Password), told apart by its contents.
    Other,
    /// A file made by this vault's own export; needs its export password.
    Sulcus,
}

fn guard(m: &Mutex<PasswordVault>) -> MutexGuard<'_, PasswordVault> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read_switch(state: &AppState) -> Switch {
    db::get(&state.db.lock().unwrap(), SWITCH_KEY).unwrap_or_default()
}

fn instance(state: &AppState) -> &'static Mutex<PasswordVault> {
    VAULT.get_or_init(|| {
        let mut v = PasswordVault::new(&state.paths.data.join("password-vault.json"));
        v.set_enabled(read_switch(state).on);
        Mutex::new(v)
    })
}

/// Locks the vault now (idle, sleep, lock screen, minimize, remote control).
/// A vault that was never used has nothing to lock.
pub fn lock_now() {
    if let Some(m) = VAULT.get() {
        guard(m).lock();
    }
}

/// Runs `f` on a blocking thread: the key derivation takes about a second and
/// must not stall the window.
async fn blocking<T: Send + 'static>(m: &'static Mutex<PasswordVault>, f: impl FnOnce(&mut PasswordVault) -> Result<T, VaultError> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || f(&mut *guard(m)).map_err(String::from))
        .await
        .map_err(|_| "The password vault stopped unexpectedly.".to_string())?
}

fn status_of(v: &PasswordVault, accepted_ms: i64) -> Status {
    Status { enabled: v.is_enabled(), set_up: v.is_set_up(), unlocked: v.is_open(), notice_accepted: accepted_ms > 0 }
}

/// Turning on needs the responsibility notice accepted; turning off locks.
fn apply_switch(v: &mut PasswordVault, current: &Switch, on: bool, accepted: bool, now: i64) -> Result<Switch, String> {
    if on && !accepted {
        return Err("Read and accept the notice before turning the password vault on.".into());
    }
    v.set_enabled(on);
    let accepted_ms = if on && accepted { now } else { current.accepted_ms };
    Ok(Switch { on, accepted_ms })
}

/// A password export read from disk, size-checked before it is read.
fn read_import(path: &Path) -> Result<Vec<u8>, VaultError> {
    let meta = std::fs::metadata(path).map_err(|e| VaultError::BadImport(format!("the file can't be read ({e})")))?;
    if !meta.is_file() {
        return Err(VaultError::BadImport("that isn't a file".into()));
    }
    if meta.len() > MAX_IMPORT_BYTES as u64 {
        return Err(VaultError::BadImport("this file is too large to be a password export".into()));
    }
    std::fs::read(path).map_err(|e| VaultError::BadImport(format!("the file can't be read ({e})")))
}

fn import_from(v: &mut PasswordVault, path: &Path, format: ImportFormat, password: Option<&str>) -> Result<Imported, VaultError> {
    let mut bytes = Zeroizing::new(read_import(path)?);
    let result = match format {
        ImportFormat::Other => v.import_file(&bytes).map(|(source, report)| Imported { source, report }),
        ImportFormat::Sulcus => {
            let password = password.filter(|p| !p.is_empty()).ok_or_else(|| VaultError::BadImport("enter the export's password".into()))?;
            v.import_export(&bytes, password).map(|report| Imported { source: None, report })
        }
    };
    bytes.zeroize();
    result
}

fn export_to(v: &PasswordVault, path: &Path, password: &str) -> Result<(), VaultError> {
    let bytes = v.export(password)?;
    crate::storage::write_safely(path, bytes).map_err(|e| VaultError::Io(e.to_string()))
}

// ---------- commands ----------

#[tauri::command]
pub async fn vault_status(state: AppStateRef<'_>) -> Result<Status, String> {
    let m = instance(&state);
    let accepted = read_switch(&state).accepted_ms;
    Ok(status_of(&guard(m), accepted))
}

#[tauri::command]
pub async fn vault_set_enabled(state: AppStateRef<'_>, on: bool, accepted: bool) -> Result<Status, String> {
    let m = instance(&state);
    let current = read_switch(&state);
    let next = apply_switch(&mut *guard(m), &current, on, accepted, chrono::Utc::now().timestamp_millis())?;
    db::set(&state.db.lock().unwrap(), SWITCH_KEY, &next)?;
    Ok(status_of(&guard(m), next.accepted_ms))
}

/// Creates the vault and returns the recovery key, shown once.
#[tauri::command]
pub async fn vault_setup(state: AppStateRef<'_>, master: Secret) -> Result<SecretOut, String> {
    let m = instance(&state);
    blocking(m, move |v| v.setup(&master.0).map(SecretOut)).await
}

#[tauri::command]
pub async fn vault_unlock(state: AppStateRef<'_>, master: Secret) -> Result<(), String> {
    let m = instance(&state);
    blocking(m, move |v| v.unlock(&master.0)).await
}

/// For a forgotten master password: the printed recovery key opens the vault
/// and the new master password replaces the old one.
#[tauri::command]
pub async fn vault_recover(state: AppStateRef<'_>, recovery_key: Secret, new_master: Secret) -> Result<(), String> {
    let m = instance(&state);
    blocking(m, move |v| v.recover(&recovery_key.0, &new_master.0)).await
}

#[tauri::command]
pub async fn vault_lock(state: AppStateRef<'_>) -> Result<(), String> {
    guard(instance(&state)).lock();
    Ok(())
}

#[tauri::command]
pub async fn vault_change_master(state: AppStateRef<'_>, current: Secret, new_master: Secret) -> Result<(), String> {
    let m = instance(&state);
    blocking(m, move |v| v.change_master(&current.0, &new_master.0)).await
}

/// Replaces the recovery key; the old one stops working.
#[tauri::command]
pub async fn vault_new_recovery_key(state: AppStateRef<'_>, master: Secret) -> Result<SecretOut, String> {
    let m = instance(&state);
    blocking(m, move |v| v.new_recovery_key(&master.0).map(SecretOut)).await
}

#[tauri::command]
pub async fn vault_list(state: AppStateRef<'_>) -> Result<Vec<EntrySummary>, String> {
    let m = instance(&state);
    blocking(m, |v| v.list()).await
}

#[tauri::command]
pub async fn vault_add(state: AppStateRef<'_>, input: EntryInput) -> Result<Saving, String> {
    let m = instance(&state);
    blocking(m, move |v| v.save_login(&input).map(|(id, result)| Saving { id, result })).await
}

#[tauri::command]
pub async fn vault_edit(state: AppStateRef<'_>, id: String, input: EntryInput) -> Result<(), String> {
    let m = instance(&state);
    blocking(m, move |v| v.update(&id, &input)).await
}

#[tauri::command]
pub async fn vault_delete(state: AppStateRef<'_>, id: String) -> Result<(), String> {
    let m = instance(&state);
    blocking(m, move |v| v.remove(&id)).await
}

/// The one way a saved password reaches the window. The log line names no
/// site and holds no secret.
#[tauri::command]
pub async fn vault_reveal(state: AppStateRef<'_>, id: String) -> Result<Revealed, String> {
    let m = instance(&state);
    let (password, notes) = blocking(m, move |v| v.reveal(&id)).await?;
    state.log("vault", "A saved password was shown in the password vault");
    Ok(Revealed { password: SecretOut(password), notes: SecretOut(notes) })
}

#[tauri::command]
pub async fn vault_generate(state: AppStateRef<'_>, options: Option<GenOptions>) -> Result<SecretOut, String> {
    let m = instance(&state);
    blocking(m, move |v| v.generate(&options.unwrap_or_default()).map(SecretOut)).await
}

#[tauri::command]
pub async fn vault_import(state: AppStateRef<'_>, path: String, format: ImportFormat, password: Option<Secret>) -> Result<Imported, String> {
    let m = instance(&state);
    let done = blocking(m, move |v| import_from(v, Path::new(&path), format, password.as_ref().map(|p| p.0.as_str()))).await?;
    state.log("vault", "Sign-ins were imported into the password vault");
    Ok(done)
}

/// Writes an encrypted copy, sealed with its own password.
#[tauri::command]
pub async fn vault_export(state: AppStateRef<'_>, path: String, password: Secret) -> Result<(), String> {
    let m = instance(&state);
    blocking(m, move |v| export_to(v, Path::new(&path), &password.0)).await?;
    state.log("vault", "The password vault was exported");
    Ok(())
}

/// Weak, reused and old passwords, as entry ids only.
#[tauri::command]
pub async fn vault_check(state: AppStateRef<'_>) -> Result<CheckReport, String> {
    let m = instance(&state);
    blocking(m, |v| v.check()).await
}

/// Keeps the window out of screenshots, recordings and screen sharing while
/// the vault page is open.
#[tauri::command]
pub async fn vault_hide_from_capture(window: tauri::WebviewWindow, on: bool) -> Result<(), String> {
    window.set_content_protected(on).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "violet anchor 47 drifting";
    const EXPORT_PW: &str = "marble kettle 83 orchard";

    struct Temp(std::path::PathBuf);

    impl Temp {
        fn new() -> Temp {
            let dir = std::env::temp_dir().join(format!("sulcus-vault-cmds-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
        fn at(&self, name: &str) -> std::path::PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ready(t: &Temp) -> PasswordVault {
        let mut v = PasswordVault::for_test(&t.at("password-vault.json"));
        v.set_enabled(true);
        v.setup(MASTER).unwrap();
        v
    }

    fn login(origin: &str, user: &str, pw: &str) -> EntryInput {
        let mut i = EntryInput::default();
        i.origin = origin.into();
        i.username = user.into();
        i.password = pw.into();
        i
    }

    #[test]
    fn turning_on_needs_the_notice_and_turning_off_locks() {
        let t = Temp::new();
        let mut v = PasswordVault::for_test(&t.at("password-vault.json"));
        let off = Switch::default();
        assert!(apply_switch(&mut v, &off, true, false, 5).is_err());
        assert!(!v.is_enabled());

        let on = apply_switch(&mut v, &off, true, true, 5).unwrap();
        assert_eq!(on, Switch { on: true, accepted_ms: 5 });
        assert!(v.is_enabled());
        v.setup(MASTER).unwrap();
        assert!(v.is_open());

        let after = apply_switch(&mut v, &on, false, false, 9).unwrap();
        assert_eq!(after, Switch { on: false, accepted_ms: 5 }, "the acceptance is remembered");
        assert!(!v.is_open() && !v.is_enabled());
        assert_eq!(v.list().unwrap_err(), VaultError::Off);
    }

    #[test]
    fn status_follows_the_vault() {
        let t = Temp::new();
        let mut v = PasswordVault::for_test(&t.at("password-vault.json"));
        assert_eq!(status_of(&v, 0), Status { enabled: false, set_up: false, unlocked: false, notice_accepted: false });
        v.set_enabled(true);
        v.setup(MASTER).unwrap();
        assert_eq!(status_of(&v, 7), Status { enabled: true, set_up: true, unlocked: true, notice_accepted: true });
        v.lock();
        assert!(!status_of(&v, 7).unlocked);
    }

    #[test]
    fn the_list_never_carries_a_password() {
        let t = Temp::new();
        let mut v = ready(&t);
        v.save_login(&login("https://example.com", "ann", "hunter2-never-shown")).unwrap();
        let json = serde_json::to_string(&v.list().unwrap()).unwrap();
        assert!(json.contains("ann") && !json.contains("hunter2-never-shown"));
        let report = serde_json::to_string(&v.check().unwrap()).unwrap();
        assert!(!report.contains("hunter2-never-shown"));
    }

    #[test]
    fn a_revealed_secret_serializes_as_plain_text() {
        let r = Revealed { password: SecretOut(Zeroizing::new("p".into())), notes: SecretOut(Zeroizing::new("n".into())) };
        assert_eq!(serde_json::to_string(&r).unwrap(), r#"{"password":"p","notes":"n"}"#);
    }

    #[test]
    fn secrets_arrive_as_plain_strings() {
        let s: Secret = serde_json::from_str("\"abc\"").unwrap();
        assert_eq!(s.0, "abc");
    }

    #[test]
    fn export_then_import_into_another_vault() {
        let t = Temp::new();
        let mut a = ready(&t);
        a.save_login(&login("https://example.com", "ann", "river-stone-904-lamp")).unwrap();
        let file = t.at("out.sulcusvault");
        export_to(&a, &file, EXPORT_PW).unwrap();

        let t2 = Temp::new();
        let mut b = ready(&t2);
        let missing = import_from(&mut b, &file, ImportFormat::Sulcus, None).unwrap_err();
        assert!(matches!(missing, VaultError::BadImport(_)));
        let wrong = import_from(&mut b, &file, ImportFormat::Sulcus, Some("not the right one 99")).unwrap_err();
        assert!(matches!(wrong, VaultError::WrongPassword { .. }));
        let done = import_from(&mut b, &file, ImportFormat::Sulcus, Some(EXPORT_PW)).unwrap();
        assert_eq!(done.report.added, 1);
        assert_eq!(b.list().unwrap().len(), 1);
    }

    #[test]
    fn a_weak_export_password_is_refused_and_nothing_is_written() {
        let t = Temp::new();
        let v = ready(&t);
        let file = t.at("out.sulcusvault");
        assert!(matches!(export_to(&v, &file, "short").unwrap_err(), VaultError::WeakMaster(_)));
        assert!(!file.exists());
    }

    #[test]
    fn imports_a_browser_csv_and_names_its_source() {
        let t = Temp::new();
        let mut v = ready(&t);
        let csv = t.at("passwords.csv");
        std::fs::write(&csv, "name,url,username,password,note\nExample,https://example.com/login,ann,river-stone-904-lamp,\n").unwrap();
        let done = import_from(&mut v, &csv, ImportFormat::Other, None).unwrap();
        assert_eq!(done.source, Some(Source::ChromeOrEdge));
        assert_eq!(done.report.added, 1);
    }

    #[test]
    fn an_import_refuses_what_is_not_a_readable_file() {
        let t = Temp::new();
        let mut v = ready(&t);
        assert!(matches!(import_from(&mut v, &t.at("nope.csv"), ImportFormat::Other, None).unwrap_err(), VaultError::BadImport(_)));
        assert!(matches!(import_from(&mut v, &t.0, ImportFormat::Other, None).unwrap_err(), VaultError::BadImport(_)));
    }

    #[test]
    fn an_import_error_leaves_the_file_path_out_of_the_text() {
        let t = Temp::new();
        let mut v = ready(&t);
        let e = import_from(&mut v, &t.at("missing-file-name.csv"), ImportFormat::Other, None).unwrap_err();
        assert!(!e.to_string().contains("missing-file-name"));
    }

    #[test]
    fn nothing_is_shown_while_locked() {
        let t = Temp::new();
        let mut v = ready(&t);
        let (id, _) = v.save_login(&login("https://example.com", "ann", "river-stone-904-lamp")).unwrap();
        v.lock();
        assert_eq!(v.reveal(&id).unwrap_err(), VaultError::Locked);
        assert_eq!(v.list().unwrap_err(), VaultError::Locked);
    }

    #[test]
    fn lock_now_with_no_vault_is_harmless() {
        lock_now();
    }

    #[test]
    fn format_names_come_from_the_window_in_snake_case() {
        assert_eq!(serde_json::from_str::<ImportFormat>("\"sulcus\"").unwrap(), ImportFormat::Sulcus);
        assert_eq!(serde_json::from_str::<ImportFormat>("\"other\"").unwrap(), ImportFormat::Other);
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Acceptance of the legal documents (docs/legal) and of the short
//! responsibility notices shown before risky features turn on, each stored
//! with the version that was accepted. Raising a version here asks everyone
//! again, so a person is only ever bound by text they were shown.
//!
//! Also "export everything" and "delete everything". Deleting can't happen
//! while the database and logs are open, so it is staged with a marker file
//! and done at the next start, before anything opens (the same way a backup
//! restore is swapped in).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::{db, AppState, AppStateRef};

const STORE: &str = "consent";
const ERASE_MARKER: &str = ".erase";
/// Typed by the person in the window; a stray call can't wipe the PC's data.
pub const ERASE_PHRASE: &str = "delete everything";
/// What a command returns when a feature's notice hasn't been accepted, so
/// the window can show it and try again.
pub const NEEDED_PREFIX: &str = "consent:";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Document,
    Notice,
}

pub struct Item {
    pub id: &'static str,
    pub version: u32,
    pub kind: Kind,
    /// Asked before the app can be used at all.
    pub at_start: bool,
    /// The file under docs/legal whose "Version N" line must match.
    pub file: Option<&'static str>,
    /// Documents accepted together with this notice.
    pub with: &'static [&'static str],
}

const fn doc(id: &'static str, version: u32, at_start: bool, file: &'static str) -> Item {
    Item { id, version, kind: Kind::Document, at_start, file: Some(file), with: &[] }
}

const fn notice(id: &'static str, version: u32, file: Option<&'static str>, with: &'static [&'static str]) -> Item {
    Item { id, version, kind: Kind::Notice, at_start: false, file, with }
}

pub const ITEMS: &[Item] = &[
    doc("terms", 1, true, "terms-of-use.md"),
    doc("privacy", 1, true, "privacy-policy.md"),
    doc("guidelines", 1, false, "community-guidelines.md"),
    notice("social", 1, None, &["guidelines"]),
    notice("vpn", 1, None, &[]),
    notice("file_sharing", 1, None, &[]),
    notice("relay", 1, Some("running-relays-and-servers.md"), &[]),
    notice("remote_control", 1, None, &[]),
    notice("uncensored", 1, None, &[]),
    notice("likeness", 1, Some("likeness-consent.md"), &[]),
    notice("market", 1, Some("market-data-disclaimer.md"), &[]),
];

pub fn item(id: &str) -> Option<&'static Item> {
    ITEMS.iter().find(|i| i.id == id)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Record {
    pub version: u32,
    pub at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    accepted: BTreeMap<String, Record>,
}

fn load(conn: &Connection) -> Store {
    db::get(conn, STORE).unwrap_or_default()
}

/// A newer version accepted (by a newer copy of the app) still counts, so
/// going back a version doesn't ask again for text that's already settled.
pub fn is_due(current: u32, accepted: Option<u32>) -> bool {
    accepted.is_none_or(|v| v < current)
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub id: &'static str,
    pub kind: Kind,
    pub version: u32,
    pub accepted: Option<u32>,
    pub accepted_at: Option<i64>,
    pub due: bool,
    pub at_start: bool,
    pub file: Option<&'static str>,
    pub with: &'static [&'static str],
}

pub fn status(conn: &Connection) -> Vec<Status> {
    let store = load(conn);
    ITEMS
        .iter()
        .map(|i| {
            let r = store.accepted.get(i.id);
            Status {
                id: i.id,
                kind: i.kind,
                version: i.version,
                accepted: r.map(|r| r.version),
                accepted_at: r.map(|r| r.at),
                due: is_due(i.version, r.map(|r| r.version)),
                at_start: i.at_start,
                file: i.file,
                with: i.with,
            }
        })
        .collect()
}

/// The documents to accept before the app opens.
pub fn start_due(conn: &Connection) -> Vec<&'static str> {
    status(conn).into_iter().filter(|s| s.at_start && s.due).map(|s| s.id).collect()
}

/// Whether a feature's notice (and the documents that come with it) still
/// needs accepting.
pub fn needs(conn: &Connection, id: &str) -> bool {
    let Some(i) = item(id) else { return false };
    let store = load(conn);
    std::iter::once(i.id).chain(i.with.iter().copied()).any(|id| {
        let current = item(id).map_or(0, |i| i.version);
        is_due(current, store.accepted.get(id).map(|r| r.version))
    })
}

/// For the code that turns a feature on: an error the window recognizes
/// until the notice is accepted.
pub fn require(conn: &Connection, id: &str) -> Result<(), String> {
    if needs(conn, id) {
        Err(format!("{NEEDED_PREFIX}{id}"))
    } else {
        Ok(())
    }
}

/// Records acceptance of `shown`, each with the version the window showed.
/// A version that isn't the current one is refused: the text changed while
/// it was open, and the person hasn't seen the new one.
pub fn accept(conn: &Connection, shown: &[(String, u32)], now: i64) -> Result<(), String> {
    let mut store = load(conn);
    for (id, version) in shown {
        let Some(i) = item(id) else { return Err(format!("There's no document called \"{id}\".")) };
        if *version != i.version {
            return Err("This text changed while it was open. Read the new version, then accept it.".into());
        }
    }
    for (id, version) in shown {
        store.accepted.insert(id.clone(), Record { version: *version, at: now });
    }
    db::set(conn, STORE, &store)
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct EraseRequest {
    pub keep_models: bool,
    pub at: i64,
}

pub fn schedule_erase(data: &Path, keep_models: bool, now: i64) -> Result<(), String> {
    let req = EraseRequest { keep_models, at: now };
    std::fs::write(data.join(ERASE_MARKER), serde_json::to_vec(&req).map_err(|e| e.to_string())?).map_err(|e| format!("Couldn't get ready to delete: {e}"))
}

pub fn erase_pending(data: &Path) -> Option<EraseRequest> {
    serde_json::from_slice(&std::fs::read(data.join(ERASE_MARKER)).ok()?).ok()
}

pub fn cancel_erase(data: &Path) -> Result<(), String> {
    match std::fs::remove_file(data.join(ERASE_MARKER)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Erased {
    pub removed: usize,
    /// Entries that couldn't be removed (in use, or no permission).
    pub left: Vec<PathBuf>,
    /// A models folder chosen outside the app's folder: it may hold other
    /// things, so it is never deleted wholesale.
    pub models_elsewhere: Option<PathBuf>,
}

/// Removes a link itself, never what it points to: a junction to a shared
/// folder must not take that folder with it.
fn remove_entry(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path))
    } else if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Carries out a staged "delete everything". Called at start, before the
/// log, the database or the vault open. Returns None when nothing was staged.
pub fn apply_pending_erase(paths: &Paths) -> Result<Option<Erased>, String> {
    let data = &paths.data;
    let Some(req) = erase_pending(data) else { return Ok(None) };
    // A data folder at a drive's root would mean a wrong setting, not a
    // folder of the app's own to clear.
    if data.parent().is_none() {
        cancel_erase(data)?;
        return Err("The data folder is a whole drive, so nothing was deleted.".into());
    }
    // The marker goes first, so a crash part way can't wipe again at every start.
    cancel_erase(data)?;
    let models_inside = paths.models.starts_with(data);
    let mut keep: Vec<PathBuf> = Vec::new();
    if req.keep_models {
        keep.extend([paths.engines.clone(), data.join(crate::storage::LOCATION_FILE)]);
        if models_inside {
            keep.push(paths.models.clone());
        }
    }
    let mut out = Erased { models_elsewhere: (!models_inside && !req.keep_models).then(|| paths.models.clone()), ..Default::default() };
    let entries = std::fs::read_dir(data).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if keep.iter().any(|k| k == &path) {
            continue;
        }
        match remove_entry(&path) {
            Ok(()) => out.removed += 1,
            Err(_) => out.left.push(path),
        }
    }
    for dir in [&paths.models, &paths.engines, &paths.logs] {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    Ok(Some(out))
}

#[derive(Serialize)]
pub struct Exported {
    pub folder: String,
    pub readable: String,
    pub backup: String,
}

/// Everything the app holds, in one new folder: chats and notes as readable
/// Markdown, plus a password-protected backup of all the rest (protected
/// chats, pictures, recordings, settings), which only the backup keeps
/// encrypted.
pub fn export_everything(state: &AppState, dir: &Path, password: &str) -> Result<Exported, String> {
    let folder = dir.join(format!("SulcusAI everything {}", chrono::Local::now().format("%Y-%m-%d %H%M")));
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let backup = folder.join("SulcusAI backup.sulcusbackup");
    crate::backup::make(state, &backup, password)?;
    let readable = crate::backup::export_markdown(state, &folder)?;
    let consent = serde_json::to_string_pretty(&status(&state.db.lock().unwrap())).map_err(|e| e.to_string())?;
    std::fs::write(folder.join("Accepted terms.json"), consent).map_err(|e| e.to_string())?;
    Ok(Exported { folder: folder.display().to_string(), readable: readable.display().to_string(), backup: backup.display().to_string() })
}

#[tauri::command]
pub fn consent_status(state: AppStateRef<'_>) -> Vec<Status> {
    status(&state.db.lock().unwrap())
}

#[derive(Deserialize)]
pub struct Shown {
    id: String,
    version: u32,
}

#[tauri::command]
pub fn consent_accept(state: AppStateRef<'_>, shown: Vec<Shown>) -> Result<Vec<Status>, String> {
    let shown: Vec<(String, u32)> = shown.into_iter().map(|s| (s.id, s.version)).collect();
    let conn = state.db.lock().unwrap();
    accept(&conn, &shown, db::now_ms())?;
    let list = shown.iter().map(|(id, v)| format!("{id} v{v}")).collect::<Vec<_>>().join(", ");
    db::log_action(&conn, "consent", &format!("Accepted {list}"));
    Ok(status(&conn))
}

#[tauri::command]
pub async fn export_everything_cmd(state: AppStateRef<'_>, dir: String, password: String) -> Result<Exported, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || export_everything(&state, Path::new(&dir), &password)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn erase_everything(state: AppStateRef<'_>, keep_models: bool, confirm: String) -> Result<(), String> {
    if confirm.trim().to_lowercase() != ERASE_PHRASE {
        return Err(format!("Type \"{ERASE_PHRASE}\" to confirm."));
    }
    schedule_erase(&state.paths.data, keep_models, db::now_ms())?;
    state.log("consent", "Everything will be deleted when SulcusAI next starts");
    Ok(())
}

#[tauri::command]
pub fn erase_status(state: AppStateRef<'_>) -> Option<EraseRequest> {
    erase_pending(&state.paths.data)
}

#[tauri::command]
pub fn cancel_erase_everything(state: AppStateRef<'_>) -> Result<(), String> {
    cancel_erase(&state.paths.data)?;
    state.log("consent", "Deleting everything was called off");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        db::init_for_test(&c);
        c
    }

    fn current(id: &str) -> u32 {
        item(id).unwrap().version
    }

    #[test]
    fn versions_compare() {
        assert!(is_due(1, None), "never accepted");
        assert!(is_due(2, Some(1)), "an older version");
        assert!(!is_due(2, Some(2)));
        assert!(!is_due(1, Some(2)), "accepted in a newer copy of the app");
    }

    #[test]
    fn asked_at_start_until_accepted() {
        let c = conn();
        assert_eq!(start_due(&c), vec!["terms", "privacy"]);
        accept(&c, &[("terms".into(), current("terms"))], 5).unwrap();
        assert_eq!(start_due(&c), vec!["privacy"]);
        accept(&c, &[("privacy".into(), current("privacy"))], 6).unwrap();
        assert!(start_due(&c).is_empty());
        let s = status(&c).into_iter().find(|s| s.id == "terms").unwrap();
        assert_eq!((s.accepted, s.accepted_at, s.due), (Some(current("terms")), Some(5), false));
    }

    #[test]
    fn asked_again_when_a_version_changes() {
        let c = conn();
        // Accepted before the version this copy of the app ships.
        let mut store = Store::default();
        store.accepted.insert("terms".into(), Record { version: 0, at: 1 });
        db::set(&c, STORE, &store).unwrap();
        assert!(start_due(&c).contains(&"terms"));
        accept(&c, &[("terms".into(), current("terms"))], 2).unwrap();
        assert!(!start_due(&c).contains(&"terms"));
    }

    #[test]
    fn a_stale_or_unknown_version_is_refused() {
        let c = conn();
        assert!(accept(&c, &[("terms".into(), current("terms") + 1)], 1).is_err());
        assert!(accept(&c, &[("nothing".into(), 1)], 1).is_err());
        // Nothing from a refused batch is kept.
        assert!(accept(&c, &[("privacy".into(), current("privacy")), ("terms".into(), 99)], 1).is_err());
        assert!(start_due(&c).contains(&"privacy"));
    }

    #[test]
    fn a_notice_needs_its_documents_too() {
        let c = conn();
        assert!(needs(&c, "social"));
        assert_eq!(require(&c, "social"), Err("consent:social".into()));
        accept(&c, &[("social".into(), current("social"))], 1).unwrap();
        assert!(needs(&c, "social"), "the guidelines aren't accepted yet");
        accept(&c, &[("guidelines".into(), current("guidelines"))], 1).unwrap();
        assert!(require(&c, "social").is_ok());
        assert!(!needs(&c, "not a feature"));
    }

    #[test]
    fn every_document_file_has_the_version_the_app_asks_for() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/legal");
        for i in ITEMS.iter().filter(|i| i.file.is_some()) {
            let file = i.file.unwrap();
            let text = std::fs::read_to_string(dir.join(file)).unwrap_or_else(|_| panic!("{file} is missing"));
            let line = text.lines().find(|l| l.starts_with("Version ")).unwrap_or_else(|| panic!("{file} has no Version line"));
            let n: u32 = line["Version ".len()..].split(',').next().unwrap().trim().parse().unwrap();
            assert_eq!(n, i.version, "{file} says version {n}, the app asks for {}", i.version);
        }
    }

    fn temp_paths() -> Paths {
        let dir = std::env::temp_dir().join(format!("sulcusai-consent-{}", uuid::Uuid::new_v4()));
        Paths::new(dir).unwrap()
    }

    #[test]
    fn erase_waits_for_the_next_start_and_can_be_called_off() {
        let p = temp_paths();
        std::fs::write(&p.db, b"db").unwrap();
        assert!(apply_pending_erase(&p).unwrap().is_none(), "nothing staged");
        schedule_erase(&p.data, false, 1).unwrap();
        assert_eq!(erase_pending(&p.data), Some(EraseRequest { keep_models: false, at: 1 }));
        cancel_erase(&p.data).unwrap();
        assert!(apply_pending_erase(&p).unwrap().is_none());
        assert!(p.db.exists());
        std::fs::remove_dir_all(&p.data).ok();
    }

    #[test]
    fn erase_removes_data_and_keeps_models_when_asked() {
        let p = temp_paths();
        std::fs::write(&p.db, b"db").unwrap();
        std::fs::create_dir_all(p.data.join("gallery")).unwrap();
        std::fs::write(p.data.join("gallery/a.png"), b"x").unwrap();
        std::fs::write(p.models.join("m.gguf"), b"m").unwrap();
        schedule_erase(&p.data, true, 1).unwrap();
        let done = apply_pending_erase(&p).unwrap().unwrap();
        assert!(done.left.is_empty());
        assert!(!p.db.exists() && !p.data.join("gallery").exists());
        assert!(p.models.join("m.gguf").exists());
        assert!(erase_pending(&p.data).is_none(), "done once, not at every start");

        schedule_erase(&p.data, false, 2).unwrap();
        apply_pending_erase(&p).unwrap().unwrap();
        assert!(!p.models.join("m.gguf").exists());
        assert!(p.models.exists() && p.logs.exists(), "the app's folders are back for the fresh start");
        std::fs::remove_dir_all(&p.data).ok();
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Backups: one password-protected file with chats, notes, memory, settings,
//! pictures and meeting recordings, restorable on this PC or another.
//! Models and engines aren't included (they're large and can be downloaded
//! or found again).
//!
//! Every file inside is encrypted (AES-256-GCM, in 4 MB pieces) with a key
//! made for that backup; the key is locked with the backup's password
//! (Argon2id). The app's own data key travels sealed with the backup key,
//! because the copy on this PC is tied to this Windows account.
//!
//! A restore is staged in `.restore/` and swapped in when the app next
//! starts, before the database opens. The data it replaces is moved to a
//! dated `before-restore-…` folder, not deleted.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::crypto::{Cipher, Vault};
use crate::{db, AppState, AppStateRef};

const FORMAT: &str = "sulcusai-backup";
const VERSION: u32 = 1;
const CHUNK: usize = 4 << 20;
const STAGE: &str = ".restore";
const READY: &str = "READY";
const RESTORED: &str = ".restored";
/// Folders in the data folder that go into a backup.
const FOLDERS: &[&str] = &["media", "meetings"];
const MIN_PASSWORD: usize = 8;

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    format: String,
    version: u32,
    pub app_version: String,
    /// Milliseconds since 1970.
    pub created: i64,
    /// The backup key, locked with the password.
    key: String,
    /// The app's data key, sealed with the backup key (base64).
    data_key: String,
    pub files: Vec<Entry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Entry {
    /// Relative to the data folder, with forward slashes.
    pub path: String,
    pub size: u64,
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Every file under `dir`, relative to `base`.
fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(base, &p, out);
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.push((rel.to_string_lossy().replace('\\', "/"), p));
        }
    }
}

/// Encrypts `src` into the zip entry being written, piece by piece.
fn seal_into(w: &mut impl Write, src: &Path, key: &Cipher) -> Result<u64, String> {
    let mut f = std::fs::File::open(src).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let mut n = 0;
        while n < CHUNK {
            let got = f.read(&mut buf[n..]).map_err(|e| e.to_string())?;
            if got == 0 {
                break;
            }
            n += got;
        }
        if n == 0 {
            return Ok(total);
        }
        let sealed = key.seal_bytes(&buf[..n]);
        w.write_all(&(sealed.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
        w.write_all(&sealed).map_err(|e| e.to_string())?;
        total += n as u64;
    }
}

fn open_from(r: &mut impl Read, dest: &Path, key: &Cipher) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut out = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut len = [0u8; 4];
    loop {
        match r.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e.to_string()),
        }
        let n = u32::from_le_bytes(len) as usize;
        if n > CHUNK + 64 {
            return Err("The backup is damaged.".into());
        }
        let mut sealed = vec![0u8; n];
        r.read_exact(&mut sealed).map_err(|_| "The backup is damaged (a file is cut short).".to_string())?;
        out.write_all(&key.open_bytes(&sealed)?).map_err(|e| e.to_string())?;
    }
}

#[derive(Serialize)]
pub struct Made {
    files: usize,
    bytes: u64,
}

/// Writes a backup to `dest`.
pub fn make(state: &AppState, dest: &Path, password: &str) -> Result<Made, String> {
    if password.chars().count() < MIN_PASSWORD {
        return Err(format!("Use a password of at least {MIN_PASSWORD} characters. You'll need it to restore."));
    }
    state.cipher()?;
    let key = Cipher::random();
    let data_key = state.vault.lock().unwrap().data_key_for_backup(&key)?;
    // A consistent copy of the database, even while it's in use.
    let snapshot = state.paths.data.join(".backup-snapshot.db");
    std::fs::remove_file(&snapshot).ok();
    state.db.lock().unwrap().execute("VACUUM INTO ?1", [snapshot.display().to_string()]).map_err(|e| format!("Couldn't copy the database: {e}"))?;

    let mut files = vec![("sulcusai.db".to_string(), snapshot.clone())];
    for f in FOLDERS {
        walk(&state.paths.data, &state.paths.data.join(f), &mut files);
    }
    let tmp = dest.with_extension("partial");
    let result = (|| {
        let out = std::fs::File::create(&tmp).map_err(|e| format!("Couldn't create the backup file: {e}"))?;
        let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(out));
        // Encrypted data doesn't compress.
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
        let mut entries = Vec::new();
        let mut bytes = 0u64;
        for (rel, src) in &files {
            zip.start_file(format!("files/{rel}"), opts).map_err(|e| e.to_string())?;
            let size = seal_into(&mut zip, src, &key)?;
            bytes += size;
            entries.push(Entry { path: rel.clone(), size });
        }
        let manifest = Manifest {
            format: FORMAT.into(),
            version: VERSION,
            app_version: env!("CARGO_PKG_VERSION").into(),
            created: db::now_ms(),
            key: key.lock_with(password)?,
            data_key: base64::Engine::encode(&b64(), data_key),
            files: entries,
        };
        zip.start_file("manifest.json", opts).map_err(|e| e.to_string())?;
        zip.write_all(serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?.as_bytes()).map_err(|e| e.to_string())?;
        zip.finish().map_err(|e| e.to_string())?;
        Ok(Made { files: manifest.files.len(), bytes })
    })();
    std::fs::remove_file(&snapshot).ok();
    match result {
        Ok(made) => {
            std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
            state.log("backup", &format!("Made a backup ({} files) at {}", made.files, dest.display()));
            Ok(made)
        }
        Err(e) => {
            std::fs::remove_file(&tmp).ok();
            Err(e)
        }
    }
}

fn archive(path: &Path) -> Result<zip::ZipArchive<std::fs::File>, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    zip::ZipArchive::new(f).map_err(|_| "That isn't a SulcusAI backup.".to_string())
}

pub fn manifest(path: &Path) -> Result<Manifest, String> {
    let mut zip = archive(path)?;
    let mut text = String::new();
    zip.by_name("manifest.json").map_err(|_| "That isn't a SulcusAI backup.".to_string())?.read_to_string(&mut text).map_err(|e| e.to_string())?;
    let m: Manifest = serde_json::from_str(&text).map_err(|_| "That backup's details are damaged.".to_string())?;
    if m.format != FORMAT {
        return Err("That isn't a SulcusAI backup.".into());
    }
    if m.version > VERSION {
        return Err("That backup was made by a newer version of SulcusAI. Update the app first.".into());
    }
    Ok(m)
}

/// Unpacks a backup into the staging folder, ready for the next start.
pub fn stage(data: &Path, src: &Path, password: &str, protector: &dyn crate::crypto::Protector) -> Result<Manifest, String> {
    let m = manifest(src)?;
    let key = Cipher::unlock_with(&m.key, password).ok_or("That password isn't right for this backup.")?;
    let stage = data.join(STAGE);
    if stage.exists() {
        std::fs::remove_dir_all(&stage).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut zip = archive(src)?;
        for e in &m.files {
            // Only plain relative paths inside the data folder.
            let rel = Path::new(&e.path);
            if rel.is_absolute() || rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
                return Err("The backup is damaged (a file has an odd name).".to_string());
            }
            let mut entry = zip.by_name(&format!("files/{}", e.path)).map_err(|_| "The backup is damaged (a file is missing).".to_string())?;
            open_from(&mut entry, &stage.join(rel), &key)?;
        }
        let sealed = base64::Engine::decode(&b64(), &m.data_key).map_err(|_| "The backup is damaged.".to_string())?;
        Vault::write_restored(&stage.join("keys.json"), protector, &sealed, &key)?;
        std::fs::write(stage.join(READY), b"").map_err(|e| e.to_string())
    })();
    if let Err(e) = result {
        std::fs::remove_dir_all(&stage).ok();
        return Err(e);
    }
    Ok(m)
}

/// Swaps a staged restore in. Call before the database opens. Returns the
/// folder the replaced data was moved to.
pub fn apply_pending(data: &Path) -> Result<Option<PathBuf>, String> {
    let stage = data.join(STAGE);
    if !stage.join(READY).exists() {
        if stage.exists() {
            std::fs::remove_dir_all(&stage).ok();
        }
        return Ok(None);
    }
    let aside = data.join(format!("before-restore-{}", chrono::Local::now().format("%Y-%m-%d-%H%M%S")));
    std::fs::create_dir_all(&aside).map_err(|e| e.to_string())?;
    for name in ["sulcusai.db", "sulcusai.db-wal", "sulcusai.db-shm", "keys.json"].iter().chain(FOLDERS) {
        let from = data.join(name);
        if from.exists() {
            std::fs::rename(&from, aside.join(name)).map_err(|e| format!("Couldn't move {name} aside: {e}"))?;
        }
    }
    for e in std::fs::read_dir(&stage).map_err(|e| e.to_string())?.flatten() {
        if e.file_name() == READY {
            continue;
        }
        std::fs::rename(e.path(), data.join(e.file_name())).map_err(|e| e.to_string())?;
    }
    std::fs::remove_dir_all(&stage).ok();
    std::fs::write(data.join(RESTORED), aside.display().to_string()).ok();
    Ok(Some(aside))
}

/// After a restore: forget models whose files aren't on this PC (they can
/// be installed or found again) and note it in Activity.
pub fn finish_restore(state: &AppState) {
    let marker = state.paths.data.join(RESTORED);
    let Ok(aside) = std::fs::read_to_string(&marker) else { return };
    std::fs::remove_file(&marker).ok();
    let conn = state.db.lock().unwrap();
    let mut dropped = 0;
    for m in db::installed_models(&conn) {
        if !crate::model_file(&state.paths, &m).exists() {
            let _ = db::remove_installed(&conn, &m.model_id);
            dropped += 1;
        }
    }
    let first = db::installed_models(&conn).first().map(|m| m.model_id.clone());
    let default_gone = db::settings(&conn).default_model.is_some_and(|d| db::installed_model(&conn, &d).is_none());
    if default_gone {
        let _ = db::update_settings(&conn, |s| s.default_model = first);
    }
    dropped += crate::speech::prune_missing(&conn) + crate::media::prune_missing(&conn, &state.paths, &state.catalog.media);
    drop(conn);
    let models = if dropped > 0 { format!(" {dropped} model(s) that aren't on this PC were taken off the list; install them again from Models.") } else { String::new() };
    state.log("backup", &format!("Restored from a backup. The data it replaced is in {aside}.{models}"));
}

// ---------- commands ----------

#[tauri::command]
pub async fn make_backup(state: AppStateRef<'_>, path: String, password: String) -> Result<Made, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || make(&state, Path::new(&path), &password)).await.map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct BackupInfo {
    created: i64,
    app_version: String,
    files: usize,
    bytes: u64,
}

#[tauri::command]
pub fn backup_info(path: String) -> Result<BackupInfo, String> {
    let m = manifest(Path::new(&path))?;
    Ok(BackupInfo { created: m.created, app_version: m.app_version, files: m.files.len(), bytes: m.files.iter().map(|f| f.size).sum() })
}

/// Checks the password, unpacks the backup and restarts the app to finish.
#[tauri::command]
pub async fn restore_backup(app: AppHandle, state: AppStateRef<'_>, path: String, password: String) -> Result<(), String> {
    let data = state.paths.data.clone();
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(windows)]
        let protector = crate::crypto::dpapi::Dpapi;
        #[cfg(windows)]
        return stage(&data, Path::new(&path), &password, &protector).map(|_| ());
        #[cfg(not(windows))]
        Err::<(), String>("Restoring needs Windows for now.".into())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.log("backup", "Unpacked a backup; restarting to finish restoring it");
    // Give the window a moment to show that it's restarting.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(600));
        app.restart();
    });
    Ok(())
}

// ---------- readable export ----------

fn safe_name(title: &str, used: &mut std::collections::HashSet<String>) -> String {
    let mut base: String = title.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '-' } else { c }).collect();
    base = base.trim().trim_end_matches('.').chars().take(80).collect();
    if base.is_empty() {
        base = "Untitled".into();
    }
    let mut name = base.clone();
    let mut n = 2;
    while !used.insert(name.to_lowercase()) {
        name = format!("{base} ({n})");
        n += 1;
    }
    name
}

/// Writes every chat and note as Markdown into a new folder in `dir`.
pub fn export_markdown(state: &AppState, dir: &Path) -> Result<PathBuf, String> {
    let c = state.cipher()?;
    let root = dir.join(format!("SulcusAI export {}", chrono::Local::now().format("%Y-%m-%d %H%M")));
    let (chats_dir, notes_dir) = (root.join("Chats"), root.join("Notes"));
    std::fs::create_dir_all(&chats_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&notes_dir).map_err(|e| e.to_string())?;
    let conn = state.db.lock().unwrap();
    let when = |ms: i64| chrono::DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
    let mut used = std::collections::HashSet::new();
    let mut chats = 0;
    for chat in db::list_chats(&conn, &c).into_iter().filter(|c| !c.incognito) {
        let mut md = format!("# {}\n\n_Started {}_\n\n", chat.title, when(chat.created_at));
        for m in db::messages(&conn, &c, &chat.id) {
            match m.role.as_str() {
                "user" => md.push_str(&format!("## You ({})\n\n{}\n\n", when(m.created_at), m.content.trim())),
                "assistant" if !m.content.trim().is_empty() => md.push_str(&format!("## Assistant\n\n{}\n\n", m.content.trim())),
                _ => {}
            }
        }
        std::fs::write(chats_dir.join(format!("{}.md", safe_name(&chat.title, &mut used))), md).map_err(|e| e.to_string())?;
        chats += 1;
    }
    let mut used = std::collections::HashSet::new();
    let notes = crate::notes::list_notes(&conn, &c);
    for n in &notes {
        let title = if n.data.title.trim().is_empty() { "Untitled" } else { n.data.title.trim() };
        let tags = if n.data.tags.is_empty() { String::new() } else { format!("\n\nTags: {}", n.data.tags.join(", ")) };
        std::fs::write(notes_dir.join(format!("{}.md", safe_name(title, &mut used))), format!("# {title}\n\n{}{tags}\n", n.data.body.trim())).map_err(|e| e.to_string())?;
    }
    drop(conn);
    state.log("backup", &format!("Exported {chats} chats and {} notes as Markdown to {}", notes.len(), root.display()));
    Ok(root)
}

#[tauri::command]
pub async fn export_markdown_cmd(state: AppStateRef<'_>, dir: String) -> Result<String, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || export_markdown(&state, Path::new(&dir)).map(|p| p.display().to_string())).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_pieces_round_trip() {
        let dir = std::env::temp_dir().join(format!("sulcusai-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.bin");
        // Bigger than one piece, not a multiple of it.
        let data: Vec<u8> = (0..(CHUNK + 12345)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        let key = Cipher::random();
        let mut sealed = Vec::new();
        assert_eq!(seal_into(&mut sealed, &src, &key).unwrap(), data.len() as u64);
        let out = dir.join("b.bin");
        open_from(&mut sealed.as_slice(), &out, &key).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), data);
        assert!(open_from(&mut sealed.as_slice(), &out, &Cipher::random()).is_err(), "another key can't open it");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_names_are_safe_and_unique() {
        let mut used = std::collections::HashSet::new();
        assert_eq!(safe_name("Plan: Q4/Q1?", &mut used), "Plan- Q4-Q1-");
        assert_eq!(safe_name("Plan: Q4/Q1?", &mut used), "Plan- Q4-Q1- (2)");
        assert_eq!(safe_name("  ", &mut used), "Untitled");
    }

    #[test]
    fn a_staged_restore_is_swapped_in_and_the_old_data_kept() {
        let data = std::env::temp_dir().join(format!("sulcusai-rs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(data.join(STAGE).join("media")).unwrap();
        std::fs::create_dir_all(data.join("media")).unwrap();
        std::fs::write(data.join("sulcusai.db"), b"old").unwrap();
        std::fs::write(data.join("media").join("x"), b"old pic").unwrap();
        std::fs::write(data.join(STAGE).join("sulcusai.db"), b"new").unwrap();
        std::fs::write(data.join(STAGE).join("media").join("y"), b"new pic").unwrap();
        // Not ready yet: nothing happens (and the half-done stage is cleared).
        assert!(apply_pending(&data).unwrap().is_none());
        assert_eq!(std::fs::read(data.join("sulcusai.db")).unwrap(), b"old");
        std::fs::create_dir_all(data.join(STAGE)).unwrap();
        std::fs::write(data.join(STAGE).join("sulcusai.db"), b"new").unwrap();
        std::fs::write(data.join(STAGE).join(READY), b"").unwrap();
        let aside = apply_pending(&data).unwrap().unwrap();
        assert_eq!(std::fs::read(data.join("sulcusai.db")).unwrap(), b"new");
        assert_eq!(std::fs::read(aside.join("sulcusai.db")).unwrap(), b"old");
        assert_eq!(std::fs::read(aside.join("media").join("x")).unwrap(), b"old pic");
        assert!(!data.join(STAGE).exists());
        std::fs::remove_dir_all(&data).ok();
    }
}

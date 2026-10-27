// SPDX-License-Identifier: AGPL-3.0-only
//! Where models are kept (DESIGN.md §4.10): the data folder's `models`, or a
//! folder the user picks on another drive. The choice lives in a small file
//! beside the database (`models-location`), so it's known before anything
//! else starts.
//!
//! Moving copies every file (or renames it, on the same drive), checks the
//! sizes, switches the location, then removes the old copies and restarts the
//! app. Installed models' stored paths need no rewrite: the app already looks
//! for a model's file in today's models folder when its old path is gone.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::AppStateRef;

pub const LOCATION_FILE: &str = "models-location";

/// The models folder for a data folder: the chosen one if it's set and
/// usable, else `<data>/models`.
pub fn models_dir(data: &Path) -> PathBuf {
    std::fs::read_to_string(data.join(LOCATION_FILE))
        .ok()
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| p.is_absolute() && std::fs::create_dir_all(p).is_ok())
        .unwrap_or_else(|| data.join("models"))
}

static MOVING: AtomicBool = AtomicBool::new(false);
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

#[derive(Serialize)]
pub struct Drive {
    mount: String,
    free: u64,
    total: u64,
}

#[derive(Serialize)]
pub struct StorageView {
    models_dir: String,
    default_dir: String,
    /// Bytes the models folder holds.
    used: u64,
    drives: Vec<Drive>,
    moving: bool,
    error: Option<String>,
}

/// The size of everything in a folder.
pub fn folder_size(dir: &Path) -> u64 {
    size_of(dir)
}

fn size_of(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => size_of(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// A file to move, and whether it was reached through a link (a symbolic
/// link or junction to somewhere else): those are copied, never renamed or
/// deleted, since the files they point at belong to that other place.
struct Item {
    path: PathBuf,
    linked: bool,
}

/// Every file under `dir`, following links; the links themselves go in `links`.
fn files_in(dir: &Path, linked: bool, depth: usize, out: &mut Vec<Item>, links: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(t) = e.file_type() else { continue };
        let is_link = t.is_symlink() || is_junction(&path);
        if is_link && !linked {
            links.push(path.clone());
        }
        // Following a link: what it points at (a file or a folder).
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.is_dir() {
            if depth < 32 {
                files_in(&path, linked || is_link, depth + 1, out, links);
            }
        } else if meta.is_file() {
            out.push(Item { path, linked: linked || is_link });
        }
    }
}

/// A Windows junction (a folder link that isn't a symbolic link).
#[cfg(windows)]
fn is_junction(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

#[cfg(not(windows))]
fn is_junction(_path: &Path) -> bool {
    false
}

/// Writes a file so a crash or power cut leaves the old contents or the new,
/// never half of either: a copy beside it, flushed to the disk, then put in
/// its place. Where that can't be done (another app holds the file open),
/// it's written in place as before.
pub fn write_safely(path: &Path, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    use std::io::Write;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".sulcusai-{}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    let swapped = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes.as_ref())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if swapped.is_ok() {
        return Ok(());
    }
    std::fs::remove_file(&tmp).ok();
    std::fs::write(path, bytes)
}

/// Free space on the drive holding `dir`.
pub fn free_on(dir: &Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks.list().iter().filter(|d| dir.starts_with(d.mount_point())).max_by_key(|d| d.mount_point().as_os_str().len()).map(|d| d.available_space())
}

#[tauri::command(async)]
pub fn storage_view(state: AppStateRef) -> StorageView {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut drives: Vec<Drive> = disks
        .list()
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| Drive { mount: d.mount_point().display().to_string(), free: d.available_space(), total: d.total_space() })
        .collect();
    drives.sort_by(|a, b| a.mount.cmp(&b.mount));
    drives.dedup_by(|a, b| a.mount == b.mount);
    StorageView {
        models_dir: state.paths.models.display().to_string(),
        default_dir: state.paths.data.join("models").display().to_string(),
        used: size_of(&state.paths.models),
        drives,
        moving: MOVING.load(Ordering::SeqCst),
        error: LAST_ERROR.lock().unwrap().clone(),
    }
}

/// What a move did so far: enough to finish it, or to undo it.
#[derive(Default)]
struct Moved {
    /// Renamed on the same drive (from, to).
    renamed: Vec<(PathBuf, PathBuf)>,
    /// Copied to another drive (from, to): the old copy goes once the move is done.
    copied: Vec<(PathBuf, PathBuf)>,
    /// Already at the destination, the same size: the old copy just goes.
    reused: Vec<PathBuf>,
    /// Links in the old folder (removed at the end; what they point at stays).
    links: Vec<PathBuf>,
}

impl Moved {
    /// Puts everything back the way it was (a later file failed).
    fn undo(&self) {
        for (from, to) in self.renamed.iter().rev() {
            std::fs::rename(to, from).ok();
        }
        for (_, to) in &self.copied {
            std::fs::remove_file(to).ok();
        }
    }

    /// Removes the old copies, once the app looks in the new folder.
    fn finish(&self) {
        for f in self.copied.iter().map(|(from, _)| from).chain(&self.reused) {
            std::fs::remove_file(f).ok();
        }
        for l in &self.links {
            // A link to a folder is removed as a folder, one to a file as a file; neither touches its target.
            if std::fs::remove_dir(l).is_err() {
                std::fs::remove_file(l).ok();
            }
        }
    }

    fn count(&self) -> usize {
        self.renamed.len() + self.copied.len() + self.reused.len()
    }
}

/// Moves everything from `from` into `to`, reporting bytes: renamed on the
/// same drive, copied (and checked) to another. Files already in `to` with
/// the same size are kept. If any file fails, what was done is undone, so a
/// failed move leaves the models where they were.
fn copy_all(from: &Path, to: &Path, progress: &dyn Fn(u64, u64)) -> Result<Moved, String> {
    let mut items = Vec::new();
    let mut moved = Moved::default();
    files_in(from, false, 0, &mut items, &mut moved.links);
    let total: u64 = items.iter().filter_map(|i| i.path.metadata().ok()).map(|m| m.len()).sum();
    let mut done = 0u64;
    for item in &items {
        if let Err(e) = move_one(item, from, to, &mut moved, &mut done, total, progress) {
            moved.undo();
            return Err(e);
        }
    }
    Ok(moved)
}

fn move_one(item: &Item, from: &Path, to: &Path, moved: &mut Moved, done: &mut u64, total: u64, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    let f = &item.path;
    let rel = f.strip_prefix(from).map_err(|e| e.to_string())?;
    let dest = to.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't create {}: {e}", parent.display()))?;
    }
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    // Already there (an earlier, stopped move, or the same model downloaded twice).
    if dest.metadata().is_ok_and(|m| m.is_file() && m.len() == len) {
        if !item.linked {
            moved.reused.push(f.clone());
        }
        *done += len;
        progress(*done, total);
        return Ok(());
    }
    // Same drive: a rename is instant. Never for a linked file, whose target isn't the app's.
    if !item.linked && std::fs::rename(f, &dest).is_ok() {
        moved.renamed.push((f.clone(), dest));
        *done += len;
        progress(*done, total);
        return Ok(());
    }
    let mut src = std::fs::File::open(f).map_err(|e| format!("Couldn't read {}: {e}", f.display()))?;
    let mut out = std::fs::File::create(&dest).map_err(|e| format!("Couldn't write {}: {e}", dest.display()))?;
    // Recorded now, so a failure below removes the partial copy.
    if item.linked {
        moved.copied.push((PathBuf::new(), dest.clone()));
    } else {
        moved.copied.push((f.clone(), dest.clone()));
    }
    let mut buf = vec![0u8; 8 << 20];
    loop {
        let n = std::io::Read::read(&mut src, &mut buf).map_err(|e| format!("Couldn't read {}: {e}", f.display()))?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut out, &buf[..n]).map_err(|e| format!("Couldn't write {}: {e}", dest.display()))?;
        *done += n as u64;
        progress(*done, total);
    }
    out.sync_all().map_err(|e| e.to_string())?;
    if dest.metadata().map(|m| m.len()).unwrap_or(0) != len {
        return Err(format!("{} didn't copy completely.", f.display()));
    }
    Ok(())
}

/// Moves the models folder to `dest` (a folder; "SulcusAI models" is made
/// inside it unless it's empty or already the app's), then restarts.
#[tauri::command]
pub async fn move_models(app: AppHandle, state: AppStateRef<'_>, dest: String) -> Result<(), String> {
    let picked = PathBuf::from(dest.trim());
    if !picked.is_absolute() {
        return Err("Pick a folder.".into());
    }
    let default = state.paths.data.join("models");
    let target = if picked == default || std::fs::read_dir(&picked).map(|mut d| d.next().is_none()).unwrap_or(true) {
        picked
    } else {
        picked.join("SulcusAI models")
    };
    let current = state.paths.models.clone();
    if target == current {
        return Err("The models are already kept there.".into());
    }
    if target.starts_with(&current) || current.starts_with(&target) && target != default {
        return Err("Pick a folder outside the current models folder.".into());
    }
    if !state.installs.lock().unwrap().is_empty() || crate::finetune::training() || crate::uncensor::running() || !crate::media::list_jobs().is_empty() {
        return Err("Wait for installs, media jobs and teaching to finish first.".into());
    }
    std::fs::create_dir_all(&target).map_err(|e| format!("Couldn't use that folder: {e}"))?;
    let used = size_of(&current);
    if let Some(free) = free_on(&target) {
        if free < used + 512 * 1024 * 1024 {
            return Err(format!("That drive has {} free; the models need {}.", crate::size_label(free), crate::size_label(used)));
        }
    }
    if MOVING.swap(true, Ordering::SeqCst) {
        return Err("Already moving the models.".into());
    }
    *LAST_ERROR.lock().unwrap() = None;
    // Nothing may hold a model file open while it moves.
    state.engine.lock().await.stop().await;
    crate::speech::stop(&state).await;
    state.log("settings", &format!("Moving models from {} to {}", current.display(), target.display()));
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let progress = |done: u64, total: u64| {
            let _ = app.emit("models:moving", json!({ "done": done, "total": total }));
        };
        let result = copy_all(&current, &target, &progress).and_then(|moved| {
            let location = st.paths.data.join(LOCATION_FILE);
            let switched = if target == st.paths.data.join("models") {
                std::fs::remove_file(&location).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
            } else {
                write_safely(&location, target.display().to_string())
            };
            if let Err(e) = switched {
                moved.undo();
                return Err(format!("Couldn't switch to the new folder: {e}"));
            }
            moved.finish();
            // Empty folders left behind (the default one stays).
            remove_empty(&current, current == st.paths.data.join("models"));
            st.log("settings", &format!("Moved {} model files", moved.count()));
            Ok(())
        });
        match result {
            Ok(()) => {
                st.log("settings", "Moved the models; restarting");
                let _ = app.emit("models:moving", json!({ "finished": true }));
                std::thread::sleep(std::time::Duration::from_millis(600));
                crate::crash::closing();
                app.restart();
            }
            Err(e) => {
                st.log("settings", &format!("Moving the models failed: {e}"));
                *LAST_ERROR.lock().unwrap() = Some(e.clone());
                MOVING.store(false, Ordering::SeqCst);
                let _ = app.emit("models:moving", json!({ "error": e }));
            }
        }
    });
    Ok(())
}

fn remove_empty(dir: &Path, keep_root: bool) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                remove_empty(&e.path(), false);
            }
        }
    }
    if !keep_root {
        std::fs::remove_dir(dir).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_file_picks_the_folder() {
        let d = std::env::temp_dir().join(format!("sulcus-storage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(models_dir(&d), d.join("models"));
        let other = d.join("elsewhere");
        std::fs::write(d.join(LOCATION_FILE), other.display().to_string()).unwrap();
        assert_eq!(models_dir(&d), other);
        assert!(other.exists());
        std::fs::write(d.join(LOCATION_FILE), "relative/path").unwrap();
        assert_eq!(models_dir(&d), d.join("models"), "a relative path is ignored");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn copy_all_moves_every_file() {
        let d = std::env::temp_dir().join(format!("sulcus-storage-{}", uuid::Uuid::new_v4()));
        let (a, b) = (d.join("a"), d.join("b"));
        std::fs::create_dir_all(a.join("qwen")).unwrap();
        std::fs::write(a.join("qwen/model.gguf"), vec![7u8; 3 << 20]).unwrap();
        std::fs::write(a.join("top.bin"), b"x").unwrap();
        let seen = std::sync::Mutex::new(0u64);
        let moved = copy_all(&a, &b, &|done, _| *seen.lock().unwrap() = done).unwrap();
        moved.finish();
        assert_eq!(std::fs::read(b.join("qwen/model.gguf")).unwrap().len(), 3 << 20);
        assert_eq!(std::fs::read(b.join("top.bin")).unwrap(), b"x");
        assert_eq!(*seen.lock().unwrap(), (3 << 20) + 1);
        assert!(!a.join("top.bin").exists(), "nothing is left behind");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_failed_move_puts_everything_back() {
        let d = std::env::temp_dir().join(format!("sulcus-storage-{}", uuid::Uuid::new_v4()));
        let (a, b) = (d.join("a"), d.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join("1.gguf"), b"first").unwrap();
        std::fs::write(a.join("2.gguf"), b"second").unwrap();
        // Something in the way of the second file: it can't be renamed or written there.
        std::fs::create_dir_all(b.join("2.gguf")).unwrap();
        assert!(copy_all(&a, &b, &|_, _| {}).is_err());
        assert_eq!(std::fs::read(a.join("1.gguf")).unwrap(), b"first", "the file moved first came back");
        assert!(!b.join("1.gguf").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn files_already_there_are_kept_not_copied_again() {
        let d = std::env::temp_dir().join(format!("sulcus-storage-{}", uuid::Uuid::new_v4()));
        let (a, b) = (d.join("a"), d.join("b"));
        std::fs::create_dir_all(a.join("media")).unwrap();
        std::fs::create_dir_all(b.join("media")).unwrap();
        std::fs::write(a.join("media/flux.gguf"), b"weights").unwrap();
        std::fs::write(b.join("media/flux.gguf"), b"weights").unwrap();
        let moved = copy_all(&a, &b, &|_, _| {}).unwrap();
        assert_eq!((moved.reused.len(), moved.renamed.len(), moved.copied.len()), (1, 0, 0));
        moved.finish();
        assert!(!a.join("media/flux.gguf").exists() && b.join("media/flux.gguf").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    /// A folder linked in from elsewhere: its files are copied, the link
    /// removed, and the files it points at stay where they are.
    #[cfg(windows)]
    #[test]
    fn linked_folders_are_copied_not_emptied() {
        let d = std::env::temp_dir().join(format!("sulcus-storage-{}", uuid::Uuid::new_v4()));
        let (a, b, elsewhere) = (d.join("a"), d.join("b"), d.join("ollama"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("blob.gguf"), b"theirs").unwrap();
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(a.join("linked")).arg(&elsewhere).output().unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stdout));
        let moved = copy_all(&a, &b, &|_, _| {}).unwrap();
        moved.finish();
        assert_eq!(std::fs::read(b.join("linked/blob.gguf")).unwrap(), b"theirs");
        assert_eq!(std::fs::read(elsewhere.join("blob.gguf")).unwrap(), b"theirs", "the other app's file stays");
        assert!(!a.join("linked").exists(), "the link itself is gone");
        std::fs::remove_dir_all(&d).ok();
    }
}

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

fn files_in(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        match e.file_type() {
            Ok(t) if t.is_dir() => files_in(&e.path(), out),
            Ok(t) if t.is_file() => out.push(e.path()),
            _ => {}
        }
    }
}

fn free_on(dir: &Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks.list().iter().filter(|d| dir.starts_with(d.mount_point())).max_by_key(|d| d.mount_point().as_os_str().len()).map(|d| d.available_space())
}

#[tauri::command]
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

/// Copies (or renames) everything from `from` into `to`, reporting bytes.
fn copy_all(from: &Path, to: &Path, progress: &dyn Fn(u64, u64)) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    files_in(from, &mut files);
    let total: u64 = files.iter().filter_map(|f| f.metadata().ok()).map(|m| m.len()).sum();
    let mut done = 0u64;
    let mut copied = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(from).map_err(|e| e.to_string())?;
        let dest = to.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't create {}: {e}", parent.display()))?;
        }
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        // Same drive: a rename is instant and leaves nothing to clean up.
        if std::fs::rename(f, &dest).is_ok() {
            done += len;
            progress(done, total);
            continue;
        }
        let mut src = std::fs::File::open(f).map_err(|e| format!("Couldn't read {}: {e}", f.display()))?;
        let mut out = std::fs::File::create(&dest).map_err(|e| format!("Couldn't write {}: {e}", dest.display()))?;
        let mut buf = vec![0u8; 8 << 20];
        loop {
            let n = std::io::Read::read(&mut src, &mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut out, &buf[..n]).map_err(|e| format!("Couldn't write {}: {e}", dest.display()))?;
            done += n as u64;
            progress(done, total);
        }
        out.sync_all().map_err(|e| e.to_string())?;
        if dest.metadata().map(|m| m.len()).unwrap_or(0) != len {
            return Err(format!("{} didn't copy completely.", f.display()));
        }
        copied.push(f.clone());
    }
    Ok(copied)
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
    if !state.installs.lock().unwrap().is_empty() || crate::finetune::training() || !crate::media::list_jobs().is_empty() {
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
        let result = copy_all(&current, &target, &progress).and_then(|copied| {
            let location = st.paths.data.join(LOCATION_FILE);
            if target == st.paths.data.join("models") {
                std::fs::remove_file(&location).ok();
            } else {
                std::fs::write(&location, target.display().to_string()).map_err(|e| e.to_string())?;
            }
            for f in copied {
                std::fs::remove_file(f).ok();
            }
            // Empty folders left behind (the default one stays).
            remove_empty(&current, current == st.paths.data.join("models"));
            Ok(())
        });
        match result {
            Ok(()) => {
                st.log("settings", "Moved the models; restarting");
                let _ = app.emit("models:moving", json!({ "finished": true }));
                std::thread::sleep(std::time::Duration::from_millis(600));
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
        copy_all(&a, &b, &|done, _| *seen.lock().unwrap() = done).unwrap();
        assert_eq!(std::fs::read(b.join("qwen/model.gguf")).unwrap().len(), 3 << 20);
        assert_eq!(std::fs::read(b.join("top.bin")).unwrap(), b"x");
        assert_eq!(*seen.lock().unwrap(), (3 << 20) + 1);
        std::fs::remove_dir_all(&d).ok();
    }
}

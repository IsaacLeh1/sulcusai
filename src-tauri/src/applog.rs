// SPDX-License-Identifier: AGPL-3.0-only
//! SulcusAI's own logs on this PC, for finding out what went wrong:
//! `logs\app.log` (what it did: startup, models loaded, pictures made and on
//! which card, downloads, updates, errors) and `logs\crashes.log` (every
//! crash, whether or not crash reports are sent). They never hold what's
//! said in chats, prompts, notes, mail or file contents. The engines keep
//! their own output beside them (engine.log, media.log, speech-*.log).
//! Settings → Logs shows them; nothing here leaves the PC.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

use crate::AppStateRef;

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// One writer at a time, so lines don't interleave.
static WRITING: Mutex<()> = Mutex::new(());
/// A log is started over (the old one kept as `.old.log`) past this size.
const MAX_BYTES: u64 = 2 * 1024 * 1024;

pub const APP_LOG: &str = "app.log";
pub const CRASH_LOG: &str = "crashes.log";

pub fn init(logs: &Path) {
    let _ = DIR.set(logs.to_path_buf());
}

fn append(name: &str, text: &str) {
    let Some(dir) = DIR.get() else { return };
    let _guard = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = dir.join(name);
    if path.metadata().is_ok_and(|m| m.len() > MAX_BYTES) {
        let old = dir.join(name.replace(".log", ".old.log"));
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&path, &old);
    }
    let _ = std::fs::create_dir_all(dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// One line in app.log: "2026-10-17 10:22:05 [media] Made a picture…".
pub fn line(category: &str, message: &str) {
    let when = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let message = message.replace(['\r', '\n'], " ⏎ ");
    append(APP_LOG, &format!("{when} [{category}] {message}\n"));
}

/// A crash or failure, in crashes.log (and a line in app.log).
pub fn crash(what: &str, details: &str) {
    let when = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    append(CRASH_LOG, &format!("===== {when} · SulcusAI {} =====\n{what}\n{}\n\n", env!("CARGO_PKG_VERSION"), details.trim_end()));
    line("crash", what);
}

// ---------- Settings → Logs ----------

#[derive(Serialize)]
pub struct LogFile {
    name: String,
    size: u64,
    modified: i64,
}

#[derive(Serialize)]
pub struct LogsView {
    /// The logs folder and the app's data folder.
    folder: String,
    data_folder: String,
    files: Vec<LogFile>,
}

fn files(dir: &Path) -> Vec<LogFile> {
    let mut out: Vec<LogFile> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let modified = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
            Some(LogFile { name: e.file_name().to_string_lossy().into_owned(), size: m.len(), modified })
        })
        .collect();
    // The app's own logs first, then the newest.
    out.sort_by_key(|f| (f.name != APP_LOG, f.name != CRASH_LOG, -f.modified));
    out
}

#[tauri::command(async)]
pub fn logs_view(state: AppStateRef) -> LogsView {
    LogsView { folder: state.paths.logs.display().to_string(), data_folder: state.paths.data.display().to_string(), files: files(&state.paths.logs) }
}

/// The last `lines` lines of a log in the logs folder (nothing outside it).
#[tauri::command(async)]
pub fn read_log(state: AppStateRef, name: String, lines: Option<usize>) -> Result<String, String> {
    let file = Path::new(&name).file_name().filter(|f| f.to_string_lossy() == name && name.ends_with(".log")).ok_or("That isn't one of SulcusAI's logs.")?;
    let path = state.paths.logs.join(file);
    let bytes = std::fs::read(&path).map_err(|_| "That log is empty or gone.".to_string())?;
    // The end of a big log is what matters: at most the last 400 KB.
    let tail = &bytes[bytes.len().saturating_sub(400 * 1024)..];
    let text = String::from_utf8_lossy(tail);
    let all: Vec<&str> = text.lines().collect();
    let keep = lines.unwrap_or(400).clamp(10, 5000);
    Ok(all[all.len().saturating_sub(keep)..].join("\n"))
}

/// Opens the logs folder (or, with `data`, the app's data folder) in File Explorer.
#[tauri::command(async)]
pub fn open_app_folder(state: AppStateRef, data: Option<bool>) -> Result<(), String> {
    let dir = if data == Some(true) { state.paths.data.clone() } else { state.paths.logs.clone() };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    crate::open_with_system(dir.as_os_str())
}

/// Removes the logs (one an engine still has open is emptied when it's done).
#[tauri::command(async)]
pub fn clear_logs(state: AppStateRef) -> LogsView {
    for f in files(&state.paths.logs) {
        let path = state.paths.logs.join(&f.name);
        if std::fs::remove_file(&path).is_err() {
            let _ = std::fs::OpenOptions::new().write(true).truncate(true).open(&path);
        }
    }
    line("app", "Cleared the logs");
    logs_view(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_go_to_app_log_and_crashes_to_their_own() {
        let dir = std::env::temp_dir().join(format!("sulcus-logs-{}", uuid::Uuid::new_v4().simple()));
        init(&dir);
        if DIR.get() != Some(&dir) {
            return; // another test set the folder first
        }
        line("media", "Made a picture\nin 28 s");
        crash("Part of SulcusAI failed (a panic).", "index out of bounds");
        let app = std::fs::read_to_string(dir.join(APP_LOG)).unwrap();
        assert!(app.contains("[media] Made a picture ⏎ in 28 s"), "one line per entry: {app}");
        assert!(app.contains("[crash] Part of SulcusAI failed"));
        assert!(std::fs::read_to_string(dir.join(CRASH_LOG)).unwrap().contains("index out of bounds"));
        let listed: Vec<String> = files(&dir).into_iter().map(|f| f.name).collect();
        assert_eq!(listed, vec![APP_LOG.to_string(), CRASH_LOG.to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }
}

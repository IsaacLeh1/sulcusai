// SPDX-License-Identifier: AGPL-3.0-only
//! Backups on their own: a locked backup file into a folder the person
//! picks (another drive, a synced folder), every day or week, keeping the
//! last few. Off until turned on. The password is kept encrypted like the
//! app's other secrets, since a backup made unattended needs it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{db, AppState, AppStateRef};

const KEY: &str = "auto_backup";
const DAY_MS: i64 = 86_400_000;
const PREFIX: &str = "SulcusAI auto backup ";

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
struct Stored {
    on: bool,
    folder: String,
    days: u32,
    keep: usize,
    /// Encrypted with the app's key.
    password: String,
    last: Option<i64>,
    last_error: Option<String>,
}

#[derive(Serialize)]
pub struct View {
    on: bool,
    folder: String,
    days: u32,
    keep: usize,
    has_password: bool,
    last: Option<i64>,
    last_error: Option<String>,
}

fn load(state: &AppState) -> Stored {
    db::get(&state.db.lock().unwrap(), KEY).unwrap_or_default()
}

fn save(state: &AppState, s: &Stored) -> Result<(), String> {
    db::set(&state.db.lock().unwrap(), KEY, s)
}

fn view(s: &Stored) -> View {
    View { on: s.on, folder: s.folder.clone(), days: s.days.max(1), keep: s.keep.max(1), has_password: !s.password.is_empty(), last: s.last, last_error: s.last_error.clone() }
}

/// Whether a backup is due now.
pub fn due(last: Option<i64>, days: u32, now: i64) -> bool {
    last.is_none_or(|l| now - l >= i64::from(days.max(1)) * DAY_MS)
}

/// The oldest auto backups past `keep`, to remove (never other files).
pub fn to_remove(mut names: Vec<String>, keep: usize) -> Vec<String> {
    names.retain(|n| n.starts_with(PREFIX) && n.ends_with(".sulcusbackup"));
    names.sort();
    let extra = names.len().saturating_sub(keep.max(1));
    names.into_iter().take(extra).collect()
}

fn run_once(state: &AppState) -> Result<(), String> {
    let s = load(state);
    let c = state.cipher()?;
    let password = c.decrypt(&s.password).map_err(|_| "The backup password couldn't be read. Set it again.".to_string())?;
    let folder = PathBuf::from(&s.folder);
    if !folder.is_dir() {
        return Err(format!("{} isn't there (an unplugged drive?).", s.folder));
    }
    let name = format!("{PREFIX}{}.sulcusbackup", chrono::Local::now().format("%Y-%m-%d %H%M"));
    crate::backup::make(state, &folder.join(name), &password)?;
    let names: Vec<String> = std::fs::read_dir(&folder).map_err(|e| e.to_string())?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    for old in to_remove(names, s.keep) {
        let _ = std::fs::remove_file(folder.join(old));
    }
    Ok(())
}

/// Checks once now and then about every hour (the app's usual backstop
/// wake), making a backup when one is due.
pub fn start(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut change = crate::quiet::change_watcher();
        loop {
            crate::quiet::awake().await;
            let s = load(&state);
            if s.on && state.cipher().is_ok() && due(s.last, s.days, db::now_ms()) {
                let st = state.clone();
                let result = tokio::task::spawn_blocking(move || run_once(&st)).await.map_err(|e| e.to_string()).and_then(|r| r);
                let mut s = load(&state);
                s.last_error = result.as_ref().err().cloned();
                if result.is_ok() {
                    s.last = Some(db::now_ms());
                    state.log("backup", "Made an automatic backup");
                }
                let _ = save(&state, &s);
            }
            crate::quiet::until_changed(&mut change, crate::quiet::BACKSTOP).await;
        }
    });
}

#[tauri::command(async)]
pub fn auto_backup_view(state: AppStateRef) -> View {
    view(&load(&state))
}

/// Turns it on or off and sets where and how often; a new password only
/// when given (empty keeps the saved one).
#[tauri::command(async)]
pub fn set_auto_backup(state: AppStateRef, on: bool, folder: String, days: u32, keep: usize, password: Option<String>) -> Result<View, String> {
    let mut s = load(&state);
    if let Some(p) = password.filter(|p| !p.is_empty()) {
        if p.chars().count() < 8 {
            return Err("Use a password of at least 8 characters. You'll need it to restore.".into());
        }
        s.password = state.cipher()?.encrypt(&p);
    }
    if on {
        if !Path::new(folder.trim()).is_dir() {
            return Err("Pick a folder for the backups.".into());
        }
        if s.password.is_empty() {
            return Err("Set a password for the backups.".into());
        }
    }
    s.on = on;
    s.folder = folder.trim().to_string();
    s.days = days.clamp(1, 30);
    s.keep = keep.clamp(1, 50);
    save(&state, &s)?;
    crate::quiet::changed();
    Ok(view(&s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_by_days_and_only_own_old_backups_go() {
        assert!(due(None, 7, 0));
        assert!(!due(Some(0), 7, 6 * DAY_MS));
        assert!(due(Some(0), 7, 7 * DAY_MS));
        let names = vec![
            format!("{PREFIX}2026-11-01 0900.sulcusbackup"),
            format!("{PREFIX}2026-11-03 0900.sulcusbackup"),
            format!("{PREFIX}2026-11-02 0900.sulcusbackup"),
            "My own backup.sulcusbackup".to_string(),
            "notes.txt".to_string(),
        ];
        assert_eq!(to_remove(names, 2), vec![format!("{PREFIX}2026-11-01 0900.sulcusbackup")]);
    }
}

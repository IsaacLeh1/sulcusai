// SPDX-License-Identifier: AGPL-3.0-only
//! Signed updates from the project's GitHub Releases (the Tauri updater; an
//! update only installs if its signature matches the key built into the
//! app). Automatic checks run only when the app may go online; "Check for
//! updates" in Settings always works, because the user asked. Installing is
//! always the user's click.

use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::net::Connectivity;
use crate::{db, AppState, AppStateRef};

const KEY: &str = "updates";
/// How often an automatic check may happen.
const EVERY: i64 = 24 * 3600 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Look for updates on its own (only when the app may go online).
    pub auto: bool,
    pub last_check: i64,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings { auto: true, last_check: 0 }
    }
}

fn settings(state: &AppState) -> UpdateSettings {
    db::get(&state.db.lock().unwrap(), KEY).unwrap_or_default()
}

/// The update found by the last check, ready to install.
static PENDING: Mutex<Option<Update>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize)]
pub struct Available {
    version: String,
    notes: String,
    date: Option<String>,
}

async fn check(app: &AppHandle, state: &AppState) -> Result<Option<Available>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    state.log("network", "Checked GitHub for a newer version of SulcusAI");
    let found = updater.check().await.map_err(|e| format!("Couldn't check for updates: {e}"))?;
    {
        let conn = state.db.lock().unwrap();
        let mut s: UpdateSettings = db::get(&conn, KEY).unwrap_or_default();
        s.last_check = db::now_ms();
        let _ = db::set(&conn, KEY, &s);
    }
    Ok(found.map(|u| {
        let a = Available { version: u.version.clone(), notes: u.body.clone().unwrap_or_default(), date: u.date.map(|d| d.date().to_string()) };
        *PENDING.lock().unwrap() = Some(u);
        a
    }))
}

/// Checks once a day in the background, when allowed.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Let the app settle first.
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            let state = app.state::<std::sync::Arc<AppState>>().inner().clone();
            let s = settings(&state);
            let online = state.settings().connectivity != Connectivity::Offline;
            if s.auto && online && db::now_ms() - s.last_check >= EVERY {
                if let Ok(Some(a)) = check(&app, &state).await {
                    app.emit("update:available", &a).ok();
                }
            }
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });
}

#[derive(Serialize)]
pub struct UpdateView {
    current: String,
    auto: bool,
    last_check: i64,
    available: Option<Available>,
}

#[tauri::command]
pub fn update_view(state: AppStateRef) -> UpdateView {
    let s = settings(&state);
    let available = PENDING.lock().unwrap().as_ref().map(|u| Available { version: u.version.clone(), notes: u.body.clone().unwrap_or_default(), date: u.date.map(|d| d.date().to_string()) });
    UpdateView { current: env!("CARGO_PKG_VERSION").into(), auto: s.auto, last_check: s.last_check, available }
}

/// Checks now (the user asked, so it works at every level).
#[tauri::command]
pub async fn check_for_update(app: AppHandle, state: AppStateRef<'_>) -> Result<Option<Available>, String> {
    check(&app, &state).await
}

#[tauri::command]
pub fn set_update_auto(state: AppStateRef, auto: bool) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    let mut s: UpdateSettings = db::get(&conn, KEY).unwrap_or_default();
    s.auto = auto;
    db::set(&conn, KEY, &s)
}

/// Downloads the update, checks its signature, installs it and restarts.
#[tauri::command]
pub async fn install_update(app: AppHandle, state: AppStateRef<'_>) -> Result<(), String> {
    let update = match PENDING.lock().unwrap().take() {
        Some(u) => u,
        None => return Err("There's no update waiting. Check for updates first.".into()),
    };
    state.log("network", &format!("Downloading SulcusAI {} from GitHub", update.version));
    let mut received = 0u64;
    let progress = app.clone();
    update
        .download_and_install(
            |chunk, total| {
                received += chunk as u64;
                progress.emit("update:progress", json!({ "received": received, "total": total })).ok();
            },
            || {},
        )
        .await
        .map_err(|e| format!("The update didn't install: {e}"))?;
    state.log("settings", &format!("Installed SulcusAI {}", update.version));
    app.restart();
}

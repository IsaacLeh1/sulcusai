// SPDX-License-Identifier: AGPL-3.0-only
//! Signed updates from the project's GitHub Releases (the Tauri updater; an
//! update only installs if its signature matches the key built into the
//! app). The automatic check runs at every level, Offline included: it only
//! asks GitHub for the latest version number, and it's a switch in Settings.
//! (It used to skip Offline, the default, so most copies never heard of an
//! update.) Updates install on their own when nothing is running, and the
//! app restarts (fixes reach everyone); the user can turn that off, and then
//! installing is their click.

use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::{db, AppState, AppStateRef};

const KEY: &str = "updates";
/// How often an automatic check may happen.
const EVERY: i64 = 6 * 3600 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Look for updates on its own.
    pub auto: bool,
    /// Install what it finds without asking (when nothing is running), once
    /// the user chose; until then it does.
    install: bool,
    /// Whether the user chose `install` (before 0.9.2 it was off unless
    /// they turned it on, so nobody's "off" was a choice).
    chosen: bool,
    pub last_check: i64,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings { auto: true, install: true, chosen: false, last_check: 0 }
    }
}

impl UpdateSettings {
    pub fn installs(&self) -> bool {
        !self.chosen || self.install
    }
}

/// Nothing a restart would cut off: a reply, a meeting, a download, a
/// picture or video being made, a model being taught, models being moved.
fn idle(state: &AppState) -> bool {
    state.generations.lock().unwrap().is_empty()
        && state.installs.lock().unwrap().is_empty()
        && crate::meeting::live_meeting().is_none()
        && crate::media::list_jobs().is_empty()
        && !crate::finetune::training()
        && !crate::tools::commands_running()
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

/// The updater, through the private connection when that's on for updates.
fn updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    crate::offline::check(crate::offline::Keep::Updates)?;
    let mut b = app.updater_builder();
    crate::privacy::check(crate::privacy::Route::Updates)?;
    if let Some(p) = crate::privacy::proxy_for(crate::privacy::Route::Updates) {
        b = b.proxy(tauri::Url::parse(&p).map_err(|e| e.to_string())?).timeout(Duration::from_secs(120));
    }
    b.build().map_err(|e| e.to_string())
}

async fn check(app: &AppHandle, state: &AppState) -> Result<Option<Available>, String> {
    let updater = updater(app)?;
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

/// Checks soon after the app opens and every few hours after, unless the
/// user turned automatic checks off.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Let the app settle first.
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            let state = app.state::<std::sync::Arc<AppState>>().inner().clone();
            let s = settings(&state);
            if s.auto && db::now_ms() - s.last_check >= EVERY {
                if let Ok(Some(a)) = check(&app, &state).await {
                    app.emit("update:available", &a).ok();
                }
            }
            // Installing on its own: once nothing is running (checked again
            // every few minutes until then).
            if s.installs() && PENDING.lock().unwrap().is_some() {
                for _ in 0..12 {
                    if idle(&state) {
                        state.log("settings", "Installing an update automatically");
                        app.emit("update:installing", json!({})).ok();
                        if let Err(e) = install(&app, &state, true).await {
                            state.log("settings", &format!("The automatic update didn't install: {e}"));
                        }
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(300)).await;
                }
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

#[derive(Serialize)]
pub struct UpdateView {
    current: String,
    auto: bool,
    install: bool,
    last_check: i64,
    available: Option<Available>,
}

#[tauri::command(async)]
pub fn update_view(state: AppStateRef) -> UpdateView {
    let s = settings(&state);
    let available = PENDING.lock().unwrap().as_ref().map(|u| Available { version: u.version.clone(), notes: u.body.clone().unwrap_or_default(), date: u.date.map(|d| d.date().to_string()) });
    UpdateView { current: env!("CARGO_PKG_VERSION").into(), auto: s.auto, install: s.installs(), last_check: s.last_check, available }
}

/// Checks now (the user asked, so it works at every level).
#[tauri::command]
pub async fn check_for_update(app: AppHandle, state: AppStateRef<'_>) -> Result<Option<Available>, String> {
    check(&app, &state).await
}

#[tauri::command(async)]
pub fn set_update_auto(state: AppStateRef, auto: bool) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    let mut s: UpdateSettings = db::get(&conn, KEY).unwrap_or_default();
    s.auto = auto;
    db::set(&conn, KEY, &s)
}

/// Install updates without asking (checking turns on with it).
#[tauri::command(async)]
pub fn set_update_install(state: AppStateRef, install: bool) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    let mut s: UpdateSettings = db::get(&conn, KEY).unwrap_or_default();
    s.install = install;
    s.chosen = true;
    if install {
        s.auto = true;
    }
    db::log_action(&conn, "settings", if install { "Turned on installing updates automatically" } else { "Turned off installing updates automatically" });
    db::set(&conn, KEY, &s)
}

/// The oldest version SulcusAI goes back to. Earlier ones have security
/// problems fixed since: 0.9.23 updated how every connection checks
/// certificates (RUSTSEC-2026-0381/0382), after the outside review's fixes
/// (0.9.3 to 0.9.16). Going back further would leave this PC open to them.
const OLDEST_SAFE: (u32, u32, u32) = (0, 9, 23);

fn parse(v: &str) -> Option<(u32, u32, u32)> {
    let mut p = v.trim().trim_start_matches('v').split('.').map(|x| x.parse::<u32>().ok());
    Some((p.next()??, p.next()??, p.next().flatten().unwrap_or(0)))
}

/// Whether going back to `version` is offered, and why not when it isn't.
pub fn can_go_back_to(version: &str) -> Result<(), String> {
    let v = parse(version).ok_or("That isn't a version.")?;
    let now = parse(env!("CARGO_PKG_VERSION")).unwrap_or_default();
    if v >= now {
        return Err(format!("{version} isn't earlier than this version."));
    }
    if v < OLDEST_SAFE {
        return Err(format!("{version} has security problems that later versions fixed, so SulcusAI doesn't go back that far (0.9.23 is the oldest)."));
    }
    Ok(())
}

/// Goes back to an earlier version: a copy of the chats and settings is
/// saved first, automatic installs turn off (so it stays there), and that
/// version's installer is downloaded from its GitHub release with its
/// signature checked like any update's, installed, and started.
#[tauri::command]
pub async fn go_back_to(app: AppHandle, state: AppStateRef<'_>, version: String) -> Result<(), String> {
    can_go_back_to(&version)?;
    if !idle(&state) {
        return Err("Wait for what's running to finish (a reply, a download or something being made), then go back.".into());
    }
    let v = version.trim().trim_start_matches('v').to_string();
    // A copy of the database first (a whole, consistent one), in case the
    // older version can't read something newer.
    let dir = state.paths.data.join("backups");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let copy = dir.join(format!("before-{v}-{}.db", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    state.db.lock().unwrap().execute("VACUUM INTO ?1", [copy.to_string_lossy().to_string()]).map_err(|e| format!("Couldn't save a copy of your data first: {e}"))?;
    {
        let conn = state.db.lock().unwrap();
        let mut s: UpdateSettings = db::get(&conn, KEY).unwrap_or_default();
        s.install = false;
        s.chosen = true;
        db::set(&conn, KEY, &s)?;
        db::log_action(&conn, "settings", &format!("Going back to SulcusAI {v} (a copy of the data is in {})", copy.display()));
    }
    let url = format!("https://github.com/IsaacLeh1/sulcusai/releases/download/v{v}/latest.json");
    crate::offline::check(crate::offline::Keep::Updates)?;
    crate::privacy::check(crate::privacy::Route::Updates)?;
    let mut b = app
        .updater_builder()
        .endpoints(vec![tauri::Url::parse(&url).map_err(|e| e.to_string())?])
        .map_err(|e| e.to_string())?
        // An earlier version is what's wanted here.
        .version_comparator(|_, _| true);
    if let Some(p) = crate::privacy::proxy_for(crate::privacy::Route::Updates) {
        b = b.proxy(tauri::Url::parse(&p).map_err(|e| e.to_string())?).timeout(Duration::from_secs(120));
    }
    let updater = b.build().map_err(|e| e.to_string())?;
    state.log("network", &format!("Checked GitHub for SulcusAI {v} (going back to it)"));
    let found = updater.check().await.map_err(|e| format!("Couldn't find SulcusAI {v} on GitHub: {e}"))?.ok_or(format!("GitHub has no installer for SulcusAI {v}."))?;
    *PENDING.lock().unwrap() = Some(found);
    install(&app, &state, false).await
}

/// Downloads the update, checks its signature, installs it and restarts.
#[tauri::command]
pub async fn install_update(app: AppHandle, state: AppStateRef<'_>) -> Result<(), String> {
    install(&app, &state, false).await
}

/// Downloads and installs the waiting update, then restarts. `auto`: it
/// began on its own, so it waits after the download until nothing is running
/// (a reply may have started meanwhile).
async fn install(app: &AppHandle, state: &AppState, auto: bool) -> Result<(), String> {
    let update = match PENDING.lock().unwrap().take() {
        Some(u) => u,
        None => return Err("There's no update waiting. Check for updates first.".into()),
    };
    state.log("network", &format!("Downloading SulcusAI {} from GitHub", update.version));
    let mut received = 0u64;
    let progress = app.clone();
    let bytes = update
        .download(
            |chunk, total| {
                received += chunk as u64;
                progress.emit("update:progress", json!({ "received": received, "total": total })).ok();
            },
            || {},
        )
        .await
        .map_err(|e| format!("The update didn't download: {e}"))?;
    if auto {
        let mut waited = 0;
        while !idle(state) {
            if waited >= 3600 {
                return Err("Something kept running for an hour after the download, so it wasn't installed.".into());
            }
            tokio::time::sleep(Duration::from_secs(20)).await;
            waited += 20;
        }
    }
    update.install(bytes).map_err(|e| format!("The update didn't install: {e}"))?;
    state.log("settings", &format!("Installed SulcusAI {}", update.version));
    crate::crash::closing();
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Earlier versions back to the oldest safe one are offered; older ones,
    /// this one and later ones aren't.
    #[test]
    fn going_back_stops_at_the_oldest_safe_version() {
        assert!(can_go_back_to("0.9.30").is_ok());
        assert!(can_go_back_to("v0.9.23").is_ok());
        assert!(can_go_back_to("0.9.22").unwrap_err().contains("security"));
        assert!(can_go_back_to("0.8.0").is_err());
        assert!(can_go_back_to(env!("CARGO_PKG_VERSION")).is_err());
        assert!(can_go_back_to("9.0.0").is_err());
        assert!(can_go_back_to("soon").is_err());
    }
}

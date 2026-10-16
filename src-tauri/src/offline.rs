// SPDX-License-Identifier: AGPL-3.0-only
//! Fully offline: Offline, and nothing at all goes out. Offline still lets a
//! few things reach the network (the update check, downloads the person
//! starts, web search when that feature is on, the local network for syncing,
//! People and the phone page, connectors' own programs). Fully offline stops
//! those too, except what the person chose to keep when turning it on (they
//! see which ones are in use then). Picking any level turns it off.
//!
//! Commands the assistant runs for the person are their own programs: they
//! can still use the network (the window says so).

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::features::{self, Feature};
use crate::{db, AppState, AppStateRef};

/// What may still use the network while fully offline, if kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Keep {
    /// Checking for (and installing) app updates.
    Updates,
    /// Downloading models, the AI engine and voices when asked.
    Downloads,
    /// Web search, reading pages, weather and location, the browser.
    Web,
    /// Syncing this user's PCs, People and the phone page.
    LocalNetwork,
    /// Connectors (their programs may go online).
    Connectors,
}

const ALL: [Keep; 5] = [Keep::Updates, Keep::Downloads, Keep::Web, Keep::LocalNetwork, Keep::Connectors];

const KEY: &str = "fully_offline";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    on: bool,
    keep: BTreeSet<Keep>,
}

/// What's kept, while fully offline is on (`None`: it's off).
static NOW: RwLock<Option<BTreeSet<Keep>>> = RwLock::new(None);

/// At startup, before anything goes online.
pub fn load(conn: &rusqlite::Connection) {
    let s: Settings = db::get(conn, KEY).unwrap_or_default();
    *NOW.write().unwrap() = s.on.then_some(s.keep);
}

pub fn on() -> bool {
    NOW.read().unwrap().is_some()
}

/// Whether fully offline stops this now.
pub fn blocks(k: Keep) -> bool {
    stops(NOW.read().unwrap().as_ref(), k)
}

/// With `kept` (`None`: fully offline is off), whether `k` is stopped.
fn stops(kept: Option<&BTreeSet<Keep>>, k: Keep) -> bool {
    kept.is_some_and(|keep| !keep.contains(&k))
}

fn what(k: Keep) -> &'static str {
    match k {
        Keep::Updates => "check for updates",
        Keep::Downloads => "download anything",
        Keep::Web => "use the web",
        Keep::LocalNetwork => "use the local network",
        Keep::Connectors => "run connectors",
    }
}

/// Err with a plain reason when fully offline stops this.
pub fn check(k: Keep) -> Result<(), String> {
    if blocks(k) { Err(reason(k)) } else { Ok(()) }
}

fn reason(k: Keep) -> String {
    format!(
        "SulcusAI is fully offline, so it doesn't {}. Turn that back on in the status bar (Fully offline → what to keep), or pick another level.",
        what(k)
    )
}

#[derive(Serialize)]
pub struct Item {
    id: Keep,
    name: &'static str,
    detail: &'static str,
    /// On now, and needs the network.
    in_use: bool,
    kept: bool,
}

#[derive(Serialize)]
pub struct OfflineView {
    on: bool,
    items: Vec<Item>,
}

fn in_use(state: &AppState, k: Keep) -> bool {
    let conn = state.db.lock().unwrap();
    match k {
        Keep::Updates => db::get::<serde_json::Value>(&conn, "updates").map_or(true, |v| v["auto"] != false),
        Keep::Downloads => !state.installs.lock().unwrap().is_empty() || !crate::downloads::list(&conn).is_empty(),
        Keep::Web => {
            features::is_on(&conn, Feature::WebSearch)
                || features::is_on(&conn, Feature::Browser)
                || conn.query_row("SELECT COUNT(*) FROM chats WHERE web = 1", [], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0
        }
        Keep::LocalNetwork => {
            crate::sync::sync_on(&conn) || features::is_on(&conn, Feature::People) || db::get::<serde_json::Value>(&conn, "phone").is_some_and(|v| v["enabled"] == true)
        }
        Keep::Connectors => features::is_on(&conn, Feature::Connectors) && conn.query_row("SELECT COUNT(*) FROM connectors", [], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0,
    }
}

fn view(state: &AppState) -> OfflineView {
    let s: Settings = db::get(&state.db.lock().unwrap(), KEY).unwrap_or_default();
    let items = ALL
        .iter()
        .map(|&k| {
            let (name, detail) = match k {
                Keep::Updates => ("Update checks", "Asks GitHub for new versions of SulcusAI, and installs them."),
                Keep::Downloads => ("Downloads", "Models, the AI engine, voices and other parts you add (only when you ask)."),
                Keep::Web => ("Web", "Web search and reading pages in chats, weather, finding your location, and the browser."),
                Keep::LocalNetwork => ("Local network", "Syncing your PCs, People, and the phone page, over your network or a VPN."),
                Keep::Connectors => ("Connectors", "Their programs run on this PC and may go online themselves."),
            };
            Item { id: k, name, detail, in_use: in_use(state, k), kept: s.keep.contains(&k) }
        })
        .collect();
    OfflineView { on: s.on, items }
}

#[tauri::command(async)]
pub fn offline_view(state: AppStateRef) -> OfflineView {
    view(&state)
}

/// Turns fully offline on (with what to keep) or off.
#[tauri::command]
pub async fn set_fully_offline(app: AppHandle, state: AppStateRef<'_>, on: bool, keep: Vec<Keep>) -> Result<OfflineView, String> {
    state.cipher()?;
    let keep: BTreeSet<Keep> = keep.into_iter().collect();
    {
        let conn = state.db.lock().unwrap();
        if on {
            db::update_settings(&conn, |s| s.connectivity = crate::net::Connectivity::Offline)?;
        }
        db::set(&conn, KEY, &Settings { on, keep: keep.clone() })?;
        db::log_action(
            &conn,
            "privacy",
            &if on {
                let kept: Vec<&str> = keep.iter().map(|k| what(*k)).collect();
                if kept.is_empty() { "Turned on Fully offline (nothing goes out)".to_string() } else { format!("Turned on Fully offline (still allowed to {})", kept.join(", ")) }
            } else {
                "Turned off Fully offline".to_string()
            },
        );
    }
    *NOW.write().unwrap() = on.then_some(keep);
    apply(&app, state.inner()).await;
    Ok(view(&state))
}

/// Picking a level ends fully offline.
pub fn turn_off(state: &AppState) {
    if !on() {
        return;
    }
    let conn = state.db.lock().unwrap();
    let mut s: Settings = db::get(&conn, KEY).unwrap_or_default();
    s.on = false;
    let _ = db::set(&conn, KEY, &s);
    db::log_action(&conn, "privacy", "Turned off Fully offline");
    *NOW.write().unwrap() = None;
}

/// Stops (or lets start again) what runs in the background.
pub async fn apply(app: &AppHandle, state: &Arc<AppState>) {
    if blocks(Keep::Web) {
        crate::browser::close(app);
    }
    if blocks(Keep::Connectors) {
        crate::connectors::stop_all(state).await;
    }
    if blocks(Keep::LocalNetwork) {
        crate::sync::people::hang_up_all();
    }
    crate::sync::wake();
    crate::phone::apply(state.clone());
    app.emit("offline:changed", json!({ "on": on() })).ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fully_offline_stops_everything_not_kept() {
        assert!(ALL.iter().all(|k| !stops(None, *k)), "off: nothing stopped");
        let kept: BTreeSet<Keep> = [Keep::Downloads].into_iter().collect();
        assert!([Keep::Updates, Keep::Web, Keep::LocalNetwork, Keep::Connectors].iter().all(|k| stops(Some(&kept), *k)));
        assert!(!stops(Some(&kept), Keep::Downloads), "kept");
        assert!(reason(Keep::Updates).contains("fully offline"));
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Features the user turns on as they need them. A new install starts
//! bare-bones (chat and models); everything else is added from the Features
//! page, installing whatever a feature needs (such as a speech model) first.
//!
//! "Off" means off, not just hidden: the assistant doesn't get that
//! feature's tools, the scheduler doesn't run, and its commands refuse.

use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::catalog::Budget;
use crate::speech::{self, Use};
use crate::{db, download, AppState, AppStateRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    Dictation,
    VoiceChat,
    ReadAloud,
    Meetings,
    Translate,
    Files,
    Memory,
    Projects,
    Scheduled,
    Connectors,
}

pub const ALL: &[Feature] = &[
    Feature::Dictation,
    Feature::VoiceChat,
    Feature::ReadAloud,
    Feature::Meetings,
    Feature::Translate,
    Feature::Files,
    Feature::Memory,
    Feature::Projects,
    Feature::Scheduled,
    Feature::Connectors,
];

impl Feature {
    pub fn name(self) -> &'static str {
        match self {
            Feature::Dictation => "Dictation",
            Feature::VoiceChat => "Voice chat",
            Feature::ReadAloud => "Read aloud",
            Feature::Meetings => "Meeting notes",
            Feature::Translate => "Translation",
            Feature::Files => "Files and coding",
            Feature::Memory => "Memory",
            Feature::Projects => "Projects",
            Feature::Scheduled => "Scheduled tasks",
            Feature::Connectors => "Connectors and plugins",
        }
    }

    /// The speech model a feature needs before it can be turned on.
    fn needs_speech(self) -> Option<Use> {
        match self {
            Feature::Dictation | Feature::VoiceChat => Some(Use::Live),
            Feature::Meetings => Some(Use::Accurate),
            _ => None,
        }
    }
}

const KEY: &str = "features";

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap_or(0)
}

/// Where an install starts. A new one is bare-bones. One from before
/// features could be turned off keeps what it was using.
fn starting_set(conn: &Connection) -> BTreeSet<Feature> {
    let mut on = BTreeSet::new();
    if !db::settings(conn).onboarded {
        return on;
    }
    let used = |t: &str| count(conn, t) > 0;
    if used("folders") {
        on.insert(Feature::Files);
    }
    if used("memories") {
        on.insert(Feature::Memory);
    }
    if used("projects") {
        on.insert(Feature::Projects);
    }
    if used("schedules") {
        on.insert(Feature::Scheduled);
    }
    if used("connectors") || used("plugins") {
        on.insert(Feature::Connectors);
    }
    if used("meetings") {
        on.insert(Feature::Meetings);
    }
    if !speech::installed(conn).is_empty() {
        on.insert(Feature::Dictation);
    }
    on
}

pub fn enabled(conn: &Connection) -> BTreeSet<Feature> {
    if let Some(set) = db::get::<BTreeSet<Feature>>(conn, KEY) {
        return set;
    }
    let set = starting_set(conn);
    let _ = db::set(conn, KEY, &set);
    set
}

pub fn is_on(conn: &Connection, f: Feature) -> bool {
    enabled(conn).contains(&f)
}

pub fn set(conn: &Connection, f: Feature, on: bool) -> Result<BTreeSet<Feature>, String> {
    let mut set = enabled(conn);
    if on {
        set.insert(f);
    } else {
        set.remove(&f);
    }
    db::set(conn, KEY, &set)?;
    // Memory's own pause switch follows the feature.
    if f == Feature::Memory {
        db::update_settings(conn, |s| s.memory_enabled = on)?;
    }
    Ok(set)
}

/// Every feature on (tests that exercise them all).
#[cfg(test)]
pub fn enable_all(conn: &Connection) {
    db::set(conn, KEY, &ALL.iter().copied().collect::<BTreeSet<_>>()).unwrap();
}

/// For commands that belong to a feature.
pub fn require(state: &AppState, f: Feature) -> Result<(), String> {
    if is_on(&state.db.lock().unwrap(), f) {
        Ok(())
    } else {
        Err(format!("{} is turned off. Turn it on in Features.", f.name()))
    }
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct Need {
    /// The model this PC would install for it.
    model_id: String,
    model_name: String,
    size: u64,
    /// A suitable model is already installed.
    met: bool,
}

#[derive(Serialize)]
pub struct FeatureView {
    id: Feature,
    enabled: bool,
    need: Option<Need>,
}

/// The model to install for a use, if none fits it yet.
fn need_for(state: &AppState, purpose: Use) -> Option<Need> {
    let hw = state.hardware.read().unwrap().clone();
    let b = Budget::from_hardware(&hw);
    let (list, settings) = {
        let conn = state.db.lock().unwrap();
        (speech::installed(&conn), speech::voice_settings(&conn))
    };
    let models = &state.catalog.speech.models;
    let have = speech::choose(&list, &settings, models, &hw, purpose);
    let want = match purpose {
        Use::Live => speech::recommended_live(models, &hw, &b).or_else(|| speech::recommended(models, &hw, &b)),
        Use::Accurate => speech::recommended(models, &hw, &b),
    }?;
    // A live model is "met" by anything installed that's quick enough; the
    // accurate one by having the suggested model (or one at least as big).
    let met = match (purpose, &have) {
        (_, None) => false,
        (Use::Live, Some(_)) => true,
        (Use::Accurate, Some(h)) => {
            let rank = |id: &str| models.iter().position(|m| m.id == id).unwrap_or(0);
            rank(&h.model_id) >= rank(&want.id)
        }
    };
    Some(Need { model_id: want.id.clone(), model_name: want.name.clone(), size: want.size, met })
}

#[tauri::command]
pub fn features_view(state: AppStateRef) -> Vec<FeatureView> {
    let on = enabled(&state.db.lock().unwrap());
    ALL.iter()
        .map(|f| FeatureView { id: *f, enabled: on.contains(f), need: f.needs_speech().and_then(|u| need_for(&state, u)) })
        .collect()
}

#[tauri::command]
pub fn set_feature(app: AppHandle, state: AppStateRef, feature: Feature, on: bool) -> Result<Vec<Feature>, String> {
    if on {
        if let Some(n) = feature.needs_speech().and_then(|u| need_for(&state, u)) {
            if !n.met && speech::installed(&state.db.lock().unwrap()).is_empty() {
                return Err(format!("{} needs a speech model first. Click Install.", feature.name()));
            }
        }
    }
    let conn = state.db.lock().unwrap();
    let set = set(&conn, feature, on)?;
    db::log_action(&conn, "feature", &format!("Turned {} {}", feature.name(), if on { "on" } else { "off" }));
    drop(conn);
    app.emit("features:changed", json!({})).ok();
    Ok(set.into_iter().collect())
}

/// Installs what a feature needs, then turns it on.
#[tauri::command]
pub fn install_feature(app: AppHandle, state: AppStateRef, feature: Feature) -> Result<(), String> {
    let Some(need) = feature.needs_speech().and_then(|u| need_for(&state, u)) else {
        // Nothing to download.
        set_feature(app, state, feature, true)?;
        return Ok(());
    };
    if need.met {
        set_feature(app, state, feature, true)?;
        return Ok(());
    }
    let spec = state.catalog.speech.models.iter().find(|m| m.id == need.model_id).cloned().ok_or("Unknown speech model")?;
    if !state.installs.lock().unwrap().is_empty() {
        return Err("Another install is in progress. Wait for it to finish first.".into());
    }
    let state: Arc<AppState> = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        let Some((cancel, _guard)) = crate::claim(&state.installs, &spec.id) else { return };
        let emit = |phase: &str, received: u64, total: u64| {
            app.emit("install:progress", json!({ "model_id": spec.id, "phase": phase, "received": received, "total": total })).ok();
        };
        state.log("model", &format!("Started installing {} for {}", spec.name, feature.name()));
        let cancel: &AtomicBool = &cancel;
        let result = speech::install(&state, &spec, cancel, &emit).await;
        let payload = match &result {
            Ok(_) => {
                let _ = set(&state.db.lock().unwrap(), feature, true);
                state.log("feature", &format!("Installed {} and turned on {}", spec.name, feature.name()));
                json!({ "model_id": spec.id, "name": feature.name(), "ok": true, "feature": feature })
            }
            Err(e) if e == download::CANCELLED => json!({ "model_id": spec.id, "name": feature.name(), "ok": false, "cancelled": true }),
            Err(e) => {
                state.log("model", &format!("Installing {} failed: {e}", spec.name));
                json!({ "model_id": spec.id, "name": feature.name(), "ok": false, "error": e })
            }
        };
        app.emit("install:finished", payload).ok();
        app.emit("features:changed", json!({})).ok();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let d = std::env::temp_dir().join(format!("sulcusai-ft-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        db::open(&d.join("t.db")).unwrap()
    }

    #[test]
    fn a_new_install_starts_bare() {
        let c = conn();
        assert!(enabled(&c).is_empty());
        set(&c, Feature::Dictation, true).unwrap();
        assert!(is_on(&c, Feature::Dictation));
        assert!(!is_on(&c, Feature::Meetings));
    }

    #[test]
    fn an_existing_install_keeps_what_it_uses() {
        let c = conn();
        db::update_settings(&c, |s| s.onboarded = true).unwrap();
        db::add_folder(&c, "C:/work").unwrap();
        c.execute("INSERT INTO projects (id, name, instructions, created_at, updated_at) VALUES ('p', 'x', '', 0, 0)", []).unwrap();
        let on = enabled(&c);
        assert!(on.contains(&Feature::Files) && on.contains(&Feature::Projects));
        assert!(!on.contains(&Feature::Scheduled) && !on.contains(&Feature::Dictation));
        // Decided once, then remembered.
        c.execute("DELETE FROM folders", []).unwrap();
        assert!(is_on(&c, Feature::Files));
    }

    #[test]
    fn memory_switch_follows_the_feature() {
        let c = conn();
        set(&c, Feature::Memory, false).unwrap();
        assert!(!db::settings(&c).memory_enabled);
        set(&c, Feature::Memory, true).unwrap();
        assert!(db::settings(&c).memory_enabled);
    }

    #[test]
    fn ids_are_stable_in_storage() {
        assert_eq!(serde_json::to_string(&Feature::VoiceChat).unwrap(), "\"voice_chat\"");
        assert_eq!(serde_json::from_str::<Feature>("\"read_aloud\"").unwrap(), Feature::ReadAloud);
    }
}

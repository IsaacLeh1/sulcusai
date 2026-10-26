// SPDX-License-Identifier: AGPL-3.0-only
//! Keeping the PC tidy: when models and chats were last used, locks that
//! keep them, the automatic cleanup that removes the ones not used for as
//! long as the person chose, and clearing caches (Settings → Cleanup).
//!
//! Everything here is kept in this PC's own settings (only the profile
//! syncs), so locks and usage don't travel. Chats deleted here are deleted
//! on the person's other synced PCs too, like any chat they delete.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{db, AppState, AppStateRef};

const USED_KEY: &str = "last_used";
const LOCKS_KEY: &str = "keep_locks";
const CLEAN_KEY: &str = "auto_clean";
const DAY_MS: i64 = 24 * 3600 * 1000;

// ---------- when things were last used ----------

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Used {
    models: HashMap<String, i64>,
    chats: HashMap<String, i64>,
}

/// What was written lately, so a busy chat doesn't write on every message.
fn recent() -> &'static Mutex<HashMap<String, i64>> {
    static R: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    R.get_or_init(Mutex::default)
}

fn touch(state: &AppState, model: bool, id: &str) {
    let now = db::now_ms();
    let key = format!("{}{id}", if model { "m:" } else { "c:" });
    {
        let mut r = recent().lock().unwrap();
        if r.get(&key).is_some_and(|t| now - t < 10 * 60_000) {
            return;
        }
        r.insert(key, now);
    }
    let conn = state.db.lock().unwrap();
    let mut u: Used = db::get(&conn, USED_KEY).unwrap_or_default();
    if model {
        u.models.insert(id.to_string(), now);
    } else {
        u.chats.insert(id.to_string(), now);
    }
    let _ = db::set(&conn, USED_KEY, &u);
}

/// A chat, picture, video or music model was used just now.
pub fn touch_model(state: &AppState, id: &str) {
    touch(state, true, id);
}

/// A chat was opened just now.
pub fn touch_chat(state: &AppState, id: &str) {
    touch(state, false, id);
}

// ---------- locks ----------

/// Chats and models the person locked: the automatic cleanup never removes them.
#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Locks {
    pub chats: Vec<String>,
    pub models: Vec<String>,
}

pub fn locks(conn: &rusqlite::Connection) -> Locks {
    db::get(conn, LOCKS_KEY).unwrap_or_default()
}

#[tauri::command(async)]
pub fn keep_locks(state: AppStateRef) -> Locks {
    locks(&state.db.lock().unwrap())
}

/// Locks or unlocks a chat ("chat") or a model ("model", any kind).
#[tauri::command(async)]
pub fn set_kept(state: AppStateRef, kind: String, id: String, keep: bool) -> Result<Locks, String> {
    let conn = state.db.lock().unwrap();
    let mut l = locks(&conn);
    let list = match kind.as_str() {
        "chat" => &mut l.chats,
        "model" => &mut l.models,
        _ => return Err("Unknown kind.".into()),
    };
    list.retain(|x| x != &id);
    if keep {
        list.push(id);
    }
    db::set(&conn, LOCKS_KEY, &l)?;
    Ok(l)
}

/// A model is gone: its lock and usage go too.
pub fn forget_model(conn: &rusqlite::Connection, id: &str) {
    let mut l = locks(conn);
    if l.models.iter().any(|m| m == id) {
        l.models.retain(|m| m != id);
        let _ = db::set(conn, LOCKS_KEY, &l);
    }
    let mut u: Used = db::get(conn, USED_KEY).unwrap_or_default();
    if u.models.remove(id).is_some() {
        let _ = db::set(conn, USED_KEY, &u);
    }
}

/// A chat is gone: its lock and usage go too.
pub fn forget_chat(conn: &rusqlite::Connection, id: &str) {
    let mut l = locks(conn);
    if l.chats.iter().any(|c| c == id) {
        l.chats.retain(|c| c != id);
        let _ = db::set(conn, LOCKS_KEY, &l);
    }
    let mut u: Used = db::get(conn, USED_KEY).unwrap_or_default();
    if u.chats.remove(id).is_some() {
        let _ = db::set(conn, USED_KEY, &u);
    }
}

// ---------- the automatic cleanup ----------

/// The person's choice: remove chats and models not used for this many days
/// (None: never). Off unless chosen.
#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoClean {
    pub chat_days: Option<u32>,
    pub model_days: Option<u32>,
    /// When it last ran (and, once turned on, a day's grace before the first run).
    pub last_run: i64,
    pub last_result: Option<String>,
}

fn settings(conn: &rusqlite::Connection) -> AutoClean {
    db::get(conn, CLEAN_KEY).unwrap_or_default()
}

/// Something the cleanup would remove (or did).
#[derive(Debug, Clone, Serialize)]
pub struct CleanItem {
    /// "chat", "model" (chat model) or "media" (picture, video, music).
    pub kind: String,
    pub id: String,
    pub name: String,
    pub last_used: i64,
    /// Bytes it frees (0 for chats).
    pub size: u64,
}

/// What the cleanup would remove now with these settings. Never: locked
/// chats and models, chats in a locked folder, chats shared with someone,
/// a chat writing a reply, the default and the loaded models, models from
/// other apps (removing them frees nothing), anything being installed.
fn plan(state: &AppState, s: &AutoClean, loaded: Option<&str>) -> Vec<CleanItem> {
    let Ok(c) = state.cipher() else { return Vec::new() };
    let now = db::now_ms();
    // Everything read under one short lock (model names and media models
    // take the lock themselves, so they're looked up after).
    let (locks, mut used, chats, installed, media_installed, mut keep_models, mut keep_chats) = {
        let conn = state.db.lock().unwrap();
        let locks = locks(&conn);
        let used: Used = db::get(&conn, USED_KEY).unwrap_or_default();
        let chats = if s.chat_days.is_some() { db::list_chats(&conn, &c) } else { Vec::new() };
        let installed = db::installed_models(&conn);
        let media_installed = crate::media::installed(&conn);
        let d = crate::media::defaults(&conn);
        let mut keep_models: HashSet<String> = [d.image, d.video, d.music].into_iter().flatten().collect();
        keep_models.extend(db::settings(&conn).default_model);
        let mut keep_chats: HashSet<String> = crate::folders::kept_chats(&conn);
        keep_chats.extend(crate::sync::people::shared_chat_ids(&conn));
        (locks, used, chats, installed, media_installed, keep_models, keep_chats)
    };
    keep_chats.extend(locks.chats.iter().cloned());
    keep_chats.extend(state.generations.lock().unwrap().keys().cloned());
    keep_models.extend(locks.models.iter().cloned());
    keep_models.extend(loaded.map(str::to_string));
    keep_models.extend(state.installs.lock().unwrap().keys().cloned());
    let mut out = Vec::new();
    if let Some(days) = s.chat_days {
        let cutoff = now - days as i64 * DAY_MS;
        for chat in chats {
            if keep_chats.contains(&chat.id) {
                continue;
            }
            let last = chat.updated_at.max(used.chats.get(&chat.id).copied().unwrap_or(0));
            if last < cutoff {
                let name = if chat.title.trim().is_empty() { "Untitled chat".to_string() } else { chat.title.clone() };
                out.push(CleanItem { kind: "chat".into(), id: chat.id, name, last_used: last, size: 0 });
            }
        }
    }
    let mut changed = false;
    if let Some(days) = s.model_days {
        let cutoff = now - days as i64 * DAY_MS;
        // Models used before usage was kept count from the first time the
        // cleanup saw them, so nothing goes the day this starts.
        let mut first_seen = |id: &str| -> i64 {
            *used.models.entry(id.to_string()).or_insert_with(|| {
                changed = true;
                now
            })
        };
        for m in installed {
            if keep_models.contains(&m.model_id) {
                continue;
            }
            let Some(spec) = state.model_spec(&m.model_id) else { continue };
            if spec.local.is_some() {
                continue;
            }
            let last = first_seen(&m.model_id);
            if last < cutoff {
                let size = folder_size(&state.paths.models.join(&m.model_id));
                out.push(CleanItem { kind: "model".into(), id: m.model_id.clone(), name: spec.name.clone(), last_used: last, size });
            }
        }
        let media = crate::media::models(state);
        for i in media_installed {
            if keep_models.contains(&i.model_id) {
                continue;
            }
            let Some(m) = media.iter().find(|m| m.id == i.model_id) else { continue };
            let last = first_seen(&i.model_id);
            if last < cutoff {
                out.push(CleanItem { kind: "media".into(), id: m.id.clone(), name: m.name.clone(), last_used: last, size: m.size() });
            }
        }
    }
    if changed {
        let _ = db::set(&state.db.lock().unwrap(), USED_KEY, &used);
    }
    out.sort_by_key(|i| i.last_used);
    out
}

/// Removes what the plan lists. Returns what went and the bytes freed.
async fn apply(state: &Arc<AppState>, items: Vec<CleanItem>, why: &str) -> (Vec<CleanItem>, u64) {
    let mut gone = Vec::new();
    let mut freed = 0;
    for item in items {
        let r = match item.kind.as_str() {
            "chat" => crate::delete_chat_now(state, &item.id),
            "model" => crate::remove_model_now(state, &item.id, false).await.map(|_| ()),
            _ => crate::media::remove_now(state, &item.id).map(|_| ()),
        };
        match r {
            Ok(()) => {
                freed += item.size;
                gone.push(item);
            }
            Err(e) => crate::applog::line("app", &format!("{why}: couldn't remove {}: {e}", item.name)),
        }
    }
    (gone, freed)
}

fn summary(gone: &[CleanItem], freed: u64) -> String {
    let chats = gone.iter().filter(|i| i.kind == "chat").count();
    let models = gone.len() - chats;
    match (chats, models) {
        (0, 0) => "Nothing needed removing".into(),
        (c, 0) => format!("Removed {c} unused chat{}", if c == 1 { "" } else { "s" }),
        (0, m) => format!("Removed {m} unused model{} ({} freed)", if m == 1 { "" } else { "s" }, crate::size_label(freed)),
        (c, m) => format!("Removed {c} unused chat{} and {m} unused model{} ({} freed)", if c == 1 { "" } else { "s" }, if m == 1 { "" } else { "s" }, crate::size_label(freed)),
    }
}

async fn loaded_model(state: &AppState) -> Option<String> {
    state.engine.lock().await.status().model_id.clone()
}

/// Runs the cleanup when it's on and a day has passed, while the app is
/// unlocked and nothing is being written or installed. Checks every 3 hours.
pub fn start(state: &Arc<AppState>) {
    let st = state.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(300)).await;
        loop {
            run_if_due(&st).await;
            tokio::time::sleep(Duration::from_secs(3 * 3600)).await;
        }
    });
}

async fn run_if_due(state: &Arc<AppState>) {
    let s = settings(&state.db.lock().unwrap());
    let on = s.chat_days.is_some() || s.model_days.is_some();
    let busy = !state.generations.lock().unwrap().is_empty() || !state.installs.lock().unwrap().is_empty();
    if !on || busy || state.work_cipher().is_err() || db::now_ms() - s.last_run < DAY_MS {
        return;
    }
    let loaded = loaded_model(state).await;
    let items = plan(state, &s, loaded.as_deref());
    let (gone, freed) = apply(state, items, "Automatic cleanup").await;
    let text = summary(&gone, freed);
    let conn = state.db.lock().unwrap();
    let mut s = settings(&conn);
    s.last_run = db::now_ms();
    s.last_result = Some(text.clone());
    let _ = db::set(&conn, CLEAN_KEY, &s);
    if !gone.is_empty() {
        let names: Vec<&str> = gone.iter().map(|i| i.name.as_str()).collect();
        db::log_action(&conn, "cleanup", &format!("Automatic cleanup: {text}: {}", names.join(", ")));
    }
}

#[derive(Serialize)]
pub struct CleanView {
    pub settings: AutoClean,
    pub locks: Locks,
    /// What it would remove with these settings now.
    pub would_remove: Vec<CleanItem>,
    /// When it runs next (it runs at most once a day).
    pub next_run: Option<i64>,
}

async fn view(state: &Arc<AppState>) -> CleanView {
    let s = settings(&state.db.lock().unwrap());
    let loaded = loaded_model(state).await;
    let would_remove = plan(state, &s, loaded.as_deref());
    let on = s.chat_days.is_some() || s.model_days.is_some();
    CleanView { locks: locks(&state.db.lock().unwrap()), next_run: on.then_some(s.last_run + DAY_MS), would_remove, settings: s }
}

#[tauri::command]
pub async fn clean_view(state: AppStateRef<'_>) -> Result<CleanView, String> {
    state.cipher()?;
    Ok(view(state.inner()).await)
}

/// Sets how long chats and models may go unused. Turning it on waits a day
/// before the first run, so there's time to look at the list and lock things.
#[tauri::command]
pub async fn set_auto_clean(state: AppStateRef<'_>, chat_days: Option<u32>, model_days: Option<u32>) -> Result<CleanView, String> {
    state.cipher()?;
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let was_on = s.chat_days.is_some() || s.model_days.is_some();
        s.chat_days = chat_days.filter(|d| *d > 0);
        s.model_days = model_days.filter(|d| *d > 0);
        let on = s.chat_days.is_some() || s.model_days.is_some();
        if on && !was_on {
            s.last_run = db::now_ms();
        }
        db::set(&conn, CLEAN_KEY, &s)?;
        let what = match (s.chat_days, s.model_days) {
            (None, None) => "Turned off the automatic cleanup".to_string(),
            (c, m) => format!(
                "Automatic cleanup: chats unused for {}, models unused for {}",
                c.map_or("ever (kept)".into(), |d| format!("{d} days")),
                m.map_or("ever (kept)".into(), |d| format!("{d} days"))
            ),
        };
        db::log_action(&conn, "cleanup", &what);
    }
    Ok(view(state.inner()).await)
}

/// "Clean up now": removes what the list shows, without waiting a day.
#[tauri::command]
pub async fn clean_now(state: AppStateRef<'_>) -> Result<CleanView, String> {
    state.cipher()?;
    let s = settings(&state.db.lock().unwrap());
    let loaded = loaded_model(&state).await;
    let items = plan(&state, &s, loaded.as_deref());
    let (gone, freed) = apply(state.inner(), items, "Cleanup").await;
    let text = summary(&gone, freed);
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        s.last_run = db::now_ms();
        s.last_result = Some(text.clone());
        db::set(&conn, CLEAN_KEY, &s)?;
        if !gone.is_empty() {
            let names: Vec<&str> = gone.iter().map(|i| i.name.as_str()).collect();
            db::log_action(&conn, "cleanup", &format!("Cleaned up now: {text}: {}", names.join(", ")));
        }
    }
    Ok(view(state.inner()).await)
}

// ---------- clearing caches ----------

fn folder_size(p: &Path) -> u64 {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_file() => m.len(),
        Ok(m) if m.is_dir() => std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| folder_size(&e.path())).sum(),
        _ => 0,
    }
}

/// Files under `dir` (any depth) whose names end with one of `ends`.
fn files_ending(dir: &Path, ends: &[&str], out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => files_ending(&p, ends, out),
            Ok(t) if t.is_file() => {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if ends.iter().any(|x| name.ends_with(x)) {
                    out.push(p);
                }
            }
            _ => {}
        }
    }
}

fn children(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| keep(&e.file_name().to_string_lossy())).map(|e| e.path()).collect()
}

/// Engine folders from older versions: a known kind of engine whose version
/// isn't the one the app uses now. Unknown folders are left alone.
fn old_engines(state: &AppState) -> Vec<PathBuf> {
    let c = &state.catalog;
    let current = [
        format!("{}-", c.engine.build),
        format!("sd-{}-", c.media.engine.build),
        format!("acestep-{}-", c.media.music_engine.build),
        format!("whisper-{}-", c.speech.engine.build),
        "whisper-public".to_string(),
        format!("onnxruntime-{}-", c.voices.runtime.build),
        c.voices.cards.as_ref().map_or_else(|| "onnxruntime-cards".to_string(), |r| format!("onnxruntime-{}-", r.build)),
    ];
    let llama = |n: &str| n.starts_with('b') && n[1..].split('-').next().is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()));
    children(&state.paths.engines, |n| {
        let known = llama(n) || ["sd-", "acestep-", "whisper-", "onnxruntime-"].iter().any(|p| n.starts_with(p));
        known && !current.iter().any(|p| n.starts_with(p.as_str())) && !n.contains('.')
    })
    .into_iter()
    .filter(|p| p.is_dir())
    // The NVIDIA cards' earlier build, once the cards' build has its CUDA part.
    .chain(crate::natural::earlier_builds(&state.paths, &c.voices))
    .collect()
}

/// One kind of cache: what it holds, and the files and folders it removes.
struct Cache {
    id: &'static str,
    name: &'static str,
    what: &'static str,
    targets: Vec<PathBuf>,
}

fn caches(state: &AppState) -> Vec<Cache> {
    let (data, models, engines) = (&state.paths.data, &state.paths.models, &state.paths.engines);
    let mut parts = Vec::new();
    files_ending(models, &[".part"], &mut parts);
    parts.extend(children(engines, |n| n.ends_with(".part") || n.starts_with(".staging-")));
    let webview = data.join("EBWebView").join("Default");
    let temp = std::env::temp_dir();
    vec![
        Cache {
            id: "media_work",
            name: "Leftovers from making pictures, video and music",
            what: "Scratch files from jobs that were stopped or crashed. Nothing in the gallery is touched.",
            targets: children(&data.join("media").join("work"), |_| true),
        },
        Cache {
            id: "downloads",
            name: "Unfinished downloads",
            what: "Model and engine downloads that didn't finish. Paused downloads start over if you resume them.",
            targets: parts,
        },
        Cache {
            id: "old_engines",
            name: "Old engine versions",
            what: "Engines from earlier versions of SulcusAI. The ones it uses now are kept.",
            targets: old_engines(state),
        },
        Cache {
            id: "undo",
            name: "Undo copies of files the assistant changed",
            what: "Copies kept so the assistant's file changes can be undone. Changes made before clearing can't be undone afterwards.",
            targets: children(&data.join("checkpoints"), |_| true),
        },
        Cache {
            id: "browser",
            name: "Built-in browser data",
            what: "History, cookies and sign-ins of SulcusAI's built-in browser. Sites there will ask you to sign in again.",
            targets: vec![data.join("browser")],
        },
        Cache {
            id: "window",
            name: "The app window's cache",
            what: "Files the app's window keeps to start faster. It makes them again as needed. Files in use stay until SulcusAI restarts.",
            targets: ["Cache", "Code Cache", "GPUCache", "DawnGraphiteCache", "DawnWebGPUCache"].iter().map(|f| webview.join(f)).collect(),
        },
        Cache {
            id: "tor",
            name: "Private connection (Tor) cache",
            what: "The saved list of Tor relays. It's downloaded again the next time the private connection starts.",
            targets: vec![data.join("tor").join("cache")],
        },
        Cache {
            id: "restore_copies",
            name: "Copies kept from before a restore",
            what: "Your old data, kept when a backup was restored over it.",
            targets: children(data, |n| n.starts_with("before-restore-") || n == ".restore"),
        },
        Cache {
            id: "crashes",
            name: "Saved crash reports",
            what: "Crash reports that weren't sent. The crash log in Settings → Logs stays.",
            targets: children(&data.join("crashes"), |n| n.ends_with(".json")),
        },
        Cache {
            id: "screens",
            name: "Screenshots for questions about the screen",
            what: "Pictures of the screen taken when you asked about it.",
            targets: children(&temp, |n| n.starts_with("sulcusai-shot-")),
        },
        Cache {
            id: "logs",
            name: "Logs",
            what: "SulcusAI's logs (Settings → Logs). They start again from now.",
            targets: children(&state.paths.logs, |_| true),
        },
    ]
}

#[derive(Serialize)]
pub struct CacheItem {
    pub id: &'static str,
    pub name: &'static str,
    pub what: &'static str,
    pub size: u64,
}

#[tauri::command(async)]
pub fn cache_view(state: AppStateRef) -> Vec<CacheItem> {
    caches(&state).into_iter().map(|c| CacheItem { id: c.id, name: c.name, what: c.what, size: c.targets.iter().map(|t| folder_size(t)).sum() }).collect()
}

#[derive(Serialize)]
pub struct Cleared {
    pub freed: u64,
    /// Files in use that couldn't be removed now.
    pub skipped: usize,
}

/// Deletes a file or folder, as much of it as isn't in use; returns
/// (bytes freed, files skipped).
fn remove(p: &Path) -> (u64, usize) {
    let Ok(m) = std::fs::symlink_metadata(p) else { return (0, 0) };
    if m.is_dir() && !m.file_type().is_symlink() {
        let (mut freed, mut skipped) = (0, 0);
        for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
            let (f, s) = remove(&e.path());
            freed += f;
            skipped += s;
        }
        let _ = std::fs::remove_dir(p);
        (freed, skipped)
    } else {
        match std::fs::remove_file(p) {
            Ok(()) => (m.len(), 0),
            Err(_) => (0, 1),
        }
    }
}

/// Clears the chosen caches.
#[tauri::command(async)]
pub fn clear_cache(state: AppStateRef, ids: Vec<String>) -> Result<Cleared, String> {
    let mut out = Cleared { freed: 0, skipped: 0 };
    let chosen: Vec<Cache> = caches(&state).into_iter().filter(|c| ids.iter().any(|i| i == c.id)).collect();
    for c in &chosen {
        // Undo copies are listed in the database too.
        if c.id == "undo" {
            let conn = state.db.lock().unwrap();
            let _ = conn.execute("DELETE FROM checkpoints", []);
        }
        if c.id == "downloads" {
            let conn = state.db.lock().unwrap();
            for p in crate::downloads::list(&conn) {
                let _ = crate::downloads::forget(&conn, &p.model_id);
            }
        }
        for t in &c.targets {
            // Never anything outside the app's folders (and its own temp screenshots).
            let inside = [&state.paths.data, &state.paths.models, &state.paths.engines, &state.paths.logs].iter().any(|root| t.starts_with(root))
                || (t.starts_with(std::env::temp_dir()) && t.file_name().is_some_and(|n| n.to_string_lossy().starts_with("sulcusai-shot-")));
            if inside {
                let (f, s) = remove(t);
                out.freed += f;
                out.skipped += s;
            }
        }
    }
    let names: Vec<&str> = chosen.iter().map(|c| c.name).collect();
    if !names.is_empty() {
        db::log_action(&state.db.lock().unwrap(), "cleanup", &format!("Cleared {} ({} freed)", names.join(", "), crate::size_label(out.freed)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_say_what_went() {
        let item = |kind: &str| CleanItem { kind: kind.into(), id: "x".into(), name: "x".into(), last_used: 0, size: 0 };
        assert_eq!(summary(&[], 0), "Nothing needed removing");
        assert_eq!(summary(&[item("chat"), item("chat")], 0), "Removed 2 unused chats");
        assert!(summary(&[item("chat"), item("model")], 5_000_000_000).starts_with("Removed 1 unused chat and 1 unused model ("));
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn the_cleanup_keeps_locked_chats_and_removes_old_ones() {
        let state = crate::e2e::scratch_state();
        let c = state.cipher().unwrap();
        let old = db::now_ms() - 100 * DAY_MS;
        let (fresh, stale, locked, foldered) = {
            let conn = state.db.lock().unwrap();
            let mut ids = Vec::new();
            for title in ["fresh", "stale", "locked", "in a locked folder"] {
                let chat = db::create_chat(&conn, &c, None).unwrap();
                db::set_chat_title(&conn, &c, &chat.id, title).unwrap();
                ids.push(chat.id);
            }
            for id in &ids[1..] {
                conn.execute("UPDATE chats SET updated_at = ?1 WHERE id = ?2", rusqlite::params![old, id]).unwrap();
            }
            db::set(&conn, LOCKS_KEY, &Locks { chats: vec![ids[2].clone()], models: vec![] }).unwrap();
            (ids[0].clone(), ids[1].clone(), ids[2].clone(), ids[3].clone())
        };
        crate::folders::create_for_test(&state, "Keep", &foldered, true);
        let s = AutoClean { chat_days: Some(30), ..Default::default() };
        let items = plan(&state, &s, None);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec![stale.as_str()], "only the old chat that isn't locked or in a locked folder");
        // Opening a chat counts as using it.
        touch_chat(&state, &stale);
        assert!(plan(&state, &s, None).is_empty());
        recent().lock().unwrap().clear();
        {
            let conn = state.db.lock().unwrap();
            let mut u: Used = db::get(&conn, USED_KEY).unwrap_or_default();
            u.chats.insert(stale.clone(), old);
            db::set(&conn, USED_KEY, &u).unwrap();
        }
        let (gone, _) = apply(&state, plan(&state, &s, None), "test").await;
        assert_eq!(gone.len(), 1);
        let conn = state.db.lock().unwrap();
        assert!(db::chat(&conn, &c, &stale).is_none());
        for id in [&fresh, &locked, &foldered] {
            assert!(db::chat(&conn, &c, id).is_some());
        }
    }

    #[test]
    fn removing_a_folder_skips_nothing_when_free_and_reports_its_size() {
        let dir = std::env::temp_dir().join(format!("sulcusai-cache-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/f.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(dir.join("g.part"), vec![0u8; 24]).unwrap();
        let mut parts = Vec::new();
        files_ending(&dir, &[".part"], &mut parts);
        assert_eq!(parts.len(), 1);
        assert_eq!(folder_size(&dir), 1024);
        assert_eq!(remove(&dir), (1024, 0));
        assert!(!dir.exists());
    }
}

/// Unit tests make `%TEMP%\sulcusai-*` folders and files and most leave them;
/// each test run clears what earlier runs left (anything over six hours old,
/// so a long run going on beside it keeps its own).
#[cfg(test)]
mod test_leftovers {
    #[test]
    fn earlier_test_runs_leave_nothing_behind() {
        let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
        let old = |e: &std::fs::DirEntry| e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 6 * 3600);
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("sulcusai-") && old(&e) {
                let p = e.path();
                let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            }
        }
    }
}

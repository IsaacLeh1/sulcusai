// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Isaac Lehman
//! SulcusAI core: commands the window calls, and the state behind them.

#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

mod advanced;
mod agent;
mod applog;
mod cardfirst;
mod cleanup;
mod doclib;
mod drafts;
mod drivers;
mod lineup;
mod study;
mod pccheck;
mod addons;
mod review;
mod folders;
mod backup;
mod api_server;
mod audio;
mod bridge;
mod browser;
mod browsers;
mod calendar;
mod catalog;
mod chat;
mod chatnotes;
mod compact;
mod cloud;
mod checkpoint;
mod connectors;
mod crash;
mod feedback;
mod crypto;
mod db;
mod devmode;
mod docs;
mod diarize;
mod download;
mod downloads;
#[cfg(all(test, windows))]
mod e2e;
mod engine;
mod features;
mod finetune;
mod found;
mod gguf;
mod handoff;
mod handoff_guard;
mod hardware;
mod hello;
mod hidden;
mod hub;
mod inbox;
mod jsrun;
mod local;
mod location;
#[cfg(test)]
mod lock_lint;
mod mail;
mod mcp;
mod media;
mod meeting;
mod memcheck;
mod memory;
mod natural;
mod net;
mod notes;
mod notify;
mod offline;
mod optimize;
mod outgoing;
mod oauth;
mod paths;
mod pcstatus;
mod people_window;
mod side_windows;
mod perf;
mod placement;
mod places;
mod practice;
mod privacy;
mod phone;
mod ports;
mod preview;
mod projects;
mod quick;
mod quotes;
mod router;
mod sandbox;
mod schedule;
mod security;
mod sitekeys;
mod speech;
mod storage;
mod subscriptions;
mod sync;
mod tools;
mod translate;
mod triage;
mod tts;
mod programs;
mod lens;
mod import;
mod typing;
mod updates;
mod vad;
mod viewer;
mod vision;
mod voice;
mod web;
mod workspace;
#[cfg(windows)]
mod winjob;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use rusqlite::Connection;
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};

use catalog::{Budget, Catalog, Hints, ModelFit, ModelSpec};
use chat::ContextInfo;
use crypto::{Cipher, Vault};
use db::{Chat, InstalledModel, Message, Profile, Settings};
use engine::{Engine, EngineStatus, JobRef, LaunchSpec};
use hardware::Hardware;
use net::Connectivity;
use paths::Paths;

/// Error text the window recognizes to show the lock screen.
pub const LOCKED: &str = "locked";

pub struct AppState {
    paths: Paths,
    db: Mutex<Connection>,
    vault: Mutex<Vault>,
    catalog: Catalog,
    hardware: RwLock<Hardware>,
    engine: tokio::sync::Mutex<Engine>,
    installs: Mutex<HashMap<String, Arc<AtomicBool>>>,
    generations: Mutex<HashMap<String, Arc<AtomicBool>>>,
    contexts: Mutex<HashMap<String, ContextInfo>>,
    approvals: agent::Approvals,
    mcp: tokio::sync::Mutex<HashMap<String, mcp::Client>>,
    speech: speech::Speech,
    player: audio::Player,
    heat: perf::Heat,
    bridge: bridge::Bridge,
    /// For work that needs a window (the browser); set once the app starts.
    app: std::sync::OnceLock<tauri::AppHandle>,
    /// When the chat model last started a piece of work (for the idle unload).
    engine_used: Mutex<std::time::Instant>,
    #[cfg(windows)]
    job: Option<winjob::Job>,
}

type AppStateRef<'a> = State<'a, Arc<AppState>>;

impl AppState {
    fn job(&self) -> Option<&JobRef> {
        #[cfg(windows)]
        return self.job.as_ref();
        #[cfg(not(windows))]
        return None;
    }

    /// A chat model by id: from the catalog, or one another app downloaded,
    /// under the name the user gave it, if any.
    /// Don't call it while holding the database lock.
    fn model_spec(&self, id: &str) -> Option<ModelSpec> {
        let conn = self.db.lock().unwrap();
        let mut spec = self
            .catalog
            .model(id)
            .cloned()
            .or_else(|| local::list(&conn).into_iter().find(|m| m.id == id))
            .or_else(|| hub::list(&conn).into_iter().find(|m| m.id == id))?;
        if let Some(name) = db::model_names(&conn).remove(id) {
            spec.name = name;
        }
        Some(spec)
    }

    fn budget(&self) -> Budget {
        Budget::from_hardware(&self.hardware.read().unwrap())
    }

    /// What the app may use right now: the performance mode's limits,
    /// stepped down by adaptive cooling when it's on.
    fn limits(&self) -> perf::Limits {
        let s = perf::settings(&self.db.lock().unwrap());
        let base = perf::limits(&s, &self.hardware.read().unwrap(), hardware::on_battery() == Some(true));
        if s.adaptive {
            perf::scaled(base, &self.heat.get())
        } else {
            base
        }
    }

    pub(crate) fn engine_idle(&self) -> std::time::Duration {
        self.engine_used.lock().unwrap().elapsed()
    }

    /// Checks the PC again (graphics cards, memory, power). When the cards
    /// changed (a driver installed, a laptop that switched its card on or
    /// off), the engines are asked for their devices again, the app log
    /// says so and the window is told. True when the cards changed.
    fn recheck_hardware(&self, why: &str) -> bool {
        let hw = hardware::detect(&self.paths.models);
        let cards = |h: &Hardware| (h.gpus.iter().map(|g| (g.name.clone(), g.vram_bytes, g.integrated)).collect::<Vec<_>>(), h.broken_cards.clone());
        let changed = cards(&self.hardware.read().unwrap()) != cards(&hw);
        if changed {
            engine::forget_devices();
            applog::line("app", &format!("Graphics after {why}: {}", graphics_line(&hw)));
        }
        *self.hardware.write().unwrap() = hw;
        if changed {
            if let Some(app) = self.app.get() {
                app.emit("hardware:changed", ()).ok();
            }
        }
        changed
    }

    fn settings(&self) -> Settings {
        db::settings(&self.db.lock().unwrap())
    }

    /// The data key, or `LOCKED` while app lock is engaged.
    fn cipher(&self) -> Result<Cipher, String> {
        self.vault.lock().unwrap().cipher().ok_or_else(|| LOCKED.to_string())
    }

    /// The data key for background work: also while the screen is locked
    /// with "keep working while locked" on.
    fn work_cipher(&self) -> Result<Cipher, String> {
        self.vault.lock().unwrap().work_cipher().ok_or_else(|| LOCKED.to_string())
    }

    fn log(&self, category: &str, summary: &str) {
        db::log_action(&self.db.lock().unwrap(), category, summary);
    }

    /// Encrypts anything stored before encryption existed, then compacts the
    /// file so the old plaintext is gone.
    fn migrate_plaintext(&self, cipher: &Cipher) {
        let mut conn = self.db.lock().unwrap();
        // Done once: reading every message at each start cost time for nothing.
        if db::get::<bool>(&conn, LEGACY_DONE).unwrap_or(false) {
            return;
        }
        match db::encrypt_legacy(&mut conn, cipher) {
            Ok(n) => {
                if n > 0 {
                    let _ = db::compact(&conn);
                    db::log_action(&conn, "privacy", &format!("Encrypted {n} previously saved items on this PC"));
                }
                let _ = db::set(&conn, LEGACY_DONE, &true);
            }
            Err(e) => eprintln!("encrypting saved data failed: {e}"),
        }
    }
}

/// Set once everything saved before encryption existed has been encrypted.
const LEGACY_DONE: &str = "legacy_encrypted";

/// Removes a cancel flag when the work it guards ends, however it ends.
struct FlagGuard<'a> {
    map: &'a Mutex<HashMap<String, Arc<AtomicBool>>>,
    key: String,
}

impl Drop for FlagGuard<'_> {
    fn drop(&mut self) {
        self.map.lock().unwrap().remove(&self.key);
    }
}

pub(crate) fn claim<'a>(map: &'a Mutex<HashMap<String, Arc<AtomicBool>>>, key: &str) -> Option<(Arc<AtomicBool>, FlagGuard<'a>)> {
    let mut m = map.lock().unwrap();
    if m.contains_key(key) {
        return None;
    }
    let flag = Arc::new(AtomicBool::new(false));
    m.insert(key.to_string(), flag.clone());
    Some((flag, FlagGuard { map, key: key.to_string() }))
}

pub(crate) fn host_of(url: &str) -> String {
    reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| "the internet".into())
}

pub(crate) fn size_label(bytes: u64) -> String {
    // Small files in MB (a 40 MB sound set read "0.0 GB").
    if bytes < catalog::GIB / 10 {
        return format!("{:.0} MB", bytes as f64 / (1u64 << 20) as f64);
    }
    format!("{:.1} GB", bytes as f64 / catalog::GIB as f64)
}

// ---------- app & hardware ----------

#[derive(Serialize)]
struct AppInfo {
    name: &'static str,
    version: &'static str,
    data_dir: String,
}

#[tauri::command(async)]
fn app_info(state: AppStateRef) -> AppInfo {
    AppInfo {
        name: "SulcusAI",
        version: env!("CARGO_PKG_VERSION"),
        data_dir: state.paths.data.display().to_string(),
    }
}

/// Saves a chart from a chat (run_code, market_quote) as an SVG file where the user picked.
#[tauri::command(async)]
fn save_chart(state: AppStateRef, path: String, svg: String) -> Result<(), String> {
    state.cipher()?;
    let p = std::path::PathBuf::from(&path);
    if !p.is_absolute() || !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) {
        return Err("Save it as an .svg file.".into());
    }
    if !svg.trim_start().starts_with("<svg") || svg.len() > 4 * 1024 * 1024 {
        return Err("That isn't a chart.".into());
    }
    std::fs::write(&p, svg).map_err(|e| format!("Couldn't save it: {e}"))?;
    state.log("files", "Saved a chart from a chat");
    Ok(())
}

/// "Check again" on the Models page and in the status bar: the cards, and
/// what the engine sees of them, are looked at afresh.
#[tauri::command(async)]
fn refresh_hardware(state: AppStateRef) -> Hardware {
    engine::forget_devices();
    if !state.recheck_hardware("checking again") {
        applog::line("app", &format!("Checked the graphics again: {}", graphics_line(&state.hardware.read().unwrap())));
    }
    state.hardware.read().unwrap().clone()
}

/// The PC's graphics for the app log: "NVIDIA GeForce RTX 3050 Laptop GPU
/// (4.3 GB), Intel(R) UHD Graphics (0.1 GB, built-in)", and any card that
/// isn't working.
fn graphics_line(hw: &Hardware) -> String {
    let gpus: Vec<String> = hw.gpus.iter().map(|g| format!("{} ({:.1} GB{})", g.name, g.vram_bytes as f64 / 1e9, if g.integrated { ", built-in" } else { "" })).collect();
    let broken: Vec<String> = hw
        .broken_cards
        .iter()
        .map(|b| {
            let what = match b.code {
                0 | 28 => "no driver".to_string(),
                22 => "turned off".to_string(),
                c => format!("stopped, code {c}"),
            };
            format!("{}{} ({what})", if b.vendor.is_empty() { String::new() } else { format!("{} card shown as ", b.vendor) }, b.name)
        })
        .collect();
    let mut line = if gpus.is_empty() { "none found".to_string() } else { gpus.join(", ") };
    if !broken.is_empty() {
        line.push_str(&format!("; not working: {}", broken.join(", ")));
    }
    line
}

/// No dedicated card at the last check: looked for again before a model
/// loads (at most once a minute), in case a driver was installed or the
/// laptop switched its card on since.
async fn recheck_if_card_missing(state: &Arc<AppState>) {
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    if state.hardware.read().unwrap().best_gpu().is_some() {
        return;
    }
    {
        let mut last = LAST.lock().unwrap();
        if last.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(60)) {
            return;
        }
        *last = Some(std::time::Instant::now());
    }
    let s = state.clone();
    tokio::task::spawn_blocking(move || s.recheck_hardware("looking for a graphics card again")).await.ok();
}

// ---------- catalog ----------

#[derive(Serialize)]
struct ModelCard {
    #[serde(flatten)]
    spec: ModelSpec,
    fit: ModelFit,
    installed: Option<InstalledModel>,
    /// A version whose file is already on this PC, so installing it skips
    /// the download.
    on_disk: Option<String>,
    /// The model's own name, when the user renamed it (`name` is theirs).
    renamed_from: Option<String>,
    /// Some version runs on this PC (fits its memory, fast enough, and on disk).
    runnable: bool,
    /// The one to suggest on this PC (`catalog::best_for`).
    best: bool,
    /// An installed model whose file is damaged or unfinished, and how
    /// (`model_file_problem`): it needs downloading again.
    damaged: Option<String>,
    /// Installed from a file without multi-token-prediction layers while the
    /// catalog now has one with them (faster): its download size.
    faster: Option<u64>,
}

#[derive(Serialize)]
struct CatalogView {
    hardware: Hardware,
    budget: Budget,
    backend: Option<String>,
    models: Vec<ModelCard>,
    hints: Hints,
    installing: Vec<String>,
}

/// The models for this PC: the ones it can run, or with `all` every one
/// (the Models page's "Show models this PC can't run").
#[tauri::command(async)]
fn catalog_view(state: AppStateRef, all: Option<bool>) -> CatalogView {
    let show_all = all.unwrap_or(false);
    let hardware = state.hardware.read().unwrap().clone();
    let budget = Budget::from_hardware(&hardware);
    let installed = db::installed_models(&state.db.lock().unwrap());
    let is_installed = |id: &str| installed.iter().any(|m| m.model_id == id);

    let mut all = state.catalog.models.clone();
    all.extend(local::list(&state.db.lock().unwrap()));
    all.extend(hub::list(&state.db.lock().unwrap()));
    let names = db::model_names(&state.db.lock().unwrap());
    let best = catalog::best_for(&state.catalog.models, &budget);
    let models = all
        .iter()
        .filter_map(|spec| {
            let mut fit = catalog::fit_model(spec, &budget);
            if spec.local.is_some() {
                // Already on disk: free space doesn't matter.
                fit.variants.iter_mut().for_each(|v| v.disk_ok = true);
            }
            let inst = installed.iter().find(|m| m.model_id == spec.id).cloned();
            let on_disk = if inst.is_some() {
                None
            } else {
                runnable_variants(spec, &fit)
                    .into_iter()
                    .find(|v| state.paths.models.join(&spec.id).join(&v.file).metadata().is_ok_and(|m| m.len() == v.size))
                    .map(|v| v.quant.clone())
            };
            let mut spec = spec.clone();
            let renamed_from = names.get(&spec.id).map(|name| std::mem::replace(&mut spec.name, name.clone()));
            let runnable = fit.visible();
            let best = best.as_deref() == Some(spec.id.as_str());
            let damaged = inst.as_ref().and_then(|i| model_file_problem(&model_file(&state.paths, i), i.size));
            let faster = inst
                .as_ref()
                .filter(|_| damaged.is_none())
                .and_then(|i| spec.variants.iter().find(|v| v.quant == i.quant && v.mtp && v.size != i.size).map(|v| (i, v.size)))
                .filter(|(i, _)| !has_mtp(&model_file(&state.paths, i)))
                .map(|(_, size)| size);
            (inst.is_some() || runnable || show_all).then(|| ModelCard { spec, fit, installed: inst, on_disk, renamed_from, runnable, best, damaged, faster })
        })
        .collect();

    CatalogView {
        hints: catalog::hints(&state.catalog, &budget, &is_installed),
        backend: engine::backend_for(&budget).ok().map(str::to_string),
        installing: state.installs.lock().unwrap().keys().cloned().collect(),
        hardware,
        budget,
        models,
    }
}

#[derive(Clone, Serialize)]
struct InstallProgress<'a> {
    model_id: &'a str,
    phase: &'a str,
    received: u64,
    total: u64,
}

#[tauri::command(async)]
fn install_model(app: AppHandle, state: AppStateRef, model_id: String, quant: String, force: Option<bool>) -> Result<(), String> {
    let spec = state.catalog.model(&model_id).cloned().or_else(|| hub::list(&state.db.lock().unwrap()).into_iter().find(|m| m.id == model_id)).ok_or("Unknown model")?;
    start_install(app, state.inner().clone(), spec, quant, force.unwrap_or(false))
}

/// Whether a model file has multi-token-prediction layers, read once per
/// file (its header is read for each Models page otherwise).
fn has_mtp(path: &std::path::Path) -> bool {
    static SEEN: std::sync::OnceLock<Mutex<HashMap<(std::path::PathBuf, u64), bool>>> = std::sync::OnceLock::new();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let key = (path.to_path_buf(), size);
    let seen = SEEN.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = seen.lock().unwrap().get(&key) {
        return *v;
    }
    let v = engine::mtp_layers(path) > 0;
    seen.lock().unwrap().insert(key, v);
    v
}

/// What a model's earlier file is kept as while its faster one downloads.
const BEFORE_FASTER: &str = "before-faster";

/// The faster (MTP) file for a model installed before the catalog had it.
/// The old file is kept until the new one is in: a download that fails or
/// is stopped leaves it to come back with Repair.
#[tauri::command(async)]
fn faster_model(app: AppHandle, state: AppStateRef, model_id: String) -> Result<(), String> {
    let installed = db::installed_model(&state.db.lock().unwrap(), &model_id).ok_or("That model isn't installed.")?;
    let spec = state.catalog.model(&model_id).cloned().ok_or("That model isn't in the catalog.")?;
    if !spec.variants.iter().any(|v| v.quant == installed.quant && v.mtp) {
        return Err("There's no faster version of this model yet.".into());
    }
    let path = model_file(&state.paths, &installed);
    let kept = path.with_extension(BEFORE_FASTER);
    std::fs::rename(&path, &kept).map_err(|e| format!("Couldn't set the old file aside: {e}"))?;
    state.log("model", &format!("Getting the faster version of {} (it guesses several words ahead); the old file is kept until it's in", spec.name));
    start_install(app, state.inner().clone(), spec, installed.quant, true)
}

/// A damaged or unfinished model file (`model_file_problem`) downloaded
/// again: the bad file goes, and the model installs as new.
#[tauri::command(async)]
fn repair_model(app: AppHandle, state: AppStateRef, model_id: String) -> Result<(), String> {
    let installed = db::installed_model(&state.db.lock().unwrap(), &model_id).ok_or("That model isn't installed.")?;
    let spec = state.model_spec(&model_id).ok_or("That model isn't available anymore.")?;
    if spec.local.is_some() {
        return Err("That model's file belongs to another app: repair it there, or remove it here.".into());
    }
    let path = model_file(&state.paths, &installed);
    // A faster version that didn't finish: the file it replaced comes back.
    let kept = path.with_extension(BEFORE_FASTER);
    if kept.is_file() && model_file_problem(&path, installed.size).is_some() {
        std::fs::remove_file(&path).ok();
        std::fs::rename(&kept, &path).map_err(|e| format!("Couldn't bring the earlier file back: {e}"))?;
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(installed.size);
        db::save_installed(&state.db.lock().unwrap(), &InstalledModel { size, ..installed })?;
        state.log("model", &format!("{}: its earlier file is back (the faster version didn't finish downloading)", spec.name));
        return Ok(());
    }
    if model_file_problem(&path, installed.size).is_some() {
        std::fs::remove_file(&path).ok();
    }
    state.log("model", &format!("Downloading {} again (its file was damaged)", spec.name));
    start_install(app, state.inner().clone(), spec, installed.quant, true)
}

/// Stops a download but keeps what's downloaded, to resume later.
#[tauri::command(async)]
fn pause_install(state: AppStateRef, model_id: String) {
    if let Some(flag) = state.installs.lock().unwrap().get(&model_id) {
        downloads::mark_pausing(&model_id);
        flag.store(true, Ordering::Relaxed);
    }
}

/// Continues a paused download from where it stopped.
#[tauri::command(async)]
fn resume_install(app: AppHandle, state: AppStateRef, model_id: String) -> Result<(), String> {
    let paused = downloads::list(&state.db.lock().unwrap()).into_iter().find(|p| p.model_id == model_id).ok_or("That download isn't paused anymore.")?;
    let spec = state.model_spec(&model_id).ok_or("That model isn't available anymore.")?;
    // It was started knowingly, so a warning it got then doesn't stop it now.
    start_install(app, state.inner().clone(), spec, paused.quant, true)
}

#[derive(Serialize)]
struct DownloadsView {
    paused: Vec<downloads::Paused>,
}

/// Paused downloads (the running one comes as `install:progress` events).
#[tauri::command(async)]
fn downloads_view(state: AppStateRef) -> DownloadsView {
    DownloadsView { paused: downloads::list(&state.db.lock().unwrap()) }
}

/// Searches Hugging Face for GGUF chat models; only files this PC can run.
#[tauri::command]
async fn hub_search(state: AppStateRef<'_>, query: String, sort: String, all: Option<bool>, purpose: Option<String>) -> Result<hub::HubResults, String> {
    hub::search(state.inner(), &query, &sort, all.unwrap_or(false), purpose.as_deref().unwrap_or("")).await
}

/// A pasted Hugging Face link, as the model it points at.
#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum HubLink {
    /// A GGUF chat model (installs with `hub_install`). `found_for`: the model
    /// the link named, when it had no GGUF file and this is a GGUF copy of it.
    Chat { model: hub::HubModel, file: Option<String>, found_for: Option<String> },
    /// A picture, video or music model (installs with `media_hub_install`).
    Media { model: media::hub::HubMediaModel, media: &'static str, file: Option<String> },
}

/// Looks up a pasted Hugging Face link: the model it points at, with every
/// file SulcusAI can use (the ones this PC can't run say why), and the file
/// the link named, if it named one.
#[tauri::command]
async fn hub_link(state: AppStateRef<'_>, link: String) -> Result<HubLink, String> {
    let (repo_id, file) = hub::parse_link(&link)?;
    let client = hub::client(&state)?;
    state.log("network", "Looked up a pasted Hugging Face link (huggingface.co)");
    let r = hub::repo(&client, &repo_id).await.map_err(|e| {
        if e.contains("401") || e.contains("404") {
            "There's no public model at that link. Check it, or the model may be private.".to_string()
        } else {
            e
        }
    })?;
    // Needs an account: a GGUF copy of it (they usually don't) is the way in.
    if !r.info["gated"].is_null() && r.info["gated"] != false {
        if let Some(found) = gguf_copy_of(&state, &client, &repo_id).await {
            return Ok(found);
        }
        return Err("That model needs a Hugging Face account to download, which SulcusAI doesn't use.".into());
    }
    // The file the link named, when it's one SulcusAI can use (else all are
    // shown); a split model's later part names the whole model.
    let file = file.map(|f| hub::listed_file(&f));
    let named = |files: Vec<&String>| file.clone().filter(|f| files.contains(&f));
    // Picture, video and music models first: their files are GGUF too.
    if let Some((model, kind)) = media::hub::from_link(&state, &r) {
        let file = named(model.files.iter().map(|f| &f.file).collect());
        return Ok(HubLink::Media { file, model, media: kind });
    }
    if hub::is_chat(&r) {
        let model = hub::describe(&r.info, &r, &state.budget(), true);
        if !model.files.is_empty() {
            let file = named(model.files.iter().map(|f| &f.file).collect());
            return Ok(HubLink::Chat { file, model, found_for: None });
        }
    }
    // A chat model's original upload (no GGUF file): a GGUF copy of it.
    if matches!(r.info["pipeline_tag"].as_str(), None | Some("text-generation" | "image-text-to-text")) {
        if let Some(found) = gguf_copy_of(&state, &client, &repo_id).await {
            return Ok(found);
        }
    }
    Err(hub_why_not(&r.info))
}

/// The most downloaded GGUF copy of a model that has a file SulcusAI can load
/// (his brother pasted openai/gpt-oss-120b, which isn't in GGUF, and SulcusAI
/// said it couldn't use it instead of finding ggml-org/gpt-oss-120b-GGUF).
async fn gguf_copy_of(state: &AppState, client: &reqwest::Client, of: &str) -> Option<HubLink> {
    for id in hub::gguf_copies(client, of).await.into_iter().take(6) {
        let Ok(copy) = hub::repo(client, &id).await else { continue };
        if !hub::is_chat(&copy) {
            continue;
        }
        let model = hub::describe(&copy.info, &copy, &state.budget(), true);
        if !model.files.is_empty() {
            state.log("network", "Found a GGUF copy of a pasted Hugging Face model (huggingface.co)");
            return Some(HubLink::Chat { file: None, model, found_for: Some(of.to_string()) });
        }
    }
    None
}

/// Why a pasted model can't be used, by what kind of model it is (his
/// brother read "GGUF" in the old message as a broken file, every time).
fn hub_why_not(info: &serde_json::Value) -> String {
    let text = |v: &serde_json::Value| v.as_str().map(str::to_lowercase).unwrap_or_default();
    let id = text(&info["id"]);
    let pipeline = text(&info["pipeline_tag"]);
    let tags: Vec<String> = info["tags"].as_array().into_iter().flatten().map(text).collect();
    let bases = text(&info["cardData"]["base_model"]) + &info["cardData"]["base_model"].as_array().into_iter().flatten().map(text).collect::<Vec<_>>().join(" ");
    let picture = matches!(pipeline.as_str(), "text-to-image" | "image-to-image") || tags.iter().any(|t| t.starts_with("diffusers"));
    let works = "Picture models from Hugging Face work when they're built on SDXL (for example Juggernaut XL or RealVisXL), Stable Diffusion 1.5 (for example Realistic Vision), FLUX.1, FLUX.2 klein, Z-Image or Qwen Image.";
    // Only the folders Python's diffusers library reads (unet/, vae/…), no single file.
    let files: Vec<String> = info["siblings"].as_array().into_iter().flatten().map(|s| text(&s["rfilename"])).collect();
    let one_file = files.iter().any(|f| !f.contains('/') && (f.ends_with(".safetensors") || f.ends_with(".ckpt") || f.ends_with(".gguf")));
    let split = files.iter().any(|f| f.starts_with("unet/") || f.starts_with("transformer/")) && !one_file;
    if let Some(why) = media::hub::music_why_not(info) {
        why.to_string()
    } else if (id.contains("flux.1") || bases.contains("flux.1")) && !id.contains("flux.2") {
        format!(
            "That FLUX.1 model isn't in a form SulcusAI runs: it needs the model as one .gguf or .safetensors file (city96's GGUF versions work), and FLUX.1 Fill, Kontext, Redux, Depth and Canny aren't supported. {works}"
        )
    } else if picture && split {
        format!(
            "That model is only there split into folders (unet, vae, text_encoder), the way Python tools load it. SulcusAI needs it as one .safetensors or .gguf file: \
             look for a version of it with a single file (often in another repository, or under Files and versions). {works}"
        )
    } else if picture {
        format!("SulcusAI can't run that picture model yet: it isn't built on one of the kinds it runs. {works}")
    } else if pipeline.contains("video") {
        "SulcusAI can't run that video model yet. Video models from Hugging Face work when they're built on Wan 2.2 5B or Wan 2.1 1.3B.".into()
    } else if matches!(pipeline.as_str(), "text-generation" | "image-text-to-text") {
        "That chat model has no GGUF file, the format SulcusAI runs chat models in. Look for the same model with \"GGUF\" in its name (for example from unsloth or bartowski) and paste that link.".into()
    } else {
        format!("SulcusAI can't use that model. Chat models need a GGUF file. {works}")
    }
}

/// Installs one file of a Hugging Face model, like a catalog model.
/// Returns the model's id, whose card shows the progress.
#[tauri::command]
async fn hub_install(app: AppHandle, state: AppStateRef<'_>, repo: String, file: String, force: Option<bool>) -> Result<String, String> {
    let spec = hub::prepare(state.inner(), &repo, &file).await?;
    let (id, quant) = (spec.id.clone(), spec.variants[0].quant.clone());
    if let Err(e) = start_install(app, state.inner().clone(), spec, quant, force.unwrap_or(false)) {
        hub::forget(&state.db.lock().unwrap(), &id).ok();
        return Err(e);
    }
    Ok(id)
}

/// Downloads, checks and speed-tests a model in the background, reporting
/// progress to its card. `force`: the person was warned this PC probably
/// can't run it and chose to go ahead.
fn start_install(app: AppHandle, state: Arc<AppState>, mut spec: ModelSpec, quant: String, force: bool) -> Result<(), String> {
    let mut variant = spec.variant(&quant).ok_or("Unknown model version")?.clone();
    let model_id = spec.id.clone();
    // Free space as it is now, not as it was when the app started.
    if let Some(free) = storage::free_on(&state.paths.models) {
        state.hardware.write().unwrap().disk_free = free;
    }
    let fit = catalog::fit_model(&spec, &state.budget());
    let vfit = fit.variant(&quant).ok_or("Unknown model version")?;
    if !vfit.runnable() && !force {
        return Err("This PC can't run that version of the model.".into());
    }
    if !vfit.disk_ok {
        return Err("There isn't enough free disk space for this model.".into());
    }
    let backend = engine::backend_for(&state.budget())?.to_string();
    if !state.installs.lock().unwrap().is_empty() {
        return Err("Another install is in progress. Wait for it to finish first.".into());
    }
    // Resumed (or started over): no longer paused.
    downloads::forget(&state.db.lock().unwrap(), &model_id)?;
    app.emit("downloads:changed", json!({})).ok();

    tauri::async_runtime::spawn(async move {
        let Some((cancel, _guard)) = claim(&state.installs, &model_id) else { return };
        // The last progress, for a pause to remember.
        let last = std::sync::Mutex::new((0u64, 0u64));
        let emit = |phase: &str, received: u64, total: u64| {
            *last.lock().unwrap() = (received, total);
            app.emit("install:progress", InstallProgress { model_id: &model_id, phase, received, total }).ok();
        };
        state.log("model", &format!("Started installing {} ({} quality)", spec.name, variant.quality));
        let result: Result<(), String> = async {
            emit("engine", 0, 0);
            if engine::installed_server(&state.paths, &state.catalog.engine, &backend).is_none() {
                let asset = state.catalog.engine.assets.get(&backend).map(|a| a.url.clone()).unwrap_or_default();
                state.log("network", &format!("Downloading the llama.cpp engine ({backend}) from {}", host_of(&asset)));
            }
            let exe = engine::ensure_installed(&state.paths, &state.catalog.engine, &backend, &cancel, |r, t| {
                emit("engine", r, t)
            })
            .await?;

            let dest = state.paths.models.join(&spec.id).join(&variant.file);
            if dest.exists() {
                emit("verify", 0, 0); // reusing an earlier download after a hash check
            } else {
                state.log(
                    "network",
                    &format!("Downloading {} ({}) from {}", spec.name, size_label(variant.size), host_of(&variant.url)),
                );
            }
            let client = net::external_client(state.settings().connectivity, net::Purpose::ModelDownload, false)?;
            if variant.parts.is_empty() {
                download::fetch_verified(&client, &variant.url, &dest, variant.size, &variant.sha256, &cancel, |r, t| emit("download", r, t)).await?;
            } else {
                // A split model: every part, into the same folder (the engine
                // loads the first and finds the rest beside it).
                let mut done = 0;
                for p in &variant.parts {
                    let to = state.paths.models.join(&spec.id).join(&p.file);
                    download::fetch_verified(&client, &p.url, &to, p.size, &p.sha256, &cancel, |r, _| emit("download", done + r, variant.size)).await?;
                    done += p.size;
                }
            }
            // Models that can see pictures also get their image encoder.
            if let Some(v) = &spec.vision {
                let vdest = state.paths.models.join(&spec.id).join(&v.file);
                if !vdest.exists() {
                    state.log("network", &format!("Downloading {}'s picture reader ({}) from {}", spec.name, size_label(v.size), host_of(&v.url)));
                }
                download::fetch_verified(&client, &v.url, &vdest, v.size, &v.sha256, &cancel, |r, t| emit("vision", r, t)).await?;
            }
            // From Hugging Face: the file's header replaces the estimates.
            if spec.id.starts_with("hf-") {
                hub::refine(&state, &mut spec, &dest);
                variant = spec.variants[0].clone();
            }

            // A faster version in place: the file it replaced goes.
            let _ = std::fs::remove_file(dest.with_extension(BEFORE_FASTER));
            let previous = db::installed_model(&state.db.lock().unwrap(), &spec.id);
            {
                let conn = state.db.lock().unwrap();
                db::save_installed(&conn, &InstalledModel {
                    model_id: spec.id.clone(),
                    quant: variant.quant.clone(),
                    path: dest.display().to_string(),
                    size: variant.size,
                    installed_at: db::now_ms(),
                    tps: None, prompt_tps: None,
                })?;
                if db::settings(&conn).default_model.is_none() {
                    db::update_settings(&conn, |s| s.default_model = Some(spec.id.clone()))?;
                }
            }
            // Replacing another version of the same model: drop the old file.
            if let Some(old) = previous.filter(|p| p.path != dest.display().to_string()) {
                // A reply still using it finishes first (up to 3 minutes):
                // stopping the engine now would cut it off.
                let asked = std::time::Instant::now();
                while !state.generations.lock().unwrap().is_empty() && asked.elapsed() < WAIT_FOR_REPLIES {
                    if state.engine.lock().await.status().model_id.as_deref() != Some(&spec.id) {
                        break;
                    }
                    emit("waiting", 0, 0);
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                let mut engine = state.engine.lock().await;
                if engine.status().model_id.as_deref() == Some(&spec.id) {
                    engine.stop().await;
                }
                local::remove_with_parts(std::path::Path::new(&old.path));
            }

            // Trying it out: loading it (the longest part for a big model), then timing a short reply.
            emit("loading", 0, 0);
            let log = state.paths.engine_log();
            // Same settings a chat will use, so the first chat doesn't reload the model.
            let limits = state.limits();
            let mmproj = vision_file(&state.paths, &spec);
            let encoder_size = mmproj.as_ref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
            let plan = launch_plan(&state, &spec, &variant.quant, encoder_size);
            let mut engine = state.engine.lock().await;
            // Not while a reply runs or a model is being taught: loading this
            // one would stop the model they use. The speed can be tested later
            // on the Models page.
            if !state.generations.lock().unwrap().is_empty() || finetune::training() {
                drop(engine);
                state.log("model", &format!("Skipped testing {}'s speed: the engine was busy", spec.name));
                return Ok(());
            }
            let measured = async {
                let ep = engine
                    .ensure_tuned(
                        LaunchSpec {
                            exe: &exe,
                            model_path: &dest,
                            model_id: &spec.id,
                            quant: &variant.quant,
                            ctx: plan.ctx,
                            gpu_layers: plan.gpu_layers,
                            threads: plan.threads,
                            low_priority: limits.low_priority,
                            log: &log,
                            mmproj: mmproj.as_deref(),
                            batch: plan.batch,
                            lora: None,
                        },
                        plan.tuning,
                        state.job(),
                    )
                    .await?;
                watch_work(&state, &ep);
                emit("benchmark", 0, 0);
                engine::measure(&ep).await
            }
            .await;
            drop(engine);
            match measured {
                Ok(sp) => db::set_speed(&state.db.lock().unwrap(), &spec.id, sp.tps, sp.prompt_tps)?,
                // The model is installed; only the speed test failed.
                Err(e) => eprintln!("benchmark failed for {}: {e}", spec.id),
            }
            // The estimate left part of it off the graphics card: the setups
            // are tried on this PC now, while the person expects a wait.
            if plan.gpu_layers > 0 && plan.gpu_layers <= spec.arch.n_layer {
                emit("optimizing", 0, 0);
                if let Err(e) = optimize_now(&app, &state, &spec.id).await {
                    state.log("model", &format!("Couldn't optimize {} for this PC: {e}", spec.name));
                }
            }
            Ok(())
        }
        .await;

        let payload = match &result {
            Ok(()) => {
                state.log("model", &format!("Installed {} ({} quality)", spec.name, variant.quality));
                json!({ "model_id": model_id, "ok": true })
            }
            Err(e) if e == download::CANCELLED && downloads::take_pausing(&model_id) => {
                let (received, total) = *last.lock().unwrap();
                let paused = downloads::Paused { model_id: model_id.clone(), quant: variant.quant.clone(), name: spec.name.clone(), received, total, at: db::now_ms() };
                downloads::remember(&state.db.lock().unwrap(), paused).ok();
                state.log("model", &format!("Paused downloading {}", spec.name));
                json!({ "model_id": model_id, "ok": false, "paused": true })
            }
            Err(e) if e == download::CANCELLED => {
                downloads::delete_parts(&state.paths, &spec);
                if spec.id.starts_with("hf-") {
                    hub::forget(&state.db.lock().unwrap(), &spec.id).ok();
                }
                state.log("model", &format!("Cancelled installing {}", spec.name));
                json!({ "model_id": model_id, "ok": false, "cancelled": true })
            }
            Err(e) => {
                state.log("model", &format!("Installing {} failed: {e}", spec.name));
                json!({ "model_id": model_id, "ok": false, "error": e })
            }
        };
        app.emit("downloads:changed", json!({})).ok();
        app.emit("install:finished", payload).ok();
    });
    Ok(())
}

/// A model's image encoder, if it has one and it's downloaded.
pub(crate) fn vision_file(paths: &Paths, spec: &ModelSpec) -> Option<std::path::PathBuf> {
    let v = spec.vision.as_ref()?;
    let p = paths.models.join(&spec.id).join(&v.file);
    p.metadata().is_ok_and(|m| m.len() == v.size).then_some(p)
}

/// Downloads a model's image encoder if it's missing (models installed
/// before pictures could be attached).
pub(crate) async fn ensure_vision(state: &Arc<AppState>, spec: &ModelSpec) -> Result<Option<std::path::PathBuf>, String> {
    let Some(v) = &spec.vision else { return Ok(None) };
    if let Some(p) = vision_file(&state.paths, spec) {
        return Ok(Some(p));
    }
    let dest = state.paths.models.join(&spec.id).join(&v.file);
    state.log("network", &format!("Downloading {}'s picture reader ({}) from {}", spec.name, size_label(v.size), host_of(&v.url)));
    let client = net::external_client(state.settings().connectivity, net::Purpose::ModelDownload, false)?;
    download::fetch_verified(&client, &v.url, &dest, v.size, &v.sha256, &AtomicBool::new(false), |_, _| {}).await?;
    Ok(Some(dest))
}

/// The stored path, or the same file in today's models folder if the data
/// folder has moved since install.
/// What's wrong with a model file, if anything: gone, cut short (or a
/// different size than downloaded), or not a model at all (a GGUF file
/// starts with "GGUF"; a copy that stopped part way, or a file another
/// program wrote over, doesn't). `size` 0: not known.
pub(crate) fn model_file_problem(path: &std::path::Path, size: u64) -> Option<String> {
    use std::io::Read;
    let Ok(meta) = std::fs::metadata(path) else { return Some("the file is missing".into()) };
    // A model split across files ("…-00001-of-00003.gguf"): `size` is all its
    // parts together (counting the first alone called every one damaged).
    let mut have = meta.len();
    if let Some((base, 1, total)) = path.to_str().and_then(hub::split_parts_of) {
        for n in 2..=total {
            match std::fs::metadata(format!("{base}-{n:05}-of-{total:05}.gguf")) {
                Ok(m) => have += m.len(),
                Err(_) => return Some(format!("part {n} of its {total} files is missing")),
            }
        }
    }
    if size > 0 && have != size {
        return Some(format!("it's {} instead of {}", size_label(have), size_label(size)));
    }
    let mut magic = [0u8; 4];
    match std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)) {
        Ok(()) if &magic == b"GGUF" => None,
        Ok(()) => Some("it isn't a model file (its contents are wrong, as when copying it stopped part way)".into()),
        Err(e) => Some(format!("it can't be read ({e})")),
    }
}

pub(crate) fn model_file(paths: &Paths, m: &InstalledModel) -> std::path::PathBuf {
    let stored = std::path::PathBuf::from(&m.path);
    if stored.exists() {
        return stored;
    }
    match stored.file_name() {
        Some(name) => paths.models.join(&m.model_id).join(name),
        None => stored,
    }
}

/// Context, GPU layers, threads and batch size to start a model with:
/// worked out for this PC, unless advanced mode says otherwise.
/// The context window the person chose (None: automatic).
fn chosen_window(state: &AppState) -> Option<u32> {
    db::get::<u32>(&state.db.lock().unwrap(), "context_window").filter(|n| *n >= 2048)
}

/// How to start a model (`launch_plan`).
pub(crate) struct Plan {
    pub(crate) ctx: u32,
    pub(crate) gpu_layers: u32,
    pub(crate) threads: usize,
    pub(crate) batch: u32,
    pub(crate) tuning: engine::Tuning,
    /// The card memory it was planned with (after the mode's limits and a
    /// picture reader beside the model).
    pub(crate) budget: catalog::Budget,
}

/// `encoder`: the size of the picture reader loaded with the model (0: none).
fn launch_plan(state: &AppState, spec: &ModelSpec, quant: &str, encoder: u64) -> Plan {
    plan_with(state, spec, quant, encoder, true)
}

/// `measured`: settings measured on this PC ("Optimize for this PC") are
/// used where they apply; without, the automatic plan (what the optimizer
/// starts from).
/// The least context window a model that uses tools gets.
pub(crate) const TOOL_CTX: u32 = 16_384;

pub(crate) fn plan_with(state: &AppState, spec: &ModelSpec, quant: &str, encoder: u64, measured: bool) -> Plan {
    let mut limits = state.limits();
    let mut budget = state.budget();
    // The picture reader takes room of its own beside the model.
    if encoder > 0 {
        catalog::set_aside(&mut budget, &mut limits, encoder + catalog::VISION_OVERHEAD);
    }
    let e = advanced::get(state).engine;
    let chosen = chosen_window(state);
    // Several cards: a model that fits the biggest at its usual window runs
    // there alone (faster than with a slower card beside it); a bigger one is
    // spread over them all.
    let usual = e.ctx.or(chosen).unwrap_or(if spec.tools { TOOL_CTX } else { spec.default_ctx });
    let spread = budget.cards > 1 && !catalog::fits_biggest_card(spec, quant, &budget, &limits, usual);
    if budget.cards > 1 && !spread {
        (budget, limits) = budget.biggest_card(&limits);
    }
    let (mut ctx, mut layers) = catalog::launch_within(spec, quant, &budget, &limits);
    // A bigger window, as far as this PC still runs the model (layers move
    // off the graphics card to make room).
    if let Some(want) = chosen {
        (ctx, layers) = catalog::launch_at(spec, quant, &budget.with_limits(&limits), want, (ctx, layers), limits.gpu_share);
    } else if spec.tools && e.ctx.is_none() && ctx < TOOL_CTX {
        // A model that uses tools gets at least 16K: in 8K a task of a few
        // steps (a page fetched, files listed, a command's output) ran out
        // of room, and the agent cut tools and results to fit. Layers move
        // off the card for it when they must (a mixture of experts keeps
        // only experts in memory).
        (ctx, layers) = catalog::launch_at(spec, quant, &budget.with_limits(&limits), TOOL_CTX, (ctx, layers), limits.gpu_share);
    }
    // The compressed cache, when it puts more of the model on the card (or,
    // with no window chosen, gives a 16K one). Not with layers, a window in
    // advanced mode or a GPU share set: those are kept as they are.
    let mut kv_q8 = false;
    if e.ctx.is_none() && e.gpu_layers.is_none() && limits.gpu_share >= 1.0 {
        // A card too small for this window: the compressed cache or a
        // smaller window still gets some of the model on it, rather than
        // all on the processor.
        if layers == 0 {
            if let Some((c, l, q8)) = catalog::on_small_card(spec, quant, &budget.with_limits(&limits), ctx, chosen.is_none()) {
                (ctx, layers, kv_q8) = (c, l, q8);
            }
        }
        if let Some((c, l)) = catalog::with_q8_cache(spec, quant, &budget.with_limits(&limits), ctx, layers, chosen.is_none()) {
            // The mode's cap is for the automatic window: a chosen one stays
            // (it was cut back to 8K in Cool mode, so choosing changed nothing).
            let c = if chosen.is_some() { c } else { c.min(limits.max_ctx.max(if spec.tools { TOOL_CTX } else { 0 })) };
            (ctx, layers, kv_q8) = (c, l, true);
        }
        // Measured on this PC for this window and this much of the card.
        let room = budget.with_limits(&limits).vram;
        if let Some(t) = optimize::get(&state.db.lock().unwrap(), &spec.id).filter(|t| measured && t.applies(quant, ctx, room)) {
            (layers, kv_q8) = (t.gpu_layers, t.kv_q8);
        }
    }
    if let Some(c) = e.ctx {
        ctx = c.clamp(512, spec.arch.max_ctx.max(512));
    }
    if let Some(l) = e.gpu_layers {
        layers = l.min(spec.arch.n_layer + 1);
    }
    // A mixture-of-experts model split with the card keeps experts, not
    // whole layers, in system memory (unless advanced mode set the layers).
    let cpu_moe = if e.gpu_layers.is_none() { catalog::moe_offload(spec, quant, layers) } else { None };
    // Wholly on the card, the processor's threads barely matter: the mode's
    // own count, so a cooling step doesn't reload the model for them.
    let threads = match e.threads {
        Some(t) => t as usize,
        None if layers > spec.arch.n_layer => {
            let s = perf::settings(&state.db.lock().unwrap());
            perf::limits(&s, &state.hardware.read().unwrap(), hardware::on_battery() == Some(true)).threads
        }
        None => limits.threads,
    };
    let est = spec.variant(quant).map_or(0.0, |v| catalog::fit_variant(spec, v, ctx, &budget.with_limits(&limits)).est_tps);
    let think_budget = think_budget(state, spec, est);
    Plan { ctx, gpu_layers: layers, threads, batch: e.batch.unwrap_or(0), tuning: engine::Tuning { cpu_moe, kv_q8, think_budget, spread, embedding: false }, budget: budget.with_limits(&limits) }
}

/// Seconds a reply may think before it's told to answer. Qwen3.6 27B, a
/// little too big for a 16 GB card, thought for minutes at a time: up to
/// 16K tokens a reply, at about 10 tokens a second.
const THINK_SECS: f64 = 75.0;

/// The most tokens a thinking model may think in a reply: about
/// `THINK_SECS` at its measured speed (else the estimate), 1K to 8K. None
/// when it doesn't think, or thinking is set on (no limit) or off.
fn think_budget(state: &AppState, spec: &ModelSpec, est_tps: f64) -> Option<u32> {
    let setting = advanced::get(state).processing.thinking;
    if setting != advanced::Thinking::Auto || !thinks(spec, setting) {
        return None;
    }
    let measured = db::installed_model(&state.db.lock().unwrap(), &spec.id).and_then(|m| m.tps).filter(|t| *t > 0.0);
    think_tokens(measured.unwrap_or(est_tps))
}

/// `THINK_SECS` of thinking at this speed, 1K to 8K tokens.
fn think_tokens(tps: f64) -> Option<u32> {
    (tps > 0.0).then(|| (tps * THINK_SECS).clamp(1024.0, 8192.0) as u32)
}

/// The versions of a model this PC can run, the best suited first.
fn runnable_variants<'a>(spec: &'a ModelSpec, fit: &ModelFit) -> Vec<&'a catalog::Variant> {
    let mut list: Vec<_> = spec.variants.iter().filter(|v| fit.variant(&v.quant).is_some_and(|f| f.runnable())).collect();
    list.sort_by_key(|v| fit.recommended.as_deref() != Some(v.quant.as_str()));
    list
}

/// Catalog chat models whose files are already on this PC (from an earlier
/// install or an exact copy in another app) join the list without a
/// download. The engine comes with the first chat if it isn't here yet.
/// Returns the names added.
fn adopt_found(state: &AppState, finder: &mut found::Finder) -> Vec<String> {
    let budget = state.budget();
    let busy: Vec<String> = state.installs.lock().unwrap().keys().cloned().collect();
    let have = db::installed_models(&state.db.lock().unwrap());
    let mut names = Vec::new();
    for spec in &state.catalog.models {
        if busy.contains(&spec.id) || have.iter().any(|m| m.model_id == spec.id) {
            continue;
        }
        let fit = catalog::fit_model(spec, &budget);
        let Some((variant, dest, place)) = runnable_variants(spec, &fit).into_iter().find_map(|v| {
            let dest = state.paths.models.join(&spec.id).join(&v.file);
            match finder.place(&dest, v.size, &v.sha256) {
                found::Place::Missing => None,
                place => Some((v, dest, place)),
            }
        }) else {
            continue;
        };
        if let Some(v) = &spec.vision {
            finder.place(&state.paths.models.join(&spec.id).join(&v.file), v.size, &v.sha256);
        }
        {
            let conn = state.db.lock().unwrap();
            let saved = db::save_installed(&conn, &InstalledModel {
                model_id: spec.id.clone(),
                quant: variant.quant.clone(),
                path: dest.display().to_string(),
                size: variant.size,
                installed_at: db::now_ms(),
                tps: None, prompt_tps: None,
            });
            if saved.is_err() {
                continue;
            }
            if db::settings(&conn).default_model.is_none() {
                let _ = db::update_settings(&conn, |s| s.default_model = Some(spec.id.clone()));
            }
        }
        state.log("model", &found::note(&spec.name, &place));
        names.push(spec.name.clone());
    }
    names
}

#[derive(Serialize)]
struct FoundModels {
    /// Names of the models added.
    added: Vec<String>,
    /// Models seen that can't be used, with why.
    skipped: Vec<local::Skipped>,
    /// The apps whose folders exist on this PC.
    looked_in: Vec<&'static str>,
}

/// Looks for models already on this PC and adds them. `again`: the user
/// asked, so models they stopped using earlier come back too.
fn find_models_now(state: &AppState, again: bool) -> FoundModels {
    let mut finder = found::Finder::new();
    let mut added = adopt_found(state, &mut finder);
    added.extend(speech::adopt_found(state, &mut finder));
    added.extend(media::adopt_found(state, &mut finder));
    let (more, skipped) = local::adopt(state, finder.entries(), again);
    added.extend(more);
    let mut looked_in: Vec<&'static str> = found::sources().into_iter().filter(|s| s.dir.exists()).map(|s| s.app).collect();
    looked_in.dedup();
    FoundModels { added, skipped, looked_in }
}

fn announce_found(app: &AppHandle, names: &[String]) {
    if !names.is_empty() {
        app.emit("models:found", json!({ "names": names })).ok();
        app.emit("features:changed", json!({})).ok();
    }
}

#[tauri::command]
async fn find_models(app: AppHandle, state: AppStateRef<'_>, again: Option<bool>) -> Result<FoundModels, String> {
    let state = state.inner().clone();
    let found = tauri::async_runtime::spawn_blocking(move || find_models_now(&state, again.unwrap_or(false))).await.map_err(|e| e.to_string())?;
    announce_found(&app, &found.added);
    Ok(found)
}

/// Adds the .gguf models in a folder (or one file) the user picked or typed;
/// they're used where they are.
#[tauri::command]
async fn import_model_folder(app: AppHandle, state: AppStateRef<'_>, path: String) -> Result<local::FolderImport, String> {
    let state = state.inner().clone();
    // Windows' "Copy as path" adds quotes.
    let path = std::path::PathBuf::from(path.trim().trim_matches('"'));
    let r = tauri::async_runtime::spawn_blocking(move || local::import_folder(&state, &path)).await.map_err(|e| e.to_string())??;
    announce_found(&app, &r.added);
    Ok(r)
}

/// Adds a .gguf model file the user picked; it's used where it is.
#[tauri::command]
async fn import_model_file(app: AppHandle, state: AppStateRef<'_>, path: String) -> Result<String, String> {
    let state = state.inner().clone();
    let name = tauri::async_runtime::spawn_blocking(move || local::import(&state, std::path::Path::new(&path))).await.map_err(|e| e.to_string())??;
    announce_found(&app, std::slice::from_ref(&name));
    Ok(name)
}

static SPEED_TESTING: AtomicBool = AtomicBool::new(false);

/// What the speed test is doing, so a Models page opened mid-test (or after)
/// shows it: the model being tested, how far along, and what didn't finish.
#[derive(Clone, Serialize)]
struct SpeedStatus {
    running: bool,
    /// Testing every installed model (not just one).
    all: bool,
    model_id: Option<String>,
    done: usize,
    total: usize,
    /// Models whose test didn't finish, with why (kept until the next test).
    errors: std::collections::BTreeMap<String, String>,
}

static SPEED_STATUS: std::sync::Mutex<SpeedStatus> =
    std::sync::Mutex::new(SpeedStatus { running: false, all: false, model_id: None, done: 0, total: 0, errors: std::collections::BTreeMap::new() });

/// Updates the status and tells the window (`models:speed`).
fn speed_progress(app: &AppHandle, update: impl FnOnce(&mut SpeedStatus), extra: serde_json::Value) {
    let mut payload = {
        let mut s = SPEED_STATUS.lock().unwrap();
        update(&mut s);
        serde_json::to_value(&*s).unwrap_or_default()
    };
    if let (Some(p), Some(e)) = (payload.as_object_mut(), extra.as_object()) {
        p.extend(e.clone());
    }
    app.emit("models:speed", payload).ok();
}

/// One model's speed test, loaded as chats load it.
async fn test_speed(state: &Arc<AppState>, model_id: &str) -> Result<engine::Speed, String> {
    let (ep, spec, _) = llm_endpoint(state, Some(model_id.to_string()), false, |_| {}).await?;
    let sp = engine::measure(&ep).await?;
    db::set_speed(&state.db.lock().unwrap(), model_id, sp.tps, sp.prompt_tps)?;
    state.log("model", &format!("Measured {}: writes {:.1} tokens/sec, reads {} tokens/sec", spec.name, sp.tps, sp.prompt_tps.map(|p| format!("{p:.0}")).unwrap_or_else(|| "?".into())));
    Ok(sp)
}

/// Swapping models mid-reply would cut a chat off.
fn speed_test_allowed(state: &AppState) -> Result<(), String> {
    if !state.generations.lock().unwrap().is_empty() {
        return Err("Wait for the reply to finish, then test the speed.".into());
    }
    if finetune::training() {
        return Err("A model is being taught right now and has the graphics card. Try again when it's done.".into());
    }
    Ok(())
}

/// Where the speed test is (or what the last one left).
#[tauri::command(async)]
fn speed_test_status() -> SpeedStatus {
    SPEED_STATUS.lock().unwrap().clone()
}

/// "Optimize for this PC" for one chat model. Progress arrives as
/// `models:optimize` events ({model_id, step, of, label}); the last one has
/// `finished` with the report or an error.
async fn optimize_now(app: &AppHandle, state: &Arc<AppState>, model_id: &str) -> Result<optimize::Report, String> {
    speed_test_allowed(state)?;
    if SPEED_TESTING.swap(true, Ordering::SeqCst) {
        return Err("A speed test is already running.".into());
    }
    let progress = |step: usize, of: usize, label: &str| {
        app.emit("models:optimize", json!({ "model_id": model_id, "step": step, "of": of, "label": label })).ok();
    };
    let r = optimize::run(state, model_id, &progress).await;
    SPEED_TESTING.store(false, Ordering::SeqCst);
    let payload = match &r {
        Ok(report) => json!({ "model_id": model_id, "finished": true, "report": report }),
        Err(e) => json!({ "model_id": model_id, "finished": true, "error": e }),
    };
    app.emit("models:optimize", payload).ok();
    r
}

#[tauri::command]
async fn optimize_model(app: AppHandle, state: AppStateRef<'_>, model_id: String) -> Result<optimize::Report, String> {
    let state = state.inner().clone();
    state.cipher()?;
    optimize_now(&app, &state, &model_id).await
}

/// Settings measured on this PC, per model.
#[tauri::command(async)]
fn model_tunings(state: AppStateRef) -> Result<HashMap<String, optimize::Tuned>, String> {
    state.cipher()?;
    Ok(optimize::all(&state.db.lock().unwrap()))
}

/// Back to the automatic settings for a model.
#[tauri::command(async)]
fn forget_optimization(state: AppStateRef, model_id: String) -> Result<(), String> {
    state.cipher()?;
    optimize::forget(&state.db.lock().unwrap(), &model_id)
}

/// Runs the speed test for an installed chat model.
#[tauri::command]
async fn measure_model_speed(app: AppHandle, state: AppStateRef<'_>, model_id: String) -> Result<engine::Speed, String> {
    let state = state.inner().clone();
    speed_test_allowed(&state)?;
    if SPEED_TESTING.swap(true, Ordering::SeqCst) {
        return Err("A speed test is already running.".into());
    }
    let id = model_id.clone();
    speed_progress(&app, |s| {
        s.errors.remove(&id);
        *s = SpeedStatus { running: true, all: false, model_id: Some(id), done: 0, total: 1, errors: std::mem::take(&mut s.errors) };
    }, json!({}));
    let r = test_speed(&state, &model_id).await;
    SPEED_TESTING.store(false, Ordering::SeqCst);
    let extra = match &r {
        Ok(sp) => json!({ "tps": sp.tps, "prompt_tps": sp.prompt_tps, "finished": true }),
        Err(e) => json!({ "error": e, "finished": true }),
    };
    let err = r.as_ref().err().cloned();
    speed_progress(&app, |s| {
        s.running = false;
        s.done = 1;
        if let Some(e) = err {
            s.errors.insert(model_id.clone(), e);
        }
    }, extra);
    r
}

/// Tests every installed chat model on this PC, one after another, then
/// unloads the last so the memory is free again. Progress arrives as
/// `models:speed` events (the status, plus tps or error for a model just
/// tested, and finished at the end).
#[tauri::command]
async fn measure_all_speeds(app: AppHandle, state: AppStateRef<'_>) -> Result<usize, String> {
    let state = state.inner().clone();
    speed_test_allowed(&state)?;
    if SPEED_TESTING.swap(true, Ordering::SeqCst) {
        return Err("A speed test is already running.".into());
    }
    let ids: Vec<String> = db::installed_models(&state.db.lock().unwrap())
        .into_iter()
        .filter(|m| !cloud::is_cloud(&m.model_id) && state.model_spec(&m.model_id).is_some())
        .map(|m| m.model_id)
        .collect();
    let total = ids.len();
    let mut ok = 0;
    for (i, id) in ids.iter().enumerate() {
        let now = id.clone();
        speed_progress(&app, |s| {
            *s = SpeedStatus { running: true, all: true, model_id: Some(now), done: i, total, errors: if i == 0 { Default::default() } else { std::mem::take(&mut s.errors) } };
        }, json!({}));
        if !state.generations.lock().unwrap().is_empty() {
            break;
        }
        match test_speed(&state, id).await {
            Ok(sp) => {
                ok += 1;
                speed_progress(&app, |s| s.done = i + 1, json!({ "tested": id, "tps": sp.tps, "prompt_tps": sp.prompt_tps }));
            }
            Err(e) => {
                let (key, msg) = (id.clone(), e.clone());
                speed_progress(&app, |s| {
                    s.done = i + 1;
                    s.errors.insert(key, msg);
                }, json!({ "tested": id, "error": e }));
            }
        }
    }
    if state.generations.lock().unwrap().is_empty() {
        state.engine.lock().await.stop().await;
    }
    SPEED_TESTING.store(false, Ordering::SeqCst);
    speed_progress(&app, |s| {
        s.running = false;
        s.model_id = None;
    }, json!({ "finished": true }));
    Ok(ok)
}

#[tauri::command(async)]
fn cancel_install(app: AppHandle, state: AppStateRef, model_id: String) {
    if let Some(flag) = state.installs.lock().unwrap().get(&model_id) {
        flag.store(true, Ordering::Relaxed);
        return;
    }
    // A paused one: drop what was downloaded.
    if let Some(spec) = state.model_spec(&model_id) {
        downloads::delete_parts(&state.paths, &spec);
        if spec.id.starts_with("hf-") && db::installed_model(&state.db.lock().unwrap(), &spec.id).is_none() {
            hub::forget(&state.db.lock().unwrap(), &spec.id).ok();
        }
    }
    downloads::forget(&state.db.lock().unwrap(), &model_id).ok();
    app.emit("downloads:changed", json!({})).ok();
}

/// Removes a chat model. `delete_file`: a model from another app or a
/// folder also has its file moved to the Recycle Bin (never Ollama's, whose
/// files only Ollama can remove safely). Returns the bytes freed.
#[tauri::command]
async fn remove_model(state: AppStateRef<'_>, model_id: String, delete_file: Option<bool>) -> Result<u64, String> {
    state.cipher()?;
    remove_model_now(state.inner(), &model_id, delete_file.unwrap_or(false)).await
}

/// Removes a chat model from this PC entirely: its files (and half-finished
/// downloads), models taught on top of it, its paused download, the name
/// given to it, and the lock and usage the cleanup kept.
pub(crate) async fn remove_model_now(state: &Arc<AppState>, model_id: &str, delete_file: bool) -> Result<u64, String> {
    let spec = state.model_spec(model_id);
    {
        let mut engine = state.engine.lock().await;
        if engine.status().model_id.as_deref() == Some(model_id) {
            // A reply or a background job using it would be cut off.
            let held = engine.current().is_some_and(|(_, ep)| engine::held(&ep.key) > 0);
            if held || !state.generations.lock().unwrap().is_empty() {
                return Err("This model is answering right now. Wait for it to finish (or stop the reply), then remove it.".into());
            }
            engine.stop().await;
        }
    }
    let from = spec.as_ref().and_then(|s| s.local.clone());
    if delete_file && from.as_ref().is_some_and(|l| l.app.eq_ignore_ascii_case("ollama")) {
        return Err("Ollama keeps this model in its own store. Remove it in Ollama instead (ollama rm <name>), so Ollama doesn't break.".into());
    }
    let mut freed = 0u64;
    // Another app's file stays where it is, unless the person asked for it to go.
    let dir = state.paths.models.join(model_id);
    if from.is_none() && dir.starts_with(&state.paths.models) && dir != state.paths.models && dir.exists() {
        freed += storage::folder_size(&dir);
        std::fs::remove_dir_all(&dir).map_err(|e| format!("Couldn't delete the model files: {e}"))?;
    }
    if let (Some(l), true) = (&from, delete_file) {
        let file = std::path::PathBuf::from(&l.path);
        if file.is_file() {
            let size = file.metadata().map_or(0, |m| m.len());
            checkpoint::recycle(&file).map_err(|e| format!("Couldn't move the model file to the Recycle Bin: {e}"))?;
            freed += size;
        }
    }
    let taught = finetune::forget_adapters_of(state, model_id);
    let conn = state.db.lock().unwrap();
    db::remove_installed(&conn, model_id)?;
    local::forget(&conn, model_id)?;
    hub::forget(&conn, model_id)?;
    downloads::forget(&conn, model_id)?;
    db::set_model_name(&conn, model_id, None)?;
    cleanup::forget_model(&conn, model_id);
    if db::settings(&conn).default_model.as_deref() == Some(model_id) {
        let next = db::installed_models(&conn).first().map(|m| m.model_id.clone());
        db::update_settings(&conn, |s| s.default_model = next)?;
    }
    let name = spec.as_ref().map_or(model_id, |m| m.name.as_str());
    let taught = if taught > 0 { format!(" and {taught} model(s) taught on it") } else { String::new() };
    match (&from, delete_file) {
        (Some(l), false) => db::log_action(&conn, "model", &format!("Stopped using {name} (its file stays in {})", l.app)),
        (Some(_), true) => db::log_action(&conn, "model", &format!("Removed {name}{taught} and moved its file to the Recycle Bin ({} freed once it's emptied)", size_label(freed))),
        (None, _) => db::log_action(&conn, "model", &format!("Removed {name}{taught} and deleted its files ({} freed)", size_label(freed))),
    }
    Ok(freed)
}

// ---------- settings & profile ----------

/// The app registrations for signing in with Microsoft and Google.
#[tauri::command(async)]
fn oauth_clients(state: AppStateRef) -> Result<oauth::ClientsView, String> {
    state.cipher()?;
    Ok(oauth::clients_view(&state.db.lock().unwrap()))
}

#[tauri::command(async)]
fn set_oauth_clients(state: AppStateRef, ids: oauth::ClientIds) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    oauth::set_clients(&conn, &ids)?;
    db::log_action(&conn, "privacy", "Changed the sign-in app registrations");
    Ok(())
}

#[tauri::command(async)]
fn get_settings(state: AppStateRef) -> Settings {
    state.settings()
}

#[tauri::command(async)]
fn set_connectivity(app: AppHandle, state: AppStateRef, level: Connectivity) -> Result<Settings, String> {
    if level == Connectivity::Offline {
        // Offline closes the browser; nothing in it should keep going online.
        browser::close(&app);
    }
    state.cipher()?;
    // Picking a level ends Fully offline (what it stopped may start again).
    if offline::on() {
        offline::turn_off(&state);
        let (app, state) = (app.clone(), state.inner().clone());
        tauri::async_runtime::spawn(async move { offline::apply(&app, &state).await });
    }
    let conn = state.db.lock().unwrap();
    let before = db::settings(&conn).connectivity;
    let s = db::update_settings(&conn, |s| s.connectivity = level)?;
    if before != level {
        let label = match level {
            Connectivity::Offline => "Offline",
            Connectivity::Web => "Local AI + Web",
            Connectivity::Cloud => "Cloud",
        };
        db::log_action(&conn, "privacy", &format!("Connectivity set to {label}"));
    }
    Ok(s)
}

/// Shows a model under a name the user picked (only the name in the app
/// changes; the file stays as it is). An empty name brings back its own.
#[tauri::command(async)]
fn rename_model(state: AppStateRef, model_id: String, name: String) -> Result<(), String> {
    let name = name.trim();
    if name.chars().count() > 60 {
        return Err("Keep the name under 60 characters.".into());
    }
    let conn = state.db.lock().unwrap();
    db::set_model_name(&conn, &model_id, (!name.is_empty()).then_some(name))
}

#[derive(Serialize)]
struct WindowOption {
    ctx: u32,
    placement: Option<catalog::Placement>,
    /// Tokens a second at the start of a chat, and once the window is full.
    est_tps: f64,
    full_tps: f64,
    /// Seconds to read a full window, at this PC's measured reading speed.
    read_full_secs: Option<f64>,
    needed: u64,
}

#[derive(Serialize)]
struct ContextOptions {
    /// What the person chose (None: automatic).
    chosen: Option<u32>,
    /// The automatic size for this model on this PC.
    auto: u32,
    /// The largest the model supports.
    max: u32,
    options: Vec<WindowOption>,
    /// The model uses tools: under 16K, a task of a few steps runs out of room.
    tools: bool,
    /// A window set in Advanced, which wins over these.
    advanced: Option<u32>,
    /// The speeds are this PC's measured ones (else estimates).
    measured: bool,
}

/// Every window size up to the model's own most (and that, if it isn't one
/// of them), how each would run here: where, how fast at the start and when
/// full, how long a full one takes to read. Speeds are scaled by this PC's
/// measured speed for the model when there is one.
#[tauri::command(async)]
fn context_options(state: AppStateRef, model_id: Option<String>) -> Result<ContextOptions, String> {
    let id = model_id.or_else(|| state.settings().default_model).ok_or("Pick a model first.")?;
    let spec = state.model_spec(&id).ok_or("That model isn't available.")?;
    let installed = db::installed_model(&state.db.lock().unwrap(), &id);
    let quant = installed.as_ref().map(|m| m.quant.clone()).unwrap_or_else(|| spec.variants[0].quant.clone());
    let variant = spec.variant(&quant).ok_or("Unknown model version")?;
    let budget = state.budget().with_limits(&state.limits());
    let (auto, _) = catalog::launch_within(&spec, &quant, &state.budget(), &state.limits());
    // Measured against estimated at the window it runs with now.
    let now = plan_with(&state, &spec, &quant, 0, true).ctx;
    let est_now = catalog::fit_variant(&spec, variant, now, &budget).est_tps;
    let scale = installed.as_ref().and_then(|m| m.tps).filter(|t| *t > 0.0 && est_now > 0.0).map(|t| (t / est_now).clamp(0.25, 4.0));
    let reading = installed.as_ref().and_then(|m| m.prompt_tps).filter(|t| *t > 0.0);
    let max = spec.arch.max_ctx;
    let mut sizes: Vec<u32> = (13..=20).map(|p| 1u32 << p).filter(|c| *c <= max).collect();
    if max >= 8192 && !sizes.contains(&max) {
        sizes.push(max);
    }
    let options = sizes
        .into_iter()
        .map(|ctx| {
            let f = catalog::fit_variant(&spec, variant, ctx, &budget);
            let start = f.est_tps * scale.unwrap_or(1.0);
            WindowOption {
                ctx,
                placement: f.placement,
                est_tps: (start * 10.0).round() / 10.0,
                full_tps: catalog::speed_when_full(&spec, variant, ctx, start),
                read_full_secs: reading.map(|r| ctx as f64 / r),
                needed: f.needed_bytes,
            }
        })
        .collect();
    Ok(ContextOptions { chosen: chosen_window(&state), auto, max, options, tools: spec.tools, advanced: advanced::get(&state).engine.ctx, measured: scale.is_some() })
}

/// Sets the context window for chats (None: automatic). Models reload with
/// it on the next message.
#[tauri::command(async)]
fn set_context_window(state: AppStateRef, size: Option<u32>) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    match size {
        Some(n) => db::set(&conn, "context_window", &n.clamp(2048, 1_048_576))?,
        None => db::set(&conn, "context_window", &0u32)?,
    }
    db::log_action(&conn, "settings", "Changed the context window size");
    Ok(())
}

/// Compacts a chat now: the model summarizes it, and from here on it works
/// from the summary and the messages after it.
#[tauri::command]
async fn compact_chat(app: AppHandle, state: AppStateRef<'_>, chat_id: String) -> Result<(), String> {
    let state = state.inner().clone();
    let cipher = state.cipher()?;
    let Some((cancel, _guard)) = claim(&state.generations, &chat_id) else {
        return Err("Wait for the reply to finish, then compact.".into());
    };
    let (chat, profile) = {
        let conn = state.db.lock().unwrap();
        (db::chat(&conn, &cipher, &chat_id).ok_or("That chat no longer exists.")?, db::profile(&conn, &cipher))
    };
    let history = db::messages(&state.db.lock().unwrap(), &cipher, &chat_id);
    let covered = compact::pending(&history);
    if covered == 0 {
        return Err("There's nothing new to compact yet.".into());
    }
    let (ep, _, _) = llm_endpoint(&state, chat.model_id.clone(), false, chat_waiting(&app, &chat_id)).await?;
    app.emit("chat:status", json!({ "chat_id": chat_id, "status": "compacting" })).ok();
    let today = chrono::Local::now().format("%A, %B %-d, %Y").to_string();
    let (base, about) = chat::system_prompt(&profile, &today);
    let base = advanced::get(&state).base_prompt(base);
    let result = async {
        let summary = compact::summarize(&ep, &base, history, &cancel).await?;
        compact::save(&state.db.lock().unwrap(), &cipher, &chat_id, &summary, covered, false)?;
        if let Ok(info) = compact::fresh_context(&ep, &base, &about, &summary).await {
            state.contexts.lock().unwrap().insert(chat_id.clone(), info.clone());
            app.emit("chat:context", json!({ "chat_id": chat_id, "context": info })).ok();
        }
        state.log("chat", "Compacted a chat");
        Ok::<(), String>(())
    }
    .await;
    app.emit("chat:compacted", json!({ "chat_id": chat_id, "auto": false })).ok();
    app.emit("chat:status", json!({ "chat_id": chat_id, "status": "idle" })).ok();
    result
}

#[tauri::command(async)]
fn set_auto_model(state: AppStateRef, on: bool) -> Result<Settings, String> {
    state.cipher()?;
    db::update_settings(&state.db.lock().unwrap(), |s| s.auto_model = on)
}

/// A model that can be picked: a cloud model turned on in Settings → Cloud
/// models, a model taught here, or one on this PC.
pub(crate) fn model_known(conn: &rusqlite::Connection, model_id: &str) -> bool {
    if cloud::is_cloud(model_id) {
        cloud::resolve(conn, model_id).is_some()
    } else if let Some(aid) = finetune::parse_ref(model_id) {
        db::get::<Vec<finetune::Adapter>>(conn, "adapters").unwrap_or_default().iter().any(|a| a.id == aid)
    } else {
        db::installed_model(conn, model_id).is_some()
    }
}

#[tauri::command]
fn set_default_model(state: AppStateRef, model_id: String) -> Result<Settings, String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    if !model_known(&conn, &model_id) {
        return Err("That model isn't available.".into());
    }
    db::update_settings(&conn, |s| s.default_model = Some(model_id))
}

#[tauri::command(async)]
fn finish_onboarding(state: AppStateRef) -> Result<Settings, String> {
    let conn = state.db.lock().unwrap();
    db::update_settings(&conn, |s| s.onboarded = true)
}

#[tauri::command(async)]
fn get_profile(state: AppStateRef) -> Result<Profile, String> {
    let c = state.cipher()?;
    Ok(db::profile(&state.db.lock().unwrap(), &c))
}

#[tauri::command(async)]
fn set_profile(state: AppStateRef, profile: Profile) -> Result<(), String> {
    let c = state.cipher()?;
    db::set_profile(&state.db.lock().unwrap(), &c, &profile)
}

// ---------- chats ----------

#[tauri::command(async)]
fn list_chats(state: AppStateRef) -> Result<Vec<Chat>, String> {
    let c = state.cipher()?;
    Ok(db::list_chats(&state.db.lock().unwrap(), &c))
}

#[tauri::command(async)]
fn create_chat(state: AppStateRef) -> Result<Chat, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let model = db::settings(&conn).default_model;
    db::create_chat(&conn, &c, model)
}

/// Deletes a chat, and tells every window (one may have it open on its own).
#[tauri::command(async)]
fn delete_chat(app: tauri::AppHandle, state: AppStateRef, chat_id: String) -> Result<(), String> {
    delete_chat_now(&state, &chat_id)?;
    app.emit("chat:deleted", serde_json::json!({ "chat_id": chat_id })).ok();
    Ok(())
}

/// Deletes a chat: its messages and pasted pictures, its place in a folder,
/// its lock, and (first) its sharing with people.
pub(crate) fn delete_chat_now(state: &AppState, chat_id: &str) -> Result<(), String> {
    let c = state.cipher()?;
    if sync::people::shared_chat_ids(&state.db.lock().unwrap()).contains(chat_id) {
        let _ = sync::people::share::set_chat_sharing(state, &c, chat_id, Vec::new());
    }
    state.contexts.lock().unwrap().remove(chat_id);
    state.approvals.forget_chat(chat_id);
    preview::stop_chat(chat_id);
    let conn = state.db.lock().unwrap();
    db::delete_chat(&conn, chat_id)?;
    media::store::delete_attachments(&conn, &state.paths, chat_id);
    folders::forget_chat(&conn, chat_id);
    cleanup::forget_chat(&conn, chat_id);
    db::log_action(&conn, "chat", "Deleted a chat and its messages");
    Ok(())
}

/// Deletes the incognito chats (but `keep`) and everything they left
/// behind: pasted and made pictures with their descriptions, file backups,
/// what they sent under "What left this PC", folder links, locks, previews.
pub(crate) fn clear_incognito(state: &AppState, keep: Option<&str>) -> Result<(), String> {
    let ids = db::incognito_chat_ids(&state.db.lock().unwrap(), keep);
    for id in &ids {
        state.contexts.lock().unwrap().remove(id);
        state.approvals.forget_chat(id);
        preview::stop_chat(id);
    }
    let conn = state.db.lock().unwrap();
    for id in &ids {
        media::store::delete_chat_items(&conn, &state.paths, id);
        folders::forget_chat(&conn, id);
        cleanup::forget_chat(&conn, id);
        checkpoint::forget_chat(&conn, id);
    }
    if let Ok(c) = state.cipher() {
        outgoing::forget_chats(&conn, &c, &ids);
    }
    db::delete_incognito_chats(&conn, keep)?;
    Ok(())
}

#[tauri::command(async)]
fn rename_chat(state: AppStateRef, chat_id: String, title: String) -> Result<(), String> {
    let c = state.cipher()?;
    let title = title.trim();
    if title.is_empty() {
        return Err("A chat needs a name.".into());
    }
    db::set_chat_title(&state.db.lock().unwrap(), &c, &chat_id, title)
}

#[tauri::command(async)]
fn set_chat_web(state: AppStateRef, chat_id: String, web: bool) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    db::set_chat_web(&conn, &chat_id, web)?;
    db::log_action(&conn, "privacy", if web { "Turned on web access for one chat" } else { "Turned off web access for one chat" });
    Ok(())
}

#[tauri::command(async)]
fn set_chat_model(state: AppStateRef, chat_id: String, model_id: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    if !model_known(&conn, &model_id) {
        return Err(if cloud::is_cloud(&model_id) {
            "That cloud model isn't turned on. Check its provider and key in Settings → Cloud models.".into()
        } else {
            "That model isn't installed.".into()
        });
    }
    state.contexts.lock().unwrap().remove(&chat_id);
    db::set_chat_model(&conn, &chat_id, &model_id)?;
    drop(conn);
    // A cloud provider's chats may go to people (Settings, Cloud models).
    if let Ok(c) = state.cipher() {
        sync::people::share::follow_provider(&state, &c, &chat_id);
    }
    Ok(())
}

#[tauri::command(async)]
fn get_messages(state: AppStateRef, chat_id: String) -> Result<Vec<Message>, String> {
    let c = state.cipher()?;
    let messages = db::messages(&state.db.lock().unwrap(), &c, &chat_id);
    // Opened just now (the automatic cleanup counts from the last use).
    cleanup::touch_chat(&state, &chat_id);
    Ok(messages)
}

/// How full a chat's context window is: as counted at its last reply (this
/// run, or kept since), else worked out from its saved messages. Chats not
/// used since SulcusAI started all showed 0.
#[tauri::command(async)]
fn get_context(state: AppStateRef, chat_id: String) -> Option<ContextInfo> {
    if let Some(c) = state.contexts.lock().unwrap().get(&chat_id).cloned() {
        return Some(c);
    }
    if let Some(c) = db::get::<ContextInfo>(&state.db.lock().unwrap(), &format!("context:{chat_id}")) {
        return Some(c);
    }
    estimate_context(&state, &chat_id)
}

/// A chat's use of its window from the saved messages (about four
/// characters a token) since its last compaction, with the instructions and
/// tools as the last counted reply had them, at the window its model runs
/// with now.
fn estimate_context(state: &AppState, chat_id: &str) -> Option<ContextInfo> {
    let cipher = state.cipher().ok()?;
    let (chat, messages, last, profile) = {
        let conn = state.db.lock().unwrap();
        (db::chat(&conn, &cipher, chat_id)?, db::messages(&conn, &cipher, chat_id), db::get::<ContextInfo>(&conn, "context:last"), db::profile(&conn, &cipher))
    };
    if messages.is_empty() {
        return None;
    }
    let id = chat.model_id.or_else(|| state.settings().default_model)?;
    let spec = state.model_spec(&id)?;
    let quant = db::installed_model(&state.db.lock().unwrap(), &id).map(|m| m.quant).unwrap_or_else(|| spec.variants[0].quant.clone());
    let ctx = plan_with(state, &spec, &quant, 0, true).ctx;
    let tokens = |s: &str| (s.chars().count() as u32).div_ceil(4);
    let from = messages.iter().rposition(|m| m.meta.as_ref().is_some_and(|v| v.get("compacted").is_some())).unwrap_or(0);
    let history = messages[from..].iter().map(|m| tokens(&m.content) + m.tool_calls.iter().flatten().map(|c| tokens(&c.arguments) + 4).sum::<u32>() + 4).sum();
    let (base, about) = chat::system_prompt(&profile, "");
    let (system_tokens, profile_tokens, tools_tokens) = match last {
        Some(l) => (l.system_tokens, l.profile_tokens, l.tools_tokens),
        None => (tokens(&base), tokens(&about), 0),
    };
    Some(ContextInfo { ctx, system_tokens, profile_tokens, tools_tokens, history_tokens: history, reply_reserve: (ctx / 4).clamp(256, 4096), estimated: true, ..Default::default() })
}

#[tauri::command(async)]
fn stop_generation(state: AppStateRef, chat_id: String) {
    if let Some(flag) = state.generations.lock().unwrap().get(&chat_id) {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Replies in progress, by chat: what has streamed so far, so a chat opened
/// again mid-reply shows it (the reply keeps going when the user leaves).
fn live_replies() -> &'static Mutex<HashMap<String, serde_json::Value>> {
    static LIVE: std::sync::OnceLock<Mutex<HashMap<String, serde_json::Value>>> = std::sync::OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn track_live(event: &str, payload: &serde_json::Value) {
    let Some(chat) = payload.get("chat_id").and_then(|c| c.as_str()) else { return };
    let mut live = live_replies().lock().unwrap();
    match event {
        "chat:start" => {
            live.insert(chat.to_string(), json!({ "message_id": payload["message_id"], "content": "", "thinking": "", "status": "thinking" }));
        }
        "chat:delta" => {
            if let Some(r) = live.get_mut(chat) {
                for key in ["content", "thinking"] {
                    if let Some(add) = payload.get(key).and_then(|v| v.as_str()) {
                        let joined = format!("{}{add}", r[key].as_str().unwrap_or(""));
                        r[key] = json!(joined);
                    }
                }
                if payload.get("content").is_some_and(|v| v.is_string()) {
                    r["status"] = json!("writing");
                }
            }
        }
        "agent:step" => {
            live.insert(chat.to_string(), json!({ "message_id": null, "content": "", "thinking": "", "status": "working" }));
        }
        "chat:status" if payload["status"] == "loading" => {
            live.insert(chat.to_string(), json!({ "message_id": null, "content": "", "thinking": "", "status": "loading" }));
        }
        _ => {}
    }
}

/// Clears a chat's live reply when its turn ends, however it ends.
struct LiveGuard(String);
impl Drop for LiveGuard {
    fn drop(&mut self) {
        live_replies().lock().unwrap().remove(&self.0);
    }
}

/// What a chat's reply in progress has written so far (for people watching it).
pub(crate) fn live_reply(chat_id: &str) -> Option<serde_json::Value> {
    live_replies().lock().unwrap().get(chat_id).cloned()
}

/// While a turn runs, the people a chat is shared with see it busy.
struct SharedTurn(Arc<AppState>, String);
impl Drop for SharedTurn {
    fn drop(&mut self) {
        sync::people::turn_changed(&self.0, &self.1, false);
    }
}

/// The reply a chat is in the middle of, if any.
#[tauri::command(async)]
fn chat_live(state: AppStateRef, chat_id: String) -> Option<serde_json::Value> {
    if !state.generations.lock().unwrap().contains_key(&chat_id) {
        return None;
    }
    Some(live_replies().lock().unwrap().get(&chat_id).cloned().unwrap_or_else(|| json!({ "message_id": null, "content": "", "thinking": "", "status": "working" })))
}

#[tauri::command]
async fn send_message(app: AppHandle, state: AppStateRef<'_>, chat_id: String, text: String, images: Option<Vec<String>>) -> Result<Option<Message>, String> {
    // A new chat on a cloud model whose provider shares its chats with people.
    if let Ok(c) = state.cipher() {
        sync::people::share::follow_provider(&state, &c, &chat_id);
    }
    // The turn is built on the heap: a command starts on the main thread,
    // and in 0.9.26 sending a message overflowed its stack (the app closed).
    Box::pin(run_turn(&app, state.inner(), chat_id, text, images.unwrap_or_default(), None)).await
}

/// Also receives every event of a turn (voice mode speaks the reply from it).
pub(crate) type Tee = Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>;

/// Saves the user's message and runs one agent turn in the chat.
pub(crate) async fn run_turn(
    app: &AppHandle,
    state: &Arc<AppState>,
    chat_id: String,
    text: String,
    images: Vec<String>,
    tee: Option<Tee>,
) -> Result<Option<Message>, String> {
    run_turn_with(app, state, chat_id, text, images, tee, None).await
}

/// Another chat's assistant asking this chat something: it answers with the
/// model that chat has loaded (one model at a time), and can't ask on.
pub(crate) struct Asking {
    pub ep: engine::Endpoint,
    pub tools: bool,
    pub vision: bool,
    /// The asking chat: id and title.
    pub from: (String, String),
}

pub(crate) async fn run_asked(app: &AppHandle, state: &Arc<AppState>, chat_id: String, text: String, asking: Asking) -> Result<Option<Message>, String> {
    run_turn_with(app, state, chat_id, text, Vec::new(), None, Some(asking)).await
}

async fn run_turn_with(
    app: &AppHandle,
    state: &Arc<AppState>,
    chat_id: String,
    text: String,
    images: Vec<String>,
    tee: Option<Tee>,
    asking: Option<Asking>,
) -> Result<Option<Message>, String> {
    let mut text = text.trim().to_string();
    if text.is_empty() && images.is_empty() {
        return Err("Type a message first.".into());
    }
    if text.is_empty() {
        text = if images.len() == 1 { "What's in this picture?".into() } else { "What's in these pictures?".into() };
    }
    let cipher = state.cipher()?;
    let Some((cancel, _guard)) = claim(&state.generations, &chat_id) else {
        return Err("Still answering the last message.".into());
    };
    let _live = LiveGuard(chat_id.clone());
    sync::people::turn_changed(state, &chat_id, true);
    let _shared = SharedTurn(state.clone(), chat_id.clone());
    // Messages typed meanwhile: added to this reply, or queued for after it.
    let inbox = inbox::Opened::new(app, &chat_id);
    let asked_in = chat_id.clone();
    let _asked = asking.as_ref().map(|_| tools::chats::Asked::new(&chat_id));

    let turn_id = uuid::Uuid::new_v4().to_string();
    let (chat, profile) = {
        let conn = state.db.lock().unwrap();
        let chat = db::chat(&conn, &cipher, &chat_id).ok_or("That chat no longer exists.")?;
        if chat.model_id.is_none() {
            if let Some(m) = db::settings(&conn).default_model {
                db::set_chat_model(&conn, &chat_id, &m)?;
            }
        }
        (chat, db::profile(&conn, &cipher))
    };
    let mode = tools::Mode::parse(&chat.mode);
    // Pictures in this message or earlier ones: start the model with its image encoder.
    let pictures = !images.is_empty()
        || db::messages(&state.db.lock().unwrap(), &cipher, &chat_id).iter().any(|m| m.meta.as_ref().is_some_and(|v| v.get("images").is_some()));
    // Its turn for the graphics card (`lineup`), held while it replies.
    let mut _line: Option<lineup::Turn> = None;
    let (ep, use_tools, vision) = match &asking {
        Some(a) => (a.ep.clone(), a.tools, a.vision),
        None => {
            // Auto: a message the chat's model isn't the best installed one
            // for goes to the one that is, for this message only (`router`).
            // Not with pictures: the other model may not see them.
            let routed = if pictures { None } else { router::route(state, chat.model_id.as_deref(), &text) };
            if let Some(p) = &routed {
                state.log("chat", &format!("Auto: {} answers this message (better at {})", p.name, p.task.word()));
                app.emit("chat:routed", json!({ "chat_id": chat_id, "model_id": p.model_id, "name": p.name, "task": p.task.word() })).ok();
            }
            let model_id = routed.map(|p| p.model_id).or_else(|| chat.model_id.clone());
            let local = model_id.clone().or_else(|| state.settings().default_model).filter(|id| !cloud::is_cloud(id) && !sync::compute::is_compute(id));
            if let Some(id) = local {
                let entry = lineup::Entry::chat(&chat_id, &chat.title, &id, chat_card_need(state, &id));
                _line = Some(
                    lineup::wait(state, entry, cancel.clone(), |place| {
                        app.emit("chat:status", json!({ "chat_id": chat_id, "status": "waiting_card", "place": place })).ok();
                    })
                    .await?,
                );
            }
            let (ep, spec, _) = llm_endpoint(state, model_id, pictures, chat_waiting(app, &chat_id)).await?;
            let vision = if ep.cloud.is_some() { spec.vision.is_some() } else { state.engine.lock().await.sees() };
            (ep, spec.tools, vision)
        }
    };

    let today = chrono::Local::now().format("%A, %B %-d, %Y").to_string();
    let (base, about) = chat::system_prompt(&profile, &today);
    let base = format!("{}\n\n{}", advanced::get(state).base_prompt(base), chat::runs_where(&ep));

    // Nearly full: compact it (summarize the earlier part and keep going
    // here), or continue in a fresh chat, as set in Advanced.
    let last_context = state.contexts.lock().unwrap().get(&chat_id).cloned().or_else(|| estimate_context(state, &chat_id));
    let mut chat_id = chat_id;
    let long = advanced::get(state).long_chat();
    if let advanced::LongChat::Compact(at) = long {
        if !chat.incognito && handoff::needed(last_context.as_ref(), Some(at)) {
            app.emit("chat:status", json!({ "chat_id": chat_id, "status": "compacting" })).ok();
            let history = db::messages(&state.db.lock().unwrap(), &cipher, &chat_id);
            let covered = compact::pending(&history);
            if let Ok(summary) = compact::summarize(&ep, &base, history, &cancel).await {
                compact::save(&state.db.lock().unwrap(), &cipher, &chat_id, &summary, covered, true)?;
                state.contexts.lock().unwrap().remove(&chat_id);
                state.log("chat", "Compacted a long chat (it was nearly full)");
                app.emit("chat:compacted", json!({ "chat_id": chat_id, "auto": true })).ok();
            }
        }
    }
    if let advanced::LongChat::NewChat(at) = long {
        if !chat.incognito && handoff::needed(last_context.as_ref(), Some(at)) {
            app.emit("chat:status", json!({ "chat_id": chat_id, "status": "handoff" })).ok();
            let history = db::messages(&state.db.lock().unwrap(), &cipher, &chat_id);
            let summary = handoff::summarize(&ep, &base, &history, &cancel).await?;
            let new = handoff::continue_in_new_chat(&state.db.lock().unwrap(), &cipher, &chat, &summary)?;
            state.log("chat", "A long chat continued in a new chat with a summary");
            app.emit("chat:handoff", json!({ "from": chat_id, "to": new.id })).ok();
            chat_id = new.id;
        }
    }

    // What a cloud model's replies cost counts toward this chat.
    if let Some(t) = &ep.cloud {
        t.for_chat(&chat_id);
    }
    let first_message;
    {
        let conn = state.db.lock().unwrap();
        let first = db::message_count(&conn, &chat_id) == 0;
        first_message = first;
        db::add_message(&conn, &cipher, &Message {
            id: turn_id.clone(),
            chat_id: chat_id.clone(),
            role: "user".into(),
            content: text.clone(),
            created_at: db::now_ms(),
            meta: match &asking {
                Some(a) => Some(json!({ "from_chat": a.from.0, "from_title": a.from.1 })),
                None => (!images.is_empty()).then(|| json!({ "images": images })),
            },
            ..Default::default()
        })?;
        // A chat started by another chat keeps the title it was given.
        if first && asking.is_none() {
            db::set_chat_title(&conn, &cipher, &chat_id, &chat::title_from(&text))?;
        }
    }
    // Shows at once in a window on this chat, even when the message came
    // from the queue, another chat or the phone rather than that window.
    app.emit("chat:changed", json!({ "chat_id": chat_id })).ok();
    inbox::sent(app, &asked_in);

    // After the first reply, the model names the chat from the request.
    let name_it = (first_message && !chat.incognito && asking.is_none()).then(|| (ep.clone(), text.clone()));
    // Its thinking, in short, once it has answered: on this PC only (online
    // it would be another paid request).
    let sum_up = ep.cloud.is_none().then(|| (ep.clone(), turn_id.clone()));
    let emitter = app.clone();
    let watched = state.clone();
    let turn = agent::Turn {
        emit: Arc::new(move |event: &str, payload: serde_json::Value| {
            if let Some(t) = &tee {
                t(event, &payload);
            }
            track_live(event, &payload);
            sync::people::chat_event(&watched, event, &payload);
            emitter.emit(event, payload).ok();
        }),
        state: state.clone(),
        chat_id: chat_id.clone(),
        turn_id,
        cipher,
        ep,
        mode,
        use_tools,
        vision,
        unattended: false,
        pre_approved: None,
        base,
        about,
        cancel,
    };
    let result = turn.run().await;
    let (last, context, tps, cancelled) = match result {
        Ok(r) => (r.last, r.context, r.tps, r.cancelled),
        Err(e) => {
            app.emit("chat:done", json!({ "chat_id": chat_id, "error": e })).ok();
            return Err(e);
        }
    };
    state.contexts.lock().unwrap().insert(chat_id.clone(), context.clone());
    // Kept, so the meter has it when the chat is opened after a restart (and
    // the instructions' share, for chats it works out from their messages).
    {
        let conn = state.db.lock().unwrap();
        let _ = db::set(&conn, &format!("context:{chat_id}"), &context);
        let _ = db::set(&conn, "context:last", &context);
    }
    if let Some((ep, request)) = name_it.filter(|_| !cancelled) {
        let (state, app, chat_id) = (state.clone(), app.clone(), chat_id.clone());
        tauri::async_runtime::spawn(async move {
            if let Some(title) = chat::summary_title(&ep, &request).await {
                if let Ok(c) = state.cipher() {
                    if db::set_chat_title(&state.db.lock().unwrap(), &c, &chat_id, &title).is_ok() {
                        app.emit("chat:titled", json!({ "chat_id": chat_id, "title": title })).ok();
                    }
                }
            }
        });
    }
    if let Some((ep, asked)) = sum_up.filter(|_| !cancelled) {
        let (state, app, chat_id) = (state.clone(), app.clone(), chat_id.clone());
        tauri::async_runtime::spawn(async move {
            sum_up_thinking(&state, &app, &ep, &chat_id, &asked).await;
            remember_from_reply(&state, &app, &ep, &chat_id, &asked).await;
        });
    }
    app.emit("chat:done", json!({ "chat_id": chat_id, "message": last, "tps": tps, "context": context, "cancelled": cancelled }))
        .ok();
    inbox.finish(state, cancelled);
    Ok(last)
}

/// Lasting facts from the reply to `asked`, saved without being asked when
/// the model finds any (Memory: "Remember things from chats by itself"):
/// not when it saved one with remember this turn, in incognito chats, or
/// with memory off. A project chat's facts about the project stay in it.
async fn remember_from_reply(state: &AppState, app: &AppHandle, ep: &engine::Endpoint, chat_id: &str, asked: &str) {
    let Ok(c) = state.cipher() else { return };
    if !state.settings().memory_enabled {
        return;
    }
    let (chat, messages, known) = {
        let conn = state.db.lock().unwrap();
        if !memory::auto_on(&conn) || !features::is_on(&conn, features::Feature::Memory) {
            return;
        }
        let Some(chat) = db::chat(&conn, &c, chat_id) else { return };
        (chat, db::messages(&conn, &c, chat_id), memory::list(&conn, &c, None))
    };
    let turn: Vec<&Message> = messages.iter().skip_while(|m| m.id != asked).collect();
    let Some(user) = turn.first().filter(|m| m.role == "user") else { return };
    if chat.incognito || turn.iter().any(|m| m.tool_calls.iter().flatten().any(|t| t.name == "remember")) {
        return;
    }
    let reply = turn.iter().filter(|m| m.role == "assistant").map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
    let Ok(raw) = chat::complete(ep, memory::auto_prompt(&known, &user.content, &reply), json!({ "response_format": memory::auto_format() }), 300).await else { return };
    let mut saved = Vec::new();
    {
        let conn = state.db.lock().unwrap();
        for (fact, about_user) in memory::auto_facts(&raw) {
            let project = if about_user { None } else { chat.project_id.as_deref() };
            if let Ok((m, true)) = memory::add(&conn, &c, &fact, project, Some(chat_id)) {
                saved.push(m.content);
            }
        }
    }
    if !saved.is_empty() {
        state.log("chat", &format!("Remembered {} thing(s) from a chat by itself", saved.len()));
        app.emit("memory:auto", json!({ "chat_id": chat_id, "saved": saved })).ok();
    }
}

/// The thinking of the reply to `asked` (the user's message), every step of
/// it, in a few short points: kept with the answer (the reply's last
/// message), shown between the whole thought process and the answer.
async fn sum_up_thinking(state: &AppState, app: &AppHandle, ep: &engine::Endpoint, chat_id: &str, asked: &str) {
    let Ok(c) = state.cipher() else { return };
    let reply: Vec<Message> = db::messages(&state.db.lock().unwrap(), &c, chat_id).into_iter().skip_while(|m| m.id != asked).skip(1).filter(|m| m.role == "assistant").collect();
    let thinking = reply.iter().filter_map(|m| m.thinking.as_deref()).fold(String::new(), |all, t| chat::joined(&all, t));
    // A short think is its own summary.
    let Some(last) = reply.last().filter(|_| thinking.trim().chars().count() >= 400) else { return };
    let Some(summary) = chat::thought_summary(ep, &thinking).await else { return };
    let mut meta = last.meta.clone().filter(|v| v.is_object()).unwrap_or_else(|| json!({}));
    meta["thought_summary"] = json!(summary);
    if db::set_message_meta(&state.db.lock().unwrap(), &c, &last.id, &meta).is_ok() {
        app.emit("chat:thought", json!({ "chat_id": chat_id })).ok();
    }
}

/// A model that didn't load: why, said plainly (what the engine's log
/// shows, else what the model's size says), and the whole story in
/// crashes.log with what each line of the engine's log means.
fn load_failed(state: &AppState, spec: &ModelSpec, quant: &str, ctx: u32, layers: u32, want: memcheck::Need, e: String) -> String {
    if !e.starts_with(engine::STOPPED_LOADING) {
        crate::applog::line("error", &format!("{} didn't load: {e}", spec.name));
        return e;
    }
    let log = crate::applog::tail(&state.paths.engine_log(), 64 * 1024);
    let last: Vec<&str> = log.lines().rev().take(60).collect::<Vec<_>>().into_iter().rev().collect();
    let last = last.join("\n");
    let gb = |b: u64| format!("{:.1} GB", b as f64 / 1e9);
    let budget = state.budget();
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let (ram_total, ram_free) = (sys.total_memory(), sys.available_memory());
    let size = spec.variant(quant).map_or(0, |v| v.size);
    let reason = match crate::applog::diagnose(&last) {
        Some(m) => m.to_string(),
        // Nothing in the log says: what the size says.
        None if size > ram_total + budget.vram => format!(
            "It's too large for this PC: the model alone is {}, and this PC has {} of memory and {} on the graphics card. Pick a smaller model or a smaller version of it (a lower quant).",
            gb(size), gb(ram_total), gb(budget.vram)
        ),
        None if want.ram > ram_free => format!(
            "This PC didn't have enough free memory: loading it this way needs about {} and {} was free. Close other apps, choose a smaller context window, or a smaller version of the model.",
            gb(want.ram), gb(ram_free)
        ),
        None if budget.vram > 0 && want.vram > budget.vram => format!(
            "It's too large for the graphics card as set up: it needed about {} there and the card has {}. Lower the GPU limit or the context window at the top of the chat, or pick a smaller version.",
            gb(want.vram), gb(budget.vram)
        ),
        None => "The engine closed without saying why. The usual causes are memory running out as it loaded (close other apps, or pick a smaller version), a graphics driver problem (update the driver), or an antivirus stopping the engine. Settings, Logs, crashes.log has the details.".to_string(),
    };
    crate::applog::crash(
        &format!("{} didn't load: the engine stopped while loading it", spec.name),
        &format!(
            "Why: {reason}\n\nThe model: {} ({quant}), {} file, {ctx}-token context window, {layers} layers planned on the graphics card.\n\
             This PC: {} memory ({} free), {} usable on the graphics card ({} card{}).\nPlanned use: about {} of memory and {} on the card.\n\n\
             The engine's last lines (engine.log), with what the known ones mean:\n{}",
            spec.name,
            gb(size),
            gb(ram_total),
            gb(ram_free),
            gb(budget.vram),
            budget.cards,
            if budget.cards == 1 { "" } else { "s" },
            gb(want.ram),
            gb(want.vram),
            crate::applog::annotated(&last)
        ),
    );
    format!("{} couldn't load: the engine stopped while loading it. {reason}", spec.name)
}

/// The graphics memory a reply on this model takes (its weights and its
/// window, as planned), for its place in the card's line; 0 when unknown.
fn chat_card_need(state: &AppState, model_id: &str) -> u64 {
    let id = finetune::parse_ref(model_id).and_then(|a| finetune::adapter(state, a)).map_or(model_id.to_string(), |a| a.base);
    let Some(spec) = state.model_spec(&id) else { return 0 };
    let quant = db::installed_model(&state.db.lock().unwrap(), &id).map(|m| m.quant).unwrap_or_else(|| spec.variants[0].quant.clone());
    let Some(v) = spec.variant(&quant) else { return 0 };
    let ctx = plan_with(state, &spec, &quant, 0, true).ctx;
    v.size + catalog::kv_cache_bytes(&spec.arch, ctx) + (700 << 20)
}

/// A chat's status while its model is on the way: loading, or its place in
/// line behind other chats using another model.
pub(crate) fn chat_waiting<'a>(app: &'a AppHandle, chat_id: &'a str) -> impl FnMut(engine::Wait) + 'a {
    move |w| {
        let status = match w {
            engine::Wait::Loading => json!({ "chat_id": chat_id, "status": "loading" }),
            engine::Wait::Line(place, other) => json!({ "chat_id": chat_id, "status": "queued", "place": place, "detail": other }),
        };
        app.emit("chat:status", status).ok();
    }
}

/// How long a request for another model waits for the replies using the
/// loaded one to finish (an agent task can take minutes).
const WAIT_FOR_REPLIES: std::time::Duration = std::time::Duration::from_secs(180);

/// How long a request waits in line for another model (the chats ahead of
/// it can each run a long agent task).
const WAIT_IN_LINE: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// The stop flags of the replies running now.
fn replies(state: &AppState) -> Vec<Arc<AtomicBool>> {
    state.generations.lock().unwrap().values().cloned().collect()
}

/// Starts (or reuses) the engine for a model: the given one, else the
/// default. Returns the endpoint and the model. Another model isn't stopped
/// while replies are using it: one model runs at a time, so this waits in
/// line (`on_wait` hears its place) for them, then loads it.
pub(crate) async fn llm_endpoint(
    state: &Arc<AppState>,
    model_id: Option<String>,
    pictures: bool,
    on_wait: impl FnMut(engine::Wait),
) -> Result<(engine::Endpoint, ModelSpec, InstalledModel), String> {
    // Replies running before this request that weren't given a model yet:
    // this request's own, when a chat makes it.
    let before = engine::USERS.lock().unwrap().unplaced(&replies(state));
    let task = tokio::task::try_id();
    let id = {
        let conn = state.db.lock().unwrap();
        let default = db::settings(&conn).default_model;
        // A chat whose model was removed (by hand or by the cleanup) uses the default.
        let gone = |id: &String| !cloud::is_cloud(id) && !sync::compute::is_compute(id) && finetune::parse_ref(id).is_none() && db::installed_model(&conn, id).is_none();
        let chosen = match model_id {
            Some(id) if gone(&id) && default.as_ref().is_some_and(|d| !gone(d)) => default.clone(),
            other => other,
        };
        chosen.or(default).ok_or("Install a model first: open Models and pick one.")?
    };
    if cloud::is_cloud(&id) {
        *state.engine_used.lock().unwrap() = std::time::Instant::now();
        let found = cloud::endpoint(state, &id, &state.cipher()?)?;
        engine::USERS.lock().unwrap().place(&replies(state), &before, "", task);
        return Ok(found);
    }
    // A model on the compute PC: its turn is taken there.
    if sync::compute::is_compute(&id) {
        let found = sync::compute::endpoint(state, &id).await?;
        engine::USERS.lock().unwrap().place(&replies(state), &before, "", task);
        return Ok(found);
    }
    if finetune::training() {
        return Err("A model is being taught right now and has the graphics card. Chats on this PC's models work again when it's done (or stop it in Settings → Advanced). Cloud models still work.".into());
    }
    // A taught model: its base model with the adapter on top.
    let taught = match finetune::parse_ref(&id) {
        Some(aid) => Some(finetune::adapter(state, aid).ok_or("That taught model was removed. Pick another one.")?),
        None => None,
    };
    let base_id = taught.as_ref().map_or(id.clone(), |a| a.base.clone());
    let installed = db::installed_model(&state.db.lock().unwrap(), &base_id).ok_or("This chat's model isn't installed anymore. Pick another one.")?;
    cleanup::touch_model(state, &installed.model_id);
    let mut spec = state.model_spec(&installed.model_id).ok_or("This model is no longer available. Pick another one.")?;
    if let Some(a) = &taught {
        spec.name = format!("{} + {}", spec.name, a.name);
    }
    let lora = taught.as_ref().map(|a| std::path::PathBuf::from(&a.path));
    recheck_if_card_missing(state).await;
    let limits = state.limits();
    let backend = engine::backend_for(&state.budget())?;
    let exe = match engine::installed_server(&state.paths, &state.catalog.engine, backend) {
        Some(exe) => exe,
        // A model found on this PC before the engine was ever downloaded.
        None => {
            let asset = state.catalog.engine.assets.get(backend).map(|a| a.url.clone()).unwrap_or_default();
            state.log("network", &format!("Downloading the llama.cpp engine ({backend}) from {}", host_of(&asset)));
            let app = state.app.get().cloned();
            let progress = |received: u64, total: u64| {
                if let Some(app) = &app {
                    app.emit("engine:download", json!({ "received": received, "total": total })).ok();
                }
            };
            let exe = engine::ensure_installed(&state.paths, &state.catalog.engine, backend, &AtomicBool::new(false), progress).await;
            if let Some(app) = &app {
                app.emit("engine:download", json!({ "done": true })).ok();
            }
            exe?
        }
    };
    *state.engine_used.lock().unwrap() = std::time::Instant::now();
    let model_path = model_file(&state.paths, &installed);
    // A damaged or unfinished file is said plainly, not as the engine's log.
    if let Some(why) = model_file_problem(&model_path, installed.size) {
        state.log("model", &format!("{}'s model file is unusable ({why}): {}", spec.name, model_path.display()));
        return Err(format!("{}'s model file is damaged: {why}. Open Models and press Download again on it.", spec.name));
    }
    let encoder = if pictures { ensure_vision(state, &spec).await? } else { vision_file(&state.paths, &spec) };
    // Said once each time it changes.
    let mut on_wait = on_wait;
    let mut told: Option<engine::Wait> = None;
    let mut tell = move |w: engine::Wait| {
        if told.as_ref() != Some(&w) {
            told = Some(w.clone());
            on_wait(w);
        }
    };
    let mut ticket: Option<engine::Ticket> = None;
    let asked = std::time::Instant::now();
    let mut ep = loop {
        let mut engine = state.engine.lock().await;
        // Keep the image encoder once it's loaded, so chats don't flip it on and off.
        let keep = engine.endpoint_for(&installed.model_id, lora.as_deref()).is_some() && engine.sees();
        let mmproj = encoder.as_deref().filter(|_| pictures || keep);
        let encoder_size = mmproj.and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
        let plan = launch_plan(state, &spec, &installed.quant, encoder_size);
        let launch = LaunchSpec {
            exe: &exe,
            model_path: &model_path,
            model_id: &installed.model_id,
            quant: &installed.quant,
            ctx: plan.ctx,
            gpu_layers: plan.gpu_layers,
            threads: plan.threads,
            low_priority: limits.low_priority,
            log: &state.paths.engine_log(),
            mmproj,
            batch: plan.batch,
            lora: lora.as_deref(),
        };
        // Other replies still using the model loaded now: it isn't stopped
        // under them (they'd be cut off mid-reply).
        let loaded = engine.current();
        let held = loaded.as_ref().map_or(0, |(_, ep)| engine::held(&ep.key));
        let busy = held + loaded.as_ref().map_or(0, |(_, ep)| engine::USERS.lock().unwrap().others(&replies(state), &before, &ep.key, task));
        let was = loaded.as_ref().map(|(_, ep)| ep.key.clone());
        if let Some(ep) = engine.endpoint_for(&installed.model_id, lora.as_deref()) {
            // Same model: new limits (mode, heat) apply between replies.
            if busy > 0 || engine.matches_tuned(&launch, plan.tuning) {
                break ep;
            }
        } else if busy > 0 || ticket.as_ref().map_or(engine::waiting() > 0, |t| t.place() > 1) {
            // Replies on the model running now, or others who asked for
            // another model first: this waits its turn in line.
            drop(engine);
            if before.iter().any(|f| f.load(Ordering::Relaxed)) {
                return Err("Stopped before the model loaded.".into());
            }
            let other = loaded.map(|(id, _)| state.model_spec(&id).map_or(id, |s| s.name)).unwrap_or_default();
            if asked.elapsed() >= WAIT_IN_LINE {
                if held == busy && busy > 0 {
                    return Err(format!("A background job (the mail report, meeting notes or a translation) is still using {other}. Try again when it's done."));
                }
                return Err(format!("This waited 30 minutes in line while other chats answered with {other}. Try again when they're done, or stop them first."));
            }
            let place = ticket.get_or_insert_with(engine::Ticket::new).place();
            tell(engine::Wait::Line(place, other));
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            continue;
        }
        // Room for it right now (a game may be holding the memory)? The
        // engine isn't held while the person decides.
        let replacing = engine.loaded().map(|(pid, id, quant, ctx, layers)| {
            (pid, state.model_spec(&id).map(|s| memcheck::need(&s, &quant, ctx, layers, 0, false)).unwrap_or_default())
        });
        drop(engine);
        let want = memcheck::need(&spec, &installed.quant, plan.ctx, plan.gpu_layers, encoder_size, plan.tuning.kv_q8);
        memcheck::before_loading(state, &spec, want, replacing).await?;
        let mut engine = state.engine.lock().await;
        // Another model was loaded, or a reply started, meanwhile (the
        // question can wait minutes): look again before stopping it.
        let now = engine.current().map(|(_, ep)| ep.key);
        if now != was || now.is_some_and(|key| engine::held(&key) + engine::USERS.lock().unwrap().others(&replies(state), &before, &key, task) > 0) {
            continue;
        }
        tell(engine::Wait::Loading);
        let ep = match engine.ensure_tuned(launch, plan.tuning, state.job()).await {
            Ok(ep) => ep,
            Err(e) => return Err(load_failed(state, &spec, &installed.quant, plan.ctx, plan.gpu_layers, want, e)),
        };
        engine.asked_as(&id);
        break ep;
    };
    engine::USERS.lock().unwrap().place(&replies(state), &before, &ep.key, task);
    watch_work(state, &ep);
    let advanced = advanced::get(state);
    ep.extra = advanced.request_extra();
    // Thinking counts against the reply's allowance: a long think would
    // otherwise use it all and leave an empty reply.
    if thinks(&spec, advanced.processing.thinking) && ep.extra.get("max_tokens").is_none() {
        ep.extra["max_tokens"] = json!((ep.ctx / 2).clamp(4096, 16_384));
    }
    // gpt-oss (Harmony) reads its reasoning effort from the request: never
    // sent before, so its template's default applied whatever was asked.
    if harmony(&spec) {
        let effort = advanced.reasoning_effort();
        ep.extra["reasoning_effort"] = json!(effort);
        if !ep.extra["chat_template_kwargs"].is_object() {
            ep.extra["chat_template_kwargs"] = json!({});
        }
        ep.extra["chat_template_kwargs"]["reasoning_effort"] = json!(effort);
    }
    Ok((ep, spec, installed))
}

/// gpt-oss, or a model built on it (huihui-ai's abliterated one): it speaks
/// OpenAI's Harmony format, with channels and `to=functions.x` tool calls.
pub(crate) fn harmony(spec: &ModelSpec) -> bool {
    let s = format!("{} {} {}", spec.id, spec.name, spec.source).to_lowercase();
    s.contains("gpt-oss") || s.contains("gpt_oss") || s.contains("gptoss")
}

/// The model thinks before it answers (Qwen3, gpt-oss, DeepSeek R1, QwQ),
/// as set in Advanced or, by default, as the model does.
pub(crate) fn thinks(spec: &ModelSpec, setting: advanced::Thinking) -> bool {
    match setting {
        advanced::Thinking::Off => false,
        advanced::Thinking::On => true,
        advanced::Thinking::Auto => {
            let name = format!("{} {}", spec.id, spec.name).to_lowercase();
            // Qwen3's own models think unless they're an instruct-only version.
            let hybrid = name.contains("qwen3") && !["instruct", "2507", "coder", "-vl", " vl"].iter().any(|w| name.contains(w));
            spec.tags.iter().any(|t| t == "thinking") || hybrid || ["thinking", "reasoning", "qwq", "-r1", "gpt-oss", "magistral"].iter().any(|w| name.contains(w))
        }
    }
}

/// The loaded model `watch_work` watches (by its key).
static WATCHING: Mutex<Option<String>> = Mutex::new(None);

/// While a loaded model runs, checks every 15 seconds whether it's working
/// (a reply, a translation, meeting notes, a program using the local API)
/// and counts that as use, so the idle unload stops only a model nobody is
/// using: a long job asks for the model once, then keeps it busy for many
/// minutes.
fn watch_work(state: &Arc<AppState>, ep: &engine::Endpoint) {
    {
        let mut w = WATCHING.lock().unwrap();
        if w.as_deref() == Some(ep.key.as_str()) {
            return;
        }
        *w = Some(ep.key.clone());
    }
    let (state, ep) = (state.clone(), ep.clone());
    tauri::async_runtime::spawn(async move {
        let client = net::local_client();
        let mine = || WATCHING.lock().unwrap().as_deref() == Some(ep.key.as_str());
        while mine() {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            let asked = client.get(ep.url("/slots")).bearer_auth(&ep.key).timeout(std::time::Duration::from_secs(5)).send().await;
            let working = match asked.and_then(|r| r.error_for_status()) {
                Ok(r) => r.json::<serde_json::Value>().await.ok().and_then(|v| v.as_array().map(|slots| slots.iter().any(|s| s["is_processing"] == true))),
                // Too busy to answer at once: it's working.
                Err(e) if e.is_timeout() => Some(true),
                Err(_) => None,
            };
            match working {
                Some(true) => *state.engine_used.lock().unwrap() = std::time::Instant::now(),
                Some(false) => {}
                // Not answering: stopped, unless it's starting again under
                // the same key (new limits) or the engine is busy loading.
                None => {
                    let still = match state.engine.try_lock() {
                        Ok(mut e) => e.current().is_some_and(|(_, now)| now.key == ep.key),
                        Err(_) => true,
                    };
                    if !still {
                        break;
                    }
                }
            }
        }
        let mut w = WATCHING.lock().unwrap();
        if w.as_deref() == Some(ep.key.as_str()) {
            *w = None;
        }
    });
}

/// For background work (meeting notes, translation): the model already
/// loaded if there is one, so a chat's model isn't swapped out mid-reply.
/// The model stays theirs (not stopped or relaunched by a chat) while the
/// returned hold lives.
pub(crate) async fn background_endpoint(state: &Arc<AppState>) -> Result<(engine::Endpoint, ModelSpec, engine::Hold), String> {
    // Use the model as it is loaded: relaunching it with new limits here
    // could cut off a chat that is mid-reply.
    let loaded = {
        let mut engine = state.engine.lock().await;
        // Held before the engine is let go, so a chat can't swap it in between.
        engine.current().and_then(|(id, ep)| Some((state.model_spec(&id)?, engine::Hold::new(&ep.key), ep)))
    };
    if let Some((spec, hold, ep)) = loaded {
        *state.engine_used.lock().unwrap() = std::time::Instant::now();
        watch_work(state, &ep);
        return Ok((ep, spec, hold));
    }
    let (ep, spec, _) = llm_endpoint(state, None, false, |_| {}).await?;
    let hold = engine::Hold::new(&ep.key);
    Ok((ep, spec, hold))
}

/// The chat model for work the person started outside a chat (a podcast's
/// script, a code review): the one they picked, loaded and held, else the
/// one already loaded or the default (`background_endpoint`).
pub(crate) async fn endpoint_for(state: &Arc<AppState>, model_id: Option<&str>) -> Result<(engine::Endpoint, ModelSpec, engine::Hold), String> {
    match model_id.filter(|m| !m.is_empty()) {
        None => background_endpoint(state).await,
        Some(id) => {
            let (ep, spec, _) = llm_endpoint(state, Some(id.to_string()), false, |_| {}).await?;
            let hold = engine::Hold::new(&ep.key);
            Ok((ep, spec, hold))
        }
    }
}

// ---------- engine ----------

#[tauri::command]
async fn engine_status(state: AppStateRef<'_>) -> Result<EngineStatus, String> {
    Ok(state.engine.lock().await.status())
}

/// Loads the default model now, so the first message doesn't wait for it.
#[tauri::command]
async fn load_default_model(state: AppStateRef<'_>) -> Result<(), String> {
    let state = state.inner().clone();
    llm_endpoint(&state, None, false, |_| {}).await?;
    Ok(())
}

#[tauri::command]
async fn unload_model(state: AppStateRef<'_>) -> Result<(), String> {
    state.engine.lock().await.stop().await;
    Ok(())
}

fn open_vault(paths: &Paths) -> Result<Vault, String> {
    Vault::open(&paths.data.join("keys.json"), crypto::platform_protector(&paths.data))
}

/// Opens a file, folder or web address with the system: Explorer on
/// Windows, `open` on macOS, `xdg-open` on Linux.
pub(crate) fn open_with_system(target: &std::ffi::OsStr) -> Result<(), String> {
    #[cfg(windows)]
    if let Some(url) = target.to_str().filter(|t| t.starts_with("https://") || t.starts_with("http://")) {
        return open_url_windows(url);
    }
    let program = if cfg!(windows) {
        "explorer.exe"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(target).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Web addresses go to the default browser through the shell: Explorer
/// opens a File Explorer window instead when an address is long, such as
/// a sign-in page's.
#[cfg(windows)]
fn open_url_windows(url: &str) -> Result<(), String> {
    use windows::core::{w, HSTRING};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let url = HSTRING::from(url);
    // The shell wants a single-threaded COM apartment: use a thread of its own.
    std::thread::spawn(move || {
        // SAFETY: plain Win32 calls with valid, NUL-terminated strings; COM
        // is initialized for this thread only and released before it ends.
        unsafe {
            let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
            let r = ShellExecuteW(None, w!("open"), &url, None, None, SW_SHOWNORMAL);
            if com {
                CoUninitialize();
            }
            // Values above 32 mean success.
            if r.0 as isize > 32 { Ok(()) } else { Err(format!("Couldn't open your browser (error {}).", r.0 as isize)) }
        }
    })
    .join()
    .map_err(|_| "Couldn't open your browser.".to_string())?
}

/// Connector mode for the browser extension (see bridge.rs).
pub fn run_bridge_host() {
    bridge::run_host()
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(quick::plugin())
        .on_window_event(|w, e| {
            quick::on_main_event(w, e);
            people_window::on_main_event(w, e);
            side_windows::on_main_event(w, e);
        })
        .setup(|app| {
            sync::compute::started();
            // Local, not roaming: models are gigabytes and must never sync.
            // SULCUSAI_DATA_DIR overrides it (testing, portable installs).
            let data = match std::env::var_os("SULCUSAI_DATA_DIR") {
                Some(dir) => std::path::PathBuf::from(dir),
                None => app.path().app_local_data_dir()?,
            };
            let paths = Paths::new(data)?;
            // A restore unpacked last time is swapped in before anything opens.
            // The app's log first, so everything startup does is in it.
            applog::init(&paths.logs);
            backup::apply_pending(&paths.data)?;
            let conn = db::open(&paths.db)?;
            let vault = open_vault(&paths)?;
            // The graphics card picked for models (by name), before the PC is checked.
            hardware::set_main(db::get::<Option<String>>(&conn, "main_gpu").flatten());
            let hardware = hardware::detect(&paths.models);
            let catalog = Catalog::bundled();
            natural::refresh(&paths, &catalog.voices);
            let state = AppState {
                db: Mutex::new(conn),
                vault: Mutex::new(vault),
                catalog,
                hardware: RwLock::new(hardware),
                engine: tokio::sync::Mutex::new(Engine::default()),
                installs: Mutex::new(HashMap::new()),
                generations: Mutex::new(HashMap::new()),
                contexts: Mutex::new(HashMap::new()),
                approvals: agent::Approvals::default(),
                mcp: tokio::sync::Mutex::new(HashMap::new()),
                speech: speech::Speech::default(),
                player: audio::Player::new(),
                heat: perf::Heat::default(),
                bridge: bridge::Bridge::default(),
                app: std::sync::OnceLock::new(),
                engine_used: Mutex::new(std::time::Instant::now()),
                #[cfg(windows)]
                job: winjob::Job::kill_on_close().ok(),
                paths,
            };
            if let Ok(c) = state.cipher() {
                state.migrate_plaintext(&c);
            }
            // Fully offline is known before anything starts going online.
            offline::load(&state.db.lock().unwrap());
            checkpoint::prune(&state.db.lock().unwrap());
            db::prune_actions(&state.db.lock().unwrap(), 90);
            // What a crash may have left: picture/video work, teaching data.
            media::clean_work(&state.paths);
            finetune::clean_leftovers(&state.paths);
            let _ = clear_incognito(&state, None);
            let _ = db::delete_empty_chats(&state.db.lock().unwrap());
            let state = Arc::new(state);
            let _ = state.app.set(app.handle().clone());
            // What this PC is (for reading the logs later).
            {
                let hw = state.hardware.read().unwrap();
                applog::line(
                    "app",
                    &format!(
                        "Started SulcusAI {} on {} · {} ({} threads) · {:.0} GB memory · graphics: {} · models in {}",
                        env!("CARGO_PKG_VERSION"),
                        hw.os,
                        hw.cpu_name,
                        hw.cpu_threads,
                        hw.ram_total as f64 / 1e9,
                        graphics_line(&hw),
                        state.paths.models.display()
                    ),
                );
            }
            // Crash reports: the failure hook and how the last run ended (always logged here; sent only if turned on).
            crash::start(&state);
            devmode::start(&state);
            cleanup::start(&state);
            backup::finish_restore(&state);
            api_server::apply(state.clone());
            // Models already on this PC, from an earlier install or another app.
            std::thread::spawn({
                let (state, app) = (state.clone(), app.handle().clone());
                move || announce_found(&app, &find_models_now(&state, false).added)
            });
            speech::start_idle_unloader(state.clone());
            app.manage(state);
            schedule::start(app.handle().clone());
            notes::start_reminders(app.handle().clone());
            perf::start_monitor(app.handle().clone());
            mail::start_sync(app.handle().clone());
            quick::apply(app.handle());
            bridge::apply(app.handle());
            calendar::start_sync(app.handle().clone());
            updates::start(app.handle().clone());
            privacy::start(app.handle().clone(), app.state::<Arc<AppState>>().inner().clone());
            sync::start(app.handle().clone());
            phone::apply(app.state::<Arc<AppState>>().inner().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            perf::perf_view,
            perf::model_idle,
            perf::gpu_now,
            perf::gpu_limit,
            crash::crash_reports_view,
            crash::set_crash_reports,
            crash::dismiss_crash_reports,
            crash::send_crash_report,
            crash::crash_ping,
            crash::report_window_error,
            feedback::send_feedback,
            offline::offline_view,
            offline::set_fully_offline,
            perf::set_gpu_limit,
            memcheck::answer_memory_warning,
            cardfirst::card_asks,
            cardfirst::card_ask_answer,
            cardfirst::processor_fallback,
            cardfirst::set_processor_fallback,
            drafts::drafts_keep,
            drafts::set_drafts_keep,
            drafts::drafts,
            drafts::draft_set,
            drivers::driver_check,
            drivers::driver_put_off,
            lineup::lineup_list,
            lineup::lineup_move,
            lineup::lineup_start_now_options,
            lineup::lineup_start_now,
            lineup::lineup_remove,
            placement::gpu_menu,
            placement::set_main_gpu,
            security::set_keep_working,
            browser::open_browser,
            browsers::browsers_view,
            location::detect_location,
            bridge::bridge_status,
            bridge::show_extension_folder,
            browsers::set_main_browser,
            browsers::open_web,
            browsers::open_outside,
            viewer::viewer_show,
            viewer::viewer_area,
            viewer::viewer_hide,
            viewer::viewer_go,
            viewer::viewer_url,
            viewer::viewer_exists,
            viewer::viewer_close,
            people_window::open_people_window,
            side_windows::open_chat_window,
            side_windows::open_studio_window,
            side_windows::open_song_studio_window,
            side_windows::show_main_page,
            media::own_voice::own_voices,
            media::own_voice::own_voice_create,
            media::own_voice::own_voice_rename,
            media::own_voice::own_voice_delete,
            media::own_voice::own_voice_record,
            media::own_voice::own_voice_stop,
            media::own_voice::own_voice_cancel,
            media::own_voice::own_voice_take,
            media::own_voice::own_voice_take_delete,
            media::own_voice::own_voice_upload,
            media::own_voice::own_voice_try,
            media::own_voice::own_voice_design,
            media::own_voice::own_voice_design_cancel,
            media::own_voice::own_voice_design_keep,
            media::own_voice::own_voice_check,
            media::own_voice::own_voice_create_from,
            media::own_voice::own_voice_checked_take,
            docs::open_document,
            docs::show_in_folder,
            quick::quick_clipboard,
            quick::quick_hide,
            quick::quick_resize,
            quick::quick_open_in_app,
            quick::quick_screenshot,
            media::media_view,
            media::install_media_model,
            media::media_hub_search,
            media::media_set_default,
            cleanup::clean_view,
            cleanup::set_auto_clean,
            cleanup::clean_now,
            cleanup::set_kept,
            cleanup::keep_locks,
            cleanup::cache_view,
            cleanup::clear_cache,
            folders::folders_view,
            folders::create_folder,
            folders::rename_folder,
            folders::set_folder_flags,
            folders::delete_folder,
            folders::move_to_folder,
            folders::folder_sharing,
            folders::share_folder,
            hub_link,
            media::media_hub_install,
            media::remove_media_model,
            media::media_start,
            media::media_cancel,
            media::media_prompt_status,
            media::media_forget_tuning,
            media::clarify::media_questions,
            media::clarify::media_with_answers,
            media::clarify::media_set_ask_questions,
            media::learn::media_set_learning,
            media::wallpaper_screens,
            media::wallpaper_set,
            media::media_jobs,
            media::media_list,
            media::media_in_chat,
            media::improve_media_prompt,
            ports::ports_view,
            applog::logs_view,
            applog::read_log,
            applog::open_app_folder,
            applog::log_meanings,
            applog::clear_logs,
            devmode::dev_mode_view,
            devmode::set_dev_mode,
            devmode::send_dev_report,
            ports::stop_port_program,
            ports::server_stop,
            ports::server_start,
            ports::preview_port,
            preview::preview_open,
            preview::preview_stop,
            preview::preview_open_external,
            save_chart,
            hidden::hidden_chats,
            hidden::set_chat_hidden,
            hidden::sidebar_layout,
            chatnotes::chat_notes,
            chatnotes::set_chat_notes,
            hidden::set_sidebar_layout,
            sitekeys::site_key_ask,
            sitekeys::site_key_give,
            sitekeys::site_keys,
            sitekeys::site_key_forget,
            handoff_guard::handoffs_view,
            handoff_guard::handoff_revoke,
            handoff_guard::handoff_forget,
            media::media_get,
            media::media_file,
            media::media_thumb,
            media::media_delete,
            media::media_set_poster,
            media::media_set_cover,
            media::loras::loras,
            media::loras::lora_add,
            media::loras::lora_save,
            media::loras::lora_delete,
            media::media_favorite,
            media::media_set_title,
            media::media_dislike,
            media::music_playlists,
            media::music_playlist_save,
            media::music_playlist_delete,
            media::music_taste,
            media::music_custom_models,
            media::music_custom_model_delete,
            media::studio_projects,
            media::studio_project_save,
            media::studio_project_delete,
            media::studio_record_start,
            media::studio_record_stop,
            media::studio_record_cancel,
            media::studio_midi,
            media::studio_save_midi,
            media::studio_ask,
            media::studio_read_midi,
            media::studio_read_sound,
            media::studio_library,
            media::studio_library_save,
            media::studio_library_delete,
            media::media_export,
            media::media_import,
            media::music_write_lyrics,
            media::music_rewrite_lines,
            media::music_artist_style,
            media::music_personas,
            media::music_persona_save,
            media::music_persona_delete,
            media::media_add,
            media::media_song_midi,
            media::media_song_map_save,
            media::compose_sounds,
            media::compose_write,
            media::compose_arrange,
            media::compose_section,
            media::compose_save,
            media::compose_midi,
            media::compose_make_real,
            mail::mail_preset,
            mail::mail_accounts,
            mail::add_mail_account,
            mail::add_mail_account_oauth,
            oauth_clients,
            set_oauth_clients,
            mail::remove_mail_account,
            mail::sync_mail,
            mail::mail_list,
            mail::mail_get,
            mail::mail_set_seen,
            mail::mail_action,
            triage::mail_triage,
            triage::mail_triage_report,
            triage::mail_triage_undo,
            triage::set_mail_auto_sort,
            triage::set_ui_language,
            triage::mail_event_draft,
            triage::mail_draft_reply,
            triage::mail_remind_missed,
            triage::set_mail_remind_missed,
            triage::mail_bulk_plan,
            subscriptions::mail_subscriptions,
            subscriptions::mail_unsubscribe,
            mail::send_mail,
            mail::mail_invite_status,
            mail::mail_invite_respond,
            calendar::calendar_presets,
            calendar::calendar_accounts,
            calendar::add_calendar_account,
            calendar::add_calendar_account_oauth,
            calendar::remove_calendar_account,
            calendar::sync_calendars,
            calendar::calendar_events,
            calendar::create_event,
            calendar::delete_event,
            perf::set_perf,
            app_info,
            refresh_hardware,
            catalog_view,
            install_model,
            repair_model,
            pause_install,
            resume_install,
            downloads_view,
            hub_search,
            hub_install,
            cancel_install,
            find_models,
            load_default_model,
            import_model_file,
            import_model_folder,
            measure_model_speed,
            optimize_model,
            model_tunings,
            forget_optimization,
            measure_all_speeds,
            speed_test_status,
            advanced::advanced_view,
            advanced::set_advanced,
            advanced::set_advanced_pin,
            advanced::check_advanced_pin,
            api_server::api_server_view,
            api_server::set_api_server,
            api_server::new_api_key,
            cloud::cloud_view,
            cloud::add_cloud_provider,
            cloud::remove_cloud_provider,
            cloud::set_cloud_provider,
            cloud::cloud_models_available,
            cloud::set_cloud_models,
            cloud::set_cloud_media,
            cloud::set_cloud_options,
            cloud::cloud_choices,
            cloud::set_cloud_sharing,
            cloud::set_cloud_proxy,
            cloud::cloud_sign_in,
            cloud::chat_cloud_spend,
            backup::make_backup,
            phone::phone_view,
            phone::set_phone,
            phone::add_phone,
            phone::remove_phone,
            phone::set_phone_while_locked,
            phone::set_phone_link,
            pcstatus::pc_status,
            pcstatus::stop_all,
            storage::storage_view,
            storage::move_models,
            sync::sync_view,
            sync::set_sync,
            sync::start_pairing,
            sync::cancel_pairing,
            sync::pair_device,
            sync::remove_device,
            sync::sync_now,
            sync::set_relay,
            sync::models::peer_models,
            sync::models::copy_model_from,
            sync::people::people_view,
            sync::people::people_unread,
            sync::people::set_people_name,
            sync::people::people_remove,
            sync::people::codes::people_connect,
            sync::people::codes::people_new_code,
            sync::people::codes::people_set_code,
            sync::people::codes::people_delete_code,
            sync::people::codes::people_block,
            sync::people::codes::people_firewall,
            sync::people::codes::people_allow_firewall,
            sync::people::people_set_address,
            sync::people::people_messages,
            sync::people::people_send,
            sync::people::people_edit,
            sync::people::people_delete,
            sync::people::people_save_file,
            sync::people::share::people_chat_sharing,
            sync::people::share::people_share_chat,
            sync::people::share::people_watch,
            sync::people::share::people_unwatch,
            sync::people::share::people_shared_chats,
            sync::people::share::people_shared_post,
            sync::people::share::people_shared_model,
            sync::people::share::people_set_shared_model,
            sync::people::share::people_shared_stop,
            sync::people::ask::people_requests,
            sync::people::ask::people_reply,
            sync::people::ask::people_stop,
            sync::people::ask::people_set_allow,
            sync::people::ask::people_ask,
            sync::people::ask::people_asks,
            sync::people::mailbox::people_set_relay_offer,
            sync::people::people_set_read_receipts,
            sync::people::people_rename,
            sync::people::lend::people_runs,
            sync::people::lend::people_set_lend,
            sync::people::lend::people_lent,
            finetune::finetune_view,
            finetune::start_finetune,
            finetune::cancel_finetune,
            finetune::remove_adapter,
            practice::start_practice,
            updates::update_view,
            updates::check_for_update,
            updates::set_update_auto,
            updates::set_update_install,
            privacy::privacy_view,
            privacy::set_privacy,
            outgoing::privacy_mode,
            outgoing::set_privacy_mode,
            outgoing::privacy_share,
            outgoing::privacy_sent,
            outgoing::privacy_sent_clear,
            outgoing::privacy_sent_delete,
            doclib::docs_library,
            doclib::docs_add,
            doclib::docs_remove,
            doclib::docs_update,
            doclib::docs_stop,
            doclib::docs_files,
            doclib::docs_search,
            doclib::docs_open,
            study::study_sets,
            study::study_make,
            study::study_delete,
            study::study_rename,
            study::study_card,
            study::study_quiz_done,
            pccheck::pc_check_start,
            pccheck::pc_check_stop,
            pccheck::pc_check_report,
            addons::addons_list,
            addons::addon_add,
            review::review_setup,
            review::reviews,
            review::review_start,
            review::review_stop,
            review::review_delete,
            review::review_save,
            sync::compute::compute_settings,
            sync::compute::compute_set,
            sync::compute::compute_status,
            sync::compute::compute_control,
            typing::typing_vocabulary,
            typing::typing_fix,
            typing::typing_can_correct,
            typing::typing_next,
            privacy::test_privacy,
            updates::install_update,
            updates::go_back_to,
            backup::backup_info,
            backup::restore_backup,
            backup::export_markdown_cmd,
            remove_model,
            get_settings,
            set_connectivity,
            set_default_model,
            set_auto_model,
            context_options,
            set_context_window,
            compact_chat,
            rename_model,
            finish_onboarding,
            get_profile,
            set_profile,
            list_chats,
            create_chat,
            delete_chat,
            rename_chat,
            set_chat_web,
            set_chat_model,
            get_messages,
            get_context,
            stop_generation,
            faster_model,
            tools::programs::programs_banner,
            tools::programs::programs_banner_stop,
            lens::lens_words,
            import::import_chats,
            import::import_memories,
            lens::lens_layers,
            lens::lens_edit,
            chat_live,
            send_message,
            engine_status,
            unload_model,
            security::security_status,
            security::enable_lock,
            security::change_pin,
            security::reset_pin,
            security::disable_lock,
            security::unlock,
            security::unlock_with_hello,
            security::lock_now,
            security::set_hello,
            security::set_auto_lock,
            security::list_actions,
            security::clear_actions,
            workspace::list_folders,
            workspace::set_folder_every_chat,
            workspace::folder_for_every_chat,
            workspace::add_folder,
            workspace::choose_folder,
            inbox::chat_add,
            inbox::chat_queue_add,
            inbox::chat_queue,
            inbox::chat_inbox,
            inbox::chat_queue_remove,
            inbox::chat_queue_send,
            workspace::desktop_access,
            workspace::set_desktop_access,
            workspace::folder_warning,
            workspace::remove_folder,
            workspace::set_chat_mode,
            workspace::answer_approval,
            workspace::pending_approvals,
            workspace::all_pending_approvals,
            workspace::undoable_turns,
            workspace::undo_turn,
            projects::list_projects,
            projects::create_project,
            projects::update_project,
            projects::delete_project,
            projects::add_project_folder,
            projects::remove_project_folder,
            projects::create_chat_in,
            projects::leave_incognito,
            projects::set_chat_project,
            projects::list_memories,
            projects::add_memory,
            projects::update_memory,
            projects::set_memory_scope,
            projects::memory_auto,
            projects::set_memory_auto,
            projects::linked_chats,
            projects::link_chats,
            projects::unlink_chats,
            quick::quick_settings,
            quick::set_quick_hotkey,
            quick::set_quick_model,
            quick::quick_chat,
            projects::delete_memory,
            projects::clear_memories,
            projects::set_memory_enabled,
            schedule::list_schedules,
            schedule::save_schedule,
            schedule::delete_schedule,
            schedule::run_schedule_now,
            connectors::list_connectors,
            connectors::save_connector,
            connectors::delete_connector,
            connectors::set_connector_enabled,
            connectors::set_tool_mode,
            connectors::list_plugins,
            connectors::inspect_plugin,
            connectors::install_plugin,
            connectors::set_plugin_enabled,
            connectors::remove_plugin,
            speech::speech_view,
            speech::install_speech_model,
            speech::remove_speech_model,
            speech::get_voice_settings,
            speech::set_voice_settings,
            speech::audio_devices,
            voice::start_dictation,
            voice::stop_dictation,
            voice::list_voices,
            voice::speak,
            voice::stop_speaking,
            voice::start_voice,
            voice::stop_voice,
            voice::interrupt_voice,
            meeting::start_meeting,
            meeting::stop_meeting,
            meeting::live_meeting,
            meeting::list_meetings,
            meeting::get_meeting,
            meeting::rename_meeting,
            meeting::set_action_done,
            meeting::delete_meeting,
            meeting::delete_meeting_audio,
            meeting::rewrite_notes,
            meeting::meeting_clip,
            meeting::export_meeting,
            meeting::ask_about_meeting,
            meeting::search_meetings,
            translate::translate,
            translate::translation_languages,
            translate::translate_file,
            natural::voice_packs,
            natural::install_voice_pack,
            natural::remove_voice_pack,
            diarize::speaker_model,
            diarize::install_speaker_model,
            diarize::remove_speaker_model,
            meeting::rename_speaker,
            features::features_view,
            features::set_feature,
            features::install_feature,
            notes::notes,
            notes::save_note_cmd,
            notes::pin_note,
            notes::delete_note,
            notes::tasks,
            notes::save_task_cmd,
            notes::delete_task,
            notes::parse_due_text,
            notes::tasks_from_meeting,
            web::get_web_settings,
            web::set_web_settings,
        ])
        .build(tauri::generate_context!())
        .expect("error while building SulcusAI");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            crash::closing();
            let state = handle.state::<Arc<AppState>>().inner().clone();
            tauri::async_runtime::block_on(async move {
                state.engine.lock().await.stop().await;
                speech::stop(&state).await;
            });
        }
    });
}

#[cfg(all(test, windows))]
mod open_tests {
    /// Opens a sign-in-length address (about 700 characters) in the default
    /// browser; Explorer used to show a folder instead. Opens a real tab.
    #[test]
    #[ignore]
    fn a_long_address_opens_in_the_browser() {
        let url = format!("https://sulcusai.sseditor.com/?test={}", "x".repeat(700));
        super::open_with_system(std::ffi::OsStr::new(&url)).unwrap();
    }
}

#[cfg(test)]
mod think_tests {
    /// About a minute and a quarter of thinking at the model's speed: his
    /// brother's Qwen3.6 27B, partly in system memory, thought for minutes.
    #[test]
    fn thinking_is_kept_to_about_a_minute() {
        assert_eq!(super::think_tokens(10.0), Some(1024), "a slow model still gets 1K");
        assert_eq!(super::think_tokens(30.0), Some(2250));
        assert_eq!(super::think_tokens(400.0), Some(8192), "a fast one at most 8K");
        assert_eq!(super::think_tokens(0.0), None, "no speed known: no limit");
    }
}

#[cfg(test)]
mod hub_link_tests {
    use serde_json::json;

    /// What his brother pasted: Stable Diffusion 1.5 and FLUX.1 models, and the
    /// message read like a broken GGUF file.
    #[test]
    fn a_model_that_cant_be_used_says_why_in_plain_words() {
        let flux = super::hub_why_not(&json!({ "id": "city96/FLUX.1-schnell-gguf", "pipeline_tag": "text-to-image", "cardData": { "base_model": "black-forest-labs/FLUX.1-schnell" } }));
        assert!(flux.starts_with("That FLUX.1 model") && flux.contains("Juggernaut XL"), "{flux}");
        let sd3 = super::hub_why_not(&json!({ "id": "stabilityai/stable-diffusion-3.5-large", "pipeline_tag": "text-to-image", "tags": ["diffusers"] }));
        assert!(sd3.contains("picture model") && !sd3.contains("GGUF"), "{sd3}");
        let split = super::hub_why_not(&json!({ "id": "Lykon/dreamshaper-8", "pipeline_tag": "text-to-image", "siblings": [{ "rfilename": "unet/diffusion_pytorch_model.safetensors" }, { "rfilename": "vae/diffusion_pytorch_model.safetensors" }] }));
        assert!(split.contains("split into folders"), "{split}");
        let chat = super::hub_why_not(&json!({ "id": "Qwen/Qwen3-8B", "pipeline_tag": "text-generation" }));
        assert!(chat.contains("no GGUF file") && chat.contains("unsloth"), "{chat}");
    }
}

#[cfg(all(test, windows))]
mod incognito_tests {
    /// Leaving an incognito chat takes everything it left with it: pictures
    /// pasted or made there, file backups, and what it sent; other chats keep theirs.
    #[test]
    fn leaving_an_incognito_chat_leaves_nothing_behind() {
        let state = crate::e2e::temp_state(Some(std::env::temp_dir().join(format!("sulcusai-inc-models-{}", uuid::Uuid::new_v4().simple()))));
        let c = state.cipher().unwrap();
        let (secret, kept) = {
            let conn = state.db.lock().unwrap();
            let secret = crate::db::create_chat_in(&conn, &c, None, None, true).unwrap().id;
            let kept = crate::db::create_chat_in(&conn, &c, None, None, false).unwrap().id;
            for (chat, hidden) in [(&secret, true), (&secret, false), (&kept, true)] {
                let item = crate::media::store::MediaItem { id: crate::media::store::new_id(), kind: "image".into(), op: "paste".into(), mime: "image/png".into(), chat_id: Some(chat.clone()), hidden, ..Default::default() };
                crate::media::store::save(&conn, &state.paths, &c, item, b"png", None).unwrap();
            }
            let backup = state.paths.data.join("x.bak");
            std::fs::write(&backup, b"old").unwrap();
            crate::db::add_checkpoint(&conn, &c, &secret, "t", "write", "C:/x.txt", None, Some(&backup.display().to_string())).unwrap();
            (secret, kept)
        };
        let r = crate::outgoing::review_event("your Outlook calendar", &[("title", "Title", "dentist")], &crate::outgoing::Me::new("", vec![], ""));
        crate::outgoing::record(&state, &c, &secret, "calendar_create_event", &r, true);
        crate::outgoing::record(&state, &c, &kept, "calendar_create_event", &r, true);
        super::clear_incognito(&state, None).unwrap();
        let conn = state.db.lock().unwrap();
        assert!(crate::media::store::in_chat(&conn, &c, &secret).is_empty());
        assert_eq!(crate::media::store::in_chat(&conn, &c, &kept).len(), 1);
        assert!(crate::db::chat_checkpoints(&conn, &secret).is_empty() && !state.paths.data.join("x.bak").exists());
        drop(conn);
        let sent = crate::outgoing::sent_list(&state.db.lock().unwrap(), &c);
        assert!(sent.iter().all(|s| s.chat_id != secret) && sent.iter().any(|s| s.chat_id == kept));
        let _ = std::fs::remove_dir_all(&state.paths.data);
    }
}

#[cfg(test)]
mod model_file_tests {
    /// A model file copied part way, overwritten or gone is said plainly;
    /// a whole GGUF file is fine.
    #[test]
    fn a_damaged_model_file_is_found() {
        let dir = std::env::temp_dir().join(format!("sulcusai-damaged-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.gguf");
        let mut bytes = b"GGUF".to_vec();
        bytes.extend_from_slice(&[0u8; 60]);
        std::fs::write(&good, &bytes).unwrap();
        assert_eq!(super::model_file_problem(&good, 64), None);
        assert_eq!(super::model_file_problem(&good, 0), None, "size not known");
        let zeros = dir.join("zeros.gguf");
        std::fs::write(&zeros, [0u8; 64]).unwrap();
        assert!(super::model_file_problem(&zeros, 64).unwrap().contains("isn't a model file"));
        assert!(super::model_file_problem(&good, 128).unwrap().contains("instead of"), "cut short");
        assert!(super::model_file_problem(&dir.join("gone.gguf"), 64).unwrap().contains("missing"));
        // A model in three files: its size is all three (the first alone
        // called every big model damaged); a part gone is said.
        let part = |n: u32| dir.join(format!("big-Q4_K_M-{n:05}-of-00003.gguf"));
        for n in 1..=3 {
            std::fs::write(part(n), &bytes).unwrap();
        }
        assert_eq!(super::model_file_problem(&part(1), 192), None);
        assert!(super::model_file_problem(&part(1), 256).unwrap().contains("instead of"));
        std::fs::remove_file(part(2)).unwrap();
        assert_eq!(super::model_file_problem(&part(1), 192).unwrap(), "part 2 of its 3 files is missing");
        std::fs::remove_dir_all(&dir).ok();
    }
}

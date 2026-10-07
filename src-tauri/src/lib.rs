// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Isaac Lehman
//! SulcusAI core: commands the window calls, and the state behind them.

mod agent;
mod audio;
mod browser;
mod catalog;
mod chat;
mod checkpoint;
mod connectors;
mod crypto;
mod db;
mod diarize;
mod download;
#[cfg(all(test, windows))]
mod e2e;
mod engine;
mod features;
mod handoff;
mod hardware;
mod hello;
mod mcp;
mod meeting;
mod memory;
mod natural;
mod net;
mod notes;
mod notify;
mod paths;
mod perf;
mod projects;
mod sandbox;
mod schedule;
mod security;
mod speech;
mod tools;
mod translate;
mod tts;
mod vad;
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

    fn engine_idle(&self) -> std::time::Duration {
        self.engine_used.lock().unwrap().elapsed()
    }

    fn settings(&self) -> Settings {
        db::settings(&self.db.lock().unwrap())
    }

    /// The data key, or `LOCKED` while app lock is engaged.
    fn cipher(&self) -> Result<Cipher, String> {
        self.vault.lock().unwrap().cipher().ok_or_else(|| LOCKED.to_string())
    }

    fn log(&self, category: &str, summary: &str) {
        db::log_action(&self.db.lock().unwrap(), category, summary);
    }

    /// Encrypts anything stored before encryption existed, then compacts the
    /// file so the old plaintext is gone.
    fn migrate_plaintext(&self, cipher: &Cipher) {
        let mut conn = self.db.lock().unwrap();
        match db::encrypt_legacy(&mut conn, cipher) {
            Ok(0) => {}
            Ok(n) => {
                let _ = db::compact(&conn);
                db::log_action(&conn, "privacy", &format!("Encrypted {n} previously saved items on this PC"));
            }
            Err(e) => eprintln!("encrypting saved data failed: {e}"),
        }
    }
}

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
    format!("{:.1} GB", bytes as f64 / catalog::GIB as f64)
}

// ---------- app & hardware ----------

#[derive(Serialize)]
struct AppInfo {
    name: &'static str,
    version: &'static str,
    data_dir: String,
}

#[tauri::command]
fn app_info(state: AppStateRef) -> AppInfo {
    AppInfo {
        name: "SulcusAI",
        version: env!("CARGO_PKG_VERSION"),
        data_dir: state.paths.data.display().to_string(),
    }
}

#[tauri::command]
fn refresh_hardware(state: AppStateRef) -> Hardware {
    let hw = hardware::detect(&state.paths.models);
    *state.hardware.write().unwrap() = hw.clone();
    hw
}

// ---------- catalog ----------

#[derive(Serialize)]
struct ModelCard {
    #[serde(flatten)]
    spec: ModelSpec,
    fit: ModelFit,
    installed: Option<InstalledModel>,
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

#[tauri::command]
fn catalog_view(state: AppStateRef) -> CatalogView {
    let hardware = state.hardware.read().unwrap().clone();
    let budget = Budget::from_hardware(&hardware);
    let installed = db::installed_models(&state.db.lock().unwrap());
    let is_installed = |id: &str| installed.iter().any(|m| m.model_id == id);

    let models = state
        .catalog
        .models
        .iter()
        .filter_map(|spec| {
            let fit = catalog::fit_model(spec, &budget);
            let inst = installed.iter().find(|m| m.model_id == spec.id).cloned();
            (inst.is_some() || fit.visible()).then(|| ModelCard { spec: spec.clone(), fit, installed: inst })
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

#[tauri::command]
fn install_model(app: AppHandle, state: AppStateRef, model_id: String, quant: String) -> Result<(), String> {
    let spec = state.catalog.model(&model_id).ok_or("Unknown model")?.clone();
    let variant = spec.variant(&quant).ok_or("Unknown model version")?.clone();
    let fit = catalog::fit_model(&spec, &state.budget());
    let vfit = fit.variant(&quant).ok_or("Unknown model version")?;
    if !vfit.runnable() {
        return Err("This PC can't run that version of the model.".into());
    }
    if !vfit.disk_ok {
        return Err("There isn't enough free disk space for this model.".into());
    }
    let backend = engine::backend_for(&state.budget())?.to_string();
    if !state.installs.lock().unwrap().is_empty() {
        return Err("Another install is in progress. Wait for it to finish first.".into());
    }

    let state = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        let Some((cancel, _guard)) = claim(&state.installs, &model_id) else { return };
        let emit = |phase: &str, received: u64, total: u64| {
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
            download::fetch_verified(&client, &variant.url, &dest, variant.size, &variant.sha256, &cancel, |r, t| {
                emit("download", r, t)
            })
            .await?;

            let previous = db::installed_model(&state.db.lock().unwrap(), &spec.id);
            {
                let conn = state.db.lock().unwrap();
                db::save_installed(&conn, &InstalledModel {
                    model_id: spec.id.clone(),
                    quant: variant.quant.clone(),
                    path: dest.display().to_string(),
                    size: variant.size,
                    installed_at: db::now_ms(),
                    tps: None,
                })?;
                if db::settings(&conn).default_model.is_none() {
                    db::update_settings(&conn, |s| s.default_model = Some(spec.id.clone()))?;
                }
            }
            // Replacing another version of the same model: drop the old file.
            if let Some(old) = previous.filter(|p| p.path != dest.display().to_string()) {
                let mut engine = state.engine.lock().await;
                if engine.status().model_id.as_deref() == Some(&spec.id) {
                    engine.stop().await;
                }
                std::fs::remove_file(&old.path).ok();
            }

            emit("benchmark", 0, 0);
            let log = state.paths.engine_log();
            // Same settings a chat will use, so the first chat doesn't reload the model.
            let limits = state.limits();
            let (ctx, gpu_layers) = catalog::launch_within(&spec, &variant.quant, &state.budget(), &limits);
            let mut engine = state.engine.lock().await;
            let measured = async {
                let ep = engine
                    .ensure(
                        LaunchSpec {
                            exe: &exe,
                            model_path: &dest,
                            model_id: &spec.id,
                            quant: &variant.quant,
                            ctx,
                            gpu_layers,
                            threads: limits.threads,
                            low_priority: limits.low_priority,
                            log: &log,
                        },
                        state.job(),
                    )
                    .await?;
                engine::benchmark(&ep).await
            }
            .await;
            drop(engine);
            match measured {
                Ok(tps) => db::set_tps(&state.db.lock().unwrap(), &spec.id, tps)?,
                // The model is installed; only the speed test failed.
                Err(e) => eprintln!("benchmark failed for {}: {e}", spec.id),
            }
            Ok(())
        }
        .await;

        let payload = match &result {
            Ok(()) => {
                state.log("model", &format!("Installed {} ({} quality)", spec.name, variant.quality));
                json!({ "model_id": model_id, "ok": true })
            }
            Err(e) if e == download::CANCELLED => {
                state.log("model", &format!("Cancelled installing {}", spec.name));
                json!({ "model_id": model_id, "ok": false, "cancelled": true })
            }
            Err(e) => {
                state.log("model", &format!("Installing {} failed: {e}", spec.name));
                json!({ "model_id": model_id, "ok": false, "error": e })
            }
        };
        app.emit("install:finished", payload).ok();
    });
    Ok(())
}

/// The stored path, or the same file in today's models folder if the data
/// folder has moved since install.
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

#[tauri::command]
fn cancel_install(state: AppStateRef, model_id: String) {
    if let Some(flag) = state.installs.lock().unwrap().get(&model_id) {
        flag.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
async fn remove_model(state: AppStateRef<'_>, model_id: String) -> Result<(), String> {
    state.cipher()?;
    {
        let mut engine = state.engine.lock().await;
        if engine.status().model_id.as_deref() == Some(&model_id) {
            engine.stop().await;
        }
    }
    let dir = state.paths.models.join(&model_id);
    if dir.starts_with(&state.paths.models) && dir != state.paths.models && dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("Couldn't delete the model files: {e}"))?;
    }
    let conn = state.db.lock().unwrap();
    db::remove_installed(&conn, &model_id)?;
    if db::settings(&conn).default_model.as_deref() == Some(&model_id) {
        let next = db::installed_models(&conn).first().map(|m| m.model_id.clone());
        db::update_settings(&conn, |s| s.default_model = next)?;
    }
    let name = state.catalog.model(&model_id).map_or(model_id.as_str(), |m| m.name.as_str());
    db::log_action(&conn, "model", &format!("Removed {name} and deleted its files"));
    Ok(())
}

// ---------- settings & profile ----------

#[tauri::command]
fn get_settings(state: AppStateRef) -> Settings {
    state.settings()
}

#[tauri::command]
fn set_connectivity(app: AppHandle, state: AppStateRef, level: Connectivity) -> Result<Settings, String> {
    if level == Connectivity::Offline {
        // Offline closes the browser; nothing in it should keep going online.
        browser::close(&app);
    }
    state.cipher()?;
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

#[tauri::command]
fn set_default_model(state: AppStateRef, model_id: String) -> Result<Settings, String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    if db::installed_model(&conn, &model_id).is_none() {
        return Err("That model isn't installed.".into());
    }
    db::update_settings(&conn, |s| s.default_model = Some(model_id))
}

#[tauri::command]
fn finish_onboarding(state: AppStateRef) -> Result<Settings, String> {
    let conn = state.db.lock().unwrap();
    db::update_settings(&conn, |s| s.onboarded = true)
}

#[tauri::command]
fn get_profile(state: AppStateRef) -> Result<Profile, String> {
    let c = state.cipher()?;
    Ok(db::profile(&state.db.lock().unwrap(), &c))
}

#[tauri::command]
fn set_profile(state: AppStateRef, profile: Profile) -> Result<(), String> {
    let c = state.cipher()?;
    db::set_profile(&state.db.lock().unwrap(), &c, &profile)
}

// ---------- chats ----------

#[tauri::command]
fn list_chats(state: AppStateRef) -> Result<Vec<Chat>, String> {
    let c = state.cipher()?;
    Ok(db::list_chats(&state.db.lock().unwrap(), &c))
}

#[tauri::command]
fn create_chat(state: AppStateRef) -> Result<Chat, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let model = db::settings(&conn).default_model;
    db::create_chat(&conn, &c, model)
}

#[tauri::command]
fn delete_chat(state: AppStateRef, chat_id: String) -> Result<(), String> {
    state.cipher()?;
    state.contexts.lock().unwrap().remove(&chat_id);
    state.approvals.forget_chat(&chat_id);
    let conn = state.db.lock().unwrap();
    db::delete_chat(&conn, &chat_id)?;
    db::log_action(&conn, "chat", "Deleted a chat and its messages");
    Ok(())
}

#[tauri::command]
fn rename_chat(state: AppStateRef, chat_id: String, title: String) -> Result<(), String> {
    let c = state.cipher()?;
    let title = title.trim();
    if title.is_empty() {
        return Err("A chat needs a name.".into());
    }
    db::set_chat_title(&state.db.lock().unwrap(), &c, &chat_id, title)
}

#[tauri::command]
fn set_chat_web(state: AppStateRef, chat_id: String, web: bool) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    db::set_chat_web(&conn, &chat_id, web)?;
    db::log_action(&conn, "privacy", if web { "Turned on web access for one chat" } else { "Turned off web access for one chat" });
    Ok(())
}

#[tauri::command]
fn set_chat_model(state: AppStateRef, chat_id: String, model_id: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    if db::installed_model(&conn, &model_id).is_none() {
        return Err("That model isn't installed.".into());
    }
    state.contexts.lock().unwrap().remove(&chat_id);
    db::set_chat_model(&conn, &chat_id, &model_id)
}

#[tauri::command]
fn get_messages(state: AppStateRef, chat_id: String) -> Result<Vec<Message>, String> {
    let c = state.cipher()?;
    Ok(db::messages(&state.db.lock().unwrap(), &c, &chat_id))
}

#[tauri::command]
fn get_context(state: AppStateRef, chat_id: String) -> Option<ContextInfo> {
    state.contexts.lock().unwrap().get(&chat_id).cloned()
}

#[tauri::command]
fn stop_generation(state: AppStateRef, chat_id: String) {
    if let Some(flag) = state.generations.lock().unwrap().get(&chat_id) {
        flag.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
async fn send_message(app: AppHandle, state: AppStateRef<'_>, chat_id: String, text: String) -> Result<Option<Message>, String> {
    run_turn(&app, state.inner(), chat_id, text, None).await
}

/// Also receives every event of a turn (voice mode speaks the reply from it).
pub(crate) type Tee = Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>;

/// Saves the user's message and runs one agent turn in the chat.
pub(crate) async fn run_turn(
    app: &AppHandle,
    state: &Arc<AppState>,
    chat_id: String,
    text: String,
    tee: Option<Tee>,
) -> Result<Option<Message>, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Type a message first.".into());
    }
    let cipher = state.cipher()?;
    let Some((cancel, _guard)) = claim(&state.generations, &chat_id) else {
        return Err("Still answering the last message.".into());
    };

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
    let (ep, spec, _) = llm_endpoint(state, chat.model_id.clone(), || {
        app.emit("chat:status", json!({ "chat_id": chat_id, "status": "loading" })).ok();
    })
    .await?;

    let today = chrono::Local::now().format("%A, %B %-d, %Y").to_string();
    let (base, about) = chat::system_prompt(&profile, &today);

    // Nearly full: summarize and continue in a fresh chat.
    let last_context = state.contexts.lock().unwrap().get(&chat_id).cloned();
    let mut chat_id = chat_id;
    if !chat.incognito && handoff::needed(last_context.as_ref()) {
        app.emit("chat:status", json!({ "chat_id": chat_id, "status": "handoff" })).ok();
        let history = db::messages(&state.db.lock().unwrap(), &cipher, &chat_id);
        let summary = handoff::summarize(&ep, &base, &history).await?;
        let new = handoff::continue_in_new_chat(&state.db.lock().unwrap(), &cipher, &chat, &summary)?;
        state.log("chat", "A long chat continued in a new chat with a summary");
        app.emit("chat:handoff", json!({ "from": chat_id, "to": new.id })).ok();
        chat_id = new.id;
    }

    {
        let conn = state.db.lock().unwrap();
        let first = db::message_count(&conn, &chat_id) == 0;
        db::add_message(&conn, &cipher, &Message {
            id: turn_id.clone(),
            chat_id: chat_id.clone(),
            role: "user".into(),
            content: text.clone(),
            created_at: db::now_ms(),
            ..Default::default()
        })?;
        if first {
            db::set_chat_title(&conn, &cipher, &chat_id, &chat::title_from(&text))?;
        }
    }

    let emitter = app.clone();
    let turn = agent::Turn {
        emit: Arc::new(move |event: &str, payload: serde_json::Value| {
            if let Some(t) = &tee {
                t(event, &payload);
            }
            emitter.emit(event, payload).ok();
        }),
        state: state.clone(),
        chat_id: chat_id.clone(),
        turn_id,
        cipher,
        ep,
        mode,
        use_tools: spec.tools,
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
    app.emit("chat:done", json!({ "chat_id": chat_id, "message": last, "tps": tps, "context": context, "cancelled": cancelled }))
        .ok();
    Ok(last)
}

/// Starts (or reuses) the engine for a model: the given one, else the
/// default. Returns the endpoint and the model.
pub(crate) async fn llm_endpoint(
    state: &Arc<AppState>,
    model_id: Option<String>,
    on_loading: impl FnOnce(),
) -> Result<(engine::Endpoint, ModelSpec, InstalledModel), String> {
    let installed = {
        let conn = state.db.lock().unwrap();
        let id = model_id
            .or_else(|| db::settings(&conn).default_model)
            .ok_or("Install a model first: open Models and pick one.")?;
        db::installed_model(&conn, &id).ok_or("This chat's model isn't installed anymore. Pick another one.")?
    };
    let spec = state.catalog.model(&installed.model_id).ok_or("This model is no longer in the catalog.")?.clone();
    let budget = state.budget();
    let limits = state.limits();
    let (ctx, gpu_layers) = catalog::launch_within(&spec, &installed.quant, &budget, &limits);
    let exe = engine::installed_server(&state.paths, &state.catalog.engine, engine::backend_for(&budget)?)
        .ok_or("The engine isn't installed. Reinstall the model from the Models page.")?;
    *state.engine_used.lock().unwrap() = std::time::Instant::now();
    let model_path = model_file(&state.paths, &installed);
    let launch = LaunchSpec {
        exe: &exe,
        model_path: &model_path,
        model_id: &installed.model_id,
        quant: &installed.quant,
        ctx,
        gpu_layers,
        threads: limits.threads,
        low_priority: limits.low_priority,
        log: &state.paths.engine_log(),
    };
    let mut engine = state.engine.lock().await;
    let ep = match engine.endpoint_for(&installed.model_id) {
        // Same model: new limits (mode, heat) apply between replies, and only
        // when no other chat is mid-reply on it.
        Some(ep) if engine.matches(&launch) || state.generations.lock().unwrap().len() > 1 => ep,
        _ => {
            on_loading();
            engine.ensure(launch, state.job()).await?
        }
    };
    Ok((ep, spec, installed))
}

/// For background work (meeting notes, translation): the model already
/// loaded if there is one, so a chat's model isn't swapped out mid-reply.
pub(crate) async fn background_endpoint(state: &Arc<AppState>) -> Result<(engine::Endpoint, ModelSpec), String> {
    // Use the model as it is loaded: relaunching it with new limits here
    // could cut off a chat that is mid-reply.
    let loaded = {
        let mut engine = state.engine.lock().await;
        let id = engine.status().model_id;
        id.and_then(|id| Some((engine.endpoint_for(&id)?, state.catalog.model(&id)?.clone())))
    };
    if let Some(found) = loaded {
        *state.engine_used.lock().unwrap() = std::time::Instant::now();
        return Ok(found);
    }
    let (ep, spec, _) = llm_endpoint(state, None, || {}).await?;
    Ok((ep, spec))
}

// ---------- engine ----------

#[tauri::command]
async fn engine_status(state: AppStateRef<'_>) -> Result<EngineStatus, String> {
    Ok(state.engine.lock().await.status())
}

#[tauri::command]
async fn unload_model(state: AppStateRef<'_>) -> Result<(), String> {
    state.engine.lock().await.stop().await;
    Ok(())
}

fn open_vault(paths: &Paths) -> Result<Vault, String> {
    #[cfg(windows)]
    let protector: Box<dyn crypto::Protector> = Box::new(crypto::dpapi::Dpapi);
    #[cfg(not(windows))]
    let protector: Box<dyn crypto::Protector> = return Err("Encryption at rest needs Windows for now.".into());
    Vault::open(&paths.data.join("keys.json"), protector)
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            // Local, not roaming: models are gigabytes and must never sync.
            // SULCUSAI_DATA_DIR overrides it (testing, portable installs).
            let data = match std::env::var_os("SULCUSAI_DATA_DIR") {
                Some(dir) => std::path::PathBuf::from(dir),
                None => app.path().app_local_data_dir()?,
            };
            let paths = Paths::new(data)?;
            let conn = db::open(&paths.db)?;
            let vault = open_vault(&paths)?;
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
                app: std::sync::OnceLock::new(),
                engine_used: Mutex::new(std::time::Instant::now()),
                #[cfg(windows)]
                job: winjob::Job::kill_on_close().ok(),
                paths,
            };
            if let Ok(c) = state.cipher() {
                state.migrate_plaintext(&c);
            }
            checkpoint::prune(&state.db.lock().unwrap());
            let _ = db::delete_incognito_chats(&state.db.lock().unwrap(), None);
            let state = Arc::new(state);
            let _ = state.app.set(app.handle().clone());
            speech::start_idle_unloader(state.clone());
            app.manage(state);
            schedule::start(app.handle().clone());
            notes::start_reminders(app.handle().clone());
            perf::start_monitor(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            perf::perf_view,
            browser::open_browser,
            perf::set_perf,
            app_info,
            refresh_hardware,
            catalog_view,
            install_model,
            cancel_install,
            remove_model,
            get_settings,
            set_connectivity,
            set_default_model,
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
            workspace::add_folder,
            workspace::remove_folder,
            workspace::set_chat_mode,
            workspace::answer_approval,
            workspace::pending_approvals,
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
            let state = handle.state::<Arc<AppState>>().inner().clone();
            tauri::async_runtime::block_on(async move {
                state.engine.lock().await.stop().await;
                speech::stop(&state).await;
            });
        }
    });
}

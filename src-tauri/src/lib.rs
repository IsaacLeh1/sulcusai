// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Isaac Lehman
//! SulcusAI core: commands the window calls, and the state behind them.

mod catalog;
mod chat;
mod db;
mod download;
#[cfg(all(test, windows))]
mod e2e;
mod engine;
mod hardware;
mod net;
mod paths;
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
use chat::{ContextInfo, Delta};
use db::{Chat, InstalledModel, Message, Profile, Settings};
use engine::{Engine, EngineStatus, JobRef, LaunchSpec};
use hardware::Hardware;
use net::Connectivity;
use paths::Paths;

pub struct AppState {
    paths: Paths,
    db: Mutex<Connection>,
    catalog: Catalog,
    hardware: RwLock<Hardware>,
    engine: tokio::sync::Mutex<Engine>,
    installs: Mutex<HashMap<String, Arc<AtomicBool>>>,
    generations: Mutex<HashMap<String, Arc<AtomicBool>>>,
    contexts: Mutex<HashMap<String, ContextInfo>>,
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

    fn settings(&self) -> Settings {
        db::settings(&self.db.lock().unwrap())
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

fn claim<'a>(map: &'a Mutex<HashMap<String, Arc<AtomicBool>>>, key: &str) -> Option<(Arc<AtomicBool>, FlagGuard<'a>)> {
    let mut m = map.lock().unwrap();
    if m.contains_key(key) {
        return None;
    }
    let flag = Arc::new(AtomicBool::new(false));
    m.insert(key.to_string(), flag.clone());
    Some((flag, FlagGuard { map, key: key.to_string() }))
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
        let result: Result<(), String> = async {
            emit("engine", 0, 0);
            let exe = engine::ensure_installed(&state.paths, &state.catalog.engine, &backend, &cancel, |r, t| {
                emit("engine", r, t)
            })
            .await?;

            let dest = state.paths.models.join(&spec.id).join(&variant.file);
            if dest.exists() {
                emit("verify", 0, 0); // reusing an earlier download after a hash check
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
                let mut settings = db::settings(&conn);
                if settings.default_model.is_none() {
                    settings.default_model = Some(spec.id.clone());
                    db::set(&conn, "settings", &settings)?;
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
            let mut engine = state.engine.lock().await;
            let measured = async {
                let ep = engine
                    .ensure(
                        LaunchSpec {
                            exe: &exe,
                            model_path: &dest,
                            model_id: &spec.id,
                            quant: &variant.quant,
                            ctx: fit.ctx,
                            gpu_layers: vfit_layers(&spec, &variant.quant, &state.budget()),
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
            Ok(()) => json!({ "model_id": model_id, "ok": true }),
            Err(e) if e == download::CANCELLED => json!({ "model_id": model_id, "ok": false, "cancelled": true }),
            Err(e) => json!({ "model_id": model_id, "ok": false, "error": e }),
        };
        app.emit("install:finished", payload).ok();
    });
    Ok(())
}

/// The stored path, or the same file in today's models folder if the data
/// folder has moved since install.
fn model_file(paths: &Paths, m: &InstalledModel) -> std::path::PathBuf {
    let stored = std::path::PathBuf::from(&m.path);
    if stored.exists() {
        return stored;
    }
    match stored.file_name() {
        Some(name) => paths.models.join(&m.model_id).join(name),
        None => stored,
    }
}

fn vfit_layers(spec: &ModelSpec, quant: &str, budget: &Budget) -> u32 {
    let fit = catalog::fit_model(spec, budget);
    fit.variant(quant).map_or(0, |v| v.gpu_layers)
}

#[tauri::command]
fn cancel_install(state: AppStateRef, model_id: String) {
    if let Some(flag) = state.installs.lock().unwrap().get(&model_id) {
        flag.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
async fn remove_model(state: AppStateRef<'_>, model_id: String) -> Result<(), String> {
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
    let mut settings = db::settings(&conn);
    if settings.default_model.as_deref() == Some(&model_id) {
        settings.default_model = db::installed_models(&conn).first().map(|m| m.model_id.clone());
        db::set(&conn, "settings", &settings)?;
    }
    Ok(())
}

// ---------- settings & profile ----------

#[tauri::command]
fn get_settings(state: AppStateRef) -> Settings {
    state.settings()
}

#[tauri::command]
fn set_connectivity(state: AppStateRef, level: Connectivity) -> Result<Settings, String> {
    let conn = state.db.lock().unwrap();
    let mut s = db::settings(&conn);
    s.connectivity = level;
    db::set(&conn, "settings", &s)?;
    Ok(s)
}

#[tauri::command]
fn set_default_model(state: AppStateRef, model_id: String) -> Result<Settings, String> {
    let conn = state.db.lock().unwrap();
    if db::installed_model(&conn, &model_id).is_none() {
        return Err("That model isn't installed.".into());
    }
    let mut s = db::settings(&conn);
    s.default_model = Some(model_id);
    db::set(&conn, "settings", &s)?;
    Ok(s)
}

#[tauri::command]
fn get_profile(state: AppStateRef) -> Profile {
    db::profile(&state.db.lock().unwrap())
}

#[tauri::command]
fn set_profile(state: AppStateRef, profile: Profile) -> Result<(), String> {
    db::set(&state.db.lock().unwrap(), "profile", &profile)
}

// ---------- chats ----------

#[tauri::command]
fn list_chats(state: AppStateRef) -> Vec<Chat> {
    db::list_chats(&state.db.lock().unwrap())
}

#[tauri::command]
fn create_chat(state: AppStateRef) -> Result<Chat, String> {
    let conn = state.db.lock().unwrap();
    let model = db::settings(&conn).default_model;
    db::create_chat(&conn, model)
}

#[tauri::command]
fn delete_chat(state: AppStateRef, chat_id: String) -> Result<(), String> {
    state.contexts.lock().unwrap().remove(&chat_id);
    db::delete_chat(&state.db.lock().unwrap(), &chat_id)
}

#[tauri::command]
fn rename_chat(state: AppStateRef, chat_id: String, title: String) -> Result<(), String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("A chat needs a name.".into());
    }
    db::update_chat(&state.db.lock().unwrap(), &chat_id, Some(title), None, None)
}

#[tauri::command]
fn set_chat_web(state: AppStateRef, chat_id: String, web: bool) -> Result<(), String> {
    db::update_chat(&state.db.lock().unwrap(), &chat_id, None, None, Some(web))
}

#[tauri::command]
fn set_chat_model(state: AppStateRef, chat_id: String, model_id: String) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    if db::installed_model(&conn, &model_id).is_none() {
        return Err("That model isn't installed.".into());
    }
    state.contexts.lock().unwrap().remove(&chat_id);
    db::update_chat(&conn, &chat_id, None, Some(&model_id), None)
}

#[tauri::command]
fn get_messages(state: AppStateRef, chat_id: String) -> Vec<Message> {
    db::messages(&state.db.lock().unwrap(), &chat_id)
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
async fn send_message(app: AppHandle, state: AppStateRef<'_>, chat_id: String, text: String) -> Result<Message, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Type a message first.".into());
    }
    let Some((cancel, _guard)) = claim(&state.generations, &chat_id) else {
        return Err("Still answering the last message.".into());
    };

    let (chat, installed, profile) = {
        let conn = state.db.lock().unwrap();
        let chat = db::chat(&conn, &chat_id).ok_or("That chat no longer exists.")?;
        let model_id = chat
            .model_id
            .clone()
            .or_else(|| db::settings(&conn).default_model)
            .ok_or("Install a model first: open Models and pick one.")?;
        let installed = db::installed_model(&conn, &model_id).ok_or("This chat's model isn't installed anymore. Pick another one.")?;
        if chat.model_id.is_none() {
            db::update_chat(&conn, &chat_id, None, Some(&model_id), None)?;
        }
        let first = db::messages(&conn, &chat_id).is_empty();
        db::add_message(&conn, &Message {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id: chat_id.clone(),
            role: "user".into(),
            content: text.clone(),
            thinking: None,
            created_at: db::now_ms(),
        })?;
        if first {
            db::update_chat(&conn, &chat_id, Some(&chat::title_from(&text)), None, None)?;
        }
        (chat, installed, db::profile(&conn))
    };
    let _ = chat;

    let spec = state.catalog.model(&installed.model_id).ok_or("This model is no longer in the catalog.")?.clone();
    let budget = state.budget();
    let ctx = catalog::fit_model(&spec, &budget).ctx;
    let backend = engine::backend_for(&budget)?;
    let exe = engine::installed_server(&state.paths, &state.catalog.engine, backend)
        .ok_or("The engine isn't installed. Reinstall the model from the Models page.")?;

    let ep = {
        let mut engine = state.engine.lock().await;
        match engine.endpoint_for(&installed.model_id) {
            Some(ep) => ep,
            None => {
                app.emit("chat:status", json!({ "chat_id": chat_id, "status": "loading" })).ok();
                let log = state.paths.engine_log();
                let model_path = model_file(&state.paths, &installed);
                engine
                    .ensure(
                        LaunchSpec {
                            exe: &exe,
                            model_path: &model_path,
                            model_id: &installed.model_id,
                            quant: &installed.quant,
                            ctx,
                            gpu_layers: vfit_layers(&spec, &installed.quant, &budget),
                            log: &log,
                        },
                        state.job(),
                    )
                    .await?
            }
        }
    };

    let today = chrono::Local::now().format("%A, %B %-d, %Y").to_string();
    let (base, about) = chat::system_prompt(&profile, &today);
    let history = db::messages(&state.db.lock().unwrap(), &chat_id);
    let (kept, mut info) = chat::fit_history(&ep, &base, &about, &history).await?;
    state.contexts.lock().unwrap().insert(chat_id.clone(), info.clone());
    app.emit("chat:context", json!({ "chat_id": chat_id, "context": info })).ok();

    let message_id = uuid::Uuid::new_v4().to_string();
    app.emit("chat:start", json!({ "chat_id": chat_id, "message_id": message_id })).ok();
    let system = if about.is_empty() { base } else { format!("{base}\n\n{about}") };
    let finished = chat::stream_reply(&ep, &system, &kept, &cancel, |d| {
        let (content, thinking) = match d {
            Delta::Content(t) => (Some(t), None),
            Delta::Thinking(t) => (None, Some(t)),
        };
        app.emit(
            "chat:delta",
            json!({ "chat_id": chat_id, "message_id": message_id, "content": content, "thinking": thinking }),
        )
        .ok();
    })
    .await?;

    let message = Message {
        id: message_id,
        chat_id: chat_id.clone(),
        role: "assistant".into(),
        content: finished.content,
        thinking: (!finished.thinking.is_empty()).then_some(finished.thinking),
        created_at: db::now_ms(),
    };
    if !message.content.is_empty() || message.thinking.is_some() {
        db::add_message(&state.db.lock().unwrap(), &message)?;
    }
    if let (Some(p), Some(c)) = (finished.prompt_tokens, finished.completion_tokens) {
        info.last_total = Some(p + c);
    }
    state.contexts.lock().unwrap().insert(chat_id.clone(), info.clone());
    app.emit(
        "chat:done",
        json!({ "chat_id": chat_id, "message": message, "tps": finished.tps, "context": info, "cancelled": finished.cancelled }),
    )
    .ok();
    Ok(message)
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

pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            // Local, not roaming: models are gigabytes and must never sync.
            let data = app.path().app_local_data_dir()?;
            let paths = Paths::new(data)?;
            let conn = db::open(&paths.db)?;
            let hardware = hardware::detect(&paths.models);
            let state = AppState {
                db: Mutex::new(conn),
                catalog: Catalog::bundled(),
                hardware: RwLock::new(hardware),
                engine: tokio::sync::Mutex::new(Engine::default()),
                installs: Mutex::new(HashMap::new()),
                generations: Mutex::new(HashMap::new()),
                contexts: Mutex::new(HashMap::new()),
                #[cfg(windows)]
                job: winjob::Job::kill_on_close().ok(),
                paths,
            };
            app.manage(Arc::new(state));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            refresh_hardware,
            catalog_view,
            install_model,
            cancel_install,
            remove_model,
            get_settings,
            set_connectivity,
            set_default_model,
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
        ])
        .build(tauri::generate_context!())
        .expect("error while building SulcusAI");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            let state = handle.state::<Arc<AppState>>().inner().clone();
            tauri::async_runtime::block_on(async move { state.engine.lock().await.stop().await });
        }
    });
}

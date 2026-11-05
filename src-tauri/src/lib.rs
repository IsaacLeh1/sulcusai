// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Isaac Lehman
//! SulcusAI core: commands the window calls, and the state behind them.

#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

mod access_keys;
mod advanced;
mod assistants;
pub mod policy;
mod agent;
mod applog;
mod cardfirst;
mod cleanup;
mod cloudfiles;
mod doclib;
pub(crate) mod medialib;
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
mod chatsearch;
mod compact;
mod compare;
mod cloud;
mod checkpoint;
mod connectors;
mod consent;
mod crash;
mod feedback;
mod crypto;
mod social;
mod db;
mod devmode;
mod devices;
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
mod integrity;
mod jsrun;
mod local;
mod location;
#[cfg(test)]
mod lock_lint;
mod mail;
mod open;
mod mcp;
mod mcp_auth;
mod media;
mod meeting;
mod memcheck;
mod memory;
mod model_fit;
mod natural;
mod net;
mod notes;
mod personal;
mod notify;
mod offline;
mod ollama_api;
mod optimize;
mod outgoing;
mod oauth;
mod paths;
mod peer_updates;
mod pcstatus;
mod people_window;
mod side_windows;
mod perf;
mod power;
mod quiet;
mod rest;
mod instance;
mod pause;
mod zero;
mod placement;
mod plugin_checks;
mod places;
mod practice;
mod privacy;
mod p2p;
mod phone;
mod ports;
mod presets;
mod prompts;
mod preview;
mod projects;
mod protected;
mod quick;
mod quotes;
mod router;
mod sandbox;
mod schedule;
mod secret_scrub;
mod security;
mod security_notice;
mod security_notice_feed;
mod shared_relay;
mod security_page;
mod selftest;
mod shield;
mod sitekeys;
mod speech;
mod storage;
mod subscriptions;
mod sync;
mod tools;
mod translate;
mod writing;
mod triage;
mod telegram;
mod matrix;
mod discord;
mod clipboard;
mod own_skills;
mod side_chat;
mod characters;
mod calls;
mod cast;
mod model_dupes;
mod backup_auto;
mod files;
mod tts;
mod programs;
mod coverage;
#[cfg_attr(windows, allow(dead_code))]
mod desktop_input;
mod lens;
mod steer;
mod uncensor;
mod screen_agent;
mod import;
mod typing;
mod updates;
mod versions;
mod vad;
mod vault;
mod vault_cmds;
mod viewer;
mod vision;
mod voice;
mod wake;
mod wakeup;
mod webpush;
mod web;
mod workspace;
#[cfg(test)]
mod testutil;
#[cfg(windows)]
mod winjob;

mod catalog_cmds;
mod model_cmds;
mod settings_cmds;
mod chat_cmds;
mod turns;
use catalog_cmds::*;
use model_cmds::*;
use settings_cmds::*;
use chat_cmds::*;
use turns::*;

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

pub(crate) use open::open_with_system;

/// Connector mode for the browser extension (see bridge.rs).
pub fn run_bridge_host() {
    bridge::run_host()
}

/// A start from the Start menu's Pause, Resume or Quit: handed to the copy
/// that's running. True when this start has nothing more to do (Resume with
/// no copy running starts the app).
pub fn hand_over_ask() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if let Some(text) = p2p::open_links::opened(&args) {
        if p2p::open_links::forward(&text) {
            return true;
        }
        p2p::open_links::hold(text);
        return false;
    }
    match instance::asked(&args) {
        Some(ask) => instance::forward(ask) || ask != instance::Ask::Resume,
        None => false,
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(quick::plugin())
        .register_asynchronous_uri_scheme_protocol(medialib::stream::SCHEME, |_ctx, req, responder| {
            std::thread::spawn(move || responder.respond(medialib::stream::serve(&req)));
        })
        .on_window_event(|w, e| {
            quick::on_main_event(w, e);
            people_window::on_main_event(w, e);
            side_windows::on_main_event(w, e);
            rest::on_main_event(w, e);
        })
        .setup(|app| {
            // The window is made hidden (tauri.conf.json) so the release
            // check can run a built copy without it ever showing, and a start
            // for scheduled tasks stays in the tray.
            let for_tasks = std::env::args().any(|a| a == pause::FOR_TASKS);
            if !selftest::begin(app.handle())? && !for_tasks {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                }
            }
            sync::compute::started();
            // Local, not roaming: models are gigabytes and must never sync.
            // SULCUSAI_DATA_DIR overrides it (testing, portable installs).
            let data = match std::env::var_os("SULCUSAI_DATA_DIR") {
                Some(dir) => std::path::PathBuf::from(dir),
                None => app.path().app_local_data_dir()?,
            };
            let paths = Paths::new(data)?;
            // "Delete everything" happens before anything opens its files. A
            // refused one mustn't stop the app starting.
            let erased = consent::apply_pending_erase(&paths);
            // A restore unpacked last time is swapped in before anything opens.
            // The app's log first, so everything startup does is in it.
            applog::init(&paths.logs);
            match &erased {
                Ok(Some(e)) => applog::line("consent", &format!("Deleted everything as asked: {} entries removed, {} left (in use)", e.removed, e.left.len())),
                Err(e) => applog::line("consent", &format!("Didn't delete everything: {e}")),
                Ok(None) => {}
            }
            backup::apply_pending(&paths.data)?;
            let conn = db::open(&paths.db)?;
            let vault = open_vault(&paths)?;
            // The graphics card picked for models (by name), before the PC is checked.
            hardware::set_main(db::get::<Option<String>>(&conn, "main_gpu").flatten());
            hardware::set_linked(db::get::<Vec<hardware::LinkedMachine>>(&conn, "linked_machines").unwrap_or_default());
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
            uncensor::clean_leftovers(&state.paths);
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
            // Off the window's thread: the first run hashes every engine once,
            // later ones only what changed.
            std::thread::spawn({
                let state = state.clone();
                move || {
                    let r = integrity::check_all(&state.paths, &state.catalog);
                    applog::line("security", &r.summary());
                    for p in r.problems() {
                        applog::line("security", &format!("{}: {} ({})", p.what, p.note, p.path));
                    }
                    if r.has_problems() {
                        if let Some(app) = state.app.get() {
                            let _ = app.emit("security:integrity", &r);
                        }
                    }
                }
            });
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
            telegram::start(app.handle().clone(), state.clone());
            matrix::start(app.handle().clone(), state.clone());
            discord::start(app.handle().clone(), state.clone());
            clipboard::apply(state.clone());
            medialib::timeline::start(state.clone());
            calls::apply(app.handle(), &state);
            backup_auto::start(state.clone());
            wake::start(app.handle().clone(), state.clone());
            app.manage(state);
            schedule::start(app.handle().clone());
            notes::start_reminders(app.handle().clone());
            perf::start_monitor(app.handle().clone());
            mail::start_sync(app.handle().clone());
            quick::apply(app.handle());
            bridge::apply(app.handle());
            calendar::start_sync(app.handle().clone());
            updates::start(app.handle().clone());
            security_notice_feed::start(app.handle().clone());
            shield::live::start(app.handle().clone(), app.state::<Arc<AppState>>().inner().clone());
            privacy::start(app.handle().clone(), app.state::<Arc<AppState>>().inner().clone());
            sync::start(app.handle().clone());
            phone::apply(app.state::<Arc<AppState>>().inner().clone());
            p2p::start(app.state::<Arc<AppState>>().inner().clone());
            p2p::sharing::init(app.handle(), app.state::<Arc<AppState>>().inner());
            p2p::open_links::listen(app.handle());
            {
                let state = app.state::<Arc<AppState>>().inner().clone();
                pause::start(app.handle(), &state, for_tasks);
                if for_tasks {
                    quick::show_paused(app.handle(), false);
                }
                power::watch_screen(perf::settings(&state.db.lock().unwrap()).away_full_screen);
                let handle = app.handle().clone();
                power::listen(move |e| {
                    let app = handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let state = app.state::<Arc<AppState>>().inner().clone();
                        match e {
                            power::Event::Power => {
                                let s = state.clone();
                                let _ = tokio::task::spawn_blocking(move || s.recheck_hardware("plugging in or unplugging")).await;
                                perf::mode_changed(&state).await;
                            }
                            power::Event::Woke => quiet::changed(),
                            power::Event::FullScreen(_) => perf::mode_changed(&state).await,
                        }
                    });
                });
                tauri::async_runtime::spawn(async move { perf::mode_changed(&state).await });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            medialib::media_subtitles,
            medialib::media_play_file,
            medialib::media_find_subtitles,
            medialib::media_timeline_day,
            medialib::media_timeline_search,
            medialib::media_timeline_image,
            medialib::media_timeline_clear,
            medialib::media_live_captions_start,
            medialib::media_live_captions_stop,
            medialib::media_live_captions_save,
            medialib::media_photo_folders,
            medialib::media_set_photo_folders,
            medialib::media_photos,
            medialib::media_photo_thumb,
            medialib::media_screenshot,
            medialib::media_feeds,
            medialib::media_feed_add,
            medialib::media_feed_remove,
            medialib::media_feed_fetch,
            medialib::media_index_add,
            medialib::media_index_list,
            medialib::media_index_get,
            medialib::media_index_remove,
            medialib::media_index_search,
            medialib::media_index_rename_speaker,
            medialib::media_index_save_captions,
            medialib::media_feed_parse,
            medialib::media_ebook_open,
            medialib::media_timeline_view,
            medialib::media_timeline_set,
            medialib::media_timeline_set_pin,
            perf::perf_view,
            media::create::commands::create_consent_check,
            media::create::commands::create_storyboard,
            media::create::commands::create_batch_parse,
            media::create::commands::create_dialogue_parts,
            media::create::commands::create_sfx_plan,
            media::create::commands::create_text_cut,
            media::create::commands::create_lipsync_pieces,
            media::create::commands::create_region_scene,
            media::create::commands::create_mic_check,
            media::create::commands::create_transcript,
            media::create::commands::create_control_nets,
            media::create::commands::create_face_chat_check,
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
            placement::linked_machines,
            placement::set_linked_machines,
            security::set_keep_working,
            browser::open_browser,
            browsers::browsers_view,
            location::detect_location,
            bridge::bridge_status,
            bridge::bridge_pair,
            bridge::browser_sites,
            bridge::browser_site_remove,
            bridge::browser_logs_get,
            bridge::browser_logs_set,
            bridge::show_extension_folder,
            browsers::set_main_browser,
            browsers::set_browser_search,
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
            quick::quick_find,
            quick::quick_open,
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
            media::media_save_captions,
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
            media::media_talk_ways,
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
            api_server::set_ollama_server,
            api_server::set_ollama_home,
            cloud::cmds::cloud_view,
            cloud::cmds::add_cloud_provider,
            cloud::cmds::remove_cloud_provider,
            cloud::cmds::set_cloud_provider,
            cloud::cmds::cloud_models_available,
            cloud::cmds::set_cloud_models,
            cloud::cmds::set_cloud_media,
            cloud::cmds::set_cloud_options,
            cloud::cmds::cloud_choices,
            cloud::cmds::set_cloud_sharing,
            cloud::cmds::set_cloud_proxy,
            cloud::cmds::cloud_sign_in,
            cloud::cmds::chat_cloud_spend,
            policy::policy_view,
            cloud::ent_cmds::set_cloud_enterprise,
            cloud::ent_cmds::cloud_entra_start,
            cloud::ent_cmds::cloud_entra_sign_out,
            cloud::ent_cmds::set_cloud_headers,
            cloud::ent_cmds::set_cloud_ca,
            cloud::routing::cloud_route,
            cloud::routing::set_cloud_route,
            cloud::usage::cloud_usage,
            cloud::usage::cloud_usage_csv,
            backup::make_backup,
            phone::phone_view,
            phone::set_phone,
            phone::add_phone,
            phone::remove_phone,
            phone::set_phone_while_locked,
            phone::set_phone_link,
            phone::set_phone_own_relay,
            phone::set_phone_worker,
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
            social::store::social_view,
            social::live::social_dm_send,
            social::live::social_dms,
            social::live::social_dm_answer,
            social::live::social_call_join,
            social::live::social_call_leave,
            social::live::social_call_piece,
            social::live::social_call_view,
            social::store::social_set_switches,
            social::store::social_set_teen,
            social::store::social_create,
            social::store::social_invite,
            social::store::social_join,
            social::store::social_act,
            social::store::social_leave,
            social::store::social_send,
            social::store::social_messages,
            social::store::social_report,
            social::store::social_mod_queue,
            social::store::social_release,
            social::store::social_resolve_report,
            social::store::social_set_rules,
            social::store::social_block,
            social::store::social_catch_up,
            social::store::social_translate,
            social::store::social_add_assistant,
            social::store::social_doc,
            social::store::social_doc_edit,
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
            uncensor::uncensor_look,
            uncensor::start_uncensor,
            uncensor::cancel_uncensor,
            uncensor::uncensor_job,
            uncensor::uncensor_clear,
            practice::start_practice,
            updates::update_view,
            updates::check_for_update,
            updates::set_update_auto,
            updates::set_update_install,
            privacy::privacy_view,
            privacy::set_privacy,
            shield::live::shield_view,
            shield::live::set_shield,
            shield::live::shield_new_address,
            shield::live::shield_wifi_settings,
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
            peer_updates::peer_updates_view,
            peer_updates::set_peer_updates,
            peer_updates::install_update_from_peer,
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
            manager_get,
            manager_set,
            manager_plan,
            manager_answer,
            manager_redo,
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
            import::preview_import,
            import::apply_import,
            import::apply_pasted_memories,
            import::list_imports,
            import::undo_import,
            lens::lens_layers,
            lens::lens_edit,
            steer::lens_directions,
            steer::lens_make_direction,
            steer::lens_set_direction,
            steer::lens_delete_direction,
            chat_live,
            send_message,
            chat_regenerate,
            chat_edit,
            chat_versions,
            versions::go_back_preview_cmd,
            versions::go_back_cmd,
            chat_switch_version,
            chatsearch::search_chats,
            chatsearch::export_chat,
            presets::model_preset,
            presets::set_model_preset,
            prompts::saved_prompts,
            prompts::save_prompt,
            prompts::delete_prompt,
            compare::compare_models,
            compare::compare_stop,
            assistants::assistants,
            assistants::save_assistant,
            assistants::delete_assistant,
            assistants::chat_with_assistant,
            assistants::chat_assistant,
            engine_status,
            unload_model,
            security::security_status,
            consent::consent_status,
            consent::consent_accept,
            consent::export_everything_cmd,
            consent::erase_everything,
            consent::erase_status,
            consent::cancel_erase_everything,
            consent::consent_decline,
            api_server::access_keys_view,
            api_server::access_key_create,
            api_server::access_key_set,
            api_server::access_key_revoke,
            model_fit::model_fit_view,
            model_fit::set_feature_model,
            model_fit::model_fit_test,
            vault_cmds::vault_status,
            vault_cmds::vault_set_enabled,
            vault_cmds::vault_setup,
            vault_cmds::vault_unlock,
            vault_cmds::vault_recover,
            vault_cmds::vault_lock,
            vault_cmds::vault_change_master,
            vault_cmds::vault_new_recovery_key,
            vault_cmds::vault_list,
            vault_cmds::vault_add,
            vault_cmds::vault_edit,
            vault_cmds::vault_delete,
            vault_cmds::vault_reveal,
            vault_cmds::vault_generate,
            vault_cmds::vault_import,
            vault_cmds::vault_export,
            vault_cmds::vault_check,
            vault_cmds::vault_hide_from_capture,
            security_page::security_overview,
            security_page::security_apply_fix,
            security_notice_feed::security_notice_view,
            security_notice_feed::security_notice_check,
            p2p::p2p_status,
            p2p::p2p_set_network,
            p2p::p2p_dial,
            security::enable_lock,
            security::change_pin,
            security::reset_pin,
            security::disable_lock,
            security::unlock,
            security::unlock_with_hello,
            security::lock_now,
            security::set_hello,
            security::set_auto_lock,
            protected::protect_chat,
            protected::open_protected_chat,
            protected::open_protected_chat_hello,
            protected::shut_protected_chat,
            cloudfiles::cloud_files,
            cloudfiles::cloud_files_connect,
            cloudfiles::cloud_files_remove,
            wake::wake_word,
            wakeup::wake_for_tasks,
            wakeup::set_wake_for_tasks,
            media::live_voice::live_voice_start,
            media::live_voice::live_voice_stop,
            wake::set_wake_word,
            telegram::telegram_view,
            telegram::telegram_connect,
            telegram::telegram_set_tasks,
            telegram::telegram_disconnect,
            matrix::matrix_view,
            matrix::matrix_connect,
            matrix::matrix_set_tasks,
            matrix::matrix_disconnect,
            discord::discord_view,
            discord::discord_connect,
            discord::discord_set_tasks,
            discord::discord_disconnect,
            clipboard::clipboard_view,
            clipboard::clipboard_set,
            clipboard::clipboard_copy,
            clipboard::clipboard_forget,
            quotes::market_analytics,
            quotes::set_market_analytics,
            projects::set_memory_category,
            projects::memory_sensitive,
            projects::set_memory_sensitive,
            own_skills::skill_from_chat,
            own_skills::save_own_skill,
            own_skills::remove_own_skill,
            side_chat::side_ask,
            meeting::set_meeting_typed,
            compare::compare_merge,
            assistants::import_character,
            media::podcast::podcast_ask,
            cast::chat_cast,
            cast::set_chat_cast,
            model_dupes::duplicate_models,
            p2p::invites::p2p_make_invite,
            p2p::p2p_set_options,
            p2p::p2p_set_relay,
            p2p::relays::p2p_join_relay,
            p2p::relays::p2p_relay_admin,
            p2p::host::p2p_local_relay,
            p2p::host::p2p_local_relay_start,
            p2p::host::p2p_local_relay_stop,
            p2p::host::p2p_hosting_guide,
            p2p::host::p2p_set_own_hosting,
            p2p::host::p2p_local_relay_at_boot,
            shared_relay::shared_relay_closes,
            p2p::invites::p2p_read_invite,
            p2p::sharing::sharing_settings,
            p2p::sharing::sharing_set,
            p2p::sharing::share_make,
            p2p::sharing::share_list,
            p2p::sharing::share_stop,
            p2p::sharing::share_read,
            p2p::sharing::model_card_sign,
            p2p::sharing::model_card_add,
            p2p::sharing::model_card_list,
            p2p::sharing::model_card_forget,
            p2p::sharing::vlan_create,
            p2p::sharing::vlan_list,
            p2p::sharing::vlan_admit,
            p2p::sharing::vlan_leave,
            p2p::sharing::remote_status,
            p2p::sharing::remote_accept,
            p2p::sharing::remote_stop,
            p2p::sharing::share_fetch,
            p2p::sharing::model_card_fetch,
            p2p::sharing::remote_connect,
            p2p::sharing::remote_send,
            p2p::sharing::remote_disconnect,
            p2p::sharing::vlan_connect,
            p2p::sharing::vlan_disconnect,
            p2p::sharing::vpn_list,
            p2p::sharing::vlan_status,
            p2p::open_links::share_opened,
            p2p::invites::p2p_use_invite,
            p2p::invites::p2p_friends,
            backup_auto::auto_backup_view,
            backup_auto::set_auto_backup,
            files::find_folders, files::find_add_folder, files::find_remove_folder, files::find_refresh, files::find_search, files::find_duplicates, files::find_set_caption,
            files::tidy_plan, files::tidy_apply, files::pdf_passages, files::pdf_compare, files::pdf_scanned_pages, files::sheet_plan, files::sheet_run,
            files::records_list, files::records_delete, files::inventory_save, files::inventory_totals, files::inventory_export, files::form_save, files::pdf_redact, files::scan::scan_save, files::scan::find_describe_pictures, files::pdf_form_fields, files::pdf_form_fill, files::scan::pdf_read_scans,
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
            quick::set_dictate_anywhere,
            quick::quick_selection,
            quick::quick_replace,
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
            connectors::connector_sign_in,
            connectors::connector_sign_out,
            plugin_checks::plugin_check_problems,
            connectors::list_plugins,
            connectors::inspect_plugin,
            connectors::install_plugin,
            connectors::set_plugin_enabled,
            connectors::remove_plugin,
            tools::allowances::list_allowances,
            tools::allowances::save_allowance,
            tools::allowances::remove_allowance,
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
            writing::writing_switches,
            writing::writing_set_switch,
            writing::partner_round,
            writing::partner_answer,
            writing::partner_progress,
            writing::writing_check,
            writing::writing_fix,
            writing::writing_rewrite,
            writing::scam_check,
            writing::pc_health,
            writing::social_accounts,
            writing::social_add,
            writing::social_remove,
            writing::social_post,
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
            personal::personal_enabled,
            personal::personal_set_enabled,
            personal::personal_list,
            personal::personal_save,
            personal::personal_delete,
            personal::personal_from_mail,
            personal::personal_tasks_from_text,
            personal::personal_profile_check,
            personal::cmds::personal_scan_mail,
            personal::cmds::personal_overview,
            personal::cmds::personal_leave_at,
            personal::cmds::personal_rota,
            personal::cmds::personal_scale_recipe,
            personal::cmds::personal_shopping_list,
            personal::cmds::personal_breathing,
            personal::cmds::personal_health_trend,
            notes::parse_due_text,
            notes::tasks_from_meeting,
            web::get_web_settings,
            web::set_web_settings,
            selftest::selftest_seen,
            rest::window_rest,
            pause::pause_view,
            pause::set_pause_settings,
            pause::pause_now,
            pause::resume_now,
            pause::quit_now,
            zero::zero_view,
        ])
        .build(tauri::generate_context!())
        .expect("error while building SulcusAI");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            crash::closing();
            let state = handle.state::<Arc<AppState>>().inner().clone();
            pause::on_exit(&state);
            tauri::async_runtime::block_on(async move {
                state.engine.lock().await.stop().await;
                speech::stop(&state).await;
            });
        }
    });
}

#[cfg(test)]
mod lib_tests;

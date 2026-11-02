// SPDX-License-Identifier: AGPL-3.0-only
//! The window's commands for settings, sign-in registrations, the context
//! window, the default model and the profile.

use super::*;

/// The app registrations for signing in with Microsoft and Google.
#[tauri::command(async)]
pub(super) fn oauth_clients(state: AppStateRef) -> Result<oauth::ClientsView, String> {
    state.cipher()?;
    Ok(oauth::clients_view(&state.db.lock().unwrap()))
}

#[tauri::command(async)]
pub(super) fn set_oauth_clients(state: AppStateRef, ids: oauth::ClientIds) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    oauth::set_clients(&conn, &ids)?;
    db::log_action(&conn, "privacy", "Changed the sign-in app registrations");
    Ok(())
}

#[tauri::command(async)]
pub(super) fn get_settings(state: AppStateRef) -> Settings {
    state.settings()
}

#[tauri::command(async)]
pub(super) fn set_connectivity(app: AppHandle, state: AppStateRef, level: Connectivity) -> Result<Settings, String> {
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
pub(super) fn rename_model(state: AppStateRef, model_id: String, name: String) -> Result<(), String> {
    let name = name.trim();
    if name.chars().count() > 60 {
        return Err("Keep the name under 60 characters.".into());
    }
    let conn = state.db.lock().unwrap();
    db::set_model_name(&conn, &model_id, (!name.is_empty()).then_some(name))
}

#[derive(Serialize)]
pub(super) struct WindowOption {
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
pub(super) struct ContextOptions {
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
pub(super) fn context_options(state: AppStateRef, model_id: Option<String>) -> Result<ContextOptions, String> {
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
pub(super) fn set_context_window(state: AppStateRef, size: Option<u32>) -> Result<(), String> {
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
pub(super) async fn compact_chat(app: AppHandle, state: AppStateRef<'_>, chat_id: String) -> Result<(), String> {
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
pub(super) fn set_auto_model(state: AppStateRef, on: bool) -> Result<Settings, String> {
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
    } else if let Some(sid) = steer::parse_ref(model_id) {
        steer::exists(conn, sid)
    } else {
        db::installed_model(conn, model_id).is_some()
    }
}

#[tauri::command]
pub(super) fn set_default_model(state: AppStateRef, model_id: String) -> Result<Settings, String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    if !model_known(&conn, &model_id) {
        return Err("That model isn't available.".into());
    }
    db::update_settings(&conn, |s| s.default_model = Some(model_id))
}

#[tauri::command(async)]
pub(super) fn finish_onboarding(state: AppStateRef) -> Result<Settings, String> {
    let conn = state.db.lock().unwrap();
    db::update_settings(&conn, |s| s.onboarded = true)
}

#[tauri::command(async)]
pub(super) fn get_profile(state: AppStateRef) -> Result<Profile, String> {
    let c = state.cipher()?;
    Ok(db::profile(&state.db.lock().unwrap(), &c))
}

#[tauri::command(async)]
pub(super) fn set_profile(state: AppStateRef, profile: Profile) -> Result<(), String> {
    let c = state.cipher()?;
    db::set_profile(&state.db.lock().unwrap(), &c, &profile)
}

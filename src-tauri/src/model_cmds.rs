// SPDX-License-Identifier: AGPL-3.0-only
//! How a model is started (its window, thinking budget and variant), models
//! found from other apps, speed tests, tuning, and removing models.

use super::*;

/// Context, GPU layers, threads and batch size to start a model with:
/// worked out for this PC, unless advanced mode says otherwise.
/// The context window the person chose (None: automatic).
pub(super) fn chosen_window(state: &AppState) -> Option<u32> {
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
pub(super) fn launch_plan(state: &AppState, spec: &ModelSpec, quant: &str, encoder: u64) -> Plan {
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
    // This model's own settings come before Advanced's.
    let mut e = advanced::get(state).engine;
    let own = presets::get(&state.db.lock().unwrap(), &spec.id);
    if own.ctx.is_some() {
        e.ctx = own.ctx;
    }
    e.kv_cache = own.kv_cache.or(e.kv_cache);
    e.flash_attn = own.flash_attn.or(e.flash_attn);
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
    // A cache type chosen in Advanced or the model's settings. A compressed
    // cache needs flash attention, so with it off the cache stays whole.
    let mut kv_q4 = false;
    match presets::cache_type(e.kv_cache.as_deref()) {
        Some("f16") => kv_q8 = false,
        Some("q8_0") => kv_q8 = true,
        Some("q4_0") => (kv_q8, kv_q4) = (false, true),
        _ => {}
    }
    if e.flash_attn == Some(false) {
        (kv_q8, kv_q4) = (false, false);
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
    Plan { ctx, gpu_layers: layers, threads, batch: e.batch.unwrap_or(0), tuning: engine::Tuning { cpu_moe, kv_q8, kv_q4, flash: e.flash_attn, think_budget, spread, embedding: false }, budget: budget.with_limits(&limits) }
}

/// Seconds a reply may think before it's told to answer. Qwen3.6 27B, a
/// little too big for a 16 GB card, thought for minutes at a time: up to
/// 16K tokens a reply, at about 10 tokens a second.
pub(super) const THINK_SECS: f64 = 75.0;

/// The most tokens a thinking model may think in a reply: about
/// `THINK_SECS` at its measured speed (else the estimate), 1K to 8K. None
/// when it doesn't think, or thinking is set on (no limit) or off.
pub(super) fn think_budget(state: &AppState, spec: &ModelSpec, est_tps: f64) -> Option<u32> {
    let setting = advanced::get(state).processing.thinking;
    if setting != advanced::Thinking::Auto || !thinks(spec, setting) {
        return None;
    }
    let measured = db::installed_model(&state.db.lock().unwrap(), &spec.id).and_then(|m| m.tps).filter(|t| *t > 0.0);
    think_tokens(measured.unwrap_or(est_tps))
}

/// `THINK_SECS` of thinking at this speed, 1K to 8K tokens.
pub(super) fn think_tokens(tps: f64) -> Option<u32> {
    (tps > 0.0).then(|| (tps * THINK_SECS).clamp(1024.0, 8192.0) as u32)
}

/// The versions of a model this PC can run, the best suited first.
pub(super) fn runnable_variants<'a>(spec: &'a ModelSpec, fit: &ModelFit) -> Vec<&'a catalog::Variant> {
    let mut list: Vec<_> = spec.variants.iter().filter(|v| fit.variant(&v.quant).is_some_and(|f| f.runnable())).collect();
    list.sort_by_key(|v| fit.recommended.as_deref() != Some(v.quant.as_str()));
    list
}

/// Catalog chat models whose files are already on this PC (from an earlier
/// install or an exact copy in another app) join the list without a
/// download. The engine comes with the first chat if it isn't here yet.
/// Returns the names added.
pub(super) fn adopt_found(state: &AppState, finder: &mut found::Finder) -> Vec<String> {
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
pub(super) struct FoundModels {
    /// Names of the models added.
    pub(super) added: Vec<String>,
    /// Models seen that can't be used, with why.
    skipped: Vec<local::Skipped>,
    /// The apps whose folders exist on this PC.
    looked_in: Vec<&'static str>,
}

/// Looks for models already on this PC and adds them. `again`: the user
/// asked, so models they stopped using earlier come back too.
pub(super) fn find_models_now(state: &AppState, again: bool) -> FoundModels {
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

pub(super) fn announce_found(app: &AppHandle, names: &[String]) {
    if !names.is_empty() {
        app.emit("models:found", json!({ "names": names })).ok();
        app.emit("features:changed", json!({})).ok();
    }
}

#[tauri::command]
pub(super) async fn find_models(app: AppHandle, state: AppStateRef<'_>, again: Option<bool>) -> Result<FoundModels, String> {
    let state = state.inner().clone();
    let found = tauri::async_runtime::spawn_blocking(move || find_models_now(&state, again.unwrap_or(false))).await.map_err(|e| e.to_string())?;
    announce_found(&app, &found.added);
    Ok(found)
}

/// Adds the .gguf models in a folder (or one file) the user picked or typed;
/// they're used where they are.
#[tauri::command]
pub(super) async fn import_model_folder(app: AppHandle, state: AppStateRef<'_>, path: String) -> Result<local::FolderImport, String> {
    let state = state.inner().clone();
    // Windows' "Copy as path" adds quotes.
    let path = std::path::PathBuf::from(path.trim().trim_matches('"'));
    let r = tauri::async_runtime::spawn_blocking(move || local::import_folder(&state, &path)).await.map_err(|e| e.to_string())??;
    announce_found(&app, &r.added);
    Ok(r)
}

/// Adds a .gguf model file the user picked; it's used where it is.
#[tauri::command]
pub(super) async fn import_model_file(app: AppHandle, state: AppStateRef<'_>, path: String) -> Result<String, String> {
    let state = state.inner().clone();
    let name = tauri::async_runtime::spawn_blocking(move || local::import(&state, std::path::Path::new(&path))).await.map_err(|e| e.to_string())??;
    announce_found(&app, std::slice::from_ref(&name));
    Ok(name)
}

pub(super) static SPEED_TESTING: AtomicBool = AtomicBool::new(false);

/// What the speed test is doing, so a Models page opened mid-test (or after)
/// shows it: the model being tested, how far along, and what didn't finish.
#[derive(Clone, Serialize)]
pub(super) struct SpeedStatus {
    running: bool,
    /// Testing every installed model (not just one).
    all: bool,
    model_id: Option<String>,
    done: usize,
    total: usize,
    /// Models whose test didn't finish, with why (kept until the next test).
    errors: std::collections::BTreeMap<String, String>,
}

pub(super) static SPEED_STATUS: std::sync::Mutex<SpeedStatus> =
    std::sync::Mutex::new(SpeedStatus { running: false, all: false, model_id: None, done: 0, total: 0, errors: std::collections::BTreeMap::new() });

/// Updates the status and tells the window (`models:speed`).
pub(super) fn speed_progress(app: &AppHandle, update: impl FnOnce(&mut SpeedStatus), extra: serde_json::Value) {
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
pub(super) async fn test_speed(state: &Arc<AppState>, model_id: &str) -> Result<engine::Speed, String> {
    let (ep, spec, _) = llm_endpoint(state, Some(model_id.to_string()), false, |_| {}).await?;
    let sp = engine::measure(&ep).await?;
    db::set_speed(&state.db.lock().unwrap(), model_id, sp.tps, sp.prompt_tps)?;
    state.log("model", &format!("Measured {}: writes {:.1} tokens/sec, reads {} tokens/sec", spec.name, sp.tps, sp.prompt_tps.map(|p| format!("{p:.0}")).unwrap_or_else(|| "?".into())));
    Ok(sp)
}

/// Swapping models mid-reply would cut a chat off.
pub(super) fn speed_test_allowed(state: &AppState) -> Result<(), String> {
    if !state.generations.lock().unwrap().is_empty() {
        return Err("Wait for the reply to finish, then test the speed.".into());
    }
    if finetune::training() {
        return Err("A model is being taught right now and has the graphics card. Try again when it's done.".into());
    }
    if uncensor::running() {
        return Err(uncensor::BUSY.into());
    }
    Ok(())
}

/// Where the speed test is (or what the last one left).
#[tauri::command(async)]
pub(super) fn speed_test_status() -> SpeedStatus {
    SPEED_STATUS.lock().unwrap().clone()
}

/// "Optimize for this PC" for one chat model. Progress arrives as
/// `models:optimize` events ({model_id, step, of, label}); the last one has
/// `finished` with the report or an error.
pub(super) async fn optimize_now(app: &AppHandle, state: &Arc<AppState>, model_id: &str) -> Result<optimize::Report, String> {
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
pub(super) async fn optimize_model(app: AppHandle, state: AppStateRef<'_>, model_id: String) -> Result<optimize::Report, String> {
    let state = state.inner().clone();
    state.cipher()?;
    optimize_now(&app, &state, &model_id).await
}

/// Settings measured on this PC, per model.
#[tauri::command(async)]
pub(super) fn model_tunings(state: AppStateRef) -> Result<HashMap<String, optimize::Tuned>, String> {
    state.cipher()?;
    Ok(optimize::all(&state.db.lock().unwrap()))
}

/// Back to the automatic settings for a model.
#[tauri::command(async)]
pub(super) fn forget_optimization(state: AppStateRef, model_id: String) -> Result<(), String> {
    state.cipher()?;
    optimize::forget(&state.db.lock().unwrap(), &model_id)
}

/// Runs the speed test for an installed chat model.
#[tauri::command]
pub(super) async fn measure_model_speed(app: AppHandle, state: AppStateRef<'_>, model_id: String) -> Result<engine::Speed, String> {
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
pub(super) async fn measure_all_speeds(app: AppHandle, state: AppStateRef<'_>) -> Result<usize, String> {
    let state = state.inner().clone();
    speed_test_allowed(&state)?;
    if SPEED_TESTING.swap(true, Ordering::SeqCst) {
        return Err("A speed test is already running.".into());
    }
    // Read first, so the lock is let go before model_spec takes it again.
    let installed = db::installed_models(&state.db.lock().unwrap());
    let ids: Vec<String> = installed
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
pub(super) fn cancel_install(app: AppHandle, state: AppStateRef, model_id: String) {
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
pub(super) async fn remove_model(state: AppStateRef<'_>, model_id: String, delete_file: Option<bool>) -> Result<u64, String> {
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

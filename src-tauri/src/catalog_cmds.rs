// SPDX-License-Identifier: AGPL-3.0-only
//! The window's commands for the app, the hardware, the model catalog,
//! installs and Hugging Face links.

use super::*;

#[derive(Serialize)]
pub(super) struct AppInfo {
    name: &'static str,
    version: &'static str,
    data_dir: String,
}

#[tauri::command(async)]
pub(super) fn app_info(state: AppStateRef) -> AppInfo {
    AppInfo {
        name: "SulcusAI",
        version: env!("CARGO_PKG_VERSION"),
        data_dir: state.paths.data.display().to_string(),
    }
}

/// Saves a chart from a chat (run_code, market_quote) as an SVG file where the user picked.
#[tauri::command(async)]
pub(super) fn save_chart(state: AppStateRef, path: String, svg: String) -> Result<(), String> {
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
pub(super) fn refresh_hardware(state: AppStateRef) -> Hardware {
    engine::forget_devices();
    if !state.recheck_hardware("checking again") {
        applog::line("app", &format!("Checked the graphics again: {}", graphics_line(&state.hardware.read().unwrap())));
    }
    state.hardware.read().unwrap().clone()
}

/// The PC's graphics for the app log: "NVIDIA GeForce RTX 3050 Laptop GPU
/// (4.3 GB), Intel(R) UHD Graphics (0.1 GB, built-in)", and any card that
/// isn't working.
pub(super) fn graphics_line(hw: &Hardware) -> String {
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
pub(super) async fn recheck_if_card_missing(state: &Arc<AppState>) {
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


#[derive(Serialize)]
pub(super) struct ModelCard {
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
pub(super) struct CatalogView {
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
pub(super) fn catalog_view(state: AppStateRef, all: Option<bool>) -> CatalogView {
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
pub(super) struct InstallProgress<'a> {
    model_id: &'a str,
    phase: &'a str,
    received: u64,
    total: u64,
}

#[tauri::command(async)]
pub(super) fn install_model(app: AppHandle, state: AppStateRef, model_id: String, quant: String, force: Option<bool>) -> Result<(), String> {
    let spec = state.catalog.model(&model_id).cloned().or_else(|| hub::list(&state.db.lock().unwrap()).into_iter().find(|m| m.id == model_id)).ok_or("Unknown model")?;
    start_install(app, state.inner().clone(), spec, quant, force.unwrap_or(false))
}

/// Whether a model file has multi-token-prediction layers, read once per
/// file (its header is read for each Models page otherwise).
pub(super) fn has_mtp(path: &std::path::Path) -> bool {
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
pub(super) const BEFORE_FASTER: &str = "before-faster";

/// The faster (MTP) file for a model installed before the catalog had it.
/// The old file is kept until the new one is in: a download that fails or
/// is stopped leaves it to come back with Repair.
#[tauri::command(async)]
pub(super) fn faster_model(app: AppHandle, state: AppStateRef, model_id: String) -> Result<(), String> {
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
pub(super) fn repair_model(app: AppHandle, state: AppStateRef, model_id: String) -> Result<(), String> {
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
pub(super) fn pause_install(state: AppStateRef, model_id: String) {
    if let Some(flag) = state.installs.lock().unwrap().get(&model_id) {
        downloads::mark_pausing(&model_id);
        flag.store(true, Ordering::Relaxed);
    }
}

/// Continues a paused download from where it stopped.
#[tauri::command(async)]
pub(super) fn resume_install(app: AppHandle, state: AppStateRef, model_id: String) -> Result<(), String> {
    let paused = downloads::list(&state.db.lock().unwrap()).into_iter().find(|p| p.model_id == model_id).ok_or("That download isn't paused anymore.")?;
    let spec = state.model_spec(&model_id).ok_or("That model isn't available anymore.")?;
    // It was started knowingly, so a warning it got then doesn't stop it now.
    start_install(app, state.inner().clone(), spec, paused.quant, true)
}

#[derive(Serialize)]
pub(super) struct DownloadsView {
    paused: Vec<downloads::Paused>,
}

/// Paused downloads (the running one comes as `install:progress` events).
#[tauri::command(async)]
pub(super) fn downloads_view(state: AppStateRef) -> DownloadsView {
    DownloadsView { paused: downloads::list(&state.db.lock().unwrap()) }
}

/// Searches Hugging Face for GGUF chat models; only files this PC can run.
#[tauri::command]
pub(super) async fn hub_search(state: AppStateRef<'_>, query: String, sort: String, all: Option<bool>, purpose: Option<String>) -> Result<hub::HubResults, String> {
    hub::search(state.inner(), &query, &sort, all.unwrap_or(false), purpose.as_deref().unwrap_or("")).await
}

/// A pasted Hugging Face link, as the model it points at.
#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum HubLink {
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
pub(super) async fn hub_link(state: AppStateRef<'_>, link: String) -> Result<HubLink, String> {
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

/// The most downloaded GGUF copy of a model that has a file SulcusAI can load.
/// Without it, a pasted openai/gpt-oss-120b (not in GGUF) was turned away
/// when ggml-org/gpt-oss-120b-GGUF was right there.
pub(super) async fn gguf_copy_of(state: &AppState, client: &reqwest::Client, of: &str) -> Option<HubLink> {
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

/// Why a pasted model can't be used, by what kind of model it is ("GGUF" in
/// the old message read as a broken file).
pub(super) fn hub_why_not(info: &serde_json::Value) -> String {
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
pub(super) async fn hub_install(app: AppHandle, state: AppStateRef<'_>, repo: String, file: String, force: Option<bool>) -> Result<String, String> {
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
pub(super) fn start_install(app: AppHandle, state: Arc<AppState>, mut spec: ModelSpec, quant: String, force: bool) -> Result<(), String> {
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
            if !state.generations.lock().unwrap().is_empty() || finetune::training() || uncensor::running() {
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
                            steer: None,
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

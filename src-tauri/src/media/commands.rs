// SPDX-License-Identifier: AGPL-3.0-only
//! The window's commands for the gallery, models, jobs, wallpapers and
//! Compose.

use super::*;

#[derive(Serialize)]
pub struct MediaCard {
    #[serde(flatten)]
    pub(super) spec: MediaModel,
    pub(super) fit: MediaFit,
    pub(super) installed: bool,
}

#[derive(Serialize)]
pub struct MediaView {
    pub(super) models: Vec<MediaCard>,
    /// Cloud picture, video and voice models usable now (Cloud level).
    pub(super) cloud: Vec<crate::cloud::MediaChoice>,
    /// Models that can't run on this PC, left out of the list.
    pub(super) hidden: usize,
    /// The same, by kind and why, so each kind's list can say so.
    pub(super) hidden_by_kind: Vec<Hidden>,
    /// Why video isn't available here, when no video model fits.
    pub(super) video_note: Option<String>,
    pub(super) installing: Vec<String>,
    pub(super) jobs: Vec<JobInfo>,
    pub(super) gallery: usize,
    /// The default model per kind.
    pub defaults: MediaDefaults,
}

/// Models of a kind left out of the list, and why: "memory" (more than this
/// PC has), "card" (a bigger graphics card) or "disk" (more free space).
#[derive(Debug, Serialize, PartialEq)]
pub struct Hidden {
    pub(super) kind: Kind,
    pub(super) why: &'static str,
    pub(super) count: usize,
}

/// Why a model that doesn't fit is left out, in one word.
pub(super) fn hidden_why(m: &MediaModel, f: &MediaFit) -> &'static str {
    if f.runnable {
        "disk"
    } else if matches!(m.kind, Kind::Music | Kind::Background) || f.reason.as_deref().is_some_and(|r| r.starts_with("needs about")) {
        "memory"
    } else {
        "card"
    }
}

pub(super) fn count_hidden(list: &mut Vec<Hidden>, kind: Kind, why: &'static str) {
    match list.iter_mut().find(|h| h.kind == kind && h.why == why) {
        Some(h) => h.count += 1,
        None => list.push(Hidden { kind, why, count: 1 }),
    }
}

/// The Studio's models: the ones this PC runs, or with `all`, every one
/// (those it can't run say why and can't be installed).
#[tauri::command(async)]
pub fn media_view(state: AppStateRef, all: Option<bool>) -> MediaView {
    view(&state, all.unwrap_or(false))
}

pub(super) fn view(state: &AppState, all: bool) -> MediaView {
    let hw = state.hardware.read().unwrap().clone();
    let b = Budget::from_hardware(&hw);
    let list = installed(&state.db.lock().unwrap());
    let mut hidden = 0;
    let mut hidden_by_kind = Vec::new();
    let mut video_note = None;
    let mut models = Vec::new();
    let installing: Vec<String> = state.installs.lock().unwrap().keys().cloned().collect();
    for m in &self::models(state) {
        let inst = list.iter().find(|i| i.model_id == m.id);
        // From Hugging Face: shown once installed (or while it installs).
        if m.from_hub.is_some() && inst.is_none() && !installing.contains(&m.id) {
            continue;
        }
        let f = fit(m, &hw, &b, missing_bytes(&state.paths, m), inst.and_then(|i| i.secs));
        if inst.is_none() && !(f.runnable && f.disk_ok) {
            hidden += 1;
            count_hidden(&mut hidden_by_kind, m.kind, hidden_why(m, &f));
            if m.kind == Kind::Video && video_note.is_none() {
                video_note = f.reason.clone().map(|r| format!("Video isn't available on this PC: the smallest video model {r}."));
            }
            if !all {
                continue;
            }
        }
        models.push(MediaCard { spec: m.clone(), fit: f, installed: inst.is_some() });
    }
    if models.iter().any(|c| c.spec.kind == Kind::Video) {
        video_note = None;
    }
    // One lock for both reads, released before the view is built: a lock
    // taken inside the struct literal lives until the literal ends, so a
    // second one there would wait on the first forever.
    let unlocked = state.cipher().is_ok();
    let (cloud, gallery) = {
        let conn = state.db.lock().unwrap();
        let gallery = if unlocked { store::count(&conn) } else { 0 };
        (crate::cloud::media_choices(&conn), gallery)
    };
    let defaults = defaults(&state.db.lock().unwrap());
    MediaView { models, cloud, hidden, hidden_by_kind, video_note, installing, jobs: list_jobs(), gallery, defaults }
}

#[tauri::command(async)]
pub fn install_media_model(app: AppHandle, state: AppStateRef, model_id: String, anyway: Option<bool>) -> Result<(), String> {
    let spec = models(&state).into_iter().find(|m| m.id == model_id).ok_or("Unknown model")?;
    start_install_as(app, state.inner().clone(), spec, None, anyway.unwrap_or(false))
}

/// The Studio's Hugging Face search: picture, video or music models built
/// on a family SulcusAI runs.
#[tauri::command]
pub async fn media_hub_search(state: AppStateRef<'_>, kind: String, query: String, sort: String, all: Option<bool>) -> Result<hub::HubMediaResults, String> {
    hub::search(state.inner(), &kind, &query, &sort, all.unwrap_or(false)).await
}

/// Installs one file of a Hugging Face repository like a catalog model.
#[tauri::command]
pub async fn media_hub_install(app: AppHandle, state: AppStateRef<'_>, repo: String, file: String, anyway: Option<bool>) -> Result<String, String> {
    let spec = hub::prepare(state.inner(), &repo, &file).await?;
    let id = spec.id.clone();
    if let Err(e) = start_install_as(app, state.inner().clone(), spec, None, anyway.unwrap_or(false)) {
        hub::forget(&state.db.lock().unwrap(), &id).ok();
        return Err(e);
    }
    Ok(id)
}

/// Downloads a model in the background; `then_feature` is turned on after.
pub(crate) fn start_install(app: AppHandle, state: Arc<AppState>, spec: MediaModel, then_feature: Option<Feature>) -> Result<(), String> {
    start_install_as(app, state, spec, then_feature, false)
}

/// `start_install`; with `anyway`, also a model this PC probably can't run
/// (the person was told and chose to): it's tried when used, with the
/// memory check and the card's limits as always.
pub(crate) fn start_install_as(app: AppHandle, state: Arc<AppState>, spec: MediaModel, then_feature: Option<Feature>, anyway: bool) -> Result<(), String> {
    let hw = state.hardware.read().unwrap().clone();
    let f = fit(&spec, &hw, &Budget::from_hardware(&hw), missing_bytes(&state.paths, &spec), None);
    if !f.runnable && !anyway {
        return Err(format!("This PC can't run {}: it {}.", spec.name, f.reason.unwrap_or_else(|| "needs more".into())));
    }
    if !f.runnable {
        crate::applog::line("media", &format!("Installing {} though it probably won't run here ({}), as asked", spec.name, f.reason.clone().unwrap_or_default()));
    }
    if !f.disk_ok {
        return Err("There isn't enough free disk space for this model.".into());
    }
    if !state.installs.lock().unwrap().is_empty() {
        return Err("Another install is in progress. Wait for it to finish first.".into());
    }
    tauri::async_runtime::spawn(async move {
        let Some((cancel, _guard)) = crate::claim(&state.installs, &spec.id) else { return };
        let emit = |phase: &str, received: u64, total: u64| {
            app.emit("install:progress", json!({ "model_id": spec.id, "phase": phase, "received": received, "total": total })).ok();
        };
        state.log("model", &format!("Started installing {}", spec.name));
        // A cancel deletes only what this install downloaded.
        let had: Vec<PathBuf> = spec.files.iter().map(|f| file_path(&state.paths, &spec, f)).filter(|p| p.exists()).collect();
        let result = install(&state, &spec, &cancel, &emit).await;
        let payload = match &result {
            Ok(()) => {
                if let Some(feat) = then_feature {
                    let _ = crate::features::set(&state.db.lock().unwrap(), feat, true);
                }
                state.log("model", &format!("Installed {}", spec.name));
                json!({ "model_id": spec.id, "name": spec.name, "ok": true })
            }
            Err(e) if e == download::CANCELLED => {
                discard_download(&state, &spec, &had);
                state.log("model", &format!("Cancelled installing {}", spec.name));
                json!({ "model_id": spec.id, "name": spec.name, "ok": false, "cancelled": true })
            }
            Err(e) => {
                state.log("model", &format!("Installing {} failed: {e}", spec.name));
                json!({ "model_id": spec.id, "name": spec.name, "ok": false, "error": e })
            }
        };
        app.emit("install:finished", payload).ok();
        app.emit("features:changed", json!({})).ok();
    });
    Ok(())
}

#[tauri::command]
pub async fn remove_media_model(state: AppStateRef<'_>, model_id: String) -> Result<u64, String> {
    state.cipher()?;
    remove_now(&state, &model_id)
}

/// Deletes a model's files and half-downloaded parts, except files a model
/// in `installed` still uses and those in `also_keep`. Returns the bytes freed.
pub(super) fn delete_files(paths: &Paths, all: &[MediaModel], spec: &MediaModel, installed: &[InstalledMedia], also_keep: &[PathBuf]) -> u64 {
    let mut keep: Vec<PathBuf> = all
        .iter()
        .filter(|m| m.id != spec.id && installed.iter().any(|i| i.model_id == m.id))
        .flat_map(|m| m.files.iter().map(|f| file_path(paths, m, f)).collect::<Vec<_>>())
        .collect();
    keep.extend_from_slice(also_keep);
    let mut freed = 0;
    for f in &spec.files {
        let p = file_path(paths, spec, f);
        if !keep.contains(&p) {
            freed += p.metadata().map_or(0, |m| m.len());
            std::fs::remove_file(&p).ok();
        }
        let part = download::part_path(&p);
        freed += part.metadata().map_or(0, |m| m.len());
        std::fs::remove_file(&part).ok();
    }
    freed
}

/// After a cancelled install: deletes what it downloaded (files that were
/// here before it started, `had`, and files another installed model shares
/// stay), and forgets a Hugging Face model that was never installed. A model
/// that was already installed keeps everything.
pub(super) fn discard_download(state: &AppState, spec: &MediaModel, had: &[PathBuf]) {
    let all = models(state);
    let conn = state.db.lock().unwrap();
    let list = installed(&conn);
    if list.iter().any(|i| i.model_id == spec.id) {
        return;
    }
    let freed = delete_files(&state.paths, &all, spec, &list, had);
    if spec.from_hub.is_some() {
        hub::forget(&conn, &spec.id).ok();
    }
    crate::applog::line("download", &format!("{}: cancelled, deleted {} it had downloaded", spec.name, crate::size_label(freed)));
}

/// Removes a picture, video or music model from this PC: its files (and any
/// half-downloaded ones), unless another installed model shares them, and
/// what SulcusAI kept about it. Returns the bytes freed.
pub(crate) fn remove_now(state: &AppState, model_id: &str) -> Result<u64, String> {
    let all = models(state);
    let spec = all.iter().find(|m| m.id == model_id).ok_or("Unknown model")?.clone();
    let conn = state.db.lock().unwrap();
    let mut list = installed(&conn);
    list.retain(|i| i.model_id != model_id);
    let freed = delete_files(&state.paths, &all, &spec, &list, &[]);
    if spec.kind == Kind::Background {
        cutout::unload();
    }
    save_installed(&conn, &list)?;
    if spec.from_hub.is_some() {
        hub::forget(&conn, &spec.id)?;
    }
    // Not the default any more.
    let mut d = defaults(&conn);
    if [&d.image, &d.video, &d.music].iter().any(|x| x.as_deref() == Some(model_id)) {
        for slot in [&mut d.image, &mut d.video, &mut d.music] {
            if slot.as_deref() == Some(model_id) {
                *slot = None;
            }
        }
        db::set(&conn, DEFAULTS_KEY, &d)?;
    }
    db::log_action(&conn, "model", &format!("Removed {} and deleted its files ({} freed)", spec.name, crate::size_label(freed)));
    Ok(freed)
}

/// Tells the window a job finished (its card closes; what it made shows).
pub(crate) fn job_done(state: &AppState, job_id: &str, result: &Result<Vec<MediaItem>, String>) {
    let payload = match result {
        Ok(items) => json!({ "job_id": job_id, "ok": true, "items": items }),
        Err(e) if e == download::CANCELLED => json!({ "job_id": job_id, "ok": false, "cancelled": true }),
        Err(e) => json!({ "job_id": job_id, "ok": false, "error": e }),
    };
    emit(state, "media:done", payload);
}

#[tauri::command(async)]
pub fn media_start(state: AppStateRef, request: Request) -> Result<String, String> {
    state.cipher()?;
    // A job for the compute PC is checked there.
    if !crate::sync::compute::media_here(&state, &request) {
        check(&state, &request)?;
    }
    let id = store::new_id();
    let (state, job_id) = (state.inner().clone(), id.clone());
    tauri::async_runtime::spawn(async move {
        let result = run(&state, job_id.clone(), request).await;
        job_done(&state, &job_id, &result);
    });
    Ok(id)
}

/// What the prompt writer knows about a picture model: its family, whether
/// it was tuned on this PC (and how well that went), and which models write
/// and check its pictures.
#[tauri::command]
pub async fn media_prompt_status(state: AppStateRef<'_>, model_id: String) -> Result<serde_json::Value, String> {
    state.cipher()?;
    let m = models(&state).into_iter().find(|m| m.id == model_id).ok_or("That model isn't in the list.")?;
    let fam = prompts::family(&m);
    let tuned = prompts::program(&state.paths, fam).tuned;
    let own = installed_file(&state, &m, "llm").is_some();
    let own_eyes = own && installed_file(&state, &m, "llm_vision").is_some();
    let reader = if own_eyes { Some(m.name.clone()) } else { crate::vision::readers(&state).into_iter().next().map(|r| r.name) };
    let learned = learn::summary(&learn::load(&state, &m));
    Ok(json!({ "family": fam, "tuned": tuned, "own_writer": own, "reader": reader, "coach": coach::trained(&state, &m), "learned": learned, "learning": learn::on(&state) }))
}

/// Goes back to the prompt writer's bundled instructions for a model's family.
#[tauri::command(async)]
pub fn media_forget_tuning(state: AppStateRef, model_id: String) -> Result<(), String> {
    state.cipher()?;
    let m = models(&state).into_iter().find(|m| m.id == model_id).ok_or("That model isn't in the list.")?;
    prompts::forget_tuning(&state.paths, prompts::family(&m));
    coach::forget(&state, &m);
    learn::forget(&state, &m);
    db::log_action(&state.db.lock().unwrap(), "model", &format!("Forgot the prompt writer's tuning for {}", m.name));
    Ok(())
}

#[tauri::command(async)]
pub fn media_cancel(job_id: String) {
    cancel(&job_id);
}

#[tauri::command(async)]
pub fn media_jobs() -> Vec<JobInfo> {
    list_jobs()
}

#[tauri::command(async)]
pub fn media_list(state: AppStateRef, kind: Option<String>, before: Option<i64>, limit: Option<usize>) -> Result<Vec<MediaItem>, String> {
    let c = state.cipher()?;
    Ok(store::list(&state.db.lock().unwrap(), &c, kind.as_deref(), limit.unwrap_or(60).min(500), before))
}

/// The Studio's "Improve": the chat model (on this PC unless the default is
/// a cloud one) turns a short idea into a fuller description.
#[tauri::command]
pub async fn improve_media_prompt(state: AppStateRef<'_>, prompt: String, kind: String) -> Result<String, String> {
    state.cipher()?;
    let idea = prompt.trim();
    if idea.is_empty() {
        return Err("Write an idea first.".into());
    }
    let guide = match kind.as_str() {
        "video" => "a short video clip: the subject, what happens, how the camera moves, the light and mood",
        "music" => "a piece of music: the genre, mood, instruments, tempo and voice (if any)",
        _ => "a picture: the subject, setting, composition, light and mood",
    };
    let (ep, _, _) = crate::llm_endpoint(state.inner(), None, false, |_| {}).await?;
    let messages = vec![
        json!({ "role": "system", "content": format!(
            "You write descriptions for a model that makes media. Rewrite the person's idea as one vivid, specific description of {guide}. \
             Keep everything they asked for, exactly: every count, color, position and relation, and any words to show in quotes, spelled as given. \
             Add nothing that changes it. If they named a style, keep it. Write it in English, in plain sentences with no lists or explanations: \
             about 40 words for a simple idea, up to 120 for a scene with many parts."
        ) }),
        json!({ "role": "user", "content": idea }),
    ];
    let better = crate::chat::complete(&ep, messages, json!({}), 400).await?;
    let better = better.trim().trim_matches('"').trim();
    Ok(if better.is_empty() { idea.to_string() } else { better.to_string() })
}

/// A chat's pictures, video and music: made there or attached.
#[tauri::command(async)]
pub fn media_in_chat(state: AppStateRef, chat_id: String) -> Result<Vec<MediaItem>, String> {
    let c = state.cipher()?;
    Ok(store::in_chat(&state.db.lock().unwrap(), &c, &chat_id))
}

#[tauri::command(async)]
pub fn media_get(state: AppStateRef, id: String) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    store::get(&state.db.lock().unwrap(), &c, &id).ok_or_else(|| "That item is no longer in the gallery.".into())
}

#[tauri::command(async)]
pub fn media_file(state: AppStateRef, id: String) -> Result<tauri::ipc::Response, String> {
    let c = state.cipher()?;
    Ok(tauri::ipc::Response::new(store::read(&state.paths, &c, &id)?))
}

#[tauri::command(async)]
pub fn media_thumb(state: AppStateRef, id: String) -> Result<tauri::ipc::Response, String> {
    let c = state.cipher()?;
    Ok(tauri::ipc::Response::new(store::read_thumb(&state.paths, &c, &id)?))
}

/// A picture kept as a song's cover art (its thumbnail, larger than a
/// tile's so the song's page can show it big).
pub(super) fn set_cover(state: &AppState, c: &crate::crypto::Cipher, song_id: &str, img: &image::DynamicImage) -> Result<MediaItem, String> {
    let conn = state.db.lock().unwrap();
    let mut song = store::get(&conn, c, song_id).ok_or("That song is no longer in the gallery.")?;
    if song.kind != "audio" {
        return Err("Cover art is for songs.".into());
    }
    let side = img.width().min(img.height());
    let square = img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side).thumbnail(768, 768).to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 86).encode_image(&square).map_err(|e| e.to_string())?;
    store::write_thumb(&state.paths, c, song_id, &jpeg)?;
    song.poster = true;
    store::update(&conn, c, &song)?;
    Ok(song)
}

/// Queues cover art for a song just made, when a picture model is here.
pub(super) fn queue_cover_art(state: &Arc<AppState>, song: &MediaItem) {
    if choose(state, "generate", None, false).is_err() {
        crate::applog::line("media", "No cover art: no picture model is installed");
        return;
    }
    let req = Request { op: "cover_art".into(), source: Some(song.id.clone()), chat_id: song.chat_id.clone(), ..Default::default() };
    let (state, id) = (state.clone(), store::new_id());
    tauri::async_runtime::spawn(async move {
        let result = run(&state, id.clone(), req).await;
        job_done(&state, &id, &result);
    });
}

/// What a song's cover art is asked to show: the person's words for it,
/// else the song's own description. Never "album cover": with those words
/// Z-Image Turbo lettered a title across the top.
pub(super) fn art_prompt(song: &MediaItem, asked: &str) -> String {
    let what = if asked.trim().is_empty() { song.prompt.trim().trim_end_matches('.').to_string() } else { asked.trim().trim_end_matches('.').to_string() };
    format!("A striking square artwork that captures the feeling of this: {what}. Cinematic light, bold composition, rich colour, one clear subject. The image has no text anywhere.")
}

/// What cover art leaves out, for picture models that take a negative prompt.
pub(super) const ART_NEGATIVE: &str = "text, letters, words, title, typography, logo, watermark, signature";

/// Cover art for a song (`req.source`): a square picture made by the
/// picture model and kept as the song's picture; the picture itself isn't
/// kept in the gallery.
pub(super) async fn cover_art(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let id = ctx.req.source.as_deref().ok_or("Pick a song first.")?;
    let c = ctx.state.work_cipher()?;
    let song = store::get(&ctx.state.db.lock().unwrap(), &c, id).ok_or("That song is no longer in the gallery.")?;
    if song.kind != "audio" {
        return Err("Cover art is for songs.".into());
    }
    let req = Request {
        op: "generate".into(),
        model_id: Some(m.id.clone()),
        prompt: art_prompt(&song, &ctx.req.prompt),
        negative: Some(ART_NEGATIVE.into()),
        shape: Some("square".into()),
        count: Some(1),
        seed: ctx.req.seed,
        ..Default::default()
    };
    let step = RunCtx { state: ctx.state, req: &req, cancel: ctx.cancel, progress: ctx.progress, reserved: ctx.reserved };
    let pic = picture(&step, m, work).await?.into_iter().next().ok_or("The picture model didn't make the cover.")?;
    let bytes = store::read(&ctx.state.paths, &c, &pic.id)?;
    let _ = store::delete(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &pic.id);
    let img = imaging::decode(&bytes)?;
    Ok(vec![set_cover(ctx.state, &c, &song.id, &img)?])
}

/// A captioned video's captions saved as a subtitle file where the person
/// chose (players and editors read .srt).
#[tauri::command(async)]
pub fn media_save_captions(state: AppStateRef, id: String, path: String) -> Result<(), String> {
    let c = state.work_cipher()?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That video is no longer in the gallery.")?;
    let text = item.lyrics.filter(|t| t.contains("-->")).ok_or("That video has no captions.")?;
    let path = std::path::PathBuf::from(path);
    let path = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("srt")) { path } else { path.with_extension("srt") };
    std::fs::write(&path, text).map_err(|e| format!("Couldn't save the captions: {e}"))
}

/// A picture of the person's own, chosen in the window, as a song's cover.
#[tauri::command(async)]
pub fn media_set_cover(state: AppStateRef, id: String, path: String) -> Result<MediaItem, String> {
    let c = state.work_cipher()?;
    let bytes = std::fs::read(&path).map_err(|e| format!("Couldn't read that picture: {e}"))?;
    let img = imaging::decode(&bytes).map_err(|_| "That file isn't a picture SulcusAI can read (PNG, JPEG, WebP, GIF or BMP).".to_string())?;
    set_cover(&state, &c, &id, &img)
}

/// Saves a still frame the window took from a video as its thumbnail
/// (the window can decode WebM; the core can't).
#[tauri::command(async)]
pub fn media_set_poster(state: AppStateRef, id: String, data: String) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|_| "That still isn't readable.".to_string())?;
    let img = image::load_from_memory(&bytes).map_err(|_| "That still isn't a picture.".to_string())?;
    let conn = state.db.lock().unwrap();
    let mut item = store::get(&conn, &c, &id).ok_or("That item is gone.")?;
    if item.kind != "video" {
        return Err("Only videos take a still.".into());
    }
    store::write_thumb(&state.paths, &c, &id, &imaging::thumbnail(&img)?)?;
    item.poster = true;
    store::update(&conn, &c, &item)?;
    Ok(item)
}

#[tauri::command(async)]
pub fn media_delete(state: AppStateRef, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    // A song sung in a recorded voice takes the version it was made from,
    // kept out of the gallery for it, along.
    if let Some(parent) = store::get(&conn, &c, &id).and_then(|i| i.parent.zip(Some(i.op))).and_then(|(p, op)| store::get(&conn, &c, &p).filter(|p| p.hidden && p.op == op && p.kind == "audio")) {
        store::delete(&conn, &state.paths, &parent.id)?;
    }
    library::forget_song(&conn, &c, &id);
    store::delete(&conn, &state.paths, &id)?;
    db::log_action(&conn, "media", "Deleted an item from the gallery");
    Ok(())
}

#[tauri::command(async)]
pub fn media_favorite(state: AppStateRef, id: String, on: bool) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut item = store::get(&conn, &c, &id).ok_or("That item is no longer in the gallery.")?;
    item.favorite = on;
    item.disliked &= !on;
    store::update(&conn, &c, &item)?;
    Ok(item)
}

/// A thumbs down (or taking it back): a liked song stops being a favorite.
#[tauri::command(async)]
pub fn media_dislike(state: AppStateRef, id: String, on: bool) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut item = store::get(&conn, &c, &id).ok_or("That item is no longer in the gallery.")?;
    item.disliked = on;
    item.favorite &= !on;
    store::update(&conn, &c, &item)?;
    Ok(item)
}

#[tauri::command(async)]
pub fn music_playlists(state: AppStateRef) -> Result<Vec<library::Playlist>, String> {
    let c = state.cipher()?;
    Ok(library::list(&state.db.lock().unwrap(), &c))
}

/// Saves a playlist (new without an id): its name and songs in order.
#[tauri::command(async)]
pub fn music_playlist_save(state: AppStateRef, playlist: library::Playlist) -> Result<library::Playlist, String> {
    let c = state.cipher()?;
    library::save(&state.db.lock().unwrap(), &c, playlist)
}

/// What the person likes in songs, from their liked, made and "not for me"
/// songs; or what some songs share (a playlist's, each counted as liked).
#[tauri::command(async)]
pub fn music_taste(state: AppStateRef, ids: Option<Vec<String>>) -> Result<library::Taste, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let items = match ids {
        Some(ids) => ids.iter().filter_map(|id| store::get(&conn, &c, id)).map(|i| MediaItem { favorite: !i.disliked, ..i }).collect(),
        None => store::list(&conn, &c, Some("audio"), 5000, None),
    };
    Ok(library::taste(&items))
}

#[tauri::command(async)]
pub fn music_playlist_delete(state: AppStateRef, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    library::remove(&state.db.lock().unwrap(), &c, &id)
}

/// Names a song (or anything in the gallery); empty goes back to none.
#[tauri::command(async)]
pub fn media_set_title(state: AppStateRef, id: String, title: String) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut item = store::get(&conn, &c, &id).ok_or("That item is no longer in the gallery.")?;
    item.title = Some(title.trim().chars().take(120).collect::<String>()).filter(|t| !t.is_empty());
    store::update(&conn, &c, &item)?;
    Ok(item)
}

/// Saves a copy, unencrypted, where the user picked (Save as…).
#[tauri::command(async)]
pub fn media_export(state: AppStateRef, id: String, path: String) -> Result<(), String> {
    let c = state.cipher()?;
    let bytes = store::read(&state.paths, &c, &id)?;
    std::fs::write(&path, bytes).map_err(|e| format!("Couldn't save it there: {e}"))?;
    state.log("media", "Saved a copy of a gallery item outside the app");
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Screen {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Top-left corner on the desktop (physical pixels).
    pub x: i32,
    pub y: i32,
    pub primary: bool,
}

/// The screens connected now, at their real resolutions.
#[tauri::command]
pub fn wallpaper_screens(app: AppHandle) -> Vec<Screen> {
    let primary = app.primary_monitor().ok().flatten().map(|m| *m.position());
    app.available_monitors()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(i, m)| Screen {
            name: m.name().cloned().filter(|n| !n.trim().is_empty() && !n.starts_with(r"\\.\")).unwrap_or_else(|| format!("Screen {}", i + 1)),
            width: m.size().width,
            height: m.size().height,
            x: m.position().x,
            y: m.position().y,
            primary: primary == Some(*m.position()),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct WallpaperSet {
    /// The copy Windows shows (Pictures\SulcusAI Wallpapers).
    pub path: String,
    /// "windows" (set), "lively" (handed to Lively Wallpaper) or "saved"
    /// (a moving one, and Lively isn't installed).
    pub how: String,
}

/// Sets a gallery picture (or looping clip) as the desktop wallpaper, on the
/// screen at `x`,`y` or on all of them. A copy goes to Pictures\SulcusAI
/// Wallpapers, since Windows shows a file, not the encrypted gallery.
#[tauri::command(async)]
pub fn wallpaper_set(state: AppStateRef, id: String, x: Option<i32>, y: Option<i32>) -> Result<WallpaperSet, String> {
    let c = state.cipher()?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That's no longer in the gallery.")?;
    let ext = match item.mime.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        _ => return Err("Only pictures and clips can be wallpapers.".into()),
    };
    let path = wallpaper::folder()?.join(format!("wallpaper-{}.{ext}", &id[..id.len().min(12)]));
    std::fs::write(&path, store::read(&state.paths, &c, &id)?).map_err(|e| format!("Couldn't save the wallpaper: {e}"))?;
    let how = if item.kind == "image" {
        wallpaper::set_still(&path, x.zip(y))?;
        "windows"
    } else {
        match wallpaper::set_moving(&path) {
            Ok(()) => "lively",
            Err(e) if e == "NO_LIVELY" => "saved",
            Err(e) => return Err(e),
        }
    };
    db::log_action(&state.db.lock().unwrap(), "media", "Set a desktop wallpaper from the gallery");
    Ok(WallpaperSet { path: path.to_string_lossy().into_owned(), how: how.into() })
}

pub(super) fn mime_for(name: &str) -> Option<(&'static str, &'static str)> {
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => ("image", "image/png"),
        "jpg" | "jpeg" => ("image", "image/jpeg"),
        "webp" => ("image", "image/webp"),
        "webm" => ("video", "video/webm"),
        "mp4" => ("video", "video/mp4"),
        "mp3" => ("audio", "audio/mpeg"),
        "wav" => ("audio", "audio/wav"),
        "ogg" => ("audio", "audio/ogg"),
        // Songs to work on: Windows reads these for the music engine.
        "m4a" => ("audio", "audio/mp4"),
        "aac" => ("audio", "audio/aac"),
        "flac" => ("audio", "audio/flac"),
        "wma" => ("audio", "audio/x-ms-wma"),
        _ => return None,
    })
}

pub(super) const MAX_IMPORT: u64 = 200 * 1024 * 1024;

/// Brings a picture, clip or sound from a file into the gallery.
#[tauri::command(async)]
pub fn media_import(state: AppStateRef, path: String, chat_id: Option<String>) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let (kind, mime) = mime_for(&path).ok_or("Pick a picture (PNG, JPEG, WebP), a clip (WebM, MP4) or a sound or song (MP3, WAV, M4A, AAC, FLAC, OGG, WMA).")?;
    // Attached to a chat: a hidden attachment, deleted with the chat. Songs
    // too, for the assistant to work on.
    if chat_id.is_some() && kind == "video" {
        return Err("Only pictures and songs can be attached to a chat.".into());
    }
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_IMPORT {
        return Err("That file is too big (200 MB at most).".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let name = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let hidden = chat_id.is_some();
    let mut item = add(&state, &c, bytes, kind, mime, if hidden { "attach" } else { "import" }, &name, None, chat_id, hidden)?;
    // A song's length, for picking a part of it to work on.
    if kind == "audio" {
        if let Some(secs) = audio_in::length(std::path::Path::new(&path)).filter(|s| *s > 0.0) {
            item.seconds = secs;
            store::update(&state.db.lock().unwrap(), &c, &item)?;
        }
    }
    Ok(item)
}

/// The artists the person saved.
#[tauri::command]
pub async fn music_personas(state: AppStateRef<'_>) -> Result<Vec<persona::Persona>, String> {
    let c = state.cipher()?;
    Ok(persona::list(&state.db.lock().unwrap(), &c))
}

/// Saves an artist (a new one without an id).
#[tauri::command]
pub async fn music_persona_save(state: AppStateRef<'_>, persona: persona::Persona) -> Result<persona::Persona, String> {
    let c = state.cipher()?;
    persona::save(&state.db.lock().unwrap(), &c, persona)
}

#[tauri::command]
pub async fn music_persona_delete(state: AppStateRef<'_>, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    persona::remove(&state.db.lock().unwrap(), &c, &id)
}

/// An artist's style for the Studio's "Use their style": the genre, voice,
/// instruments and tempo to fill in, and their sound and way of writing.
#[tauri::command]
pub async fn music_artist_style(state: AppStateRef<'_>, name: String) -> Result<artist::Style, String> {
    state.cipher()?;
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err("Type the name of a singer or band.".into());
    }
    match artist::look_up(state.inner(), name).await? {
        Some(style) => Ok(style),
        None => Err(format!("The chat model doesn't know {name}. Describe their style in your own words instead.")),
    }
}

/// Lyrics for a song, written before it's made so the person can read and
/// change them (the Studio's "Write lyrics for me").
#[tauri::command]
pub async fn music_write_lyrics(state: AppStateRef<'_>, prompt: String, music: Option<MusicSpec>, seconds: Option<f32>, language: Option<String>) -> Result<String, String> {
    state.cipher()?;
    let req = Request { op: "music".into(), prompt, music, ..Default::default() };
    // "Sing as …" in the description, or "Sound like" not yet looked up.
    let styled = artist::resolve(state.inner(), &req, &|_, _| {}).await;
    let req = styled.unwrap_or(req);
    let words = song_caption(&req);
    if words.trim().is_empty() {
        return Err("Describe the song first (or pick its style), so the lyrics fit it.".into());
    }
    let spec = req.music.clone().unwrap_or_default();
    // No length (Auto): one named in the description, else the sections', else the writer's choice.
    let seconds = seconds.map(|s| s.clamp(10.0, 600.0)).or_else(|| lyrics::named_length(&req.prompt));
    lyrics::write(state.inner(), &words, seconds, language.as_deref().filter(|l| *l != "auto"), &spec.sections(), &spec.lyrics_guide(), spec.flow(&req.prompt))
        .await
        .map_err(|why| match why {
            lyrics::NotWritten::NoModel(_) => "There's no chat model on this PC to write lyrics. Install one from Models, or type your own.".to_string(),
            lyrics::NotWritten::Unusable(e) => format!("The lyrics didn't come out right ({e}). Try again, or type your own."),
        })
}

/// Some lines of the lyrics (`part`, from `whole`) written again by the chat
/// model, changed as `how` says (the Studio's "Write these lines again").
#[tauri::command]
pub async fn music_rewrite_lines(state: AppStateRef<'_>, whole: String, part: String, how: String, language: Option<String>) -> Result<String, String> {
    state.cipher()?;
    lyrics::rewrite(state.inner(), &whole, &part, &how, language.as_deref()).await
}

pub(crate) fn add(state: &AppState, c: &crate::crypto::Cipher, bytes: Vec<u8>, kind: &str, mime: &str, op: &str, prompt: &str, parent: Option<String>, chat_id: Option<String>, hidden: bool) -> Result<MediaItem, String> {
    let mut item = MediaItem { kind: kind.into(), op: op.into(), prompt: prompt.into(), mime: mime.into(), parent, chat_id, hidden, ..Default::default() };
    let mut thumb = None;
    let mut bytes = bytes;
    if kind == "image" {
        let img = imaging::decode(&bytes)?;
        (item.width, item.height) = img.dimensions();
        thumb = Some(imaging::thumbnail(&img)?);
        // WebP and JPEG are kept as they are; anything else becomes PNG.
        if !matches!(mime, "image/png" | "image/jpeg" | "image/webp") {
            bytes = imaging::png(&img)?;
            item.mime = "image/png".into();
        }
    }
    store::save(&state.db.lock().unwrap(), &state.paths, c, item, &bytes, thumb.as_deref())
}

/// Adds bytes made in the window (a trimmed sound, a pasted or attached
/// picture). `hidden` keeps chat attachments out of the studio.
#[tauri::command(async)]
pub fn media_add(
    state: AppStateRef,
    data: String,
    mime: String,
    op: String,
    prompt: Option<String>,
    parent: Option<String>,
    chat_id: Option<String>,
    hidden: Option<bool>,
    seconds: Option<f64>,
    lyrics: Option<String>,
    note: Option<String>,
) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|_| "That file couldn't be read.".to_string())?;
    if bytes.len() as u64 > MAX_IMPORT {
        return Err("That file is too big (200 MB at most).".into());
    }
    let kind = match mime.split('/').next() {
        Some("image") => "image",
        Some("video") => "video",
        Some("audio") => "audio",
        _ => return Err("Only pictures, clips and sounds can be added.".into()),
    };
    let op = if matches!(op.as_str(), "trim" | "paste" | "attach" | "screenshot" | "import" | "studio" | "recording") { op } else { "import".into() };
    let mut item = add(&state, &c, bytes, kind, &mime, &op, prompt.as_deref().unwrap_or(""), parent, chat_id, hidden.unwrap_or(false))?;
    // What was done to it in the window ("Faded out over 8 s").
    item.note = note.map(|n| n.trim().chars().take(200).collect::<String>()).filter(|n| !n.is_empty());
    if seconds.is_some() || lyrics.is_some() || item.note.is_some() {
        if let Some(s) = seconds {
            item.seconds = s;
        }
        // A song with a section cut out or added keeps its words.
        if let Some(l) = lyrics.filter(|l| kind == "audio" && lyrics::has_singing(l)) {
            item.lyrics = Some(l);
        }
        store::update(&state.db.lock().unwrap(), &c, &item)?;
    }
    Ok(item)
}

/// A song's map (`songmap`) saved as a MIDI file, a track a part: the one
/// given (notes edited in the song view), else the song's own.
#[tauri::command]
pub fn media_song_midi(state: AppStateRef, id: String, path: String, map: Option<songmap::SongMap>) -> Result<(), String> {
    let c = state.cipher()?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That song is no longer in the gallery.")?;
    let map = map.or(item.map).ok_or("See how the song is built first.")?;
    std::fs::write(&path, songmap::midi(&map)).map_err(|e| format!("Couldn't save it there: {e}"))?;
    state.log("media", "Saved a song's notes as a MIDI file");
    Ok(())
}

/// Keeps the notes as edited in the song view on the song (its sound
/// doesn't change: the notes are read from it, not played into it).
#[tauri::command]
pub fn media_song_map_save(state: AppStateRef, id: String, map: songmap::SongMap) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut item = store::get(&conn, &c, &id).ok_or("That song is no longer in the gallery.")?;
    if item.kind != "audio" || map.parts.iter().map(|p| p.notes.len()).sum::<usize>() > 200_000 {
        return Err("Those notes can't be kept with this song.".into());
    }
    item.map = Some(map);
    store::update(&conn, &c, &item)?;
    Ok(item)
}


/// The sampled instruments Compose plays its notes on, downloaded the first
/// time (`compose:getting` says how far), as bytes for the window's player.
#[tauri::command]
pub async fn compose_sounds(state: AppStateRef<'_>) -> Result<tauri::ipc::Response, String> {
    crate::features::require(&state, Feature::Music)?;
    let h = state.catalog.media.instruments.clone().ok_or("The instruments aren't available.")?;
    let st = state.inner().clone();
    let cancel = AtomicBool::new(false);
    // A message a percent: one a download chunk (thousands) left the window minutes behind.
    let shown = std::sync::atomic::AtomicU32::new(u32::MAX);
    let path = fetch_helper(&st, &h, "Getting the instruments (once)", &cancel, &|_, f| {
        let pct = (f * 100.0) as u32;
        if shown.swap(pct, Ordering::Relaxed) != pct {
            emit(&st, "compose:getting", json!({ "fraction": f }));
        }
    })
    .await?;
    let bytes = std::fs::read(&path).map_err(|e| format!("Couldn't read the instruments: {e}"))?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// A song written as notes: the chat model plans it, the arranger writes
/// the notes (`compose`).
#[tauri::command]
pub async fn compose_write(state: AppStateRef<'_>, mut ask: compose::Ask) -> Result<compose::Composition, String> {
    crate::features::require(&state, Feature::Music)?;
    if ask.description.trim().is_empty() {
        return Err("Describe the song first.".into());
    }
    if ask.beats == 0 || ask.unit == 0 {
        (ask.beats, ask.unit) = (4, 4);
    }
    if ask.sections == 0 {
        ask.sections = 6;
    }
    if ask.seed == 0 {
        ask.seed = random_seed() as u64;
    }
    let plan = compose::write_plan(state.inner(), &ask).await?;
    state.log("media", &format!("Wrote a song as notes: {} sections in {}", plan.sections.len(), plan.time()));
    Ok(compose::Composition { parts: compose::arrange(&plan), plan })
}

/// Every note written again from the plan (after a new time signature).
#[tauri::command(async)]
pub fn compose_arrange(plan: compose::Plan) -> compose::Composition {
    let plan = compose::tidy(plan);
    compose::Composition { parts: compose::arrange(&plan), plan }
}

/// A section changed (`change`: in the person's words, by the chat model),
/// written again, added, copied or taken out; the other notes stay.
#[tauri::command]
pub async fn compose_section(state: AppStateRef<'_>, composition: compose::Composition, index: usize, op: compose::SectionOp, change: Option<String>) -> Result<compose::Composition, String> {
    let op = match change.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(words) => compose::SectionOp::Set { section: compose::edit_section(state.inner(), &composition.plan, index, words).await? },
        None => op,
    };
    compose::apply(&composition, index, op)
}

/// Keeps a composition in the gallery with its demo (WAV, base64, played in
/// the window): a new item, or the one it was opened from.
#[tauri::command(async)]
pub fn compose_save(state: AppStateRef, id: Option<String>, composition: compose::Composition, audio: String, seconds: f64) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(audio.trim()).map_err(|_| "The demo couldn't be read.".to_string())?;
    if bytes.len() as u64 > MAX_IMPORT || composition.parts.iter().map(|p| p.notes.len()).sum::<usize>() > 200_000 {
        return Err("That song is too big to keep.".into());
    }
    let words = composition.plan.lyrics();
    let lyrics = Some(words).filter(|l| lyrics::has_singing(l));
    let prompt = if composition.plan.description.is_empty() { composition.plan.title.clone() } else { composition.plan.description.clone() };
    let existing = id.and_then(|id| store::get(&state.db.lock().unwrap(), &c, &id)).filter(|i| i.op == "compose");
    let mut item = match existing {
        Some(mut item) => {
            store::write_file(&state.paths, &c, &item.id, &bytes)?;
            item.size = bytes.len() as u64;
            item
        }
        None => add(&state, &c, bytes, "audio", "audio/wav", "compose", &prompt, None, None, false)?,
    };
    item.seconds = if seconds > 0.0 { seconds } else { composition.plan.seconds() };
    item.prompt = prompt;
    item.lyrics = lyrics;
    item.composition = Some(composition);
    store::update(&state.db.lock().unwrap(), &c, &item)?;
    Ok(item)
}

/// A composition as a MIDI file: saved at `path`, else its bytes (what the
/// window's player plays).
#[tauri::command(async)]
pub fn compose_midi(state: AppStateRef, composition: compose::Composition, path: Option<String>) -> Result<tauri::ipc::Response, String> {
    let bytes = compose::midi(&composition);
    let Some(path) = path else {
        return Ok(tauri::ipc::Response::new(bytes));
    };
    std::fs::write(&path, bytes).map_err(|e| format!("Couldn't save it there: {e}"))?;
    state.log("media", "Saved a composition as a MIDI file");
    Ok(tauri::ipc::Response::new(Vec::new()))
}

/// "Make it real": the music model records a song that follows the
/// composition's demo (a cover of it: its tempo, chords and tune) and sings
/// its words. A new song in the gallery; the composition stays as it is.
/// `strength`: how closely it follows the demo (0 to 1).
#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
pub fn compose_make_real(
    state: AppStateRef,
    id: String,
    strength: Option<f32>,
    model_id: Option<String>,
    chat_id: Option<String>,
    song: Option<MusicSpec>,
    lyrics: Option<String>,
    language: Option<String>,
) -> Result<String, String> {
    let c = state.cipher()?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("Keep the song first.")?;
    let plan = item.composition.as_ref().map(|x| x.plan.clone()).ok_or("That isn't a composition.")?;
    let request = real_request(&plan, &id, strength, model_id, chat_id, song, lyrics, language);
    media_start(state, request)
}

/// The music job that makes a composition real (`compose_make_real`): every
/// song option picked (in `song`, else the ones it was written with) and
/// the words given (else its own), with the composition's tempo, key and
/// time signature, as a cover of its demo.
#[allow(clippy::too_many_arguments)]
pub(crate) fn real_request(
    plan: &compose::Plan,
    id: &str,
    strength: Option<f32>,
    model_id: Option<String>,
    chat_id: Option<String>,
    song: Option<MusicSpec>,
    lyrics: Option<String>,
    language: Option<String>,
) -> Request {
    let words = lyrics.map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).unwrap_or_else(|| plan.lyrics());
    let sung = lyrics::has_singing(&words);
    let ins = &plan.instruments;
    let filled = |o: Option<String>| o.filter(|s| !s.trim().is_empty());
    let mut spec = song.or_else(|| plan.song.clone()).unwrap_or_default();
    // The parts as the music model knows them, unless instruments were
    // picked; the tune is sung when there are words.
    if spec.instruments.is_empty() {
        let mut list = vec![ins.chords.clone(), ins.bass.clone(), if ins.drums.contains("kit") { "drums".into() } else { ins.drums.clone() }];
        if !sung {
            list.push(ins.melody.clone());
        }
        spec.instruments = list.into_iter().filter(|i| !i.trim().is_empty()).collect();
    }
    spec.genre = filled(spec.genre).or_else(|| filled(Some(plan.style.clone())));
    spec.era = filled(spec.era).or_else(|| filled(Some(plan.era.clone())));
    spec.voice = if sung { filled(spec.voice).or_else(|| filled(Some(plan.voice.clone()))) } else { None };
    // The composition's own tempo, key, time signature and shape.
    spec.task = Some("cover".into());
    spec.audio = Some(id.to_string());
    spec.bpm = Some(plan.bpm);
    spec.key = Some(plan.key.clone());
    spec.time_signature = Some(plan.time());
    spec.strength = Some(strength.unwrap_or(if sung { REAL_STRENGTH_SUNG } else { REAL_STRENGTH_PLAYED }).clamp(0.0, 1.0));
    spec.sections.clear();
    spec.structure = None;
    spec.auto_length = false;
    spec.harmony_layer = false;
    spec.region = None;
    spec.extend_by = None;
    spec.track = None;
    let prompt = if plan.description.trim().is_empty() { format!("{} song", plan.style) } else { plan.description.clone() };
    Request {
        op: "music".into(),
        model_id,
        prompt,
        lyrics: sung.then_some(words),
        instrumental: Some(!sung),
        language: filled(language).filter(|l| l != "auto").or_else(|| plan.language.clone()),
        chat_id,
        music: Some(spec),
        ..Default::default()
    }
}

/// How closely a composition made real follows its demo, unless asked. The
/// engine follows the demo for `(steps × strength)` of its steps, rounded
/// down (8 for the turbo models: 0.5 and 0.6 are the same song), so the
/// window offers eighths. A sung song follows less closely (the demo has no
/// real voice; with thin lyrics Whisper heard 47 of 104 words at 3 steps and
/// 1 at 4), an instrumental more (its note transitions were better at 4).
pub(crate) const REAL_STRENGTH_SUNG: f32 = 0.375;
pub(crate) const REAL_STRENGTH_PLAYED: f32 = 0.5;

/// A screenshot of the screen the mouse is on, kept as a hidden attachment.
pub fn screenshot(state: &AppState, chat_id: Option<String>) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let shot = screen::for_model(screen::capture()?);
    let img = image::DynamicImage::ImageRgba8(shot);
    let png = imaging::png(&img)?;
    let (w, h) = img.dimensions();
    let item = MediaItem { kind: "image".into(), op: "screenshot".into(), mime: "image/png".into(), width: w, height: h, chat_id, hidden: true, ..Default::default() };
    let thumb = imaging::thumbnail(&img)?;
    let item = store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &png, Some(&thumb))?;
    state.log("privacy", "Took a screenshot for a question");
    Ok(item)
}

/// A picture for a vision model: PNG if transparent, else JPEG under ~1.5 megapixels, as a data URL.
pub fn image_data_url(state: &AppState, c: &crate::crypto::Cipher, id: &str) -> Result<String, String> {
    let bytes = store::read(&state.paths, c, id)?;
    let img = imaging::decode(&bytes)?;
    let (w, h) = img.dimensions();
    let (tw, th) = imaging::work_size(w, h, 1536 * 1024);
    let small = if (tw, th) == (w, h) { img } else { img.resize_exact(tw, th, image::imageops::FilterType::Triangle) };
    if imaging::has_alpha(&small) {
        let rgb = imaging::composite_white(&small);
        let mut png_bytes = Vec::new();
        rgb.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).map_err(|e| e.to_string())?;
        return Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png_bytes)));
    }
    let mut jpg = Vec::new();
    let rgb = image::DynamicImage::ImageRgb8(small.to_rgb8());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 88).encode_image(&rgb).map_err(|e| e.to_string())?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpg)))
}

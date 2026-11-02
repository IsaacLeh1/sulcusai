// SPDX-License-Identifier: AGPL-3.0-only
//! A song taken apart into its parts, a dry voice, new parts for the song
//! studio, and the studio's own commands.

use super::*;

/// ONNX Runtime and a helper model (`MediaCatalog::separator`,
/// `note_reader`), downloaded the first time, with `getting` shown meanwhile.
pub(super) async fn ensure_helper(state: &Arc<AppState>, h: &Helper, getting: &str, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(PathBuf, PathBuf), String> {
    let dll = crate::natural::ensure_runtime(state, cancel, &|_, _, _| {}).await?;
    Ok((dll, fetch_helper(state, h, getting, cancel, progress).await?))
}

/// The part separator for work outside a job (a recording added to a
/// voice): ONNX Runtime, HTDemucs, and the NVIDIA card when its runtime is
/// already here and loaded (it isn't downloaded for this; the processor
/// does it otherwise).
pub(crate) async fn separator(state: &Arc<AppState>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(PathBuf, PathBuf, Option<crate::natural::Card>), String> {
    let sep = state.catalog.media.separator.clone().ok_or("The part separator isn't available.")?;
    let (dll, model) = ensure_helper(state, &sep, "Getting the part separator (once)", cancel, progress).await?;
    // Short work: CUDA when its part is here; the processor otherwise (the
    // separator can't run through DirectML).
    let ready = crate::natural::card_runtime_dll(&state.paths, &state.catalog.voices).is_some() && !crate::natural::loaded_for_processor();
    let card = if ready { crate::natural::pick_card(state, false).await.ok().filter(|p| !p.needs_cuda && !p.card.dml).map(|p| p.card) } else { None };
    Ok((dll, model, card))
}

/// A helper's file, downloaded the first time.
pub(super) async fn fetch_helper(state: &Arc<AppState>, h: &Helper, getting: &str, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<PathBuf, String> {
    let dest = state.paths.models.join("media").join("helpers").join(&h.file.file);
    if !dest.metadata().is_ok_and(|m| m.len() == h.file.size) {
        progress(getting, 0.0);
        state.log("network", &format!("Downloading {} ({}) from {}", h.file.file, crate::size_label(h.file.size), crate::host_of(&h.file.url)));
        let client = net::external_client(state.settings().connectivity, net::Purpose::ModelDownload, false)?;
        std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        download::fetch_verified(&client, &h.file.url, &dest, h.file.size, &h.file.sha256, cancel, |r, t| progress(getting, r as f64 / t.max(1) as f64)).await?;
    }
    Ok(dest)
}

/// An installed music model that adds parts to a song (ACE-Step 1.5 Base).
pub(super) fn parts_model(state: &AppState) -> Option<MediaModel> {
    let have: Vec<String> = installed(&state.db.lock().unwrap()).into_iter().map(|i| i.model_id).collect();
    models(state).into_iter().find(|m| m.kind == Kind::Music && m.can("parts") && have.contains(&m.id))
}

/// A song from the gallery as stereo at the part separator's rate.
pub(super) fn decoded_stereo(bytes: &[u8], mime: &str) -> Result<stems::Stereo, String> {
    let dir = std::env::temp_dir().join(format!("sulcusai-song-{}", store::new_id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(if mime.contains("wav") { "song.wav" } else { "song.mp3" });
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let decoded = audio_in::decode(&path);
    let _ = std::fs::remove_dir_all(&dir);
    let (samples, rate, channels) = decoded?;
    let ch = channels.max(1) as usize;
    let left: Vec<f32> = samples.chunks(ch).map(|f| f[0]).collect();
    let right: Vec<f32> = samples.chunks(ch).map(|f| f[if ch > 1 { 1 } else { 0 }]).collect();
    Ok(stems::Stereo { rate, ch: [left, right] }.resampled(stems::RATE))
}

/// A song's parts as songs of their own under it (Suno's Get stems): its
/// vocals and the instrumental, or (`MusicSpec::track` "all") the vocals,
/// drums, bass and the rest, taken apart by HTDemucs.
pub(super) async fn stems_job(ctx: &RunCtx<'_>) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let id = ctx.req.source.as_deref().ok_or("Pick a song first.")?;
    let song = store::get(&state.db.lock().unwrap(), &c, id).ok_or("That song is no longer in the gallery.")?;
    if song.kind != "audio" {
        return Err("Only a song or a sound has parts.".into());
    }
    let every = ctx.req.music.as_ref().and_then(|m| m.track.as_deref()) == Some("all");
    let sep = state.catalog.media.separator.clone().ok_or("The part separator isn't available.")?;
    let card = map_card(ctx).await?;
    let (dll, separator) = ensure_helper(state, &sep, "Getting the part separator (once)", ctx.cancel, ctx.progress).await?;
    (ctx.progress)("Taking the parts apart", 0.1);
    let bytes = store::read(&state.paths, &c, &song.id)?;
    let threads = state.limits().threads;
    let ask = crate::cardfirst::hook_for_job(state, JOB_ID.try_with(|j| j.clone()).ok());
    let parts = tokio::task::block_in_place(|| crate::cardfirst::asking(ask, || -> Result<Vec<stems::Stereo>, String> {
        let mix = decoded_stereo(&bytes, &song.mime)?;
        let (parts, _) = stems::separate(&dll, &separator, &mix, threads, &[0, 1, 2, 3], card, ctx.cancel)?;
        stems::unload();
        Ok(parts)
    }))?;
    if ctx.cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    // HTDemucs gives the drums, bass, the rest and the vocals, in that order.
    let wanted: Vec<(&str, Vec<usize>)> = if every { vec![("vocals", vec![3]), ("drums", vec![0]), ("bass", vec![1]), ("other instruments", vec![2])] } else { vec![("vocals", vec![3]), ("instrumental", vec![0, 1, 2])] };
    let name = song.title.clone().unwrap_or_else(|| song.prompt.chars().take(60).collect());
    let mut out = Vec::new();
    for (i, (part, idx)) in wanted.iter().enumerate() {
        (ctx.progress)("Saving the parts", 0.8 + 0.2 * i as f64 / wanted.len() as f64);
        let len = parts[0].len();
        let [l, r] = [0, 1].map(|ch| (0..len).map(|s| idx.iter().map(|k| parts[*k].ch[ch][s]).sum::<f32>()).collect::<Vec<f32>>());
        let item = MediaItem {
            kind: "audio".into(),
            op: "stem".into(),
            prompt: song.prompt.clone(),
            title: Some(format!("{name} ({part})")),
            mime: "audio/wav".into(),
            seconds: len as f64 / stems::RATE as f64,
            parent: Some(song.id.clone()),
            chat_id: ctx.req.chat_id.clone().or_else(|| song.chat_id.clone()),
            lyrics: (*part == "vocals").then(|| song.lyrics.clone()).flatten(),
            ..Default::default()
        };
        out.push(store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &own_voice::wav_stereo(&l, &r, stems::RATE), None)?);
    }
    crate::applog::line("media", &format!("Took a song apart into {} parts", out.len()));
    Ok(out)
}

/// Remove FX (Suno Studio's): a sound's reverb and echo taken out
/// (`dereverb`), as a new sound kept out of the gallery: the Studio puts it
/// in as a take of the clip it came from.
pub(super) async fn dry_job(ctx: &RunCtx<'_>) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let id = ctx.req.source.as_deref().ok_or("Pick a sound first.")?;
    let song = store::get(&state.db.lock().unwrap(), &c, id).ok_or("That sound is no longer in the gallery.")?;
    if song.kind != "audio" {
        return Err("Only a sound can have its effects removed.".into());
    }
    let helper = state.catalog.media.dereverb.clone().ok_or("Removing effects isn't available.")?;
    let card = map_card(ctx).await?;
    let (dll, model) = ensure_helper(state, &helper, "Getting the effects remover (once)", ctx.cancel, ctx.progress).await?;
    (ctx.progress)("Removing the reverb and echo", 0.05);
    let bytes = store::read(&state.paths, &c, &song.id)?;
    let threads = state.limits().threads;
    let started = std::time::Instant::now();
    let ask = crate::cardfirst::hook_for_job(state, JOB_ID.try_with(|j| j.clone()).ok());
    let (dry, on_card) = tokio::task::block_in_place(|| crate::cardfirst::asking(ask, || {
        let sound = decoded_stereo(&bytes, &song.mime)?;
        let done = dereverb::dry(&dll, &model, &sound, threads, card, ctx.cancel, &|f| (ctx.progress)("Removing the reverb and echo", 0.05 + 0.9 * f));
        dereverb::unload();
        done
    }))?;
    let name = song.title.clone().unwrap_or_else(|| song.prompt.chars().take(60).collect());
    let item = MediaItem {
        kind: "audio".into(),
        op: "dry".into(),
        prompt: song.prompt.clone(),
        title: Some(format!("{name} (dry)")),
        mime: "audio/wav".into(),
        seconds: dry.len() as f64 / dereverb::RATE as f64,
        parent: Some(song.id.clone()),
        chat_id: ctx.req.chat_id.clone().or_else(|| song.chat_id.clone()),
        lyrics: song.lyrics.clone(),
        hidden: true,
        ..Default::default()
    };
    let saved = store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &own_voice::wav_stereo(&dry.ch[0], &dry.ch[1], dereverb::RATE), None)?;
    crate::applog::line("media", &format!("Removed the effects from {:.0} s of sound in {:.1} s ({})", saved.seconds, started.elapsed().as_secs_f64(), if on_card { "graphics card" } else { "processor" }));
    Ok(vec![saved])
}

/// A new part for the Studio (Suno Studio's generated stems): the mix so
/// far (`req.source`, rendered by the window) with the part added by
/// ACE-Step Base, then that part taken out alone; `count` takes (2 by
/// default, as Suno makes). Kept out of the gallery: they're the project's.
pub(super) async fn studio_part(ctx: &RunCtx<'_>, model: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let source = req.source.clone().ok_or("Pick a song first.")?;
    let spec = req.music.clone().unwrap_or_default();
    let track = spec.track.clone().ok_or("Pick the part to make.")?;
    let takes = req.count.unwrap_or(2).clamp(1, 3);
    let seed = req.seed.unwrap_or_else(random_seed);
    let c = ctx.state.work_cipher()?;
    let mut out = Vec::new();
    for v in 0..takes {
        let share = |stage: u32, f: f64| (v as f64 + (stage as f64 + f) / 2.0) / takes as f64;
        let ask = |task: &str, audio: &str| Request {
            op: "music".into(),
            model_id: Some(model.id.clone()),
            prompt: req.prompt.clone(),
            lyrics: req.lyrics.clone(),
            seed: Some(seed + v as i64),
            language: req.language.clone(),
            music: Some(MusicSpec { task: Some(task.into()), audio: Some(audio.into()), track: Some(track.clone()), genre: spec.genre.clone(), mood: spec.mood.clone(), instruments: spec.instruments.clone(), ..Default::default() }),
            ..Default::default()
        };
        // The part added to the mix…
        let add = ask("add", &source);
        let p1 = |s: &str, f: f64| (ctx.progress)(&format!("Take {} of {takes}: {}", v + 1, s.to_lowercase()), share(0, f));
        let step = RunCtx { state: ctx.state, req: &add, cancel: ctx.cancel, progress: &p1, reserved: ctx.reserved };
        let dir = work.join(format!("part-{v}-add"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mixed = song(&step, model, &dir).await?.into_iter().next().ok_or("The music model didn't add the part.")?;
        // …then taken out alone.
        let take_out = ask("extract", &mixed.id);
        let p2 = |s: &str, f: f64| (ctx.progress)(&format!("Take {} of {takes}: {}", v + 1, s.to_lowercase()), share(1, f));
        let step = RunCtx { state: ctx.state, req: &take_out, cancel: ctx.cancel, progress: &p2, reserved: ctx.reserved };
        let dir = work.join(format!("part-{v}-alone"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let alone = song(&step, model, &dir).await;
        let _ = store::delete(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &mixed.id);
        let mut part = alone?.into_iter().next().ok_or("The music model didn't take the part out.")?;
        part.hidden = true;
        part.op = "studio_part".into();
        part.title = Some(format!("{} take {}", track.replace('_', " "), v + 1));
        store::update(&ctx.state.db.lock().unwrap(), &c, &part)?;
        out.push(part);
    }
    crate::applog::line("media", &format!("Made {} take(s) of a {} part for the Studio", out.len(), track));
    Ok(out)
}

#[tauri::command(async)]
pub fn studio_projects(state: AppStateRef) -> Result<Vec<serde_json::Value>, String> {
    let c = state.cipher()?;
    Ok(studio::list(&state.db.lock().unwrap(), &c))
}

#[tauri::command(async)]
pub fn studio_project_save(state: AppStateRef, project: serde_json::Value) -> Result<(), String> {
    let c = state.cipher()?;
    studio::save(&state.db.lock().unwrap(), &c, project)
}

#[tauri::command(async)]
pub fn studio_project_delete(state: AppStateRef, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    studio::remove(&state.db.lock().unwrap(), &c, &id)
}

#[tauri::command(async)]
pub fn studio_record_start(app: AppHandle, state: AppStateRef) -> Result<(), String> {
    crate::features::require(&state, Feature::Music)?;
    state.cipher()?;
    let mic = crate::speech::voice_settings(&state.db.lock().unwrap()).mic;
    studio::record_start(app, mic)
}

/// Stops recording and keeps it (out of the gallery: it's the project's).
#[tauri::command(async)]
pub fn studio_record_stop(state: AppStateRef) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let samples = studio::record_stop()?;
    let mut item = add(&state, &c, studio::recording_wav(&samples), "audio", "audio/wav", "recording", "Recording", None, None, true)?;
    item.seconds = samples.len() as f64 / 48_000.0;
    store::update(&state.db.lock().unwrap(), &c, &item)?;
    Ok(item)
}

/// A MIDI file the window wrote (a MIDI track's notes), saved where the
/// person picked in the save dialog: a .mid file only.
#[tauri::command(async)]
pub fn studio_save_midi(state: AppStateRef, path: String, data: String) -> Result<(), String> {
    state.cipher()?;
    let lower = path.to_lowercase();
    if !(lower.ends_with(".mid") || lower.ends_with(".midi")) {
        return Err("A MIDI file's name ends in .mid.".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|_| "That MIDI couldn't be read.".to_string())?;
    if bytes.len() > 20_000_000 || !bytes.starts_with(b"MThd") {
        return Err("That isn't a MIDI file.".into());
    }
    std::fs::write(&path, bytes).map_err(|e| format!("Couldn't save it there: {e}"))
}

/// A MIDI file the person picked in the open dialog, as base64.
#[tauri::command(async)]
pub fn studio_read_midi(state: AppStateRef, path: String) -> Result<String, String> {
    state.cipher()?;
    let lower = path.to_lowercase();
    if !(lower.ends_with(".mid") || lower.ends_with(".midi")) {
        return Err("Pick a MIDI file (.mid).".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("Couldn't read it: {e}"))?;
    if bytes.len() > 20_000_000 || !bytes.starts_with(b"MThd") {
        return Err("That isn't a MIDI file.".into());
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// A sound file the person picked in the song studio, for the wavetable
/// synth to make a wavetable from (the window decodes it).
#[tauri::command(async)]
pub fn studio_read_sound(state: AppStateRef, path: String) -> Result<String, String> {
    state.cipher()?;
    let lower = path.to_lowercase();
    if ![".wav", ".mp3", ".flac", ".ogg", ".m4a", ".aac"].iter().any(|e| lower.ends_with(e)) {
        return Err("Pick a sound file (WAV, MP3, FLAC, OGG, M4A or AAC).".into());
    }
    let meta = std::fs::metadata(&path).map_err(|e| format!("Couldn't read it: {e}"))?;
    if meta.len() > 50_000_000 {
        return Err("That file is too big for a wavetable (50 MB at most).".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("Couldn't read it: {e}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// What the song studio keeps by name: `kind` "synth" (the wavetable
/// synth's sounds) or "effects" (effects built from words or saved).
#[tauri::command(async)]
pub fn studio_library(state: AppStateRef, kind: String) -> Result<Vec<serde_json::Value>, String> {
    let c = state.cipher()?;
    Ok(studio::presets(&state.db.lock().unwrap(), &c, &kind))
}

#[tauri::command(async)]
pub fn studio_library_save(state: AppStateRef, kind: String, item: serde_json::Value) -> Result<(), String> {
    let c = state.cipher()?;
    studio::save_preset(&state.db.lock().unwrap(), &c, &kind, item)
}

#[tauri::command(async)]
pub fn studio_library_delete(state: AppStateRef, kind: String, name: String) -> Result<(), String> {
    let c = state.cipher()?;
    studio::remove_preset(&state.db.lock().unwrap(), &c, &kind, &name)
}

/// The song studio's chat bar: a request turned into actions on the
/// project (`studio::ask_messages`) by the chat model; the window does them.
#[tauri::command]
pub async fn studio_ask(state: AppStateRef<'_>, request: String, summary: String) -> Result<Vec<serde_json::Value>, String> {
    state.cipher()?;
    ask_studio(state.inner(), &request, &summary).await
}

pub(crate) async fn ask_studio(state: &Arc<AppState>, request: &str, summary: &str) -> Result<Vec<serde_json::Value>, String> {
    let (ep, _, _hold) = crate::background_endpoint(state).await.map_err(|_| "The chat bar needs a chat model. Install one from Models.".to_string())?;
    let answer = crate::chat::complete(&ep, studio::ask_messages(request.trim(), summary), json!({ "temperature": 0.2 }), 800).await?;
    let actions = studio::actions_of(&answer);
    if actions.is_empty() {
        return Err("The chat model didn't say what to do. Try saying it another way.".into());
    }
    Ok(actions)
}

#[tauri::command(async)]
pub fn studio_record_cancel() {
    studio::record_cancel();
}

/// A track's notes read from its sound (Suno Studio's audio to MIDI):
/// Basic Pitch's notes, or with `drums` the kick, snare and hat hits.
#[tauri::command]
pub async fn studio_midi(state: AppStateRef<'_>, id: String, drums: bool) -> Result<Vec<notes::Note>, String> {
    state.cipher()?;
    read_notes(state.inner(), &id, drums).await
}

#[cfg(test)]
pub(crate) async fn studio_midi_for_test(state: &Arc<AppState>, id: &str, drums: bool) -> Result<Vec<notes::Note>, String> {
    read_notes(state, id, drums).await
}

#[cfg(test)]
pub(crate) fn audio_in_decode_for_test(path: &std::path::Path) -> (Vec<f32>, u32, u16) {
    audio_in::decode(path).unwrap()
}

pub(super) async fn read_notes(state: &Arc<AppState>, id: &str, drums: bool) -> Result<Vec<notes::Note>, String> {
    let c = state.work_cipher()?;
    let state = state.clone();
    let id = id.to_string();
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That sound is no longer here.")?;
    let bytes = store::read(&state.paths, &c, &id)?;
    let threads = state.limits().threads;
    if drums {
        return tokio::task::spawn_blocking(move || -> Result<Vec<notes::Note>, String> {
            let mix = decoded_stereo(&bytes, &item.mime)?;
            Ok(notes::drum_hits(&mix.mono(), stems::RATE))
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    let reader = state.catalog.media.note_reader.clone().ok_or("The note reader isn't available.")?;
    let never = AtomicBool::new(false);
    let (dll, model) = ensure_helper(&state, &reader, "Getting the note reader (once)", &never, &|_, _| {}).await?;
    tokio::task::spawn_blocking(move || -> Result<Vec<notes::Note>, String> {
        let mix = decoded_stereo(&bytes, &item.mime)?;
        let found = notes::melodic(&dll, &model, &stems::resample(&mix.mono(), stems::RATE, notes::RATE), threads, None);
        notes::unload();
        found
    })
    .await
    .map_err(|e| e.to_string())?
}

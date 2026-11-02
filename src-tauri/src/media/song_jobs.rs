// SPDX-License-Identifier: AGPL-3.0-only
//! Custom music models trained, song videos, songs heard and mapped, sung
//! again in a recorded voice, remade, and narration.

use super::*;

/// About how long a training job takes: labelling each song, turning it
/// into tensors, and its epochs (about 0.8 s a song an epoch on an RTX 5070
/// for songs of up to four minutes), plus setting the trainer up the first time.
pub(super) fn train_estimate(state: &AppState, req: &Request) -> f64 {
    let n = req.train.as_ref().map_or(0, |t| t.files.len() + t.songs.len());
    let setup = match &state.catalog.media.trainer {
        Some(t) if !train::ready(&state.paths, t) => 900.0,
        _ => 60.0,
    };
    setup + n as f64 * 40.0 + train::epochs_for(n) as f64 * n as f64 * 0.8
}

/// The trainer, set up the first time (`train`): uv, ACE-Step's source and
/// its locked Python packages, and the Base model's training files.
#[cfg(windows)]
pub(super) async fn ensure_trainer(ctx: &RunCtx<'_>, t: &train::TrainerCatalog) -> Result<PathBuf, String> {
    let state = ctx.state;
    let paths = &state.paths;
    (ctx.progress)("Getting the trainer (once)", 0.01);
    let uv = engine::ensure_unpacked(paths, "trainer-uv-bin", &t.uv, "uv.exe", ctx.cancel, |r, all| (ctx.progress)("Getting the trainer (once)", 0.01 * r as f64 / all.max(1) as f64)).await?;
    let source = train::source_dir(paths, t);
    let dir_name = source.file_name().unwrap_or_default().to_string_lossy().into_owned();
    engine::ensure_unpacked(paths, &dir_name, &t.source, "train.py", ctx.cancel, |r, all| (ctx.progress)("Getting the trainer (once)", 0.01 + 0.01 * r as f64 / all.max(1) as f64)).await?;
    if !source.join(".venv").join("Scripts").join("python.exe").is_file() {
        // Its Python and packages (PyTorch for NVIDIA cards and the rest), as locked.
        state.log("network", "Downloading the music trainer's Python packages from pypi.org and download.pytorch.org");
        (ctx.progress)("Setting up the trainer (once, about 6 GB)", 0.02);
        let ran = proc::run_env(
            proc::Run {
                exe: &uv,
                args: ["sync", "--frozen", "--python", "3.12"].map(std::ffi::OsString::from).to_vec(),
                cwd: &source,
                log: &paths.logs.join("music-trainer-setup.log"),
                low_priority: state.limits().low_priority,
                job: state.job(),
            },
            &train::trainer_env(paths),
            ctx.cancel,
            |_| {},
        )
        .await;
        if let Err(e) = ran {
            // A half-made environment would look ready next time.
            let _ = std::fs::remove_dir_all(source.join(".venv"));
            return Err(e);
        }
    }
    let ckpt = train::checkpoints_dir(paths);
    let missing: Vec<&train::TrainerFile> = t.checkpoints.iter().filter(|f| !ckpt.join(&f.file).is_file()).collect();
    if !missing.is_empty() {
        state.log("network", "Downloading ACE-Step 1.5 Base's training files from huggingface.co");
        let client = net::external_client(net::Connectivity::Offline, net::Purpose::ModelDownload, false)?;
        let total: u64 = missing.iter().map(|f| f.size).sum::<u64>().max(1);
        let mut done = 0u64;
        for f in missing {
            let base = done;
            download::fetch_verified(&client, &f.url, &ckpt.join(&f.file), f.size, &f.sha256, ctx.cancel, |r, _| {
                (ctx.progress)("Getting the Base model's training files (once)", 0.03 + 0.05 * (base + r) as f64 / total as f64)
            })
            .await?;
            done += f.size;
        }
    }
    Ok(uv)
}

/// A custom model trained on the person's songs (`train`): each song's
/// words heard, the songs turned into the trainer's tensors, then trained;
/// the adapter is kept with the music models and the model listed.
#[cfg(windows)]
pub(super) async fn train_job(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let spec = ctx.req.train.clone().ok_or("Pick your songs first.")?;
    let t = state.catalog.media.trainer.clone().ok_or("Training isn't available on this system yet.")?;
    let started = Instant::now();
    let uv = ensure_trainer(ctx, &t).await?;
    let c = state.work_cipher()?;
    // The songs, copied for the trainer (it reads MP3, WAV, FLAC, OGG, Opus, M4A).
    let audio = work.join("audio");
    std::fs::create_dir_all(&audio).map_err(|e| e.to_string())?;
    let mut copies: Vec<PathBuf> = Vec::new();
    for f in &spec.files {
        let from = PathBuf::from(f);
        let ext = from.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !matches!(ext.as_str(), "mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a") {
            return Err(format!("{} isn't a song file the trainer reads (MP3, WAV, FLAC, OGG, Opus or M4A).", from.file_name().unwrap_or_default().to_string_lossy()));
        }
        let to = audio.join(format!("{:02}.{ext}", copies.len() + 1));
        std::fs::copy(&from, &to).map_err(|e| format!("Couldn't read {}: {e}", from.display()))?;
        copies.push(to);
    }
    // A recorded voice's singing (short takes teach little), joined into
    // clips as long as the trainer wants.
    if let Some(vid) = spec.voice.as_deref().filter(|v| !v.is_empty()) {
        let voice = own_voice::get(&state.db.lock().unwrap(), &c, vid).ok_or("That recorded voice is no longer here.")?;
        let takes: Vec<Vec<f32>> = own_voice::sung_takes(&state.paths, &c, &voice)?.into_iter().filter(|t| t.len() >= 48_000 * 4).collect();
        let seconds = takes.iter().map(|t| t.len()).sum::<usize>() as f64 / 48_000.0;
        if seconds < 10.0 {
            return Err(format!("“{}” has {seconds:.0} seconds of singing recorded: training needs 10 or more (half a minute or more teaches it much better). Sing more of the singing lines in Your voice, or add songs where you sing.", voice.name));
        }
        for clip in train::singing_clips(takes, 48_000) {
            let to = audio.join(format!("{:02}.wav", copies.len() + 1));
            std::fs::write(&to, crate::audio::wav_encode(&clip, 48_000)).map_err(|e| e.to_string())?;
            copies.push(to);
        }
    }
    for id in &spec.songs {
        let item = store::get(&state.db.lock().unwrap(), &c, id).ok_or("A song picked is no longer in the gallery.")?;
        let ext = if item.mime.contains("wav") { "wav" } else { "mp3" };
        let to = audio.join(format!("{:02}.{ext}", copies.len() + 1));
        std::fs::write(&to, store::read(&state.paths, &c, id)?).map_err(|e| e.to_string())?;
        copies.push(to);
    }
    // Each song's words, heard by speech recognition (none installed: all
    // are taught as they sound, without words).
    let listen = !crate::speech::installed(&state.db.lock().unwrap()).is_empty();
    let mut samples = Vec::new();
    for (i, file) in copies.iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        (ctx.progress)(&format!("Listening to song {} of {}", i + 1, copies.len()), 0.08 + 0.1 * i as f64 / copies.len() as f64);
        let path = file.clone();
        let (mono, seconds) = tokio::task::spawn_blocking(move || -> Result<(Vec<f32>, f64), String> {
            let (samples, rate, channels) = audio_in::decode(&path)?;
            let ch = channels.max(1) as usize;
            let mono: Vec<f32> = samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
            let seconds = mono.len() as f64 / rate as f64;
            Ok((stems::resample(&mono, rate, crate::audio::RATE), seconds))
        })
        .await
        .map_err(|e| e.to_string())??;
        if seconds < 10.0 {
            return Err(format!("Song {} is only {seconds:.0} seconds: use whole songs (30 seconds to 4 minutes).", i + 1));
        }
        let lyrics = if listen {
            let heard = crate::speech::transcribe_song(state, &mono, None).await?;
            train::lyrics_from(&heard.into_iter().map(|s| s.text).collect::<Vec<_>>())
        } else {
            None
        };
        samples.push(train::Sample { file: file.clone(), lyrics, seconds });
    }
    let trigger = train::trigger_of(&spec.name);
    let description = Some(spec.description.trim()).filter(|d| !d.is_empty()).unwrap_or("song").to_string();
    std::fs::write(work.join("dataset.json"), train::dataset_json(&samples, &description, &trigger).to_string()).map_err(|e| e.to_string())?;
    let n = samples.len();
    // (Tests train briefly.)
    let short = cfg!(test).then(|| std::env::var("SULCUSAI_TRAIN_EPOCHS").ok()).flatten().and_then(|e| e.parse().ok());
    let epochs = short.unwrap_or_else(|| train::epochs_for(n));
    let source = train::source_dir(&state.paths, &t);
    let ckpt = train::checkpoints_dir(&state.paths);
    let env = train::trainer_env(&state.paths);
    let log = state.paths.logs.join("music-trainer.log");
    let low = state.limits().low_priority;
    // Turned into tensors (two passes over the songs), then trained.
    let (mut pass1, mut pass2) = (0u32, 0u32);
    proc::run_env(
        proc::Run { exe: &uv, args: train::trainer_args(&source, &ckpt, work, epochs, true), cwd: work, log: &log, low_priority: low, job: state.job() },
        &env,
        ctx.cancel,
        |l| match train::progress_of(l, n) {
            Some(("pass1", ..)) => {
                pass1 += 1;
                (ctx.progress)("Reading your songs", 0.18 + 0.06 * pass1 as f64 / n as f64)
            }
            Some(("pass2", ..)) => {
                pass2 += 1;
                (ctx.progress)("Reading your songs", 0.24 + 0.06 * pass2 as f64 / n as f64)
            }
            _ => {}
        },
    )
    .await?;
    proc::run_env(
        proc::Run { exe: &uv, args: train::trainer_args(&source, &ckpt, work, epochs, false), cwd: work, log: &log, low_priority: low, job: state.job() },
        &env,
        ctx.cancel,
        |l| {
            if let Some(("epoch", done, of)) = train::progress_of(l, n) {
                (ctx.progress)(&format!("Learning your songs: pass {done} of {of}"), 0.3 + 0.68 * done as f64 / of.max(1) as f64);
            }
        },
    )
    .await?;
    let made = work.join("out").join("final");
    let id = store::new_id();
    let dest = train::adapters_dir(&state.paths).join(&id);
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    for f in ["adapter_model.safetensors", "adapter_config.json"] {
        std::fs::copy(made.join(f), dest.join(f)).map_err(|_| "The trainer didn't write the model. See music-trainer.log in Settings › Logs.".to_string())?;
    }
    let minutes = started.elapsed().as_secs_f64() / 60.0;
    let model = train::CustomModel { id, name: spec.name.trim().chars().take(60).collect(), trigger, description, songs: n, epochs, minutes, created_at: db::now_ms() };
    train::add(&state.db.lock().unwrap(), &c, model.clone())?;
    crate::applog::line("media", &format!("Trained a custom model on {n} songs ({epochs} passes) in {minutes:.0} min"));
    emit(state, "media:trained", json!(model));
    Ok(Vec::new())
}

#[tauri::command(async)]
pub fn music_custom_models(state: AppStateRef) -> Result<serde_json::Value, String> {
    let c = state.cipher()?;
    let models = train::list(&state.db.lock().unwrap(), &c);
    let t = state.catalog.media.trainer.as_ref();
    Ok(json!({
        "models": models,
        "available": t.is_some() && cfg!(windows),
        // Recorded voices and how much singing each has, to train on.
        "voices": own_voice::list(&state.db.lock().unwrap(), &c).iter().map(|v| json!({ "id": v.id, "name": v.name, "sung": own_voice::sung_count(v) })).collect::<Vec<_>>(),
        "nvidia": crate::natural::nvidia_here(&state),
        "ready": t.is_some_and(|t| train::ready(&state.paths, t)),
        "setup_bytes": t.map_or(0, train::setup_bytes),
        "base_installed": installed(&state.db.lock().unwrap()).iter().any(|i| i.model_id == train::BASE_MODEL),
    }))
}

/// The ways a picture can be made to talk here: quick (Ditto, NVIDIA
/// cards) and detailed (Wan 2.2 S2V, when installed).
#[tauri::command(async)]
pub fn media_talk_ways(state: AppStateRef) -> serde_json::Value {
    let d = state.catalog.media.ditto.as_ref();
    let detailed = installed(&state.db.lock().unwrap()).iter().any(|i| state.catalog.media.models.iter().any(|m| m.id == i.model_id && m.kind == Kind::Video && m.can("speech")));
    let processor = !crate::natural::nvidia_here(&state);
    json!({
        "quick": d.is_some() && cfg!(windows),
        "processor": processor,
        "ready": d.is_some_and(|d| ditto::ready(&state.paths, d, processor)),
        "setup_bytes": d.map_or(0, |d| ditto::setup_bytes(d, processor)),
        "detailed": detailed,
    })
}

#[tauri::command(async)]
pub fn music_custom_model_delete(state: AppStateRef, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    train::remove(&state.db.lock().unwrap(), &c, &state.paths, &id)
}

/// A song's video (Suno's Create video, `songvideo`): its cover art, title
/// and lyrics as they're sung, with the song as the sound, kept in the
/// gallery under the song. Where the lines are sung comes from the
/// listener, who listens first when the song hasn't been listened to.
#[cfg(windows)]
pub(super) async fn song_video_job(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    use songvideo::{FPS, SIDE};
    let state = ctx.state;
    let c = state.work_cipher()?;
    let id = ctx.req.source.as_deref().ok_or("Pick a song first.")?;
    let mut song = store::get(&state.db.lock().unwrap(), &c, id).ok_or("That song is no longer in the gallery.")?;
    if song.kind != "audio" {
        return Err("Only a song can be made into a video.".into());
    }
    let sung = song.lyrics.as_deref().is_some_and(lyrics::has_singing);
    if sung && song.review.is_none() && !crate::speech::installed(&state.db.lock().unwrap()).is_empty() {
        (ctx.progress)("Listening for where the words are", 0.05);
        song.review = Some(hear(state, &song, ctx.cancel).await?);
        store::update(&state.db.lock().unwrap(), &c, &song)?;
    }
    let art = if song.poster { store::read_thumb(&state.paths, &c, &song.id).ok().and_then(|b| image::load_from_memory(&b).ok()).map(|i| i.to_rgba8()) } else { None };
    let bytes = store::read(&state.paths, &c, &song.id)?;
    let out = work.join("song.mp4");
    let name = song.title.clone().unwrap_or_else(|| song.prompt.chars().take(80).collect());
    let lines: Vec<(String, Option<[f64; 2]>)> = match (&song.review, song.lyrics.as_deref()) {
        (Some(r), _) if !r.lines.is_empty() => r.lines.iter().map(|l| (l.text.clone(), l.at)).collect(),
        (_, Some(l)) if sung => l.lines().map(str::trim).filter(|x| !x.is_empty() && !(x.starts_with('[') && x.ends_with(']'))).map(|x| (x.to_string(), None)).collect(),
        _ => Vec::new(),
    };
    let first = tokio::task::block_in_place(|| -> Result<image::RgbaImage, String> {
        // The sound at 48 kHz (or 44.1), 16-bit stereo.
        let mix = decoded_stereo(&bytes, &song.mime)?;
        let rate = if mix.rate == 44_100 { 44_100 } else { 48_000 };
        let mix = mix.resampled(rate);
        let seconds = mix.len() as f64 / rate as f64;
        let pcm: Vec<i16> = (0..mix.len()).flat_map(|i| [mix.ch[0][i], mix.ch[1][i]]).map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
        let timed = songvideo::timings(&lines, seconds);
        let title = songvideo::text_mask(&name, 54, true, SIDE - 120)?;
        let base = songvideo::backdrop(art.as_ref(), &name, Some(&title));
        let lyric_y = if art.is_some() { 690 + title.height() + 24 } else { 300 + title.height() + 80 };
        let mut masks: std::collections::HashMap<(usize, bool), image::GrayImage> = std::collections::HashMap::new();
        // The picture changes a line at a time: 1.5 Mbps is plenty.
        let mut writer = mp4::Writer::create_with_sound(&out, SIDE, SIDE, FPS, Some(rate), Some(1_500_000))?;
        let frames = (seconds * FPS as f64).ceil() as usize;
        let mut first = None;
        let mut sent = 0usize;
        for f in 0..frames {
            if f % FPS as usize == 0 {
                if ctx.cancel.load(Ordering::Relaxed) {
                    return Err(download::CANCELLED.into());
                }
                (ctx.progress)("Making the video", 0.1 + 0.85 * f as f64 / frames as f64);
                // The sound a second ahead of the pictures.
                let upto = (((f / FPS as usize) + 1) * rate as usize * 2).min(pcm.len());
                writer.sound(&pcm[sent..upto])?;
                sent = upto;
            }
            let t = f as f64 / FPS as f64;
            let mut frame = base.clone();
            let (now, next) = songvideo::showing(&timed, t);
            let mut y = lyric_y;
            for (i, big) in [(now, true), (next, false)].into_iter().filter_map(|(i, b)| i.map(|i| (i, b))) {
                if !masks.contains_key(&(i, big)) {
                    masks.insert((i, big), songvideo::text_mask(&timed[i].text, if big { 44 } else { 34 }, big, SIDE - 140)?);
                }
                let m = &masks[&(i, big)];
                songvideo::stamp(&mut frame, m, (SIDE - m.width()) / 2, y, [255, 255, 255], if big { 1.0 } else { 0.5 });
                y += m.height() + 10;
            }
            // How far along: a thin bar at the bottom.
            let done = ((t / seconds.max(0.1)) * SIDE as f64) as u32;
            for x in 0..done.min(SIDE) {
                for yy in SIDE - 10..SIDE - 4 {
                    frame.put_pixel(x, yy, image::Rgba([255, 255, 255, 255]));
                }
            }
            if first.is_none() && (now.is_some() || f as f64 >= seconds * 0.2) {
                first = Some(frame.clone());
            }
            writer.frame(&frame)?;
        }
        writer.sound(&pcm[sent..])?;
        writer.finish()?;
        Ok(first.unwrap_or(base))
    })?;
    let mp4 = std::fs::read(&out).map_err(|e| e.to_string())?;
    let thumb = imaging::thumbnail(&image::DynamicImage::ImageRgba8(first))?;
    let item = MediaItem {
        kind: "video".into(),
        op: "song_video".into(),
        prompt: song.prompt.clone(),
        title: song.title.clone(),
        mime: "video/mp4".into(),
        width: SIDE,
        height: SIDE,
        seconds: song.seconds,
        parent: Some(song.id.clone()),
        chat_id: ctx.req.chat_id.clone().or_else(|| song.chat_id.clone()),
        lyrics: song.lyrics.clone(),
        poster: true,
        ..Default::default()
    };
    crate::applog::line("media", &format!("Made a song video: {:.0} s, {} lines", song.seconds, lines.len()));
    Ok(vec![store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &mp4, Some(&thumb))?])
}

/// What the listener hears in a song from the gallery (`listen`): which of
/// its words come through and where, from speech recognition. Nothing heard
/// when no speech model is installed (the review then says so).
pub(super) async fn hear(state: &Arc<AppState>, item: &MediaItem, cancel: &AtomicBool) -> Result<listen::Review, String> {
    let c = state.work_cipher()?;
    let bytes = store::read(&state.paths, &c, &item.id)?;
    let lyrics = item.lyrics.clone().unwrap_or_default();
    let samples = tokio::task::spawn_blocking(move || -> Result<Vec<f32>, String> {
        let dir = std::env::temp_dir().join(format!("sulcusai-listen-{}", store::new_id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("song.mp3");
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        let decoded = audio_in::decode(&path);
        let _ = std::fs::remove_dir_all(&dir);
        let (samples, rate, channels) = decoded?;
        let ch = channels.max(1) as usize;
        let mono: Vec<f32> = samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
        Ok(stems::resample(&mono, rate, crate::audio::RATE))
    })
    .await
    .map_err(|e| e.to_string())??;
    if cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    // No speech model installed: the review says so.
    let installed = !crate::speech::installed(&state.db.lock().unwrap()).is_empty();
    let heard = if installed { Some(crate::speech::transcribe_song(state, &samples, None).await?) } else { None };
    Ok(listen::review(&lyrics, heard.as_deref(), item.seconds))
}

/// Of several versions of a sung song, the one whose words were heard best
/// (the first of equals), with its review; the others are deleted. All are
/// kept, as made, when there are no words or nothing to listen with.
pub(super) async fn keep_clearest(ctx: &RunCtx<'_>, items: Vec<MediaItem>) -> Result<Vec<MediaItem>, String> {
    if !items.iter().all(|i| i.lyrics.as_deref().is_some_and(lyrics::has_singing)) || crate::speech::installed(&ctx.state.db.lock().unwrap()).is_empty() {
        crate::applog::line("media", "Kept every version: nothing sung, or no speech model to listen with");
        return Ok(items);
    }
    let c = ctx.state.work_cipher()?;
    let n = items.len();
    let mut heard = Vec::new();
    for (i, item) in items.iter().enumerate() {
        (ctx.progress)(&format!("Listening to version {} of {n}", i + 1), i as f64 / n as f64);
        heard.push(hear(ctx.state, item, ctx.cancel).await?);
    }
    let best = (0..n).fold(0, |b, i| if heard[i].score > heard[b].score { i } else { b });
    crate::applog::line("media", &format!("Kept version {} of {n}: scores {:?}", best + 1, heard.iter().map(|r| r.score).collect::<Vec<_>>()));
    let mut kept = None;
    for (i, (mut item, review)) in items.into_iter().zip(heard).enumerate() {
        if i == best {
            let clearest = format!("The clearest of {n} versions: {} of {} words heard.", review.heard, review.words);
            item.note = Some(item.note.take().map_or(clearest.clone(), |was| format!("{was} {clearest}")));
            item.review = Some(review);
            store::update(&ctx.state.db.lock().unwrap(), &c, &item)?;
            kept = Some(item);
        } else {
            store::delete(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &item.id)?;
        }
    }
    Ok(kept.into_iter().collect())
}

/// Listens to a song (`req.source`) and keeps the review on it.
pub(super) async fn listen_job(ctx: &RunCtx<'_>) -> Result<Vec<MediaItem>, String> {
    let id = ctx.req.source.as_deref().ok_or("Pick a song to listen to.")?;
    let c = ctx.state.work_cipher()?;
    let mut item = store::get(&ctx.state.db.lock().unwrap(), &c, id).ok_or("That song is no longer in the gallery.")?;
    if item.kind != "audio" || item.lyrics.as_deref().is_none_or(|l| !lyrics::has_singing(l)) {
        return Err("Only a sung song can be listened to.".into());
    }
    (ctx.progress)("Listening", 0.2);
    item.review = Some(hear(ctx.state, &item, ctx.cancel).await?);
    store::update(&ctx.state.db.lock().unwrap(), &c, &item)?;
    crate::applog::line("media", &format!("Listened to a song: score {}", item.review.as_ref().map_or(0, |r| r.score)));
    Ok(vec![item])
}

/// How a song (`req.source`) is built, for the song view: its parts taken
/// apart (HTDemucs), their notes read (Basic Pitch; drums by their hits),
/// and its sections placed where their words are sung (the listener's
/// review, listened for first when the song has none). Kept on the song.
pub(super) async fn map_job(ctx: &RunCtx<'_>) -> Result<Vec<MediaItem>, String> {
    let id = ctx.req.source.as_deref().ok_or("Pick a song first.")?;
    let item = store::get(&ctx.state.db.lock().unwrap(), &ctx.state.work_cipher()?, id).ok_or("That song is no longer in the gallery.")?;
    Ok(vec![map_item(ctx, item).await?])
}

/// The graphics card a song's work (its parts, its voice sung again) runs
/// on, the cards' ONNX Runtime downloaded the first time. When no card can
/// take it, why is said and the person asked before the processor does it
/// (`cardfirst`); None then.
pub(super) async fn map_card(ctx: &RunCtx<'_>) -> Result<Option<crate::natural::Card>, String> {
    let state = ctx.state;
    // Started now on the processor, as they asked.
    if ON_PROCESSOR.try_with(|p| *p).unwrap_or(false) {
        (ctx.progress)("On the processor, as you asked (slower)", 0.0);
        return Ok(None);
    }
    loop {
        let why = match card_for_onnx(ctx).await {
            // The rest runs on the card through DirectML; the part separator
            // can't (`stems::separate`), so it's the processor's, once they say yes.
            Ok(card) if card.dml => {
                let job = JOB_ID.try_with(|j| j.clone()).ok();
                let why = "this graphics card works here through DirectML, which can't run the part separator (the rest of the work stays on the card)";
                crate::cardfirst::processor_ok(state, job.as_deref(), "Taking the song apart", why, ctx.cancel, ctx.progress).await?;
                return Ok(Some(card));
            }
            Ok(card) => return Ok(Some(card)),
            Err(e) if e == download::CANCELLED => return Err(e),
            Err(why) => why,
        };
        let job = JOB_ID.try_with(|j| j.clone()).ok();
        match crate::cardfirst::processor_ok(state, job.as_deref(), "This song's work", &why, ctx.cancel, ctx.progress).await? {
            crate::cardfirst::Go::Again => continue,
            crate::cardfirst::Go::Processor => {
                (ctx.progress)(PROCESSOR_STAGE, 0.0);
                return Ok(None);
            }
        }
    }
}

/// The card for ONNX Runtime's work now (`natural::pick_card`), or why not.
pub(super) async fn card_for_onnx(ctx: &RunCtx<'_>) -> Result<crate::natural::Card, String> {
    let state = ctx.state;
    let voices = &state.catalog.voices;
    if !crate::natural::card_here(state) {
        return Err(if state.hardware.read().unwrap().best_gpu().is_some() { "there's no ONNX Runtime for graphics cards on this system".into() } else { crate::natural::no_card(state) });
    }
    if crate::natural::card_runtime_dll(&state.paths, voices).is_none() {
        let getting = format!("Getting the graphics card's libraries (once, {})", crate::size_label(crate::natural::card_runtime_size(&state.paths, voices, false)));
        (ctx.progress)(&getting, 0.0);
        match crate::natural::ensure_card_runtime(state, false, ctx.cancel, &|r, t| (ctx.progress)(&getting, r as f64 / t.max(1) as f64)).await {
            Ok(_) => {}
            Err(e) if e == download::CANCELLED => return Err(e),
            Err(e) => return Err(format!("the graphics card's libraries couldn't be downloaded ({e})")),
        }
    }
    if crate::natural::loaded_for_processor() {
        return Err("SulcusAI loaded the processor's libraries before the graphics card's were here, and only one can be loaded until it starts again".into());
    }
    // The second card: the job's place in line, or handed over to it.
    let job = JOB_ID.try_with(|j| j.clone()).ok();
    let second = SECOND_CARD.try_with(|s| *s).unwrap_or(false) || job.as_deref().and_then(crate::lineup::card_of) == Some(crate::lineup::Card::Second);
    let mut p = crate::natural::pick_card(state, second).await?;
    // CUDA's part, the first time an NVIDIA card it runs on takes work.
    if p.needs_cuda {
        let getting = format!("Getting the graphics card's libraries (once, {})", crate::size_label(crate::natural::card_runtime_size(&state.paths, voices, true)));
        (ctx.progress)(&getting, 0.0);
        match crate::natural::ensure_card_runtime(state, true, ctx.cancel, &|r, t| (ctx.progress)(&getting, r as f64 / t.max(1) as f64)).await {
            Ok(_) => {}
            Err(e) if e == download::CANCELLED => return Err(e),
            // DirectML on the same card meanwhile.
            Err(e) => match state.hardware.read().unwrap().gpus.iter().find(|g| g.name == p.name).and_then(|g| g.dxgi) {
                Some(i) => {
                    crate::applog::line("media", &format!("Couldn't get CUDA's part ({e}); DirectML runs the work on {} instead", p.name));
                    p.card = crate::natural::Card { device: i as i32, dml: true };
                }
                None => return Err(format!("the graphics card's libraries couldn't be downloaded ({e})")),
            },
        }
    }
    crate::applog::line("media", &format!("On {}{} ({})", p.name, if p.second { ", the second card" } else { "" }, if p.card.dml { "DirectML" } else { "CUDA" }));
    Ok(p.card)
}

/// `map_job` for one song, which is kept with its map.
pub(super) async fn map_item(ctx: &RunCtx<'_>, mut item: MediaItem) -> Result<MediaItem, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    if item.kind != "audio" {
        return Err("Only a song can be shown this way.".into());
    }
    let sep = state.catalog.media.separator.clone().ok_or("The part separator isn't available.")?;
    let reader = state.catalog.media.note_reader.clone().ok_or("The note reader isn't available.")?;
    let card = map_card(ctx).await?;
    let (dll, separator) = ensure_helper(state, &sep, "Getting the part separator (once)", ctx.cancel, ctx.progress).await?;
    let (_, note_reader) = ensure_helper(state, &reader, "Getting the note reader (once)", ctx.cancel, ctx.progress).await?;
    if item.review.is_none() && item.lyrics.as_deref().is_some_and(lyrics::has_singing) {
        (ctx.progress)("Listening for where the words are", 0.1);
        item.review = Some(hear(state, &item, ctx.cancel).await?);
    }
    (ctx.progress)("Taking the parts apart", 0.4);
    let bytes = store::read(&state.paths, &c, &item.id)?;
    let threads = state.limits().threads;
    let mime = item.mime.clone();
    let ask = crate::cardfirst::hook_for_job(state, JOB_ID.try_with(|j| j.clone()).ok());
    let (parts, on_card, times) = tokio::task::spawn_blocking(move || crate::cardfirst::asking(ask, || -> Result<(Vec<songmap::Part>, bool, [f64; 2]), String> {
        let song = decoded_stereo(&bytes, &mime)?;
        let started = Instant::now();
        let (stems, on_card) = stems::separate(&dll, &separator, &song, threads, &[0, 1, 2, 3], card, &AtomicBool::new(false))?;
        stems::unload();
        let apart = started.elapsed().as_secs_f64();
        let card = card.filter(|_| on_card);
        let mut parts = Vec::new();
        // The voice first, then the other instruments, the bass and the drums.
        for i in [3usize, 2, 1, 0] {
            let mono = stems[i].mono();
            let notes = if i == 0 {
                notes::drum_hits(&mono, stems::RATE)
            } else {
                let mut n = notes::melodic(&dll, &note_reader, &stems::resample(&mono, stems::RATE, notes::RATE), threads, card)?;
                // A bass part's notes are low; what's higher is another instrument's spill.
                if i == 1 {
                    n.retain(|x| x.pitch < 60);
                }
                n
            };
            parts.push(songmap::Part { name: stems::PARTS[i].to_string(), notes });
        }
        notes::unload();
        Ok((parts, on_card, [apart, started.elapsed().as_secs_f64() - apart]))
    }))
    .await
    .map_err(|e| e.to_string())??;
    crate::applog::line(
        "media",
        &format!("Took a song's parts apart in {:.1}s and read their notes in {:.1}s on the {}", times[0], times[1], if on_card { "graphics card" } else { "processor" }),
    );
    if card.is_some() && !on_card {
        (ctx.progress)(PROCESSOR_STAGE, 0.9);
    }
    let map = songmap::map(item.review.as_ref(), parts, item.seconds);
    crate::applog::line(
        "media",
        &format!("Mapped a song: {} sections, {} notes", map.sections.len(), map.parts.iter().map(|p| p.notes.len()).sum::<usize>()),
    );
    item.map = Some(map);
    store::update(&state.db.lock().unwrap(), &c, &item)?;
    Ok(item)
}

/// Sings a song from the gallery (`req.source`) again in a recorded voice
/// (`req.voice`).
pub(super) async fn revoice_job(ctx: &RunCtx<'_>) -> Result<Vec<MediaItem>, String> {
    let id = ctx.req.source.as_deref().ok_or("Pick a song first.")?;
    let voice = ctx.req.voice.clone().ok_or("Pick whose voice sings it.")?;
    let item = store::get(&ctx.state.db.lock().unwrap(), &ctx.state.work_cipher()?, id).ok_or("That song is no longer in the gallery.")?;
    if item.kind != "audio" {
        return Err("Only a song can be sung in another voice.".into());
    }
    Ok(vec![revoice_item(ctx, &item, &voice).await?])
}

/// A song with its voice sung again in a recorded one: the voice taken out
/// (HTDemucs), sung in theirs (`svc`) in a key that suits their range
/// (`svc::fit_key`: the band moved with it when octaves alone don't), and
/// put back over the band at its old loudness. Kept as a new song beside
/// the first.
pub(super) async fn revoice_item(ctx: &RunCtx<'_>, item: &MediaItem, voice_id: &str) -> Result<MediaItem, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let voice = own_voice::get(&state.db.lock().unwrap(), &c, voice_id).ok_or("That recorded voice is no longer here.")?;
    let (reference, sung) = own_voice::singing_reference(&state.paths, &c, &voice)?;
    let all_singing = own_voice::all_singing(&state.paths, &c, &voice);
    let sep = state.catalog.media.separator.clone().ok_or("The part separator isn't available.")?;
    // With a second card and something waiting for the main one: sung there,
    // and the main card goes on to the next in line.
    if let Some(job) = JOB_ID.try_with(|j| j.clone()).ok() {
        crate::lineup::hand_over(state, &job);
    }
    let card = map_card(ctx).await?;
    let (dll, separator) = ensure_helper(state, &sep, "Getting the part separator (once)", ctx.cancel, ctx.progress).await?;
    let models = own_voice::sing_parts(state, ctx.cancel, ctx.progress).await?;
    (ctx.progress)("Taking the voice out", 0.05);
    let bytes = store::read(&state.paths, &c, &item.id)?;
    let threads = state.limits().threads;
    let singing = format!("Singing it in the recorded voice “{}”", voice.name);
    // Long work that reports as it goes and can be stopped: it borrows the
    // job's progress and cancel, so it runs here rather than on its own thread.
    let ask = crate::cardfirst::hook_for_job(state, JOB_ID.try_with(|j| j.clone()).ok());
    let (out, on_card, secs, band_moved) = tokio::task::block_in_place(|| crate::cardfirst::asking(ask, || -> Result<(stems::Stereo, (bool, bool), f64, i32), String> {
        let started = Instant::now();
        let song = decoded_stereo(&bytes, &item.mime)?;
        let (parts, apart_on_card) = stems::separate(&dll, &separator, &song, threads, &[0, 1, 2, 3], card, ctx.cancel)?;
        stems::unload();
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        // The drums apart: they have no key, and stay as they were when the
        // rest of the band moves into the singer's.
        let drums = parts[0].clone();
        let tonal = stems::Stereo { rate: stems::RATE, ch: [0, 1].map(|c| (0..song.len()).map(|i| parts[1].ch[c][i] + parts[2].ch[c][i]).collect()) };
        let sung_part = parts[3].mono();
        drop(parts);
        (ctx.progress)(&singing, 0.25);
        let mut conv = svc::Converter::new(&dll, &models, threads, card)?;
        let v = conv.voice(&reference)?;
        // The tune put where the person sings it, by their sung range; with
        // only reading recorded, by its middle (speech sits about four
        // semitones under the same person's singing).
        let person_range = match &all_singing {
            Some(s) => svc::range_of(&conv.pitch_track(s)?),
            None => None,
        };
        let tune = conv.pitch_track(&sung_part)?;
        let fit = match person_range {
            Some(person) => svc::fit_key(&tune, person),
            None => svc::Fit { voice: svc::octave_shift(conv.middle_pitch_of(&sung_part)?, if sung { v.pitch } else { v.pitch * 1.26 }), band: 0 },
        };
        let semitones = fit.voice;
        if let (Some(song), Some(person)) = (svc::range_of(&tune), person_range) {
            crate::applog::line("media", &format!("The song's tune {} to {}, their singing {} to {}: the voice moved {} semitones, the band {}", svc::note_name(song.0), svc::note_name(song.1), svc::note_name(person.0), svc::note_name(person.1), fit.voice, fit.band));
        }
        // Seed-VC's 30 steps on a card; on the processor each is ~6 s for
        // 30 s of song, so fewer.
        let steps = if conv.on_card { 30 } else { 10 };
        let new_voice = conv.convert(&v, &sung_part, svc::Settings { steps, semitones, ..Default::default() }, ctx.cancel, &|f| (ctx.progress)(&singing, 0.25 + 0.68 * f))?;
        let on_card = (apart_on_card, conv.on_card);
        drop(conv);
        if fit.band != 0 {
            (ctx.progress)("Putting the song in their key", 0.94);
        }
        let moved = transpose::shift(&tonal, fit.band);
        let band = stems::Stereo { rate: stems::RATE, ch: [0, 1].map(|c| drums.ch[c].iter().zip(&moved.ch[c]).map(|(a, b)| a + b).collect()) };
        crate::applog::line("media", &format!("Sang a song again in a recorded voice: {semitones} semitones, {steps} steps"));
        Ok((own_voice::mix(&band, &sung_part, &new_voice), on_card, started.elapsed().as_secs_f64(), fit.band))
    }))?;
    let on = |card: bool| if card { "graphics card" } else { "processor" };
    crate::applog::line("media", &format!("Sang a {:.0} s song in a recorded voice in {secs:.0} s: taken apart on the {}, sung on the {}", out.len() as f64 / out.rate as f64, on(on_card.0), on(on_card.1)));
    let wav = own_voice::wav_stereo(&out.ch[0], &out.ch[1], out.rate);
    let new = MediaItem {
        kind: "audio".into(),
        op: item.op.clone(),
        prompt: item.prompt.clone(),
        model_id: item.model_id.clone(),
        mime: "audio/wav".into(),
        seconds: out.len() as f64 / out.rate as f64,
        parent: Some(item.id.clone()),
        chat_id: ctx.req.chat_id.clone().or_else(|| item.chat_id.clone()),
        seed: item.seed,
        title: item.title.clone(),
        music: item.music.clone().map(|mut m| {
            m.key = m.key.map(|k| music::moved_key(&k, band_moved));
            m
        }),
        lyrics: item.lyrics.clone(),
        note: Some(match band_moved {
            0 => format!("Sung in the recorded voice “{}”", voice.name),
            b => format!("Sung in the recorded voice “{}”, {} semitone{} {} to suit it", voice.name, b.abs(), if b.abs() == 1 { "" } else { "s" }, if b < 0 { "lower" } else { "higher" }),
        }),
        ..Default::default()
    };
    store::save(&state.db.lock().unwrap(), &state.paths, &c, new, &wav, None)
}

/// Makes again the stretches of a song (`req.source`) whose words weren't
/// clear, one after another on the same song (the music model's repaint,
/// asked for a clear voice), then listens to the result. The steps between
/// are kept out of the gallery; the song it started from stays.
pub(super) async fn remake_job(ctx: &RunCtx<'_>, model: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let id = ctx.req.source.clone().ok_or("Pick a song to make again.")?;
    let c = ctx.state.work_cipher()?;
    let original = store::get(&ctx.state.db.lock().unwrap(), &c, &id).ok_or("That song is no longer in the gallery.")?;
    let review = original.review.clone().ok_or("Listen to the song first.")?;
    if review.unclear.is_empty() {
        return Err("Every part of this song was already clear.".into());
    }
    let mut current = original.clone();
    let spans: Vec<[f64; 2]> = review.unclear.iter().take(4).copied().collect();
    for (i, span) in spans.iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        let req = Request {
            op: "music".into(),
            model_id: Some(model.id.clone()),
            prompt: format!("{}, clear lead vocals upfront, crisp diction", original.prompt.trim().trim_end_matches('.')),
            lyrics: original.lyrics.clone(),
            seed: original.seed.map(|s| s + 1 + i as i64),
            music: Some(MusicSpec { task: Some("repaint".into()), audio: Some(current.id.clone()), region: Some([span[0] as f32, span[1] as f32]), ..Default::default() }),
            chat_id: original.chat_id.clone(),
            ..Default::default()
        };
        let label = format!("Making {} to {} again", clock(span[0]), clock(span[1]));
        let progress = |stage: &str, f: f64| (ctx.progress)(&format!("{label}: {}", stage.to_lowercase()), (i as f64 + f) / (spans.len() as f64 + 1.0));
        let step = RunCtx { state: ctx.state, req: &req, cancel: ctx.cancel, progress: &progress, reserved: ctx.reserved };
        let dir = work.join(format!("remake-{i}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let made = song(&step, model, &dir).await?.into_iter().next().ok_or("The music model didn't make the part again.")?;
        // A step between: out of the gallery.
        if current.id != original.id {
            current.hidden = true;
            store::update(&ctx.state.db.lock().unwrap(), &c, &current)?;
        }
        current = made;
    }
    (ctx.progress)("Listening to the new version", spans.len() as f64 / (spans.len() as f64 + 1.0));
    let after = hear(ctx.state, &current, ctx.cancel).await?;
    current.parent = Some(original.id.clone());
    current.prompt = original.prompt.clone();
    if after.score <= review.score {
        current.note = Some("This version wasn't clearer than the one it was made from, so both are kept.".into());
    }
    current.review = Some(after);
    store::update(&ctx.state.db.lock().unwrap(), &c, &current)?;
    crate::applog::line("media", &format!("Made {} unclear part(s) of a song again: score {} -> {}", spans.len(), review.score, current.review.as_ref().map_or(0, |r| r.score)));
    Ok(vec![current])
}

pub(super) fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

pub(super) async fn narrate(ctx: &RunCtx<'_>) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let text = crate::tts::speakable(req.prompt.trim());
    if text.trim().is_empty() {
        return Err("Type the text to read aloud.".into());
    }
    let (voice, language) = (req.voice.clone(), req.language.clone().filter(|l| l != "auto"));
    let (floats, rate) = if let Some(own) = recorded_voice(req) {
        own_voice::speak(ctx.state, own, &text, language.as_deref(), ctx.cancel, ctx.progress).await?
    } else {
        (ctx.progress)("Reading", 0.3);
        let speech = tokio::task::spawn_blocking(move || crate::tts::synthesize(&text, voice.as_deref(), language.as_deref(), 1.0))
            .await
            .map_err(|e| e.to_string())??;
        (speech.samples.iter().map(|s| *s as f32 / 32768.0).collect(), speech.rate)
    };
    let wav = crate::audio::wav_encode(&floats, rate);
    let c = ctx.state.work_cipher()?;
    let item = MediaItem {
        kind: "audio".into(),
        op: "narrate".into(),
        prompt: req.prompt.trim().to_string(),
        mime: "audio/wav".into(),
        seconds: floats.len() as f64 / rate.max(1) as f64,
        chat_id: req.chat_id.clone(),
        ..Default::default()
    };
    Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &wav, None)?])
}

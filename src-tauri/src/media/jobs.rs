// SPDX-License-Identifier: AGPL-3.0-only
//! Jobs in line and running: what each needs, where it runs (the card, the
//! processor, the compute PC or the cloud), its progress, and its engine
//! arguments fitted to the card.

use super::*;

/// What a job does, for the question when the card can't take it.
pub(super) fn op_what(op: &str) -> &'static str {
    match op {
        "generate" | "edit" | "fill" | "extend" | "restyle" | "cover_art" => "Making the picture",
        "upscale" => "Making the picture bigger",
        "video" => "Making the video",
        "speak" | "talk" => "Making the picture talk",
        "music" | "remake" | "studio_part" => "Making the song",
        "sound" => "Making the sound",
        "map" | "stems" | "dry" => "Taking the song apart",
        "revoice" => "Singing the song in another voice",
        "tune" | "coach" => "Working on the tune",
        "train" => "Training a custom model",
        "train_picture" => "Training a picture LoRA",
        "video_edit" => "Editing the video",
        "video_sound" => "Making sound for the video",
        "dub" => "Dubbing the video",
        "captions" => "Adding captions",
        "trainer" => "Getting the trainer",
        _ => "This job",
    }
}


pub(super) fn emit(state: &AppState, event: &str, payload: serde_json::Value) {
    if let Some(app) = state.app.get() {
        app.emit(event, payload).ok();
    }
}

pub(super) fn update(state: &AppState, id: &str, f: impl FnOnce(&mut JobInfo)) {
    let info = {
        let mut list = jobs().lock().unwrap();
        let Some(j) = list.iter_mut().find(|j| j.info.id == id) else { return };
        f(&mut j.info);
        j.info.clone()
    };
    emit(state, "media:progress", json!(info));
}

/// A job made on the compute PC: shown here as it goes, and what it makes
/// kept here.
pub(super) async fn on_compute(state: &Arc<AppState>, id: String, req: Request) -> Result<Vec<MediaItem>, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let pc = crate::sync::compute::peer_name(state).unwrap_or_else(|| "the compute PC".into());
    let info = JobInfo {
        id: id.clone(),
        op: req.op.clone(),
        prompt: req.prompt.chars().take(200).collect(),
        model_id: req.model_id.clone(),
        status: "running".into(),
        stage: format!("Sending it to {pc}"),
        fraction: 0.0,
        eta_secs: None,
        est_secs: 60.0,
        chat_id: req.chat_id.clone(),
        on: Some("compute PC".into()),
        device: Some(pc.clone()),
        gpu_usage: None,
        gpu_temp: None,
        cpu_usage: None,
        ram_usage: None,
        gpu_mem: None,
    };
    jobs().lock().unwrap().push(Job { info: info.clone(), cancel: cancel.clone() });
    crate::quiet::stir();
    emit(state, "media:progress", json!(info));
    struct Done(String);
    impl Drop for Done {
        fn drop(&mut self) {
            jobs().lock().unwrap().retain(|j| j.info.id != self.0);
        }
    }
    let _done = Done(id.clone());
    let progress = |stage: &str, fraction: f64, device: Option<String>| {
        update(state, &id, |i| {
            i.stage = stage.to_string();
            i.fraction = fraction.clamp(0.0, 1.0);
            if let Some(d) = &device {
                i.device = Some(format!("{d} on {pc}"));
            }
        });
    };
    let made = crate::sync::compute::media(state, &req, &cancel, &progress).await?;
    let c = state.work_cipher()?;
    let mut out = Vec::new();
    for (item, data, thumb) in made {
        let item = MediaItem { id: String::new(), chat_id: req.chat_id.clone(), parent: req.source.clone(), hidden: false, ..item };
        out.push(store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &data, thumb.as_deref())?);
    }
    state.log("media", &format!("Made {} on {pc}", out.len()));
    Ok(out)
}

/// Checks a request before it's queued, so mistakes show at once.
/// What a cloud model can do with a job.
pub(super) fn cloud_can(c: &crate::cloud::MediaChoice, op: &str) -> bool {
    use crate::cloud::MediaKind;
    let edits = c.id.contains(":gpt-image") || c.id.contains(":dall-e-2");
    match (c.kind, op) {
        (MediaKind::Image, "generate") => true,
        (MediaKind::Image, "edit" | "restyle" | "fill") => edits,
        (MediaKind::Video, "video") => true,
        (MediaKind::Speech, "narrate") => true,
        _ => false,
    }
}

/// The cloud model a job runs on: the one asked for, or one that can do it
/// when no model on this PC can (narration always has this PC's voices).
pub(super) fn cloud_pick(state: &AppState, req: &Request) -> Option<String> {
    if let Some(id) = req.model_id.as_deref().filter(|m| crate::cloud::is_cloud(m)) {
        return Some(id.to_string());
    }
    if req.op == "narrate" || choose(state, model_op(req), req.model_id.as_deref(), req.source.is_some()).is_ok() {
        return None;
    }
    crate::cloud::media_choices(&state.db.lock().unwrap()).into_iter().find(|c| cloud_can(c, &req.op)).map(|c| c.id)
}

/// The online model a job would go to, with its provider: (provider, model name).
pub(crate) fn cloud_target(state: &AppState, req: &Request) -> Option<(String, String)> {
    let id = cloud_pick(state, req)?;
    crate::cloud::media_choices(&state.db.lock().unwrap()).into_iter().find(|c| c.id == id).map(|c| (c.provider, c.name))
}

/// Any model, here or in the cloud, can make this kind of thing.
pub fn can_make(state: &AppState, kind: Kind) -> bool {
    use crate::cloud::MediaKind;
    has_kind(state, kind)
        || crate::cloud::media_choices(&state.db.lock().unwrap()).iter().any(|c| match kind {
            Kind::Image => c.kind == MediaKind::Image,
            Kind::Video => c.kind == MediaKind::Video,
            _ => false,
        })
}

/// The recorded voice (`own_voice`) a job uses: the one a song is sung again
/// in ("revoice", `voice`), narration's "own:<id>", or a new song's.
pub(super) fn recorded_voice(req: &Request) -> Option<&str> {
    match req.op.as_str() {
        "revoice" => req.voice.as_deref(),
        "narrate" => req.voice.as_deref().and_then(|v| v.strip_prefix("own:")),
        "music" => req.music.as_ref().and_then(|m| m.own_voice.as_deref()),
        "podcast" | "dialogue" | "explainer" => req.talk.as_ref().and_then(|t| t.speakers.iter().find_map(|s| s.voice.as_deref().and_then(|v| v.strip_prefix("own:")))),
        _ => None,
    }
}

/// Jobs that need no media model: speech, and work on songs already made.
pub(super) fn no_model(op: &str) -> bool {
    matches!(op, "narrate" | "podcast" | "dialogue" | "explainer" | "listen" | "map" | "revoice" | "stems" | "dry" | "song_video" | "train" | "trainer" | "train_picture" | "video_edit" | "video_sound" | "dub" | "captions" | "talk")
}

pub fn check(state: &AppState, req: &Request) -> Result<(), String> {
    let f = op_feature(&req.op).ok_or("Unknown kind of job.")?;
    crate::features::require(state, f)?;
    if crate::uncensor::running() {
        return Err(crate::uncensor::BUSY.into());
    }
    if let Some(id) = req.music.as_ref().and_then(|m| m.custom.as_deref()).filter(|c| !c.is_empty() && req.op == "music") {
        train::get(&state.db.lock().unwrap(), &state.work_cipher()?, id).ok_or("That custom model is no longer here.")?;
        if !installed(&state.db.lock().unwrap()).iter().any(|i| i.model_id == train::BASE_MODEL) {
            return Err("Custom models play on ACE-Step 1.5 Base: install it from Models first.".into());
        }
    }
    if req.op == "studio_part" && req.music.as_ref().and_then(|m| m.track.as_deref()).is_none_or(|t| !music::PARTS.contains(&t)) {
        return Err(format!("Pick the part to make ({}).", music::PARTS.join(", ").replace('_', " ")));
    }
    if req.op == "train" || req.op == "trainer" {
        let t = req.train.clone().unwrap_or_default();
        if req.op == "train" && t.name.trim().is_empty() {
            return Err("Give the custom model a name.".into());
        }
        if req.op == "train" && t.files.len() + t.songs.len() == 0 && t.voice.as_deref().is_none_or(str::is_empty) {
            return Err("Pick at least one of your songs, or a voice with singing (3 or more songs teach its sound, 10 or more teach it best).".into());
        }
        if state.catalog.media.trainer.is_none() || !cfg!(windows) {
            return Err("Training isn't available on this system yet.".into());
        }
        if !crate::natural::nvidia_here(state) {
            return Err("Training needs an NVIDIA graphics card (8 GB or more).".into());
        }
    }
    if req.op == "train_picture" {
        let p = req.picture_train.clone().unwrap_or_default();
        if p.name.trim().is_empty() {
            return Err("Give the LoRA a name.".into());
        }
        if !(picture_train::FEWEST..=picture_train::MOST).contains(&p.pictures.len()) {
            return Err(format!("Pick {} to {} photos (10 to 20 of the same person, pet, thing or style teach it best).", picture_train::FEWEST, picture_train::MOST));
        }
        if state.catalog.media.trainer.is_none() || !cfg!(windows) {
            return Err("Training isn't available on this system yet.".into());
        }
        if !crate::natural::nvidia_here(state) {
            return Err("Training needs an NVIDIA graphics card (12 GB or more for pictures).".into());
        }
    }
    if req.op == "talk" {
        if state.catalog.media.ditto.is_none() || !cfg!(windows) {
            return Err("Quick talking pictures aren't available on this system yet.".into());
        }
        if req.source.is_none() {
            return Err("Pick a picture to work on first.".into());
        }
    }
    if let Some(id) = cloud_pick(state, req) {
        let c = crate::cloud::media_choices(&state.db.lock().unwrap()).into_iter().find(|c| c.id == id);
        match c {
            Some(c) if !cloud_can(&c, &req.op) => return Err(format!("{} can't do that kind of job.", c.name)),
            None => return Err("That cloud model isn't available: it needs the Cloud level, the provider turned on and its key.".into()),
            _ => {}
        }
    } else if !no_model(&req.op) {
        choose(state, model_op(req), req.model_id.as_deref(), req.source.is_some())?;
    }
    // Podcasts and readings check their speakers and voices themselves.
    if matches!(req.op.as_str(), "podcast" | "dialogue") {
        return podcast::check(state, req);
    }
    if req.op == "explainer" {
        if !cfg!(windows) {
            return Err("Explainer videos aren't available on this system yet.".into());
        }
        if req.prompt.trim().is_empty() && req.talk.as_ref().is_none_or(|t| t.files.is_empty()) {
            return Err("Say what the video explains, or add files.".into());
        }
        if let Some(id) = recorded_voice(req) {
            let v = own_voice::get(&state.db.lock().unwrap(), &state.work_cipher()?, id).ok_or("That recorded voice is no longer here.")?;
            if !v.can_speak() {
                return Err(format!("Read at least three lines for {} in Studio › Your voice first.", v.name));
            }
        }
        return Ok(());
    }
    if matches!(req.op.as_str(), "listen" | "remake" | "map" | "revoice" | "cover_art" | "stems" | "dry" | "song_video" | "studio_part") && req.source.is_none() {
        return Err("Pick a song first.".into());
    }
    // A recorded voice: one that's here and has what the job needs.
    if let Some(id) = recorded_voice(req) {
        let v = own_voice::get(&state.db.lock().unwrap(), &state.work_cipher()?, id).ok_or("That recorded voice is no longer here.")?;
        if req.op == "narrate" && !v.can_speak() {
            return Err(format!("Read at least three lines for {} in Studio › Your voice first.", v.name));
        }
        if req.op != "narrate" && !v.can_sing() {
            return Err(format!("Record a little of {} in Studio › Your voice first.", v.name));
        }
    } else if req.op == "revoice" {
        return Err("Pick whose voice sings it.".into());
    }
    let needs_source = matches!(req.op.as_str(), "edit" | "fill" | "extend" | "restyle" | "upscale" | "remove_background");
    if needs_source && req.source.is_none() {
        return Err("Pick a picture to work on first.".into());
    }
    // Work on a song the person has: the song, and for some the part.
    let task = req.music.as_ref().and_then(|m| m.task.as_deref()).filter(|t| *t != "new");
    if req.op == "music" && task.is_some() {
        let m = req.music.as_ref().map(|m| m.clone()).unwrap_or_default();
        if m.audio.is_none() {
            return Err("Pick the song to work on first.".into());
        }
        if matches!(task, Some("add" | "extract")) && m.track.as_deref().is_none_or(|t| t.trim().is_empty()) {
            return Err("Pick the part (vocals, drums, guitar…).".into());
        }
        if let Some(t) = m.track.as_deref().map(|t| t.trim().to_lowercase().replace(' ', "_")).filter(|t| !t.is_empty()) {
            if !music::PARTS.contains(&t.as_str()) {
                return Err(format!("The music model knows these parts: {}.", music::PARTS.join(", ").replace('_', " ")));
            }
        }
        if !matches!(task, Some("cover" | "repaint" | "extend" | "add" | "extract" | "complete")) {
            return Err("Unknown kind of change to a song.".into());
        }
    }
    let described = req.music.as_ref().is_some_and(MusicSpec::describes);
    let needs_prompt = matches!(req.op.as_str(), "generate" | "edit" | "restyle" | "video" | "music" | "sound" | "narrate")
        && !(req.op == "music" && (described || matches!(task, Some("repaint" | "extend" | "add" | "extract" | "complete"))));
    if needs_prompt && req.prompt.trim().is_empty() && !(req.op == "video" && req.source.is_some()) {
        return Err(if req.op == "narrate" { "Type the text to read aloud." } else { "Describe what you'd like first." }.into());
    }
    if req.op == "fill" && req.mask.is_none() {
        return Err("Paint over the part to change first.".into());
    }
    if let Some(w) = &req.wallpaper {
        w.checked()?;
        if !matches!(req.op.as_str(), "generate" | "video") {
            return Err("Wallpapers are new pictures or clips.".into());
        }
    }
    Ok(())
}

/// Queues a job and runs it when it's its turn. Returns what it made.
pub async fn run(state: &Arc<AppState>, id: String, req: Request) -> Result<Vec<MediaItem>, String> {
    // Built on the heap: the job's future is large, and awaited on a 2 MB
    // worker thread (or a test's) it overflowed the stack.
    Box::pin(run_job_now(state, id, req)).await
}

pub(super) async fn run_job_now(state: &Arc<AppState>, id: String, req: Request) -> Result<Vec<MediaItem>, String> {
    if crate::sync::compute::media_here(state, &req) {
        return on_compute(state, id, req).await;
    }
    // A custom model plays on the model it was trained on.
    let req = match req.music.as_ref().and_then(|m| m.custom.as_deref()).filter(|c| !c.is_empty()) {
        Some(_) if req.op == "music" => Request { model_id: Some(train::BASE_MODEL.into()), ..req },
        _ => req,
    };
    check(state, &req)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let on_cloud = cloud_pick(state, &req);
    let model = if no_model(&req.op) || on_cloud.is_some() { None } else { choose(state, model_op(&req), req.model_id.as_deref(), req.source.is_some()).ok() };
    if let Some(m) = &model {
        fetch_added(state, m, &cancel).await?;
        files_here(state, m)?;
    }
    let est = match (&model, &on_cloud) {
        (Some(m), _) => estimate(state, m, &req),
        (None, Some(_)) if req.op == "video" => 180.0,
        _ if req.op == "train" => train_estimate(state, &req),
        _ if req.op == "trainer" => 900.0,
        _ if req.op == "talk" => {
            let processor = !crate::natural::nvidia_here(state);
            ditto::estimate(req.seconds.unwrap_or(15.0) as f64, state.catalog.media.ditto.as_ref().is_some_and(|d| ditto::ready(&state.paths, d, processor)), processor)
        }
        // Writing, then reading about as fast as it plays.
        _ if req.op == "podcast" => 60.0 + 50.0 * req.talk.as_ref().and_then(|t| t.minutes).unwrap_or(5.0) as f64,
        _ if req.op == "dialogue" => 10.0 + req.prompt.len() as f64 / 40.0,
        _ if req.op == "explainer" => 60.0 + 40.0 * req.talk.as_ref().and_then(|t| t.minutes).unwrap_or(3.0) as f64,
        (None, Some(_)) => 20.0,
        _ => 10.0,
    };
    {
        let info = JobInfo {
            id: id.clone(),
            op: req.op.clone(),
            prompt: req.prompt.chars().take(200).collect(),
            model_id: model.as_ref().map(|m| m.id.clone()).or_else(|| on_cloud.clone()),
            status: "queued".into(),
            stage: "Waiting for the job before it".into(),
            fraction: 0.0,
            eta_secs: None,
            est_secs: est,
            chat_id: req.chat_id.clone(),
            on: None,
            device: None,
            gpu_usage: None,
            gpu_temp: None,
            cpu_usage: None,
            ram_usage: None,
            gpu_mem: None,
        };
        jobs().lock().unwrap().push(Job { info: info.clone(), cancel: cancel.clone() });
        crate::quiet::stir();
        emit(state, "media:progress", json!(info));
    }
    struct Done(String);
    impl Drop for Done {
        fn drop(&mut self) {
            jobs().lock().unwrap().retain(|j| j.info.id != self.0);
        }
    }
    let _done = Done(id.clone());
    // Which card is which, so every media engine runs on the dedicated one
    // (after the job shows, as it can take a few seconds after a start).
    crate::engine::learn_devices(state).await;

    // Its turn for the graphics card (`lineup`): after what's ahead of it,
    // or now when it's part of a chat's reply. Taking it out of the line,
    // or Stop, ends the wait (a chat's Stop hung until the job before
    // finished). Cloud jobs don't use the card.
    let entry = crate::lineup::Entry::studio(&id, if req.prompt.trim().is_empty() { op_what(&req.op) } else { req.prompt.trim() }, op_what(&req.op), card_need(state, &req, model.as_ref()))
        .second_ok(matches!(req.op.as_str(), "map" | "stems" | "dry" | "revoice"))
        .processor_ok(on_the_processor_too(&req.op));
    if on_cloud.is_none() {
        crate::perf::heavy_allowed(state, "make pictures, videos or music on this PC")?;
    }
    let turn = match &on_cloud {
        Some(_) => None,
        None => match req.chat_id.as_deref().and_then(|c| crate::lineup::inside_reply(state, c, entry.clone())) {
            Some(t) => Some(t),
            None => Some(
                crate::lineup::wait(state, entry, cancel.clone(), |place| {
                    update(state, &id, |i| i.stage = if place <= 1 { "Next in line for the graphics card".into() } else { format!("Waiting in line for the graphics card ({place} in line)") });
                })
                .await?,
            ),
        },
    };
    let (second_card, forced_processor) = match turn.as_ref().map(|t| t.card) {
        Some(Some(crate::lineup::Card::Second)) => (true, false),
        Some(None) => (false, true),
        _ => (false, false),
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    let started = Instant::now();
    update(state, &id, |i| {
        i.status = "running".into();
        i.stage = "Starting".into();
    });
    // What makes it: the dedicated card for pictures and video (and music,
    // with the card's music engine), else the processor, or a cloud model.
    let card_engine = matches!(req.op.as_str(), "generate" | "edit" | "fill" | "extend" | "restyle" | "upscale" | "video" | "speak" | "talk" | "tune" | "coach" | "cover_art")
        || (matches!(req.op.as_str(), "music" | "sound" | "studio_part" | "video_sound") && music_engine_dir(state).is_some_and(|d| d.to_string_lossy().contains("vulkan")))
        // HeartMuLa only runs on a card.
        || (req.op == "music" && model.as_ref().is_some_and(|m| m.file("heartmula").is_some()))
        // A song's parts taken apart and read, or its voice sung again, on a
        // card (CUDA or DirectML).
        || (matches!(req.op.as_str(), "map" | "stems" | "dry" | "revoice" | "dub") && crate::natural::card_here(state))
        // Read in a recorded or made voice: its engine's card build.
        || (matches!(req.op.as_str(), "narrate" | "podcast" | "dialogue" | "explainer") && recorded_voice(&req).is_some());
    let (gpu_name, cpu_name) = {
        let hw = state.hardware.read().unwrap();
        (hw.best_gpu().map(|g| g.name.clone()), hw.cpu_name.clone())
    };
    let has_card = state.budget().vram > 0 && gpu_name.is_some();
    // The quick way to make a picture talk runs on NVIDIA cards only.
    let talk_off_card = req.op == "talk" && has_card && !crate::natural::nvidia_here(state);
    // Meant for the card, with none to use: said why, and asked first.
    if on_cloud.is_none() && card_engine && (!has_card || talk_off_card) && !forced_processor {
        let why = if talk_off_card {
            format!("the quick way runs on NVIDIA graphics cards, and this PC's is {}", gpu_name.clone().unwrap_or_default())
        } else if gpu_name.is_none() {
            crate::natural::no_card(state)
        } else {
            "SulcusAI's limit for the graphics card is set to none (Settings › This PC)".to_string()
        };
        let ask_progress = |stage: &str, f: f64| update(state, &id, |i| {
            i.stage = stage.to_string();
            i.fraction = f;
        });
        crate::cardfirst::processor_ok(state, Some(&id), op_what(&req.op), &why, &cancel, &ask_progress).await?;
    }
    let on_processor = Arc::new(AtomicBool::new(!(card_engine && has_card) || forced_processor || talk_off_card));
    let device_now = {
        let (on_processor, gpu_name, cpu_name, cloud) = (on_processor.clone(), gpu_name.clone(), cpu_name.clone(), on_cloud.clone());
        move || -> (String, Option<String>) {
            match &cloud {
                Some(c) => ("cloud".into(), Some(cloud_label(c))),
                None if on_processor.load(Ordering::Relaxed) => ("processor".into(), (!cpu_name.is_empty()).then(|| cpu_name.clone())),
                None => ("graphics card".into(), gpu_name.clone()),
            }
        }
    };
    {
        let (on, device) = device_now();
        update(state, &id, |i| {
            i.on = Some(on);
            i.device = device;
        });
    }
    let progress = |stage: &str, fraction: f64| {
        if stage == PROCESSOR_STAGE {
            on_processor.store(true, Ordering::Relaxed);
        }
        let elapsed = started.elapsed().as_secs_f64();
        let eta = if fraction > 0.12 { elapsed / fraction - elapsed } else { (est - elapsed).max(5.0) };
        update(state, &id, |i| {
            i.stage = stage.to_string();
            i.fraction = fraction.clamp(0.0, 1.0);
            i.eta_secs = Some(eta.max(0.0).round());
        });
    };
    // The card and processor, every 2 seconds while it works: shown live,
    // and the peak temperature and average use kept with what's made.
    let watching = Arc::new(AtomicBool::new(true));
    let readings: Arc<std::sync::Mutex<Vec<Reading>>> = Arc::default();
    if on_cloud.is_none() {
        let (st, id2, w, r, device_now) = (state.clone(), id.clone(), watching.clone(), readings.clone(), device_now.clone());
        tauri::async_runtime::spawn(async move {
            crate::perf::cpu_usage();
            while w.load(Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let g = if has_card { crate::perf::gpu_reading(&st).await } else { Default::default() };
                let cpu = tokio::task::spawn_blocking(crate::perf::cpu_usage).await.ok();
                let ram = {
                    let mut sys = sysinfo::System::new();
                    sys.refresh_memory();
                    (sys.total_memory() > 0).then(|| sys.used_memory() as f32 * 100.0 / sys.total_memory() as f32)
                };
                let gpu_mem = match (g.mem_used, g.mem_total) {
                    (Some(u), Some(t)) if t > 0 => Some(u as f32 * 100.0 / t as f32),
                    _ => None,
                };
                let (on, device) = device_now();
                r.lock().unwrap().push(Reading { gpu_usage: g.usage, gpu_temp: g.temp, cpu_usage: cpu, on_card: on == "graphics card" });
                update(&st, &id2, |i| {
                    i.gpu_usage = g.usage;
                    i.gpu_temp = g.temp;
                    i.cpu_usage = cpu;
                    i.ram_usage = ram;
                    i.gpu_mem = gpu_mem;
                    i.on = Some(on);
                    i.device = device;
                });
            }
        });
    }
    let result = match &on_cloud {
        Some(cid) => {
            let ctx = RunCtx { state, req: &req, cancel: &cancel, progress: &progress, reserved: 0 };
            match crate::cloud::media_target(state, cid, &state.work_cipher()?) {
                Ok(t) => cloud::run(&ctx, cid, &t).await,
                Err(e) => Err(e),
            }
        }
        // Stop reaches the helper models too (planning, checking, writing).
        // And Stop always stops: whatever the job is waiting on (lyrics from
        // a chat model, a download, an engine that hung) is let go, and the
        // engines it started end with it (a song stuck "making" ignored Stop).
        None => {
            let work = JOB_ID.scope(id.clone(), SECOND_CARD.scope(second_card, ON_PROCESSOR.scope(forced_processor, crate::vision::JOB_STOP.scope(cancel.clone(), Box::pin(run_job(state, &id, &req, model.as_ref(), &cancel, &progress))))));
            tokio::pin!(work);
            loop {
                tokio::select! {
                    r = &mut work => break r,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(300)) => {
                        if cancel.load(Ordering::Relaxed) {
                            // A moment to stop by itself first (it keeps what it can).
                            match tokio::time::timeout(std::time::Duration::from_secs(3), &mut work).await {
                                Ok(r) => break r,
                                Err(_) => {
                                    crate::applog::line("media", &format!("Stopped a job that didn't stop by itself ({})", req.op));
                                    break Err(download::CANCELLED.into());
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    if let (Ok(items), Some(m)) = (&result, &model) {
        if !items.is_empty() {
            learn_speed(state, m, &req, started.elapsed().as_secs_f64());
            crate::cleanup::touch_model(state, &m.id);
        }
    }
    // A saved artist's first new song becomes their voice for the next ones.
    let artist_of = req.music.as_ref().filter(|m| matches!(m.task.as_deref(), None | Some("new"))).and_then(|m| m.persona.clone());
    if let (Ok(items), Some(name), Ok(c)) = (&result, artist_of, state.work_cipher()) {
        if let Some(first) = items.first().filter(|i| i.kind == "audio") {
            let conn = state.db.lock().unwrap();
            if let Some(p) = persona::find(&conn, &c, &name) {
                if persona::first_voice(&conn, &c, &p.id, &first.id, first.seed, |id| store::get(&conn, &c, id).is_some()).unwrap_or(false) {
                    crate::applog::line("media", &format!("{}'s first song is their voice now", p.name));
                }
            }
        }
    }
    let name = match req.op.as_str() {
        "video" => "a video",
        "speak" | "talk" => "a talking picture",
        "music" => "a song",
        "sound" => "a sound",
        "narrate" => "a narration",
        "podcast" => "a podcast",
        "dialogue" => "a reading",
        "explainer" => "an explainer video",
        "video_edit" => "an edited video",
        "video_sound" => "a video with sound",
        "dub" => "a dubbed video",
        "captions" => "a captioned video",
        // A failed training was logged as "Making a picture failed".
        "train" | "trainer" => "a custom model",
        "train_picture" => "a picture LoRA",
        "revoice" => "a song in another voice",
        "song_video" => "a song video",
        "stems" | "dry" | "studio_part" => "a song part",
        "map" | "listen" => "a song's notes",
        _ => "a picture",
    };
    watching.store(false, Ordering::Relaxed);
    let took = started.elapsed().as_secs_f64();
    let with = model.as_ref().map(|m| m.id.clone()).or_else(|| on_cloud.clone()).unwrap_or_else(|| if req.op == "video_sound" { "the music model".into() } else if matches!(req.op.as_str(), "video_edit" | "dub" | "captions") { "FFmpeg".into() } else { "the built-in voice".into() });
    // What making it took, kept with each item (and shown under it).
    let result = result.map(|mut items| {
        let r = readings.lock().unwrap();
        let picture_or_video = matches!(req.op.as_str(), "generate" | "edit" | "fill" | "extend" | "restyle" | "video");
        let steps = picture_or_video.then(|| req.steps.or(model.as_ref().map(|m| m.defaults.steps)).unwrap_or(0)).filter(|s| *s > 0);
        let (on, device) = device_now();
        let on_card = on == "graphics card";
        // The readings from while the one that made it was working.
        let mine: Vec<&Reading> = r.iter().filter(|x| x.on_card == on_card).collect();
        let avg = |v: Vec<f32>| (!v.is_empty()).then(|| (v.iter().sum::<f32>() / v.len() as f32).round());
        let peak = |v: Vec<f32>| v.into_iter().reduce(f32::max).map(f32::round);
        let gpu = |f: fn(&Reading) -> Option<f32>| if on_card { mine.iter().filter_map(|x| f(x)).collect() } else { Vec::new() };
        let cpu: Vec<f32> = if on == "processor" { mine.iter().filter_map(|x| x.cpu_usage).collect() } else { Vec::new() };
        let stats = store::MadeStats {
            secs: (took * 10.0).round() / 10.0,
            on,
            device,
            steps,
            prompt_tokens: (!req.prompt.trim().is_empty()).then(|| approx_tokens(&req.prompt)),
            audio_tokens: None,
            frames: None,
            gpu_peak_temp: peak(gpu(|x| x.gpu_temp)),
            gpu_avg_usage: avg(gpu(|x| x.gpu_usage)),
            gpu_peak_usage: peak(gpu(|x| x.gpu_usage)),
            cpu_avg_usage: avg(cpu.clone()),
            cpu_peak_usage: peak(cpu),
        };
        if let Ok(c) = state.work_cipher() {
            let conn = state.db.lock().unwrap();
            for item in items.iter_mut() {
                let mut s = stats.clone();
                if item.kind == "audio" && matches!(req.op.as_str(), "music" | "sound") {
                    s.audio_tokens = Some((item.seconds * 5.0).round() as u32);
                }
                if item.kind == "video" {
                    s.frames = Some((item.seconds * model.as_ref().map_or(24, |m| m.defaults.fps.max(1)) as f64).round() as u32);
                }
                item.stats = Some(s);
                let _ = store::update(&conn, &c, item);
            }
        }
        items
    });
    match &result {
        Ok(items) => {
            let how = items.first().and_then(|i| i.stats.as_ref()).map(|s| {
                let mut out = format!(" on the {}", s.on);
                if let Some(d) = &s.device {
                    out += &format!(" ({d})");
                }
                if let Some(t) = s.gpu_peak_temp {
                    out += &format!(", card up to {t:.0} °C");
                }
                if let Some(u) = s.gpu_avg_usage.or(s.cpu_avg_usage) {
                    out += &format!(", {u:.0}% busy on average");
                }
                out
            });
            crate::applog::line("media", &format!("{} with {with}: done in {took:.0} s{}", req.op, how.unwrap_or_default()))
        }
        Err(e) if e == download::CANCELLED => crate::applog::line("media", &format!("{} with {with}: stopped after {took:.0} s", req.op)),
        Err(e) => {
            crate::applog::line("media", &format!("{} with {with}: failed after {took:.0} s: {}", req.op, e.lines().take(6).collect::<Vec<_>>().join(" | ")));
            crate::devmode::soon(state);
        }
    }
    match &result {
        Ok(_) if on_cloud.is_some() => state.log("media", &format!("Made {name} with a cloud model")),
        Ok(_) => state.log("media", &format!("Made {name} on this PC")),
        Err(e) if e != download::CANCELLED => state.log("media", &format!("Making {name} failed: {}", e.lines().next().unwrap_or(""))),
        Err(_) => {}
    }
    result
}

/// How many "default jobs" this request is, for timing estimates.
pub(super) fn job_scale(m: &MediaModel, req: &Request) -> f64 {
    match m.kind {
        Kind::Video => (req.seconds.unwrap_or(m.defaults.seconds).max(1.0) / m.defaults.seconds.max(1.0)) as f64,
        Kind::Music => (req.seconds.unwrap_or(m.defaults.seconds).max(10.0) / m.defaults.seconds.max(10.0)) as f64 * req.count.unwrap_or(1).clamp(1, 3) as f64,
        Kind::Image => req.count.unwrap_or(1).clamp(1, 4) as f64,
        _ => 1.0,
    }
}

pub(super) fn estimate(state: &AppState, m: &MediaModel, req: &Request) -> f64 {
    let hw = state.hardware.read().unwrap().clone();
    let record = installed(&state.db.lock().unwrap()).into_iter().find(|i| i.model_id == m.id);
    if req.op == "edit" {
        if let Some(s) = record.as_ref().and_then(|i| i.edit_secs) {
            return s.max(3.0);
        }
    }
    let measured = record.and_then(|i| i.secs);
    let f = fit(m, &hw, &state.budget(), 0, measured);
    if req.op == "tune" {
        // About 27 practice pictures at 768 pixels, plus writing and checking them.
        return f.est_secs * 27.0 * 0.6 + 300.0;
    }
    if req.op == "coach" {
        // About 110 practice pictures and edit steps at 768 pixels, the
        // checking, and a training run.
        return f.est_secs * 110.0 * 0.6 + 1500.0;
    }
    // A checked edit not timed yet: about three plain pictures (plan, make, check).
    let edit = if req.op == "edit" { 3.0 } else { 1.0 };
    (f.est_secs * job_scale(m, req) * edit).max(3.0)
}

/// Keeps a running average of how long a default job takes here.
pub(super) fn learn_speed(state: &AppState, m: &MediaModel, req: &Request, secs: f64) {
    let per = secs / job_scale(m, req).max(0.1);
    let conn = state.db.lock().unwrap();
    let mut list = installed(&conn);
    if let Some(i) = list.iter_mut().find(|i| i.model_id == m.id) {
        let slot = if req.op == "edit" { &mut i.edit_secs } else { &mut i.secs };
        *slot = Some(match *slot {
            Some(old) => (old * 0.6 + per * 0.4).round(),
            None => per.round(),
        });
        let _ = save_installed(&conn, &list);
    }
}

pub fn cancel(id: &str) {
    if let Some(j) = jobs().lock().unwrap().iter().find(|j| j.info.id == id) {
        j.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn list_jobs() -> Vec<JobInfo> {
    jobs().lock().unwrap().iter().map(|j| j.info.clone()).collect()
}

/// A scratch folder for one job's files, removed when it's dropped.
pub(super) struct Work(pub(super) PathBuf);

impl Drop for Work {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// At startup: work folders a crash left behind (nothing is being made yet).
pub fn clean_work(paths: &Paths) {
    if let Ok(entries) = std::fs::read_dir(store::dir(paths).join("work")) {
        for e in entries.flatten() {
            std::fs::remove_dir_all(e.path()).ok();
        }
    }
    // Half-written gallery files a crash left (written as .part, then named).
    if let Ok(entries) = std::fs::read_dir(store::dir(paths)) {
        for e in entries.flatten() {
            if e.path().extension().is_some_and(|x| x == "part") {
                std::fs::remove_file(e.path()).ok();
            }
        }
    }
}

pub(super) fn work_dir(paths: &Paths, id: &str) -> Result<Work, String> {
    let dir = store::dir(paths).join("work").join(id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(Work(dir))
}

pub(super) fn random_seed() -> i64 {
    (uuid::Uuid::new_v4().as_u128() & 0x7fff_ffff) as i64
}

pub(super) struct RunCtx<'a> {
    pub(super) state: &'a Arc<AppState>,
    pub(super) req: &'a Request,
    pub(super) cancel: &'a AtomicBool,
    pub(super) progress: &'a (dyn Fn(&str, f64) + Sync),
    /// Graphics memory the loaded chat model holds.
    pub(super) reserved: u64,
}

/// Graphics memory the loaded chat model holds, estimated the way the
/// catalog plans it. The engines can't see it reliably themselves: they
/// read the card as nearly empty and run out partway through.
pub(crate) async fn chat_model_vram(state: &Arc<AppState>) -> u64 {
    let (st, sees) = {
        let mut e = state.engine.lock().await;
        (e.status(), e.sees())
    };
    let (Some(id), Some(quant), Some(ctx), Some(layers)) = (st.model_id, st.quant, st.ctx, st.gpu_layers) else { return 0 };
    let Some(spec) = state.model_spec(&id) else { return 0 };
    let Some(v) = spec.variant(&quant) else { return 0 };
    let share = (layers as f64 / (spec.arch.n_layer + 1) as f64).min(1.0);
    let kv = crate::catalog::kv_cache_bytes(&spec.arch, ctx);
    let encoder = if sees { spec.vision.as_ref().map_or(0, |v| v.size) } else { 0 };
    ((v.size + kv) as f64 * share) as u64 + encoder + 700 * 1024 * 1024
}

/// The graphics card ran short or its driver failed (as opposed to, say, a
/// broken model file, which another try wouldn't fix).
pub(super) fn card_trouble(e: &str) -> bool {
    let l = e.to_lowercase();
    ["make enough memory", "compute failed", "out of memory", "vulkan", "device lost", "vk_error", "ggml_backend"].iter().any(|w| l.contains(w))
}

/// One look at the card and processor while a job runs.
pub(super) struct Reading {
    pub(super) gpu_usage: Option<f32>,
    pub(super) gpu_temp: Option<f32>,
    pub(super) cpu_usage: Option<f32>,
    /// The card was making it then (not the processor).
    pub(super) on_card: bool,
}

/// "gpt-image-1 (openai)" from "cloud:openai:gpt-image-1".
pub(super) fn cloud_label(id: &str) -> String {
    let mut parts = id.splitn(3, ':').skip(1);
    match (parts.next(), parts.next()) {
        (Some(provider), Some(model)) => format!("{model} ({provider})"),
        _ => id.to_string(),
    }
}

/// What progress says when a picture moved to the processor.
pub const PROCESSOR_STAGE: &str = "The graphics card couldn't make it, so it's being made on the processor (slower)";

/// About how many tokens a description is for a model's text reader (words
/// and punctuation, the way most tokenizers split English).
pub(super) fn approx_tokens(text: &str) -> u32 {
    let words = text.split_whitespace().count() as f64;
    let marks = text.chars().filter(|c| c.is_ascii_punctuation()).count() as f64;
    (words * 1.3 + marks * 0.5).round().max(1.0) as u32
}

/// The value after a flag, if it's there.
pub(super) fn flag_value<'a>(args: &'a [std::ffi::OsString], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|v| v.to_str())
}

/// `flag value` set (replaced, or put before the output path, which stays last).
pub(super) fn with_flag(args: &[std::ffi::OsString], flag: &str, value: Option<&str>) -> Vec<std::ffi::OsString> {
    let mut out = args.to_vec();
    if let Some(i) = out.iter().position(|a| a == flag) {
        match value {
            Some(v) if i + 1 < out.len() => out[i + 1] = v.into(),
            _ => {}
        }
        return out;
    }
    let at = out.iter().position(|a| a == "-o").unwrap_or(out.len());
    let mut add: Vec<std::ffi::OsString> = vec![flag.into()];
    if let Some(v) = value {
        add.push(v.into());
    }
    out.splice(at..at, add);
    out
}

/// A job planned to stay on the graphics card: 1 GiB of it kept free for
/// spikes and other programs (unless a budget is set already), and the
/// picture decoded in tiles on cards under 12 GB or for big pictures.
pub(super) fn card_friendly(args: &[std::ffi::OsString], vram_gib: f64) -> Vec<std::ffi::OsString> {
    let mut out = if flag_value(args, "--max-vram").is_some() || streamed_only(args) { args.to_vec() } else { with_flag(args, "--max-vram", Some("-1")) };
    let pixels = flag_value(args, "-W").and_then(|w| w.parse::<f64>().ok()).unwrap_or(0.0) * flag_value(args, "-H").and_then(|h| h.parse::<f64>().ok()).unwrap_or(0.0);
    if vram_gib < 12.0 || pixels > 1.2e6 {
        out = with_flag(&out, "--vae-tiling", None);
    }
    // Video: the weights wait in memory and go to the card only while
    // they're used. Kept on the card, Wan's 7 GB left its frame decoder
    // 2.6 GB of a 12 GB card, and decoding in tiny tiles took 19 minutes.
    if vram_gib < 20.0 && args.iter().any(|a| a == "vid_gen") {
        out = with_flag(&out, "--offload-to-cpu", None);
    }
    out
}

/// Wan 2.2 S2V (a clip moved by a voice): its 14B weights only stream to the
/// card. The engine's own splitting on top of that kept each part's output
/// on the card too, and a 12 GB card ran out partway through a 5 s clip.
pub(super) fn streamed_only(args: &[std::ffi::OsString]) -> bool {
    args.iter().any(|a| a == "--audio")
}

/// After the card failed: more of it left free (or half the budget), the
/// text reader on the processor, and no fast attention. With the card
/// `pinned` (the only one the engine sees), the rest is named for it, since
/// the engine's own placement otherwise may move it to integrated graphics.
pub(super) fn lighter_on_card(args: &[std::ffi::OsString], pinned: bool) -> Vec<std::ffi::OsString> {
    let budget = flag_value(args, "--max-vram").and_then(|v| v.parse::<f64>().ok()).unwrap_or(-1.0);
    let lighter = if budget > 0.0 { format!("{:.1}", (budget / 2.0).max(1.0)) } else { "-3".to_string() };
    let capped = if streamed_only(args) { args.to_vec() } else { with_flag(args, "--max-vram", Some(&lighter)) };
    // LTX keeps fast attention: without it its frames came out white.
    let ltx = args.iter().any(|a| a == "--audio-vae");
    let mut out: Vec<std::ffi::OsString> = capped.into_iter().filter(|a| ltx || a != "--diffusion-fa").collect();
    out = with_flag(&out, "--vae-tiling", None);
    if pinned && flag_value(&out, "--backend").is_none() {
        out = with_flag(&out, "--backend", Some("diffusion=Vulkan0,te=cpu,vae=Vulkan0"));
    }
    out
}

/// The argument list with the picture's size brought down to `side` pixels
/// on its longer edge (multiples of 16, as the engine needs).
pub(super) fn smaller(args: &[std::ffi::OsString], side: u32) -> Vec<std::ffi::OsString> {
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|v| v.to_str()?.parse::<u32>().ok());
    let (Some(w), Some(h)) = (value("-W"), value("-H")) else { return args.to_vec() };
    if w.max(h) <= side {
        return args.to_vec();
    }
    let scale = side as f64 / w.max(h) as f64;
    let fit = |v: u32| (((v as f64 * scale) / 16.0).round() as u32 * 16).max(256);
    let (nw, nh) = (fit(w), fit(h));
    let mut out = args.to_vec();
    for (flag, v) in [("-W", nw), ("-H", nh)] {
        if let Some(i) = out.iter().position(|a| a == flag) {
            out[i + 1] = v.to_string().into();
        }
    }
    out
}

/// The argument list without a graphics-memory budget (for the processor build).
pub(super) fn without_vram(args: &[std::ffi::OsString]) -> Vec<std::ffi::OsString> {
    let mut out = Vec::with_capacity(args.len());
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if a == "--max-vram" {
            skip = true;
        } else {
            out.push(a.clone());
        }
    }
    out
}


impl RunCtx<'_> {
    pub(super) fn limits(&self) -> sd::Limits {
        let l = self.state.limits();
        let b = self.state.budget();
        let vram = l.vram_bytes.min(b.vram_card).min(b.vram_card.saturating_sub(self.reserved));
        sd::Limits {
            threads: l.threads,
            // Only cap it when the performance mode or a loaded chat model leaves less than the card has.
            max_vram_gib: (b.vram_card > 0 && vram < b.vram_card).then(|| vram as f64 / GIB as f64),
        }
    }

    pub(super) fn files(&self, m: &MediaModel) -> Vec<(String, PathBuf)> {
        m.files.iter().map(|f| (f.role.clone(), file_path(&self.state.paths, m, f))).collect()
    }

    pub(super) async fn run_sd(&self, args: Vec<std::ffi::OsString>, work: &std::path::Path) -> Result<(), String> {
        let vram_gib = self.state.budget().vram_card as f64 / GIB as f64;
        let on_card = vram_gib > 0.0;
        // Started now on the processor while the card is busy: its build, smaller.
        if on_card && ON_PROCESSOR.try_with(|p| *p).unwrap_or(false) {
            let cpu = match SD_ENGINE.try_with(Clone::clone) {
                Ok(own) => own,
                Err(_) => self.cpu_engine().await?,
            };
            return self.run_sd_once(smaller(&without_vram(&args), 640), work, Some(cpu)).await;
        }
        if !on_card {
            return self.run_sd_once(args, work, None).await;
        }
        // Kept on the dedicated card: on a PC with integrated graphics too, the
        // engine would otherwise use whichever card it lists first.
        let pinned = crate::engine::learn_devices(self.state).await;
        // Planned to stay on the card: headroom it keeps free, and the
        // picture decoded in tiles where the last step's burst could overflow it.
        let mut first = card_friendly(&args, vram_gib);
        // LTX-2.3 (22B, with Gemma 3 12B reading the prompt) on a card under
        // 20 GB starts the lighter way: on a 12 GB card the first way ran out
        // partway after 5 minutes, and the lighter one made 5 s in 526 s.
        if vram_gib < 20.0 && args.iter().any(|a| a == "--audio-vae") {
            first = lighter_on_card(&first, pinned);
        }
        let mut last = match self.run_sd_once(first.clone(), work, None).await {
            Err(e) if card_trouble(&e) => e,
            other => return other,
        };
        // Still on the card, lighter each time: more headroom, the text reader
        // on the processor, no fast attention (some drivers fail with it),
        // then a smaller picture.
        let lighter = lighter_on_card(&first, pinned);
        let tries = [(lighter.clone(), "with more of its memory left free"), (smaller(&lighter, 768), "smaller")];
        for (try_args, how) in tries {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(download::CANCELLED.into());
            }
            crate::applog::line("media", &format!("The graphics card failed ({}); trying again on it {how}", last.lines().take(3).collect::<Vec<_>>().join(" | ")));
            (self.progress)("The graphics card ran short, so it's trying again with less of its memory", 0.0);
            match self.run_sd_once(try_args, work, None).await {
                Err(e) if card_trouble(&e) => last = e,
                other => return other,
            }
        }
        // The card failed three times: as a last resort, a small picture on
        // the processor (slow, but it doesn't depend on the card). Not videos,
        // which would take hours there.
        if self.cancel.load(Ordering::Relaxed) || !args.iter().any(|a| a == "img_gen") {
            return Err(last);
        }
        let why = format!("the graphics card failed three times ({})", last.lines().next().unwrap_or("").trim());
        let job = JOB_ID.try_with(|j| j.clone()).ok();
        if crate::cardfirst::processor_ok(self.state, job.as_deref(), "Making the picture", &why, self.cancel, self.progress).await? == crate::cardfirst::Go::Again {
            // The card was freed: once more on it before the processor.
            match self.run_sd_once(lighter.clone(), work, None).await {
                Err(e) if card_trouble(&e) => last = e,
                other => return other,
            }
        }
        crate::applog::line("media", &format!("The graphics card failed three times ({}); making the picture on the processor, smaller", last.lines().take(3).collect::<Vec<_>>().join(" | ")));
        (self.progress)(PROCESSOR_STAGE, 0.0);
        let cpu = self.cpu_engine().await.map_err(|why| format!("{last}\n\nCouldn't switch to the processor: {why}"))?;
        // At full size the processor takes many minutes; 640 pixels keeps it to a few.
        self.run_sd_once(smaller(&without_vram(&args), 640), work, Some(cpu)).await.map_err(|on_cpu| format!("{last}\n\nOn the processor too: {on_cpu}"))
    }

    /// The processor build of the picture engine, downloaded the first time it's needed (about 18 MB).
    pub(super) async fn cpu_engine(&self) -> Result<PathBuf, String> {
        let c = &self.state.catalog.media;
        let (key, asset) = [engine::cpu_key(), "cpu-x64", "linux-cpu-x64"]
            .into_iter()
            .find_map(|k| c.engine.assets.get(k).map(|a| (k, a)))
            .ok_or("There's no processor build of the picture engine for this system.")?;
        let dir = sd_dir(&c.engine, key);
        if let Some(exe) = engine::find_exe(&self.state.paths.engines.join(&dir), SD_EXE) {
            return Ok(exe);
        }
        self.state.log("network", &format!("Downloading the stable-diffusion.cpp engine ({key}) from {}", crate::host_of(&asset.url)));
        engine::ensure_unpacked(&self.state.paths, &dir, asset, SD_EXE, self.cancel, |_, _| {}).await
    }

    pub(super) async fn run_sd_once(&self, args: Vec<std::ffi::OsString>, work: &std::path::Path, exe: Option<PathBuf>) -> Result<(), String> {
        let exe = match exe {
            Some(e) => e,
            None => match SD_ENGINE.try_with(Clone::clone) {
                Ok(own) => own,
                Err(_) => sd_exe(self.state)?,
            },
        };
        let mut t = sd::Tracker::default();
        (self.progress)(t.stage.label(), t.fraction());
        proc::run(
            proc::Run {
                exe: &exe,
                args,
                cwd: work,
                log: &self.state.paths.logs.join("media.log"),
                low_priority: self.state.limits().low_priority,
                job: self.state.job(),
            },
            self.cancel,
            |l| {
                if t.line(l) {
                    (self.progress)(t.stage.label(), t.fraction());
                }
            },
        )
        .await
    }

    /// A practice picture for prompt tuning: square, 768 pixels, not kept.
    pub(super) async fn render(&self, m: &MediaModel, prompt: &str, negative: &str, seed: i64, dir: &std::path::Path) -> Result<image::DynamicImage, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let d = &m.defaults;
        let side = if d.align > 16 { (768 / d.align) * d.align } else { 768 };
        let out = dir.join("out_%d.png");
        let args = sd::picture_args(
            &self.files(m),
            d,
            &sd::Picture {
                prompt,
                negative: Some(negative).filter(|n| !n.trim().is_empty()).or((!d.negative.is_empty()).then_some(d.negative.as_str())),
                width: side,
                height: side,
                steps: d.steps.clamp(1, 60),
                seed,
                count: 1,
                init: None,
                mask: None,
                refs: vec![],
                strength: None,
                out: &out,
            },
            &self.limits(),
        );
        self.run_sd(args, dir).await?;
        let made = std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("out_")));
        imaging::decode(&std::fs::read(made.ok_or("The picture engine made nothing.")?).map_err(|e| e.to_string())?)
    }

    /// The request's example pictures (at most four).
    pub(super) fn examples(&self) -> Result<Vec<image::DynamicImage>, String> {
        if self.req.examples.is_empty() {
            return Ok(Vec::new());
        }
        let c = self.state.work_cipher()?;
        Ok(self
            .req
            .examples
            .iter()
            .take(4)
            .filter_map(|id| store::read(&self.state.paths, &c, id).ok())
            .filter_map(|b| imaging::decode(&b).ok())
            .collect())
    }

    pub(super) fn source(&self) -> Result<(MediaItem, image::DynamicImage), String> {
        let id = self.req.source.as_deref().ok_or("Pick a picture to work on first.")?;
        let c = self.state.work_cipher()?;
        let item = store::get(&self.state.db.lock().unwrap(), &c, id).ok_or("That picture is no longer in the gallery.")?;
        if item.kind != "image" {
            return Err("That isn't a picture.".into());
        }
        let bytes = store::read(&self.state.paths, &c, id)?;
        let img = imaging::decode(&bytes)?;
        // A cut-out's see-through parts are black underneath, and the models
        // read them as black: they get a plain white background instead.
        // The upscaler keeps the transparency itself.
        let img = if matches!(self.req.op.as_str(), "upscale" | "remove_background") { img } else { imaging::flatten(&img, [255, 255, 255]) };
        Ok((item, img))
    }

    /// For a picture made by editing another (and that by editing another…),
    /// the first one in the chain: what was made or attached before any edit.
    /// None when the picture isn't an edit, or its original is gone.
    pub(super) fn original_of(&self, item: &MediaItem) -> Result<Option<image::DynamicImage>, String> {
        let c = self.state.work_cipher()?;
        let edited = |i: &MediaItem| matches!(i.op.as_str(), "edit" | "restyle" | "fill" | "extend");
        if !edited(item) {
            return Ok(None);
        }
        let mut at = item.clone();
        for _ in 0..20 {
            let Some(parent) = at.parent.as_deref() else { break };
            let Some(p) = store::get(&self.state.db.lock().unwrap(), &c, parent) else { break };
            at = p;
            if !edited(&at) {
                break;
            }
        }
        if at.id == item.id || at.kind != "image" {
            return Ok(None);
        }
        Ok(store::read(&self.state.paths, &c, &at.id).ok().and_then(|b| imaging::decode(&b).ok()))
    }

    /// Saves a made picture into the gallery.
    pub(super) fn keep_picture(&self, img: &image::DynamicImage, m: Option<&MediaModel>, parent: Option<&str>, seed: Option<i64>) -> Result<MediaItem, String> {
        self.keep_as(&self.req.op, img, m, parent, seed)
    }

    /// A picture enlarged to cover `w`×`h`: with the upscaler when one is
    /// installed and it's much smaller, else smoothly resized.
    pub(super) async fn enlarge_for(&self, img: &image::DynamicImage, w: u32, h: u32, work: &std::path::Path) -> image::DynamicImage {
        let (iw, ih) = img.dimensions();
        let need = (w as f64 / iw as f64).max(h as f64 / ih as f64);
        if need <= 1.25 {
            return img.clone();
        }
        let Ok(up) = choose(self.state, "upscale", None, false) else { return img.clone() };
        (self.progress)("Enlarging it to your screen's size", 0.95);
        let (src, out) = (work.join("small.png"), work.join("big.png"));
        let Ok(png) = imaging::png(img) else { return img.clone() };
        if std::fs::write(&src, png).is_err() {
            return img.clone();
        }
        let args = sd::upscale_args(&self.files(&up), &src, &out, &self.limits());
        match self.run_sd(args, work).await {
            Ok(()) => std::fs::read(&out).ok().and_then(|b| imaging::decode(&b).ok()).unwrap_or_else(|| img.clone()),
            Err(e) => {
                crate::applog::line("media", &format!("The upscaler failed ({e}); resizing the wallpaper instead"));
                img.clone()
            }
        }
    }

    pub(super) fn keep_as(&self, op: &str, img: &image::DynamicImage, m: Option<&MediaModel>, parent: Option<&str>, seed: Option<i64>) -> Result<MediaItem, String> {
        let c = self.state.work_cipher()?;
        let png = imaging::png(img)?;
        let thumb = imaging::thumbnail(img)?;
        let (w, h) = img.dimensions();
        let item = MediaItem {
            kind: "image".into(),
            op: op.to_string(),
            prompt: self.req.asked.as_deref().unwrap_or(&self.req.prompt).trim().to_string(),
            model_id: m.map(|m| m.id.clone()),
            mime: "image/png".into(),
            width: w,
            height: h,
            parent: parent.map(str::to_string),
            chat_id: self.req.chat_id.clone(),
            seed,
            ..Default::default()
        };
        store::save(&self.state.db.lock().unwrap(), &self.state.paths, &c, item, &png, Some(&thumb))
    }
}

/// Unloads the chat model when nothing is using it, so a picture or video
/// gets the whole graphics card.
///
/// For a job a chat asked for, whose reply goes on afterwards: the chat model
/// steps aside only when the picture model fits the card alone but not beside
/// it (a 14B chat model took 9 of 12 GB, and pictures fell back to the
/// processor: minutes instead of seconds). It's paused, keeping its address,
/// and `give_back` brings it back for the rest of the reply.
pub(super) async fn make_room(state: &Arc<AppState>, req: &Request, model: Option<&MediaModel>) -> Option<crate::engine::Paused> {
    let vram = state.budget().vram_card;
    if vram == 0 {
        return None;
    }
    if req.chat_id.is_none() {
        if state.generations.lock().unwrap().is_empty() {
            let mut e = state.engine.lock().await;
            if e.status().model_id.is_some() {
                e.stop().await;
                emit(state, "engine:unloaded", json!({}));
            }
        }
        return None;
    }
    // Another chat replying on it too: it stays.
    if state.generations.lock().unwrap().len() > 1 {
        return None;
    }
    let need = (model?.min_vram_gb * GIB as f64) as u64;
    let reserved = chat_model_vram(state).await;
    if reserved == 0 || vram < need || vram.saturating_sub(reserved) >= need {
        return None;
    }
    let paused = state.engine.lock().await.pause().await?;
    crate::applog::line("media", &format!("Paused {} so the picture gets the graphics card", paused.model_id));
    emit(state, "engine:unloaded", json!({}));
    Some(paused)
}

/// Starts a model paused for a chat's picture again, at its old address, so
/// the chat's reply carries on.
pub(crate) async fn give_back(state: &Arc<AppState>, paused: crate::engine::Paused) {
    match crate::llm_endpoint(state, Some(paused.model_id.clone()), paused.vision, |_| {}).await {
        Ok(_) => crate::applog::line("media", &format!("Started {} again after the picture", paused.model_id)),
        Err(e) => crate::applog::line("media", &format!("Couldn't start {} again after the picture: {e}", paused.model_id)),
    }
}

pub(super) async fn run_job(state: &Arc<AppState>, id: &str, req: &Request, model: Option<&MediaModel>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<MediaItem>, String> {
    let music_on_gpu = matches!(req.op.as_str(), "music" | "sound" | "remake" | "studio_part") && (music_engine_dir(state).is_some_and(|d| d.to_string_lossy().contains("vulkan")) || model.is_some_and(|m| m.file("heartmula").is_some()));
    // A saved artist's sound and voice, under the song's own choices.
    let as_artist = with_persona(state, req)?;
    let req = as_artist.as_ref().unwrap_or(req);
    // An artist's style is read by the chat model too, before the lyrics.
    let styled = artist::resolve(state, req, progress).await;
    let req = styled.as_ref().unwrap_or(req);
    // Auto length: what's known before the lyrics are written.
    let timed = auto_seconds(req).map(|s| Request { seconds: Some(s), ..req.clone() });
    let req = timed.as_ref().unwrap_or(req);
    let sexed = with_own_voice_sex(state, req);
    let req = sexed.as_ref().unwrap_or(req);
    // Lyrics come from the chat model: written before the card is cleared for
    // the music engine, not after (that loaded the chat model again beside it).
    let mut with_lyrics;
    let req = if wants_written_lyrics(req) {
        progress("Writing lyrics", 0.02);
        let spec = req.music.clone().unwrap_or_default();
        // Auto with nothing to go on: the writer picks the sections, and the
        // song is as long as they need.
        let open = spec.auto_length && req.seconds.is_none() && matches!(spec.task.as_deref(), None | Some("new"));
        let seconds = if open { None } else { Some(req.seconds.unwrap_or(model.map_or(30.0, |m| m.defaults.seconds.max(30.0))).clamp(10.0, 600.0)) };
        with_lyrics = req.clone();
        let flow = spec.flow(&req.prompt);
        // Told the style, mood and voice too, not only the person's words.
        match lyrics::write(state, &song_caption(req), seconds, req.language.as_deref(), &spec.sections(), &spec.lyrics_guide(), flow).await {
            Ok(l) => {
                if open {
                    with_lyrics.seconds = Some(lyrics::length_of(&l, flow));
                }
                with_lyrics.lyrics = Some(l);
            }
            Err(why) => {
                // Nothing written here: the music model writes its own.
                crate::applog::line("media", &format!("Couldn't write lyrics before the song ({why:?}); the music model writes them"));
                with_lyrics.lyrics = None;
                with_lyrics.lyrics_problem = Some(why);
            }
        }
        &with_lyrics
    } else {
        req
    };
    // A podcast's script is written by the chat model, also before the card
    // is cleared for a recorded voice.
    let with_script;
    let req = if req.op == "podcast" && req.lyrics.is_none() {
        let script = podcast::write(state, req, cancel, progress).await?;
        with_script = Request { lyrics: Some(script), ..req.clone() };
        &with_script
    } else if req.op == "explainer" && req.lyrics.is_none() {
        let slides = explainer::write(state, req, cancel, progress).await?;
        with_script = Request { lyrics: Some(slides), ..req.clone() };
        &with_script
    } else {
        req
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    // Not for upscaling: its model is 64 MB, and stopping a 14 GB chat model
    // for it cost the next message a reload.
    let forced = ON_PROCESSOR.try_with(|p| *p).unwrap_or(false);
    let paused = if forced {
        None
    } else if music_on_gpu || recorded_voice(req).is_some() || matches!(req.op.as_str(), "generate" | "edit" | "fill" | "extend" | "restyle" | "video" | "tune" | "coach" | "cover_art") {
        make_room(state, req, model).await
    } else {
        None
    };
    let result = match model.filter(|m| m.kind != Kind::Upscale) {
        Some(m) if matches!(req.op.as_str(), "generate" | "video") => Box::pin(made_well(state, id, req, m, cancel, progress)).await,
        // Edits are planned and checked unless that's turned off, when something here reads pictures.
        Some(m) if req.op == "edit" && req.check != Some(false) && (crate::vision::can_read(state, None) || installed_file(state, m, "llm_vision").is_some()) => {
            Box::pin(edited_well(state, id, req, m, cancel, progress)).await
        }
        _ => Box::pin(run_kind(state, id, req, model, cancel, progress)).await,
    };
    if let Some(p) = paused {
        give_back(state, p).await;
    }
    result
}

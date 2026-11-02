// SPDX-License-Identifier: AGPL-3.0-only
//! Songs and sounds made by the music models.

use super::*;

/// The vocal language for the music model: the one asked for; with no
/// lyrics and none asked for, English (left to itself, the music model can
/// sing made-up words); else none, and it tells from the lyrics.
pub(super) fn vocal_language(asked: Option<&str>, words: &lyrics::Words) -> Option<String> {
    match (asked.filter(|l| !l.is_empty() && *l != "auto"), words) {
        (_, lyrics::Words::Instrumental) => None,
        (Some(l), _) => Some(l.to_string()),
        // Always named: the music model sings more clearly told the language.
        (None, lyrics::Words::Sung(l)) => Some(lyrics::guess_language(l).to_string()),
        (None, lyrics::Words::MusicModelWrites) => Some("en".into()),
    }
}

/// The request with its saved artist applied (`MusicSpec::persona`), or
/// None when it names none. An artist that's gone is said plainly.
pub(super) fn with_persona(state: &AppState, req: &Request) -> Result<Option<Request>, String> {
    let Some(name) = req.music.as_ref().and_then(|m| m.persona.as_deref()).map(str::trim).filter(|n| !n.is_empty()) else { return Ok(None) };
    let c = state.work_cipher()?;
    let conn = state.db.lock().unwrap();
    let p = persona::find(&conn, &c, name).ok_or_else(|| format!("There's no saved artist called {name}."))?;
    let mut out = req.clone();
    let spec = out.music.get_or_insert_with(Default::default);
    p.apply(spec, &mut out.language, |id| store::get(&conn, &c, id).is_some_and(|i| i.kind == "audio"));
    // Their first song's seed: the same start keeps the voice closer.
    out.seed = out.seed.or(p.seed);
    Ok(Some(out))
}

/// The length of a new song set to Auto, when it's known before any lyrics
/// are written: one named in the description ("a 90-second jingle"), what
/// the person's own lyrics need, what the chosen sections take, or a
/// minute and a half for an instrumental. None otherwise: written lyrics
/// decide it.
pub(super) fn auto_seconds(req: &Request) -> Option<f32> {
    let spec = req.music.as_ref().filter(|m| m.auto_length && matches!(m.task.as_deref(), None | Some("new")))?;
    if req.op != "music" || req.seconds.is_some() {
        return None;
    }
    let flow = spec.flow(&req.prompt);
    let own = req.lyrics.as_deref().filter(|l| lyrics::sung_lines(l) > 0).filter(|_| req.instrumental != Some(true));
    lyrics::named_length(&req.prompt)
        .or_else(|| own.map(|l| lyrics::length_of(l, flow)))
        .or_else(|| lyrics::short_piece(&req.prompt))
        .or_else(|| (!spec.sections().is_empty()).then(|| lyrics::length_of_sections(&spec.sections(), flow)))
        .or_else(|| (req.instrumental == Some(true)).then_some(90.0))
}

/// A new song in a recorded voice with no singer picked or described: sung
/// by a man or a woman as the recording is (left to itself the music model
/// often picked the other, and the voice was then changed across sexes).
pub(super) fn with_own_voice_sex(state: &AppState, req: &Request) -> Option<Request> {
    let spec = req.music.as_ref().filter(|m| req.op == "music" && matches!(m.task.as_deref(), None | Some("new")))?;
    let id = spec.own_voice.as_deref()?;
    if MusicSpec::some(&spec.voice).is_some() || names_a_singer(&req.prompt) {
        return None;
    }
    let c = state.work_cipher().ok()?;
    let voice = own_voice::get(&state.db.lock().unwrap(), &c, id)?;
    let sex = own_voice::sex_of(&state.paths, &c, &voice)?;
    crate::applog::line("media", &format!("A song in a recorded voice: sung by a {} singer, as the recording is", sex));
    let mut out = req.clone();
    out.music.as_mut()?.voice = Some(sex.into());
    Some(out)
}

/// Whether a description already says who sings ("a female singer", "a boy
/// band", "a duet").
pub(super) fn names_a_singer(prompt: &str) -> bool {
    const WHO: &[&str] = &["female", "male", "woman", "women", "man", "men", "girl", "girls", "boy", "boys", "duet", "choir", "baritone", "tenor", "alto", "soprano", "mezzo"];
    prompt.to_lowercase().split(|c: char| !c.is_alphabetic()).any(|w| WHO.contains(&w))
}

/// Whether a song job wants lyrics written before it starts: a new song
/// with vocals and none given, or vocals added to a song the person has.
pub(super) fn wants_written_lyrics(req: &Request) -> bool {
    let spec = req.music.as_ref();
    let task = spec.and_then(|m| m.task.as_deref()).filter(|t| *t != "new");
    let sings = match task {
        None => true,
        Some("add") => spec.and_then(|m| m.track.as_deref()).is_some_and(|t| t.contains("vocals")),
        _ => false,
    };
    req.op == "music" && sings && lyrics::words(false, req.instrumental == Some(true), req.lyrics.as_deref()) == lyrics::Words::MusicModelWrites
}

/// What the lyrics writer and the music model are told the song is.
pub(super) fn song_caption(req: &Request) -> String {
    let instrumental = req.instrumental == Some(true);
    match &req.music {
        Some(m) => m.caption(&req.prompt, instrumental),
        None => req.prompt.trim().to_string(),
    }
}

pub(super) async fn song(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let sound = req.op == "sound";
    let spec = req.music.clone().unwrap_or_default();
    let task = spec.task.as_deref().filter(|t| *t != "new" && !sound);
    let c = ctx.state.work_cipher()?;
    // A song from the gallery as a file the engine reads, with its length.
    let audio_file = |id: &str, name: &str| -> Result<(MediaItem, std::path::PathBuf, f64), String> {
        let item = store::get(&ctx.state.db.lock().unwrap(), &c, id).ok_or("That song is no longer in the gallery.")?;
        if item.kind != "audio" {
            return Err("That isn't a song or a sound.".into());
        }
        let bytes = store::read(&ctx.state.paths, &c, id)?;
        let dir = work.join(name);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let (path, secs) = audio_in::for_engine(&bytes, &item.mime, &dir)?;
        Ok((item.clone(), path, if secs > 0.0 { secs } else { item.seconds }))
    };
    let source = match (task, spec.audio.as_deref()) {
        (Some(_), Some(id)) => Some(audio_file(id, "source")?),
        (Some(_), None) => return Err("Pick the song to work on first.".into()),
        _ => None,
    };
    let reference = match spec.reference.as_deref() {
        Some(id) if !sound => Some(audio_file(id, "reference")?),
        _ => None,
    };
    // A new song in a recorded voice is made with no reference of their
    // singing: as the music model's reference it moved the sound a little
    // toward them but garbled the words, and the voice is changed to theirs
    // afterwards anyway.
    // A new song: as long as asked, up to ten minutes (sound effects 30 s).
    let seconds = if sound { req.seconds.unwrap_or(10.0).clamp(10.0, 30.0) } else { req.seconds.unwrap_or(m.defaults.seconds.max(30.0)).clamp(10.0, 600.0) };
    let source_len = source.as_ref().map_or(0.0, |s| s.2 as f32);
    let track = spec.track.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(|t| t.to_lowercase().replace(' ', "_"));
    let (engine_task, region) = match task {
        Some("cover") => ("cover", None),
        // The whole song when no part is given.
        Some("repaint") => ("repaint", Some(spec.region.map_or((0.0, -1.0), |r| (r[0].max(0.0), if r[1] > r[0] { r[1] } else { -1.0 })))),
        // More after its end: a part past the source's end is made new.
        Some("extend") => ("repaint", Some((source_len, source_len + spec.extend_by.unwrap_or(30.0).clamp(5.0, 300.0)))),
        Some("add") => ("lego", None),
        Some("extract") => ("extract", None),
        Some("complete") => ("complete", None),
        _ => ("text2music", None),
    };
    let caption_words = song_caption(req);
    let mut problem = req.lyrics_problem.clone();
    let sings = matches!(task, None) || (task == Some("add") && track.as_deref().is_some_and(|t| t.contains("vocals")));
    let mut words = if matches!(task, Some("extract" | "complete")) {
        lyrics::Words::Instrumental
    } else if sings {
        lyrics::words(sound, req.instrumental == Some(true), req.lyrics.as_deref())
    } else {
        // A cover, a part made again or more after the end sings the words
        // given, else the song's own (a cover of a song keeps its words).
        let own = source.as_ref().and_then(|s| s.0.lyrics.clone()).filter(|l| lyrics::has_singing(l));
        match req.lyrics.as_deref().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).or(own) {
            Some(l) => lyrics::Words::Sung(l),
            None if req.instrumental == Some(true) => lyrics::Words::Instrumental,
            None => lyrics::Words::MusicModelWrites,
        }
    };
    if sings && words == lyrics::Words::MusicModelWrites && problem.is_none() {
        // Not written before the job (it always is for the Studio's and chats' songs).
        (ctx.progress)("Writing lyrics", 0.02);
        let length = if task.is_some() { source_len.max(10.0) } else { seconds };
        match lyrics::write(ctx.state, &caption_words, Some(length), req.language.as_deref(), &spec.sections(), &spec.lyrics_guide(), spec.flow(&req.prompt)).await {
            Ok(l) => words = lyrics::Words::Sung(l),
            Err(why) => problem = Some(why),
        }
    }
    let caption = if sound {
        format!("{}. A sound effect recording, not music: no melody, no singing.", req.prompt.trim().trim_end_matches('.'))
    } else if caption_words.is_empty() {
        // Taking a part out, or building around one, needs no description.
        source.as_ref().map(|s| s.0.prompt.clone()).filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "music".into())
    } else {
        caption_words.clone()
    };
    // Directions written in the lyrics' brackets, Suno's way, go into the
    // description; the tags keep only a word or two (`lyrics::tidy`).
    let caption = match req.lyrics.as_deref().map(lyrics::directions_of).filter(|d| !d.is_empty() && !sound) {
        Some(d) => format!("{caption}, {}", d.join(", ")),
        None => caption,
    };
    // A custom model: its trigger word starts the description, as in training.
    let custom = match spec.custom.as_deref().filter(|c| !c.is_empty()) {
        Some(id) => Some(train::get(&ctx.state.db.lock().unwrap(), &c, id).ok_or("That custom model is no longer here.")?),
        None => None,
    };
    let caption = match &custom {
        Some(cm) => format!("{}, {caption}", cm.trigger),
        None => caption,
    };
    let adapters = train::adapters_dir(&ctx.state.paths);
    let yue = m.file("yue").zip(m.file("yue_vae"));
    if yue.is_some() && (task.is_some() || sound) {
        return Err("YuE2 makes new songs. To work on a song or make a sound effect, pick an ACE-Step model.".into());
    }
    let heart = m.file("heartmula");
    if heart.is_some() && (task.is_some() || sound) {
        return Err("HeartMuLa makes new songs. To work on a song or make a sound effect, pick an ACE-Step model.".into());
    }
    let seed = req.seed.unwrap_or_else(random_seed);
    // Words to sing and nothing wrote them: said now, not after the whole
    // song (the music model's own lyrics came out as garbage).
    if sings && words == lyrics::Words::MusicModelWrites && !sound {
        return Err(lyrics::no_singing_message(problem.as_ref()));
    }
    // A new song gets its tempo, key and time signature (see `music::settle`).
    let (bpm, key, time_signature) = if task.is_none() {
        let (b, k, t) = music::settle(&caption, spec.bpm, spec.key.as_deref(), spec.time_signature.as_deref(), seed);
        (Some(b), Some(k), Some(t))
    } else {
        (spec.bpm, spec.key.clone(), spec.time_signature.clone())
    };
    let language = vocal_language(req.language.as_deref(), &words);
    let lyrics_text = if task.is_some() && words == lyrics::Words::MusicModelWrites {
        String::new()
    } else if matches!(words, lyrics::Words::Sung(_)) && task.is_none() {
        // A screamed chorus or a whispered verse is said on its tag too.
        spec.mark_sections(words.for_engine())
    } else {
        words.for_engine().to_string()
    };
    let models = model_dir(&ctx.state.paths, m);
    let versions = if task.is_none() && !sound { req.count.unwrap_or(1).clamp(1, 3) } else { 1 };
    let made = if let Some((backbone, vae)) = yue {
        let engine_dir = yue_engine_dir(ctx.state).ok_or("YuE2's engine isn't installed. Reinstall YuE2 from the Studio's Models list.")?;
        // YuE2 reads the language among its style tags.
        let style = match language.as_deref() {
            Some(l) => format!("{caption}, {}", crate::translate::language_name(l)),
            None => caption.clone(),
        };
        yue::make(
            &music::Engine { dir: &engine_dir, models: &models, low_priority: ctx.state.limits().low_priority, job: ctx.state.job(), logs: &ctx.state.paths.logs },
            work,
            &yue::YueRequest { style: &style, lyrics: &lyrics_text, seconds, seed, versions, backbone: &backbone.file, vae: &vae.file, weirdness: spec.weirdness },
            ctx.cancel,
            ctx.progress,
        )
        .await?
    } else if let Some(file) = heart {
        let engine_dir = heartmula_engine_dir(ctx.state).ok_or("HeartMuLa's engine isn't installed. Reinstall HeartMuLa from the Studio's Models list.")?;
        let mut made = heartmula::make(
            &music::Engine { dir: &engine_dir, models: &models, low_priority: ctx.state.limits().low_priority, job: ctx.state.job(), logs: &ctx.state.paths.logs },
            work,
            &heartmula::HeartRequest {
                caption: &caption,
                lyrics: &heartmula::lyrics_for(&lyrics_text),
                seconds,
                seed,
                versions,
                model: &file.file,
                // Weirdness and Style influence, their own at the middle.
                temperature: spec.weirdness.map(|w| 0.7 + 0.6 * w.clamp(0.0, 1.0)),
                guidance: spec.influence.map(|x| 1.0 + x.clamp(0.0, 1.0)),
            },
            ctx.cancel,
            ctx.progress,
        )
        .await?;
        for song in &mut made {
            song.lyrics = lyrics_text.clone();
        }
        made
    } else {
    // The planning model isn't used (see `music::make`); older installs still have it.
    let dit = m.file("dit").ok_or("The music model is missing a file.")?;
    let lm_file = m.file("lm").map(|f| f.file.clone()).unwrap_or_default();
    let engine_dir = music_engine_dir(ctx.state).ok_or("The music engine isn't installed. Reinstall the music model from the Studio's Models list.")?;
    music::make(
        &music::Engine {
            dir: &engine_dir,
            models: &models,
            low_priority: ctx.state.limits().low_priority,
            job: ctx.state.job(),
            logs: &ctx.state.paths.logs,
        },
        work,
        &music::SongRequest {
            caption: &caption,
            lyrics: &lyrics_text,
            seconds: if task.is_some() { source_len.max(10.0) } else { seconds },
            seed,
            language: language.as_deref(),
            lm_file: &lm_file,
            dit_file: &dit.file,
            bpm,
            key: key.as_deref(),
            time_signature: time_signature.as_deref(),
            task: engine_task,
            track: track.as_deref(),
            region,
            strength: spec.strength,
            source: source.as_ref().map(|s| s.1.as_path()),
            reference: reference.as_ref().map(|r| r.1.as_path()),
            versions,
            guidance: music::guidance(&dit.file, spec.influence),
            negative: spec.exclude.as_deref(),
            adapters: custom.as_ref().map(|_| adapters.as_path()),
            adapter: custom.as_ref().map(|cm| cm.id.as_str()),
            adapter_scale: spec.custom_scale,
            ..Default::default()
        },
        ctx.cancel,
        ctx.progress,
    )
    .await?
    };
    let planned = made[0].lyrics.clone();
    // Vocals were asked for, and the music model planned nothing to sing:
    // say so rather than hand back an instrumental as if it were the song.
    if task.is_none() {
        if let Err(e) = lyrics::check_sung(&words, &planned, problem.as_ref()) {
            crate::applog::line("media", &format!("The song came out without words to sing (planned lyrics: {:?})", planned.chars().take(80).collect::<String>()));
            return Err(e);
        }
    }
    // Harmonies layered on afterwards, by the model that adds parts: the
    // sung ones as backing vocals, the instrument's as its part.
    let mut made = made;
    let mut note = None;
    if task.is_none() && !sound && spec.harmony_layer {
        let mut layers: Vec<(&str, String)> = Vec::new();
        if req.instrumental != Some(true) {
            if let Some(h) = spec.harmony_vocals() {
                layers.push(("backing_vocals", h));
            }
        }
        if let (Some(part), Some(h)) = (spec.harmony_instrument.as_deref().and_then(part_for), spec.harmony_line()) {
            layers.push((part, h));
        }
        match (layers.is_empty(), parts_model(ctx.state)) {
            (true, _) => {}
            (false, None) => note = Some("The harmonies were made with the song: layering them on afterwards needs ACE-Step 1.5 Base (Models).".to_string()),
            (false, Some(base)) => {
                let base_dit = base.file("dit").ok_or("ACE-Step 1.5 Base is missing a file.")?;
                let base_models = model_dir(&ctx.state.paths, &base);
                let engine_dir = music_engine_dir(ctx.state).ok_or("The music engine isn't installed. Reinstall the music model from the Studio's Models list.")?;
                for (vi, song) in made.iter_mut().enumerate() {
                    for (li, (part, words)) in layers.iter().enumerate() {
                        (ctx.progress)(if *part == "backing_vocals" { "Layering the harmony vocals" } else { "Layering the harmony instrument" }, 0.0);
                        let dir = work.join(format!("layer-{vi}-{li}"));
                        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                        let (src, _) = audio_in::for_engine(&song.mp3, "audio/mpeg", &dir)?;
                        let layered = music::make(
                            &music::Engine {
                                dir: &engine_dir,
                                models: &base_models,
                                low_priority: ctx.state.limits().low_priority,
                                job: ctx.state.job(),
                                logs: &ctx.state.paths.logs,
                            },
                            &dir,
                            &music::SongRequest {
                                caption: &format!("{words}, {caption}"),
                                lyrics: &song.lyrics,
                                seconds: song.seconds as f32,
                                seed,
                                language: language.as_deref(),
                                lm_file: "",
                                dit_file: &base_dit.file,
                                bpm,
                                key: key.as_deref(),
                                time_signature: time_signature.as_deref(),
                                task: "lego",
                                track: Some(part),
                                source: Some(src.as_path()),
                                versions: 1,
                                guidance: music::guidance(&base_dit.file, None),
                                ..Default::default()
                            },
                            ctx.cancel,
                            ctx.progress,
                        )
                        .await?;
                        if let Some(l) = layered.into_iter().next() {
                            song.mp3 = l.mp3;
                        }
                    }
                }
                crate::applog::line("media", &format!("Layered {} harmony part(s) with {}", layers.len(), base.name));
            }
        }
    }
    // What it is, in the gallery: the person's words, else what was done to which song.
    let named = |s: &(MediaItem, std::path::PathBuf, f64)| if s.0.prompt.trim().is_empty() { "the song".to_string() } else { format!("“{}”", s.0.prompt.trim()) };
    let prompt = match (task, source.as_ref()) {
        (Some("extract"), Some(s)) => format!("The {} of {}", track.as_deref().unwrap_or("part").replace('_', " "), named(s)),
        (Some("add"), Some(s)) => format!("{} with {} added", named(s), track.as_deref().unwrap_or("a part").replace('_', " ")),
        (Some("complete"), Some(s)) => format!("{} made into a full song", named(s)),
        (Some("extend"), Some(s)) if req.prompt.trim().is_empty() => format!("{}, made longer", named(s)),
        (Some("repaint"), Some(s)) if req.prompt.trim().is_empty() => format!("{} with a part made again", named(s)),
        _ if req.prompt.trim().is_empty() => caption.clone(),
        _ => req.prompt.trim().to_string(),
    };
    let mut items = Vec::new();
    for song in made {
        let item = MediaItem {
            kind: "audio".into(),
            op: req.op.clone(),
            prompt: prompt.clone(),
            model_id: Some(m.id.clone()),
            mime: "audio/mpeg".into(),
            seconds: song.seconds,
            parent: source.as_ref().map(|s| s.0.id.clone()),
            chat_id: req.chat_id.clone(),
            seed: Some(seed),
            // A new song is named as asked, else by its hook; work on a song keeps its name.
            title: if task.is_none() {
                spec.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).or_else(|| (!sound).then(|| lyrics::title_of(&song.lyrics)).flatten())
            } else {
                source.as_ref().and_then(|s| s.0.title.clone())
            },
            lyrics: (!sound && lyrics::has_singing(&song.lyrics)).then_some(song.lyrics),
            // A new song's settings, to make it again (none of the title: a new take gets its own).
            music: (task.is_none() && !sound).then(|| MusicSpec { title: None, ..spec.clone() }),
            note: note.clone(),
            ..Default::default()
        };
        items.push(store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &song.mp3, None)?);
    }
    Ok(items)
}
